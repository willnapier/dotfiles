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

Once William has accepted the decision and the thread is `status: decided`,
turn it into one bounded implementation assignment:

```nu
forum dispatch meta-example \
  --assignee codex \
  --scope "forum CLI and its documentation" \
  --acceptance "cargo test passes" \
  --acceptance "the decided thread contains a dispatch receipt" \
  --reviewer claude-code \
  --reviewer grok-build \
  --dry-run

# Remove --dry-run after checking the exact work order.
```

`dispatch` refuses open threads, null decisions, empty bounds, duplicate
reviewers, and an assignee reviewing their own work. It posts through the
guarded `messageboard-edit` command, then records an idempotency marker and
dispatch receipt in the decided thread. It assigns work; it never launches an
implementation model or edits product code.

Queue a round and return immediately:

```nu
forum convene meta-example --caller codex --panel others --background
forum jobs
```

The nimbini systemd user service runs `forum worker --poll-seconds 10`. Jobs
live under the Syncthing-shared `design-forum/.orchestrator/`, so either machine
can enqueue work without running competing workers. A claimed job is retried up to
three times with exponential backoff; successful, failed, and cancelled job
records remain inspectable with `forum jobs --all`.

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
  refused by deliberation commands. `dispatch` has the inverse guard and accepts
  only decided threads.
- A single coordinator host is still required. Syncthing does not provide a
  distributed lock across Mac and nimbini. nimbini is the elected worker host;
  the installed Mac LaunchAgent remains disabled as a manual fallback.

## Background worker operations

```nu
forum worker --once                    # process available jobs, then exit
forum worker --poll-seconds 10         # long-running worker
forum jobs                             # queued/running/failed/cancelled
forum jobs --all                       # include completed jobs
forum cancel <job-id>                  # queued jobs only
```

The queue is explicit. Editing or saving Markdown never launches models. A
worker that restarts moves abandoned `running/` jobs back to `queue/`; round
markers make replay safe after partial completion.

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
