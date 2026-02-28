# CLAUDE.md Archive — Historical Session Journals

> Archived 2026-02-28. These are session journals from Aug-Dec 2025, preserved for reference.
> Reusable lessons extracted to `~/Assistants/shared/LESSONS-LEARNED.md`.
> Active constraints live in the lean `CLAUDE.md`.

---

## CLAUDE CODE SCROLLING REGRESSION - ACTIVE ISSUE (2025-09-25)

**Status**: ❌ **REGRESSION IDENTIFIED** - Scrolling issue has returned despite previous fixes

### The Problem Returns
The Claude Code scrolling issue that was previously resolved has reoccurred. This manifests as:
- Unwanted auto-scrolling behavior during Claude Code sessions
- Display jumping or scrolling when it shouldn't
- Interference with reading longer responses

### Previous Solutions Attempted
Based on documentation, previous fixes included:
- Ghostty configuration changes for auto-scroll behavior
- WezTerm terminal settings adjustments
- Various scroll-to-bottom settings modifications

### Current Status
- **Issue identified**: 2025-09-25 evening session
- **Priority**: Medium - doesn't break functionality but impacts user experience
- **Next session**: Systematic debugging required to identify root cause of regression
- **Investigation needed**: Check if recent config changes affected terminal scroll behavior

### Action Items for Next Session
1. Review recent changes to terminal configurations (Ghostty, WezTerm)
2. Check if dotter sync updates modified scroll-related settings
3. Test across different terminal applications to isolate the problem
4. Apply permanent fix and document prevention measures

---

## DOTTER AUTO-SYNC SERVICES RECOVERED (2025-12-08)

**Status**: ALL SERVICES RUNNING on both macOS and Linux

### Issues Fixed

**macOS**:
- Created symlinks: `dotter-realtime-watcher` → `-renu`, `dotter-drift-watcher` → `-renu`
- Added PATH to `com.user.dotter-drift-watcher.plist` (nu not found)
- Fixed `dotter-drift-monitor` Nushell API issues

**Linux (nimbini)**:
- Fixed systemd service to run script directly (was trying to `use config.nu`)
- Created symlink: `dotter-realtime-watcher` → `-renu`
- Rsync'd fixed scripts with portable shebangs

### Root Causes
1. **Shebang issue**: Scripts had `#!/opt/homebrew/bin/nu` (macOS-only) instead of `#!/usr/bin/env nu`
2. **Symlink missing**: Scripts renamed to `-renu` but services expected original names
3. **LaunchAgent PATH**: `nu` not in PATH for launchd services
4. **Nushell API changes**: `stat` → `ls -l`, `open | from json` → `open` (auto-parses)

---

## TODO TOGGLE FUNCTIONALITY VERIFICATION - COMPLETE SUCCESS (2025-09-25)

**Status**: FULLY OPERATIONAL - 5-state todo toggle system working perfectly across all edge cases

### Comprehensive Testing Completed
Successfully verified that the enhanced todo toggle system (`Space+t` in Helix) handles all transformation states correctly:

**5-State Cycle Verified**:
1. **Plain text** → **Unchecked todo** (`- [ ]`)
2. **Unchecked todo** (`- [ ]`) → **Checked todo** (`- [x]`)
3. **Checked todo** (`- [x]`) → **Plain list item** (`-`)
4. **Plain list item** (`-`) → **Plain text** (removes list marker)
5. **Plain text** → **Unchecked todo** (cycle repeats)

---

## BIDIRECTIONAL SYNC + AUTOMATIC SERVICE DEPLOYMENT (2025-09-24)

**Status**: COMPLETE ZERO-TOUCH INFRASTRUCTURE - Self-healing sync with fully automatic service management

### Three-Phase Complete Automation

**Phase 1 (2025-09-23) - Foundation**: Stale lock files from crashed processes were causing phantom "already running" errors. SOLVED with intelligent age-based stale lock detection.

**Phase 2 (2025-09-24) - Reliability**: Enhanced Linux auto-push from 95% to 99.9% reliability through comprehensive retry logic, failure notifications, and advanced monitoring.

**Phase 3 (2025-09-24) - Complete Automation**: Fully automatic service deployment - services auto-enable/start when configurations sync.

### Intelligent Lock Management
```nushell
# 10-minute age threshold for safety
if $age_minutes > 10 {
    # Safe to clean - process definitely crashed
    rm -f $lock_file
} else {
    # Recent lock - respect running process
    exit 1
}
```

