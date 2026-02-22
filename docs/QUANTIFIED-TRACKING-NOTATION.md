# Quantified Tracking Notation

A notation specification for embedded activity tracking in daily markdown notes. Activities, durations, and events are recorded inline using a structured `key:: value` syntax that is simultaneously human-readable journal text and machine-parseable data.

## Core Syntax

```
key:: duration details
```

Every tracking entry starts with an activity key, the `::` reification boundary, and then space-separated attributes.

### The `::` Reification Boundary

The double-colon `::` is the defining syntactic element. It marks the boundary between **what is being tracked** (the key) and **the tracking data** (the value). This was chosen because:

1. It doesn't conflict with YAML frontmatter (single `:`) or markdown syntax
2. It's visually distinctive in plain text
3. It enables reliable regex extraction: `^key:: (.*)$`
4. It naturally reads as "key is-recorded-as value"

## Duration Formats

Durations use Nushell-native format for direct parsing:

| Format | Meaning | Example |
|--------|---------|---------|
| `30min` | 30 minutes | `piano:: 30min scales` |
| `1hr` | 1 hour | `dev:: 1hr rust-project` |
| `2hr30min` | 2.5 hours | `dev:: 2hr30min deep-focus` |
| `1430-1500` | Time span (30min) | `ex.walk:: 1430-1500 reservoir` |
| `0900-1145` | Time span (2hr45min) | `dev:: 0900-1145 writing` |

Time spans in `HHMM-HHMM` format are automatically expanded to include the computed duration:

```
# Input (what you write):
ex.walk:: 1430-1500 reservoir

# After processing (what appears in the file):
ex.walk:: 30min 1430-1500 reservoir
```

This processing is triggered on save by a file watcher or the `fdur` command.

## Activity Keys

Long-form descriptive keys are preferred for readability:

```
piano:: 45min piano-practice
sleep:: 7hr 2330-0630
read:: 1hr reading
dev:: 2hr coding
diet:: shake-protein-powder-50g-chia-seeds-100g
ex.walk:: 4k-steps reservoir-short
clinical-notes:: 12x 30min across day
r:: next Tuesday: Call dentist           # reminder (see below)
```

### Hierarchical Dot Notation

Sub-categories use dot notation to create a parent-child hierarchy:

```
piano.c:: 45min JSBach-WTC-I-Prelude-C   # piano → classical
piano.j:: 30min Monk-Blue-Monk           # piano → jazz
ex.walk:: 4k-steps reservoir-short       # exercise → walk
ex.hiit:: 16min-1+1x8                    # exercise → HIIT
ex.res:: deadlift-10+10+bar-10x3         # exercise → resistance
ex.walkpad:: 50min 5.5k-steps            # exercise → walkpad
dev.rust:: 2hr forge-graph               # dev → rust
dev.nu:: 1hr watcher-refactor            # dev → nushell
```

Each sub-activity automatically links to its parent. The file `piano.c.md` contains a reference to `piano.md`.

### Social Activity Keys

Activities involving other people use the person's name as the key:

```
jenny:: 1.5hr clearing-out-garage
jenny.tv:: 2hr SlowHorses
jenny.qt:: lovely
mum:: 1hr phone-call
```

Social keys support the same dot notation for sub-activities:

```
jenny.tv:: 1hr SlowHorses               # jenny → watching TV
jenny.qt:: lovely morning                # jenny → quality time
```

## Type Markers

Special attribute types are distinguished by format:

| Type | Format | Example |
|------|--------|---------|
| Duration | `Nmin` or `Nhr` | `30min`, `2hr` |
| Time span | `HHMM-HHMM` | `1430-1500` |
| Currency | `£N` or `$N` | `£25`, `$40` |
| Distance | `Nkm` or `Nmi` | `5km`, `3mi` |
| Steps | `Nk-steps` | `4k-steps`, `5.5k-steps` |
| Location | Descriptive | `reservoir`, `home`, `gym` |

