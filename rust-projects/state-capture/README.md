# state-capture

Config-driven system state capture and drift detection for Linux and macOS.

Define what to capture in a TOML file. The tool runs your commands, saves the output as baseline files, and can later diff live state against those baselines to detect drift.

## Install

```sh
cargo build --release
cp target/release/state-capture ~/.local/bin/
```

## Quick start

```sh
# Create a config with platform-appropriate defaults
state-capture init                # auto-detects platform/distro
state-capture init --preset arch  # or specify explicitly

# Capture current state
state-capture capture

# Later, check for drift
state-capture check
```

## Subcommands

| Command | Description | Exit code |
|---|---|---|
| `state-capture` | Run all captures (same as `capture`) | 0 ok, 1 failure |
| `state-capture capture` | Run all captures, write output files | 0 ok, 1 failure |
| `state-capture capture --dry-run` | Show what would be captured | 0 |
| `state-capture check` | Diff live state against baselines | 0 clean, 1 drift |
| `state-capture check --quiet` | Exit code only, no output | 0 clean, 1 drift |
| `state-capture check --json` | Drift report as JSON | 0 clean, 1 drift |
| `state-capture init [--preset]` | Create config file | 0 ok, 1 failure |
| `state-capture list` | Show configured captures | 0 |
| `state-capture show <name>` | Print a baseline file's content | 0 ok, 1 not found |

Use `--config <path>` to specify a config file other than the default.

## Config format

Default location: `~/.config/state-capture/config.toml` (Linux) or `~/Library/Application Support/state-capture/config.toml` (macOS).

```toml
[settings]
state_dir = "~/dotfiles"    # where baseline files are written

[[capture]]
name = "arch-packages"
command = "pacman -Qqe"      # any sh -c command
output = "arch-packages.txt" # filename within state_dir
sort = true                  # sort output lines before saving

[[capture]]
name = "user-groups"
command = 'id -nG | tr " " "\n"'
output = "user-groups.txt"
sort = true
```

Each `[[capture]]` entry defines:

- **name** — identifier used by `show` and in drift reports
- **command** — shell command run via `sh -c`; its stdout becomes the baseline
- **output** — filename written to `state_dir`
- **sort** — whether to sort output lines (useful for stable diffs)

## How it works

The tool is a thin runner. It doesn't know anything about package managers, service systems, or operating systems. It runs whatever `sh -c` commands you put in the config, saves stdout to files, and compares those files later.

This means:

- **Any command that produces line-based output works.** If you can run it in a terminal and get a list, you can capture it.
- **The presets are just convenience.** They generate a config with sensible commands for your platform. You can (and should) edit the result.
- **If a command isn't available, that capture fails gracefully** — the tool reports the error and continues with the rest.

### Drift detection

`state-capture check` re-runs each command and compares its output against the saved baseline using set comparison:

- **Added lines** — present in live output but not in baseline
- **Removed lines** — present in baseline but not in live output

This treats each file as a set of lines, not an ordered sequence. It works well for package lists, service lists, group memberships — anything where you care about "what's present" rather than "what order they're in".

### JSON output

```json
{
  "has_drift": true,
  "captures": [
    {
      "name": "arch-packages",
      "status": "drift",
      "added": ["firefox-nightly"],
      "removed": ["firefox"]
    },
    {
      "name": "user-groups",
      "status": "clean"
    }
  ]
}
```

Status values: `clean`, `drift`, `nobaseline`, `error`.

## Presets

`state-capture init` auto-detects your platform. On macOS it detects Darwin. On Linux it checks which package manager is in PATH. You can override with `--preset arch|debian|fedora|macos`.

### Arch (8 captures)

- `pacman -Qqe` — official packages
- `pacman -Qqem` — AUR packages
- systemctl services, timers, user units (enabled)
- npm globals
- user groups
- cargo crates (via `capture-cargo-crates` helper)

### Debian (6 captures)

- `dpkg-query` packages
- systemctl services, timers, user units (enabled)
- npm globals
- user groups

### Fedora (6 captures)

- `dnf list installed` packages
- systemctl services, timers, user units (enabled)
- npm globals
- user groups

### macOS (5 captures)

- `brew list --formula` — Homebrew formulae
- `brew list --cask` — Homebrew casks
- LaunchAgents — plist filenames in `~/Library/LaunchAgents/`
- npm globals
- user groups

## macOS notes

The tool works on macOS the same way it works on Linux — it runs `sh -c` commands and saves their output. The macOS preset gives you a starting point, but you'll likely want to customise it.

**What the preset captures:**

Homebrew is the obvious one. `brew list --formula -1` and `brew list --cask -1` give you clean one-per-line lists that work perfectly with set-based drift detection. If someone installs a cask or a formula gets removed, `state-capture check` will catch it.

LaunchAgents are captured by listing plist filenames in `~/Library/LaunchAgents/`. This tells you which agents are *installed*, not whether they're currently loaded or running — that's a separate concern better handled by a health-check script. The drift check will catch agents that were added or removed.

**What the preset doesn't capture (and why):**

- **System LaunchDaemons** (`/Library/LaunchDaemons/`) — requires root to list reliably, and third-party apps change these. Add a capture if you want to track them.
- **Mac App Store apps** — `mas list` works if you have [mas](https://github.com/mas-cli/mas) installed. Add it: `command = "mas list | awk '{print $2}'"`.
- **System Preferences state** — no clean CLI for this. `defaults` can read individual domains but there's no universal "list all settings" command.
- **Cargo crates** — the Arch preset includes this via a helper script. If you track cargo installs on macOS, add a capture with `cargo install --list`.

**Scheduling on macOS:**

Instead of systemd timers, use a launchd plist:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.user.state-capture</string>
    <key>ProgramArguments</key>
    <array>
        <string>/Users/you/.local/bin/state-capture</string>
        <string>capture</string>
    </array>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Hour</key>
        <integer>4</integer>
        <key>Minute</key>
        <integer>30</integer>
    </dict>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
    <key>StandardOutPath</key>
    <string>/tmp/state-capture.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/state-capture.log</string>
</dict>
</plist>
```

Save to `~/Library/LaunchAgents/com.user.state-capture.plist`, then:

```sh
launchctl load ~/Library/LaunchAgents/com.user.state-capture.plist
```

Note the `EnvironmentVariables` block — launchd does not inherit your shell PATH, so Homebrew commands like `brew` won't be found without it. `/opt/homebrew/bin` is the default Homebrew location on Apple Silicon; use `/usr/local/bin` on Intel Macs.

**Integrating with a health-check script:**

The JSON output works the same on both platforms:

```sh
state-capture check --json --quiet | jq '.has_drift'
```

## Dependencies

The tool itself has no runtime dependencies beyond a POSIX shell (`sh`). The *commands in your config* are your dependencies — if `brew` isn't installed, the `brew list` capture will fail gracefully while other captures continue.

The Arch preset includes a `capture-cargo-crates` capture that expects a helper script in PATH. If you don't use it, remove that entry from the config.

## Typical setup

Run captures on a schedule, check for drift separately.

**Linux (systemd):**

```ini
[Service]
Type=oneshot
Environment=PATH=/usr/local/bin:/usr/bin:%h/.cargo/bin:%h/.local/bin
ExecStart=%h/.local/bin/state-capture capture
```

**macOS (launchd):** See the plist example in the macOS notes above.

**Manual / cron:**

```sh
# crontab -e
30 4 * * * $HOME/.local/bin/state-capture capture
```

## Build dependencies

Rust edition 2021. Crate dependencies: `clap`, `anyhow`, `serde`, `serde_json`, `toml`, `dirs`.

## License

MIT