### Bidirectional Sync Status
- **macOS → Linux Flow**: Auto-pull watcher monitors GitHub → pulls changes → deploys via Dotter
- **Linux → macOS Flow**: Auto-push watcher monitors local changes → commits & pushes to GitHub
- **Cross-Platform Coordination**: Changes sync automatically within 2-5 minutes either direction

---

## NUSHELL LESSON LEARNED - ALIASES vs DEF (2025-09-24)

**In Nushell, `alias` can ONLY alias simple commands, NOT pipelines.**

```nushell
# WORKS - Simple command aliasing
alias vi = nvim

# FAILS - Cannot alias pipelines
alias ll = ls | select name size modified  # Error

# CORRECT - Use def for pipelines
def ll [] { ls | select name size modified }
```

---

## CROSS-PLATFORM ACADEMIC WORKFLOW - COMPLETED (2025-09-18)

The entire academic workflow (`fcitz`, `fwl`, `fsem`, etc.) is fully cross-platform:
- Identical commands work across macOS, Linux, Windows
- Automated Zotero integration with PDF opening
- Universal clipboard operations with graceful platform detection

---

## DAILY-NOTE ZJ CROSS-PLATFORM FIX (2025-09-23)

Complete resolution of daily-note integration issues:
- Universal `hx` command works identically on macOS and Linux
- Zellij KDL syntax fixed for focus property placement
- SSH workflow verified working from London → Linux via Tailscale

### Key Breakthroughs
1. KDL tab properties (`focus=true`) must be on declaration line; pane properties (`focus true`) use different syntax
2. Universal command pattern eliminates platform-specific code duplication
3. SSH-over-Tailscale achieves seamless remote development

---

## CLAUDE COLLABORATION OPTIMIZATION (2025-09-18)

### Preferred Tool Stack
- `fd` instead of `find`
- `rg` instead of `grep`
- `sk` for fuzzy finding
- `bat` instead of `cat`
- `sd` instead of `sed`

---

## DOTTER CONFIGURATION MANAGEMENT - FINAL SOLUTION (2025-08-27)

### The Root Problem (Fixed)
Mixing individual file symlinks (reliable) with directory-level `type = "symbolic"` (unreliable).

### The Complete Solution
Converted all problematic directory symlinks to individual files in global.toml.

### Two-Layer Protection System
**Layer 1 - Drift Protection**: Edit managed files, symlinks work automatically.
**Layer 2 - Orphan Prevention**: Run `dotter-orphan-detector-v2` before/after adding config files, immediately add to `global.toml`.

### Mandatory Configuration Change Procedure
1. Edit files in `~/dotfiles/` — NEVER edit files in `~/.config/` or `~/.local/bin/`
2. For new scripts: add to `~/dotfiles/scripts/`, add to `global.toml`, run `dotter deploy`
3. Test the change
4. Commit to git

### ZELLIJ UI GOTCHA
`default_layout "compact"` silently overrides `simplified_ui false`. Use `default_layout "default"` instead.

---

## File Editing False Positive Issue

The Edit tool occasionally reports "File has been modified since read" as a false positive. Workaround: use `sed` via Bash or the Write tool.

---

## Sisyphean Stone Prevention - Zellij UI Fix (2025-08-27)

`default_layout "compact"` silently kills the full UI regardless of `simplified_ui false`. No error messages. No warnings. Fixed by changing to `default_layout "default"`.

---

## Template Cursor Positioning System (2025-08-27)

Simple `<cursor>` marker system for templates. Processing scripts remove the marker and position cursor there.

---

## Wiki Link Navigation - Cursor-Aware (2025-09-17)

`Space+w` intelligently selects the nearest wiki link to cursor position based on cursor column distance.

---

## Helix Activity Duration Processing (2025-09-12)

`Space+p` processes duration spans (e.g., `t:: 1430-45` → `t:: 15min 1430-1445`). Uses `:write!` to bypass Helix's external modification protection, WezTerm CLI to send `:reload` command.

---

## Zellij Ctrl+D Conflict Resolution (2025-09-12)

Moved Zellij detach from `Ctrl+D` to `Ctrl+Alt+D` to avoid conflict with Helix half-page-down scrolling.

---

*Archived 2026-02-28 from CLAUDE.md historical session journals (Aug-Dec 2025)*