## Activity vs. State

The notation distinguishes between **activities** (things you do) and **states** (conditions you're in):

### Activities (Verb-like)

```
piano:: 45min piano-practice
dev:: 2hr coding
read:: 1hr reading-Kahneman
```

Activities have a duration and involve intentional action.

### Reminders

```
r:: in 3 days: Call dentist
r:: next Tuesday: Follow up with Leigh
r:: 2026-03-15: Jenny birthday party
```

Reminder entries use the `r::` key with format `r:: <when>: <message>`. The `<when>` part is a natural language date expression parsed by Nushell's `date from-human`. `collect-entries` routes these to `~/Forge/NapierianLogs/Reminders/<target-date>.md`.

The `remind` CLI wraps this: `remind "Call dentist" in 3 days` queues `r:: in 3 days: Call dentist` via `daypage-append`.

### States (Adjective-like)

```
sleep:: 7hr30min 2300-0630
state:: mixed anxiety-6 engaged-6 tired-5
health:: pulled-muscle-neck-shoulders
```

States describe conditions rather than actions. Sleep is tracked with both duration and time span.

## Nested Activities

Activities can be nested when two things happen simultaneously:

```
jenny:: 3hr dinner restaurant-name
  # Nested within the jenny time:
  read:: 30min menu-discussion
```

Or expressed on a single line with multiple keys:

```
ex.walk:: 45min reservoir read:: 45min reading-on-walk
```

## Parsing Patterns

### Regex Extraction

Extract all entries from a DayPage:

```
^([a-zA-Z][a-zA-Z0-9._-]*):: (.+)$
```

- Group 1: Activity key (e.g., `piano.c`, `dev.rust`, `jenny.tv`)
- Group 2: Attributes (duration, description, etc.)

### Duration Extraction

Extract duration from attributes:

```
(\d+hr)?(\d+min)?
```

### Time Span Extraction

Extract and compute time spans:

```
(\d{4})-(\d{4})
```

Parse as `HHMM` to compute duration: `end_minutes - start_minutes`.

### Currency Extraction

```
[£$€][\d.]+
```

## Collection and Aggregation

Entries written in daily journal files are automatically collected into per-key files:

```
~/notes/logs/piano.c.md    # All piano-classical entries, grouped by date
~/notes/logs/dev.rust.md   # All Rust development entries
~/notes/logs/jenny.md      # All jenny activity entries
```

Each file contains entries grouped by date, enabling:
- **Duration totals** per activity per day/week/month
- **Trend analysis** over time
- **Cross-referencing** with other activities on the same day

### Aggregation Queries (Nushell)

```nushell
# Total piano practice this week
open ~/notes/logs/piano.md | lines | where {|l| $l starts-with "- "} | length

# Average daily dev time this month
# (handled by dedicated Nushell functions)
```

## Design Decisions

**Why not YAML frontmatter?**
- Entries are part of the journal narrative, not metadata
- Multiple entries per day require inline notation
- YAML would separate the data from the context

**Why `::` instead of other delimiters?**
- `:` conflicts with YAML and time notation (`14:30`)
- `->` is ambiguous with markdown
- `=` conflicts with common text patterns
- `::` is visually distinctive and rarely appears in prose

**Why Nushell-native durations?**
- `30min` parses directly in Nushell as a duration type
- No conversion needed for aggregation
- Consistent with Nushell's data-first philosophy

**Why hierarchical dot notation?**
- Natural parent-child relationships (`piano.c` is a child of `piano`)
- Aggregation at any level (sum all `piano.*` for total piano time)
- File system mapping (`piano.c.md` is a real file)
- Extensible without schema changes (add `piano.c.bach` when needed)

**Why long-form keys?**
- `piano::` is immediately legible; `p::` requires a lookup table
- Consistent with the DayPage-as-journal philosophy — entries should read naturally
- Dot notation still provides brevity for sub-categories (`ex.hiit::` not `exercise.high-intensity-interval-training::`)
