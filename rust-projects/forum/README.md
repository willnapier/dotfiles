# forum

`forum` is the vendor-neutral command-line orchestrator for William's shared
multi-assistant Design Forum. It is deliberately separate from Codex, Claude
Code, and Grok Build: each harness is a replaceable headless contributor, while
the binary owns thread creation, serialized writes, round idempotency, and job
records.

## Core workflow

```nu
forum open --id meta-example --system assistant-infrastructure --area meta --level architecture --title "Example decision" --context "What should we decide?" --opened-by codex
forum post meta-example --author codex --body "**Claim:** ..."
forum convene meta-example --caller codex --panel others
forum status meta-example
```

Panels:

- `core`: Codex, Claude Code, and Grok Build
- `others`: the core panel excluding `--caller`
- `all`: every default core harness (customizable in config)
- explicit: `--panel codex,grok-build`

The first orchestrated round produces Positions. Use `--new-round` for a Reply
round after the panel has seen the accumulated thread. Every appended response
gets a `(thread, round, harness)` marker, so rerunning a partially failed round
only invokes missing contributors.

Use `--dry-run` to inspect selection without model calls:

```nu
forum convene meta-example --caller codex --panel others --dry-run
```

## Safety model

- Contributors receive one fixed snapshot, preserving independent first-round
  positions.
- Contributors are told not to use tools, edit files, or convene more agents.
- Codex runs read-only and ephemeral; Claude and Grok run in plan/read-only
  permission modes.
- Harness output is staged under `~/.local/state/forum/jobs/`.
- Only `forum` appends to the canonical thread, under a local exclusive lock and
  atomic file replacement.
- Forum threads must remain `status: open`; decided/parked/rejected threads are
  refused.
- A single coordinator host is still required. Syncthing does not provide a
  distributed lock across Mac and nimbini.

## Optional configuration

Defaults work with the installed `codex`, `claude`, and `grok` commands. Override
or extend them at `~/.config/forum/config.toml` (Dotter-manage that file before
using it):

```toml
[harnesses.grok-build]
enabled = true
command = "grok"
args = ["--permission-mode", "plan", "--no-subagents", "--single"]
prompt_mode = "argument"

[panels]
core = ["codex", "claude-code", "grok-build"]
```

Arguments may contain `{forum_root}`, which is replaced with the active forum
directory. `prompt_mode` is `stdin` or `argument`; commands are executed
directly, never through a shell.

## Deployment

```nu
cargo test --manifest-path ~/dotfiles/rust-projects/forum/Cargo.toml
cargo install --path ~/dotfiles/rust-projects/forum --root ~/.local --force
forum doctor
```

The installed binary is a copy, not a Dotter symlink into `target/`, so Cargo's
build directory can be cleaned safely.
