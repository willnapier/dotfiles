# Dotter Orphan Prevention System

## The Problem

### The Desktop-Quarters Incident

A Zellij layout file (`desktop-quarters.kdl`) mysteriously disappeared, causing a terminal multiplexer session to fail. Investigation revealed:

1. The file existed in the git repository
2. The detection and session naming logic was correct
3. The file vanished from the working directory
4. **The file was never managed by Dotter** -- it was committed to git but not included in Dotter's deployment configuration

### The Scope

Running an orphan detector revealed the full extent:
- **195 total files** in the dotfiles repository
- **78 managed files** by Dotter
- **124 orphaned files** -- 61% of configs unprotected

Critical unmanaged files included keyboard configuration, window manager settings, terminal themes, and numerous essential scripts.

### Why This Kept Happening

Previous attempts solved the wrong problems:

| What was being solved | What actually needed solving |
|---|---|
| Configuration drift (managed files changing) | **Orphan detection** (files in dotfiles not managed by Dotter) |
| Symlink verification (existing links working) | **Addition prevention** (stopping unmanaged files from being created) |
| File modification detection | **Monitoring** (automated detection of orphaned files) |

The process gap was simple: create config file, commit to git, **forget to add to Dotter config**, file exists but is not deployed, eventually disappears during cleanup.

---

## The Solution

### Two-Layer Protection System

#### Layer 1: Drift Protection (Existing Managed Files)

For files already in the Dotter config:
- Edit directly in `~/dotfiles/[app]/[file]`
- Changes appear instantly via symlinks
- No additional steps required

#### Layer 2: Orphan Prevention (New/Unmanaged Files)

For any new config file:
1. **Check baseline**: `dotter-orphan-detector-v2`
2. **Create file** in dotfiles
3. **Immediately add** to `.dotter/global.toml`
4. **Deploy**: `dotter deploy`
5. **Verify**: `dotter-orphan-detector-v2` (should show one fewer orphan)

---

## Orphan Detection Tool (v3)

### What It Does

- Scans all config files in the dotfiles repository
- Intelligently filters to only existing directories (no fd errors)
- Compares against files explicitly managed by Dotter configuration
- Detects directories managed as symbolic links (`type = "symbolic"`)
- Filters out files inside symbolic directories (not orphans)
- Reports unmanaged files with suggested Dotter config entries
- Returns proper exit codes (0 = clean, 1 = orphans found)

### Example Output

```bash
$ dotter-orphan-detector-v2
Scanning for unmanaged config files in dotfiles...
Found 144 config files in dotfiles
Found 178 files managed by Dotter
Found 5 directories managed as symbolic links
All config files are managed by Dotter! No orphans found.

$ echo $?
0  # Exit code 0 = success
```

### Symbolic Directory Detection

The v3 enhancement eliminates false positives from files inside Dotter-managed symbolic directories. Before this fix, the detector reported 143 "orphans" that were mostly false positives (e.g., `yazi/flavors/dracula.yazi/LICENSE`). After: 0 false positives.

```toml
# In .dotter/global.toml:
"yazi/flavors/dracula.yazi" = { target = "~/.config/yazi/flavors/dracula.yazi", type = "symbolic" }

# Detector recognizes ALL files inside are managed:
# yazi/flavors/dracula.yazi/LICENSE
# yazi/flavors/dracula.yazi/flavor.toml
# yazi/flavors/dracula.yazi/tmtheme.xml
```

### Algorithm

1. **Scan dotfiles**: `fd` finds all config files (excluding docs/git)
2. **Parse Dotter config**: Extract all managed file paths from TOML
3. **Compare**: Files in dotfiles but not in Dotter = orphans
4. **Report**: Show orphans with suggested Dotter config entries
5. **Smart categorization**: Suggests correct section (shared/macos/linux)

---

## The Missing local.toml Problem

### Discovery

During a theme deployment, it was discovered that a Linux system had **no `~/.dotter/local.toml` file** for over two weeks since initial setup. macOS was also missing this critical file.

Without `local.toml`, platform-specific configs silently fail to deploy. Shared configs (Helix, Zellij, Nushell) worked fine, masking the problem entirely.

### Prevention

**Pre-commit hook** blocks commits if `~/.dotter/local.toml` is missing:

