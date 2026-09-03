# Cross-Platform Sync Architecture

## Overview

The Mac and nimbini use different transports according to what is being
carried. The important boundary is that `~/Assistants` is a
Syncthing-transported working tree whose Git repository records history; Git
is not allowed to become a second working-file transport.

| Scope | Working-file transport | History / recovery |
|---|---|---|
| `~/dotfiles` | Git | GitHub; Dotter deploys the checked-out source |
| `~/Assistants` | Syncthing | One nimbini Git historian, GitHub, Syncthing versioning, backups |
| `~/Forge`, `~/Books` | Syncthing | Syncthing staggered versioning and backups |
| Assistant coordination | Messageboard + design forum, carried by Syncthing | Git history where applicable |

## Dotfiles: Git is the transport

Both hosts may commit and push dotfiles. `git-auto-pull-watcher` polls only
`~/dotfiles`, fast-forwards with `--ff-only`, and runs `dotter deploy` after a
successful pull. It does not target `~/Assistants`.

Always edit `~/dotfiles`, never the deployed paths under `~/.config`. After a
configuration change, run `dotter deploy` and `dotter-orphan-detector-v2`.

Services:

- Mac: `com.williamnapier.git-auto-push-watcher` and
  `com.williamnapier.git-auto-pull-watcher`
- nimbini: `git-auto-push-watcher.service` and
  `git-auto-pull-watcher.service`

## Assistants: Syncthing carries bytes; one Git historian records them

The 2026-09-03 reconciliation replaced two autonomous Git commit clocks with
the dedicated `assistants-git-sync` service:

- **nimbini is the historian.** It is the only unattended process allowed to
  create commits or push `~/Assistants` `main`.
- **The Mac is a follower.** It fetches, proves both fast-forward ancestry and
  exact equality between the already-synchronised disk tree and
  `origin/main`, then moves only its ref and index with `update-ref` and
  `read-tree` (without `-u`).
- **Unattended Git never writes a working file in `~/Assistants`.** No service
  may run `checkout`, `pull`, `merge`, `rebase`, or a worktree-writing reset
  there.
- `continuum-usage/`, `continuum-logs/`, and `health/` are mutable operational
  state carried by Syncthing and backup, not Git.

The historian waits until the exact dirty content tree has remained unchanged
for 90 seconds and the Syncthing REST API reports the local folder idle and
all connected folder peers complete. It fetches again, validates changed Rust
and Nushell sources, proves that neither refs nor content moved during the
gates, creates one commit from a temporary index, and pushes. A failed push is
retried; a later non-fast-forward is a refusal, never an automatic merge.

The role is fenced by hostname and by a single per-host PID lock. A tree
mismatch, divergence, conflict copy, failed validation, or publication failure
is written to a role-specific heartbeat under
`~/.local/state/watchers/` and surfaced by `system-health-check`.

Supervisor names were retained to make the migration explicit and reversible:

| Host | Supervisor | Effective role |
|---|---|---|
| Mac | `com.williamnapier.assistants-auto-push` | follower (despite legacy label) |
| nimbini | `assistants-docs-watcher.service` | historian |

Detailed invariants and recovery commands are in
`~/Assistants/shared/ASSISTANTS-DOCS-AUTO-PUSH.md`.

## Syncthing

`~/Forge`, `~/Assistants`, and `~/Books` use continuous peer-to-peer delivery
with staggered versioning. Overwritten files can be recovered from
`.stversions/`.

Useful read-only checks:

```bash
syncthing cli show system
syncthing cli show folder Assistants
```

To request an immediate scan after a coordination edit:

```bash
curl -s -X POST "http://127.0.0.1:8384/rest/db/scan?folder=Assistants" \
  -H "X-API-Key: $(syncthing cli config gui apikey get)"
```

Do not sync `.git` directories. Each host keeps its own Git object database;
the follower protocol aligns bookkeeping only after proving the bytes.

## Messageboard and forum

`~/Assistants/shared/MESSAGEBOARD.md` is the compact operational signal
surface. The design forum owns discussion, decisions, and lifecycle state.
Use `messageboard-edit` for board changes and the `forum` CLI for forum state;
do not treat participation as permission to clear a forum pointer.

## Failure handling

For `~/dotfiles`, investigate dirty work before an ordinary fast-forward. Do
not automate a merge or rebase.

For `~/Assistants`, do **not** repair a refusal with `git pull`, stash/reapply,
checkout, rebase, or reset. Leave both refs and files untouched, then inspect:

```bash
cd ~/Assistants
git fetch origin main
git status --short --branch
git merge-base --is-ancestor HEAD origin/main
fd --hidden --no-ignore 'sync-conflict' .
```

Check the role heartbeat and service log:

```bash
# Mac
bat ~/.local/state/watchers/assistants-git-sync-follower.json
bat ~/.local/share/assistants-git-sync-follower.log

# nimbini
bat ~/.local/state/watchers/assistants-git-sync-historian.json
journalctl --user -u assistants-docs-watcher.service -n 100 --no-pager
```

A follower may be adopted manually only when `HEAD` is an ancestor of
`origin/main` **and** an independently built temporary-index tree exactly
equals `origin/main^{tree}`. Otherwise stop and preserve both states for a
supervised reconciliation.

Service restart commands:

```bash
# Mac (legacy label, follower behaviour)
launchctl kickstart -k gui/$(id -u)/com.williamnapier.assistants-auto-push

# nimbini
systemctl --user restart assistants-docs-watcher.service
```

PID locks are stale only when their recorded process is dead. Do not delete a
lock held by a live PID. The Assistants service lock is
`/tmp/assistants-git-sync.lock`.

## Architecture summary

```text
                    GitHub origin/main
                      ^             |
            commit + push           | fetch + proved ref/index adoption
                      |             v
        nimbini historian       Mac follower
                  \                /
                   \              /
                 Syncthing working bytes
                  ~/Assistants (no .git)
```

The asymmetry is deliberate: availability may delay history, but no outage or
race is allowed to create a second autonomous history or overwrite a working
file.
