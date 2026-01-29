# Entry Notation System

A notation and collection architecture for structured entries in daily journal files. Entries use a `key:: value` syntax that is simultaneously human-readable and machine-parseable.

## Notation Syntax

### Basic Format

```
key:: attributes
```

Every entry begins with a **key** followed by the `::` reification boundary, then space-separated attributes.

### Entry Types

**Project entries** use uppercase `P.`:
```
P.project-name:: duration description
P.website-redesign:: 2hr implemented-responsive-navigation
P.research-paper:: 1hr reviewing-literature
```

**Activity entries** use lowercase keys:
```
p:: 45min piano-practice
t:: 30min commute
r:: 1hr reading
dev:: 2hr rust-project
```

### Hierarchical Dot Notation

Both project and activity keys support hierarchical sub-categories using dot notation:

```
p.c:: 45min JSBach-WTC-I-Prelude-C       # piano → classical
p.j:: 30min Monk-Blue-Monk               # piano → jazz
t.u:: 30min uber-ride                     # travel → uber
dev.rust:: 2hr forge-graph                # dev → rust
```

### Attribute Syntax

Attributes are space-separated tokens after the `::` boundary. Order is flexible:

```
key:: duration description location cost
t.u:: 30min airport-transfer LHR £25
```

Common attribute types:
- **Duration**: `30min`, `1hr`, `2hr30min` (Nushell-native format)
- **Time span**: `1430-1500` (start-end in 24h format)
- **Cost**: `£25`, `$40` (currency-prefixed amounts)
- **Location**: Arbitrary location codes
- **Description**: Hyphenated phrases (`morning-commute`, `piano-practice`)
- **Wikilinks**: `[[target-note]]` for knowledge base links

### The `::` Reification Boundary

The `::` delimiter serves as a **reification boundary** -- it marks the transition from key (what kind of entry) to value (the entry's content). This is distinct from a single `:` (used in YAML frontmatter) and from `::` inside code blocks.

The double-colon was chosen because:
1. It doesn't conflict with standard markdown syntax
2. It's visually distinctive in plain text
3. It enables reliable regex extraction: `^key:: (.*)$`
4. It naturally reads as "key is-recorded-as value"

## Collection Architecture

### Three-Tier System

Entries are written in daily journal files (DayPages) and automatically collected into dedicated files:

**Tier 1: Near-Instant Collection**
- File watcher monitors the DayPages directory for changes
- Triggers collection within 2-3 seconds of saving
- Runs as a background service (launchd on macOS, systemd on Linux)

**Tier 2: Daily Backup Collection**
- Cron job runs nightly as a safety net
- Catches anything the watcher might have missed

**Tier 3: Manual Collection**
- CLI command for on-demand collection
- Supports `--verbose` and `--dry-run` flags

### Routing

The collection system routes entries based on key type:

| Pattern | Destination | Example |
|---------|-------------|---------|
| `P.name` | `~/notes/Projects/name.md` | `P.website-redesign` |
| `key` | `~/notes/logs/key.md` | `p.c`, `t.u`, `dev` |

### Duplicate Prevention

Four-layer deduplication:
1. **Collection-level**: Compares extracted entries against existing file content
2. **Append-level**: Checks for identical entries before writing
3. **Watcher debouncing**: Prevents rapid-fire collection from multiple saves
4. **Event filtering**: Ignores non-content filesystem events

### File Templates

**Project files** get structured templates:
```markdown
# project-name

**Type**:
**Status**: active

## Overview

## Journal Entries

### 2025-10-09
- 2hr implemented-responsive-navigation
```

**Activity files** get date-grouped entries:
```markdown
# p.c

## 2025-10-09
- 45min JSBach-WTC-I-Prelude-C

## 2025-10-08
- 30min Chopin-Ballade-1
```

## Cross-Platform Support

The collection system runs identically on macOS and Linux:

| Component | macOS | Linux |
|-----------|-------|-------|
| File watcher | LaunchAgent | systemd service |
| Backup collection | launchd calendar | cron |
| Shell | Nushell | Nushell |
| Watcher engine | Native Nushell `watch` | Native Nushell `watch` |

No external dependencies (fswatch, inotifywait, etc.) are required -- the watchers use Nushell's built-in `watch` command.