```
COMMIT BLOCKED: Missing ~/.dotter/local.toml

Dotter doesn't know which platform to deploy for!
Without local.toml, platform-specific configs won't deploy.

To fix (choose your platform):

   macOS:
     mkdir -p ~/.dotter
     echo 'packages = ["macos"]' > ~/.dotter/local.toml

   Linux:
     mkdir -p ~/.dotter
     echo 'packages = ["linux"]' > ~/.dotter/local.toml
```

**Runtime health monitoring** checks Dotter platform configuration on both platforms.

### Critical Rule: local.toml Must Be Machine-Specific

`local.toml` must **never** be tracked in git. Each machine maintains its own platform setting:

```toml
# macOS: ~/dotfiles/.dotter/local.toml
includes = []
packages = ["macos"]

# Linux: ~/dotfiles/.dotter/local.toml
includes = []
packages = ["linux"]
```

If `local.toml` is tracked in git, the macOS platform setting syncs to Linux and vice versa, causing the wrong platform configs to deploy.

---

## Automated Monitoring

A weekly cron job scans for orphaned files:

```bash
# Weekly Sunday 10:00 AM
0 10 * * 0 ~/.local/bin/dotter-orphan-detector-v2 > /tmp/orphan-report.log 2>&1
```

This catches orphaned files before they disappear and provides early warning of configuration drift.

---

## File Coverage

**Monitored directories:**
- `helix/` -- Text editor configuration
- `nushell/` -- Shell configuration
- `yazi/` -- File manager configuration
- `zellij/` -- Terminal multiplexer configuration
- `scripts/` -- Custom automation scripts
- `nvim/` -- Neovim editor configuration
- `wezterm/` -- Terminal emulator (macOS)
- `ghostty/` -- Terminal emulator themes
- `karabiner/` -- Keyboard remapping (macOS)
- `aerospace/` -- Window manager (macOS)
- `sketchybar/` -- Status bar (macOS)
- `wayland/` -- Linux desktop components

**Exclusions:**
- `.git/` -- Git repository data
- `.DS_Store` -- macOS system files
- `*.md` -- Documentation files
- `docs/` -- Documentation directory
- `.dotter/` -- Dotter configuration itself
- Backup files ending in `~`

---

## Procedures

### For Existing Managed Files (Layer 1)

Edit directly in `~/dotfiles/[app]/[file]`. Changes work immediately via symlinks. No verification or deployment needed.

### For New/Unmanaged Files (Layer 2)

1. **Check**: `dotter-orphan-detector-v2` (see current state)
2. **Create**: File in `~/dotfiles/[app]/[file]`
3. **Add**: Entry to `.dotter/global.toml` in the correct section:
   ```toml
   [shared.files]  # or [macos.files] or [linux.files]
   "app/file.ext" = "~/.config/app/file.ext"
   ```
4. **Deploy**: `cd ~/dotfiles && dotter deploy`
5. **Verify**: `dotter-orphan-detector-v2` (should show improvement)

### Red Flags

- "Layout not found" errors (like the desktop-quarters incident)
- Commands referencing files that "should exist"
- Git shows files but they are not deployed
- Weekly orphan report shows new unmanaged files

### Emergency Recovery

If files disappear:
1. Check git history: `git log --name-status -- [missing-file]`
2. Restore if needed: `git restore [path-to-file]`
3. Run orphan detector to check Dotter coverage
4. Add to Dotter config if missing
5. Deploy and verify

---

## Cross-Platform Deployment

The orphan prevention system operates on both macOS and Linux:

- **macOS**: Dotter + orphan detection + weekly monitoring
- **Linux**: Dotter + sync watcher + automatic deployment
- **Syncthing**: Cross-platform sync between machines

A sync watcher on Linux automatically runs `dotter deploy` when Syncthing updates are detected, ensuring configuration changes propagate within minutes.

### The Universal Workflow

For any configuration change (macOS or Linux):
1. **Edit**: File in `~/dotfiles/[app]/[file]`
2. **Sync**: Changes propagate automatically via Syncthing
3. **Deploy**: Automatic via sync watcher (Linux) or instant symlinks (macOS)
4. **Result**: Change active on both machines within 2-5 minutes
