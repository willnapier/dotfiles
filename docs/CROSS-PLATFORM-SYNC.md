# Cross-Platform Sync Architecture

## Overview

This document describes the synchronization architecture between a macOS primary workstation and a linux-desktop system for configuration files, knowledge base content, and assistant coordination state.

The architecture has three layers:
1. **Git-based sync** for dotfiles and documentation (explicit commits, 2-minute polling)
2. **Syncthing** for knowledge base, references, and large file sets (continuous, peer-to-peer)
3. **Messageboard** for async coordination between AI assistant instances on different machines

---

## Layer 1: Git-Based Sync (Dotfiles and Documentation)

### What Syncs

Two directories sync via GitHub with full automation:

| Directory | Contents | Polling Interval |
|-----------|----------|-----------------|
| `~/dotfiles` | Configuration files | Every 2 minutes |
| `~/Assistants` | Documentation and knowledge base | Every 2 minutes |

Both directories sync bidirectionally: changes on either machine are automatically committed, pushed to GitHub, and pulled on the other machine.

### How It Works

**Automatic commit/push flow:**
1. Service polls every 2 minutes
2. Detects uncommitted changes via `git status --porcelain`
3. Runs `git add .` and creates commit with standardized message
4. Pushes to `origin/main` on GitHub

**Automatic pull flow:**
1. Runs `git fetch origin main` every 2 minutes
2. Checks if local is behind remote via `git rev-list --count HEAD..origin/main`
3. If behind, runs `git pull origin main`
4. For dotfiles: also runs `dotter deploy` to update symlinked configs

Changes appear on the other machine within 2-4 minutes with zero manual intervention.

### Services

**macOS (primary workstation):**
- Dotfiles pull watcher (LaunchAgent, every 2 min)
- Assistants auto-pull (LaunchAgent, every 2 min)
- Assistants auto-push (LaunchAgent, every 2 min)

**linux-desktop:**
- Dotfiles push watcher (systemd user service, every 2 min)
- Dotfiles pull watcher (systemd user service, every 2 min)
- Assistants push watcher (systemd user service, every 2 min)

### Critical Rule

Always edit in `~/dotfiles`, never in `~/.config`. Dotter creates symlinks from `~/.config` to `~/dotfiles`, so direct edits to `~/.config` will be overwritten on next deployment.

---

## Layer 2: Knowledge and State (Syncthing)

### What Syncs

| Folder | Purpose | Versioning |
|--------|---------|------------|
| `~/Forge` | Knowledge base (6,400+ files) | Staggered, 30 days |
| `~/Assistants` | Shared docs, continuum logs, skills | Staggered, 30 days |
| `~/Books` | Reference library | Staggered, 30 days |

### Versioning Configuration

All Syncthing folders use 30-day staggered versioning, providing recovery from accidental deletions or sync conflicts:

```bash
syncthing cli config folders <FOLDER> versioning type set staggered
syncthing cli config folders <FOLDER> versioning params set maxAge 2592000
```

Overwritten files can be recovered from `.stversions/`.

### Common Operations

**Check status:**
```bash
syncthing cli show system         # Overall status
syncthing cli show folder <NAME>  # Specific folder
```

**Force immediate scan** (useful after messageboard posts):
```bash
curl -s -X POST "http://127.0.0.1:8384/rest/db/scan?folder=<FOLDER>" \
  -H "X-API-Key: $(syncthing cli config gui apikey get)"
```

---

## Layer 3: Messageboard (Assistant Coordination)

### Purpose

Async communication between Claude Code instances running on different machines.

**Location:** `~/Assistants/shared/MESSAGEBOARD.md`

### Protocol

- Format: `### YYYY-MM-DD -- device`
- Newest messages at top
- Receiving assistant clears messages once actioned
- 30-day fallback for stale items

### Trigger Instant Sync

After posting to the messageboard, trigger an immediate Syncthing scan:

```bash
curl -s -X POST "http://127.0.0.1:8384/rest/db/scan?folder=Assistants" \
  -H "X-API-Key: $(syncthing cli config gui apikey get)"
```

---

## Failure Modes and Recovery

### Git Conflicts (Dotfiles/Assistants)

If `git pull` fails with conflicts:
1. `git stash` local changes
2. `git pull`
3. `git stash pop`
4. Resolve conflicts manually

Or commit local first, then `git pull --rebase`.

### Syncthing Conflicts

Syncthing creates `.sync-conflict-*` files when both machines modify the same file simultaneously:
1. Compare versions
2. Keep the correct one
3. Delete the conflict file

Staggered versioning allows recovery of overwritten files from `.stversions/`.

### Service Recovery

**macOS:**
```bash
# Services auto-restart, but manual restart if needed:
launchctl kickstart -kp gui/$(id -u)/com.user.assistants-auto-pull
launchctl kickstart -kp gui/$(id -u)/com.user.assistants-auto-push
```

**linux-desktop:**
```bash
systemctl --user restart assistants-docs-watcher
systemctl --user restart git-auto-push-watcher
```

### Lock File Issues

If services fail with "already running" errors, remove stale lock files:

```bash
# macOS
rm -f /tmp/assistants-auto-pull.lock /tmp/assistants-auto-push.lock

# linux-desktop
rm -f /tmp/assistants-auto-push.lock
```

Then restart the affected service.

### Messageboard Out of Sync

Re-read the file -- Syncthing may not have propagated yet. Use the manual scan trigger if urgent.

---

## Troubleshooting

### Changes Not Syncing

1. **Check service status** using `launchctl list` (macOS) or `systemctl --user status` (Linux)
2. **Check logs** for errors:
   - macOS: `~/.local/share/assistants-auto-*.log`
   - Linux: `~/.local/share/assistants-auto-push.log`
3. **Verify network**: `git fetch origin main` should succeed
4. **Check for conflicts**: `cd ~/Assistants && git status`

### Services Not Starting

1. Check for stale lock files (see above)
2. Verify script permissions: scripts should be executable
3. Check for syntax errors by running scripts manually

---

## Design Principles

1. **Dotfiles are authoritative** -- `~/dotfiles` is the source of truth; `~/.config` is derived via symlinks
2. **Syncthing for state, Git for config** -- Knowledge syncs continuously; configuration requires explicit commits
3. **Async coordination via messageboard** -- No real-time requirement between assistant instances
4. **30-day safety net** -- Staggered versioning on all Syncthing folders provides recovery from accidental changes

---

## Architecture Summary

```
                    GitHub (remote)
                    /            \
               git push       git pull
              /                    \
   primary workstation          linux-desktop
   (macOS)                      (Linux)
   - LaunchAgent services       - systemd user services
   - 2-min polling              - 2-min polling
   - dotter deploy              - dotter deploy
              \                    /
               \                  /
                Syncthing (P2P)
                - ~/Forge (knowledge base)
                - ~/Assistants (shared docs)
                - ~/Books (references)
                - 30-day staggered versioning
```
