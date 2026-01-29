# AI-Powered Activity Classification System

Automatic semantic tagging for social interactions and activities.

---

## Overview

### The Problem

Activity tracking via `collect-entries` creates files in `~/notes/logs/` for all activities (social interactions, exercise, creative work, etc.). These files exist in a flat structure with no inherent categorization, making it difficult to:
- Find all social/professional contacts from a conference
- Distinguish people interactions from activity tracking
- Query by activity type without complex filename patterns

### The Solution

Automatic semantic classification using OpenAI API to analyze file contents and add appropriate tags. The system:
- Runs automatically on linux-desktop after `collect-entries`
- Classifies once per new file (skips already-tagged files)
- Adds YAML tags in canonical format (one per line)
- Enables discovery via semantic queries, not filename parsing
- Prevents race conditions by running only on linux-desktop

### Key Innovation

Rather than creating hierarchical folder structures or complex routing rules, the flat structure is preserved with **semantic metadata** that enables rich discovery. Simple structure + smart queries > complex hierarchy.

---

## Design Principles

### Parsimony Over Complexity

Following the "delete before automate" principle:
- Do not create `~/notes/logs/Social/`, `/Professional/`, `/Family/` subdirectories
- Do create a flat structure with semantic tags
- This eliminates routing rules, prevents category edge cases, and maintains simplicity

### Source-Agnostic Classification

The system does not rely on filename patterns or entry notation:
- `partner::` -- AI determines: `[social, family]`
- `partner.tv::` -- AI determines: `[social, entertainment]` or `[entertainment]`
- `alex.chen::` -- AI analyzes content: `[social, professional]`

This handles ambiguity, adapts to natural writing, and avoids brittle pattern matching.

### Single Source of Truth

Classification runs **only on linux-desktop**:
- **Problem**: Two machines both modifying files creates merge conflicts
- **Solution**: linux-desktop classifies, other machines receive finished files
- **Benefit**: Zero race conditions, guaranteed consistency

### Cost-Efficient AI Usage

One API call per new file, then cached forever:
- First time: New file -- classify -- add tags -- $0.001
- Subsequent: Tags exist -- skip -- $0.00
- Annual cost: ~150 new files/year x $0.001 = **$0.15/year**

### Canonical YAML Format

```yaml
---
tags:
- social
- professional
---
```

Standard YAML compliance, better readability, easier parsing, consistent with literature note frontmatter.

---

## Technical Architecture

### Component Overview

```
+---------------------------------------------------------+
|                   linux-desktop (Source)                  |
|  +--------------------------------------------------+   |
|  |  DayPages/*.md (daily notes)                      |   |
|  |  - alex.chen:: 45min AI safety discussion         |   |
|  |  - partner:: evening call                         |   |
|  |  - ex.walk:: 35min 3546steps                      |   |
|  +-------------------+------------------------------+   |
|                      |                                   |
|                      v                                   |
|  +--------------------------------------------------+   |
|  |  collect-entries (automatic/cron)                 |   |
|  |  - Scans DayPages for activity:: patterns         |   |
|  |  - Creates/updates logs/*.md files                |   |
|  +-------------------+------------------------------+   |
|                      |                                   |
|                      v                                   |
|  +--------------------------------------------------+   |
|  |  auto-tag-activities (integrated)                 |   |
|  |  - Finds new files (no "tags:" in frontmatter)    |   |
|  |  - Calls classify-activity for each new file      |   |
|  |  - Skips already-tagged files                     |   |
|  +-------------------+------------------------------+   |
|                      |                                   |
|                      v                                   |
|  +--------------------------------------------------+   |
|  |  classify-activity (OpenAI API)                   |   |
|  |  - Sends file content to GPT-4o-mini              |   |
|  |  - Returns 1-3 semantic tags                      |   |
|  |  - Cost: ~$0.001 per file                         |   |
|  +-------------------+------------------------------+   |
|                      |                                   |
|                      v                                   |
|  +--------------------------------------------------+   |
|  |  Tagged files in logs/                            |   |
|  |  ~/notes/logs/alex.chen.md                        |   |
|  |  ---                                              |   |
|  |  tags:                                            |   |
|  |  - social                                         |   |
|  |  - professional                                   |   |
|  |  ---                                              |   |
|  +-------------------+------------------------------+   |
|                      |                                   |
|                      v                                   |
|              Git push to GitHub                          |
+---------------------------------------------------------+
                       |
                       | Bidirectional Sync
                       v
+---------------------------------------------------------+
|                    Mac (Read-Only)                        |
|  +--------------------------------------------------+   |
|  |  Git pull from GitHub                             |   |
|  |  - Receives fully classified files                |   |
|  |  - Never modifies activity tags                   |   |
|  +-------------------+------------------------------+   |
|                      |                                   |
|                      v                                   |
|  +--------------------------------------------------+   |
|  |  Discovery Commands (Nushell)                     |   |
|  |  - social-list                                    |   |
|  |  - social-recent 7                                |   |
|  |  - social-search "AI safety"                      |   |
|  +--------------------------------------------------+   |
+---------------------------------------------------------+
```

### Scripts and Components

**Core Scripts** (`~/dotfiles/scripts/`):

1. **`classify-activity`** -- Single file classifier
   - Input: Activity file path
   - Process: Sends content to OpenAI GPT-4o-mini
   - Output: JSON with tags, confidence, reasoning
   - Cost: ~$0.001 per call

2. **`auto-tag-activities`** -- Batch classifier
   - Input: Scans all activity log files
   - Process: Skips already-tagged, classifies new files
   - Output: Adds tags to frontmatter in canonical YAML format

3. **`collect-entries`** -- Modified to integrate classification
   - Automatically calls `auto-tag-activities` after collection
   - Only if OPENAI_API_KEY is set and not dry-run
   - Classification failure does not break collection

**Discovery Functions** (Nushell config):

1. **`social-list`** -- List all files with "social" tag
2. **`social-recent [days]`** -- Recent social interactions (default 7 days)
3. **`social-search [query]`** -- Search social interaction file contents

---

## Workflow and Automation

### Fully Automatic Pipeline

```
collect-entries
  |-- Scans DayPages for entries
  |-- Collects explicit activities (activity::)
  |-- Collects mentions (activity without ::, social-tagged only)
  |-- Creates/updates activity files
  |-- Detects new files (no tags in frontmatter)
  |-- Calls auto-tag-activities
  |     |-- classify-activity file1 -> [social, professional]
  |     |-- classify-activity file2 -> [exercise]
  |     +-- classify-activity file3 -> [social, family]
  |-- Adds tags to frontmatter
  +-- Git push syncs to Mac
```

### Manual Classification

```bash
# Classify single file
classify-activity ~/notes/logs/alex.chen.md --verbose

# Batch classify (dry-run to preview)
auto-tag-activities --dry-run

# Batch classify (apply tags)
auto-tag-activities --verbose
```

---

## Tag Categories

### Available Tags

**Social Categories**:
- `social` -- Any interaction with people (umbrella category)
- `professional` -- Work-related, academic, career networking
- `family` -- Spouse, children, relatives, family gatherings
- `friends` -- Personal friendships, social gatherings

**Activity Categories**:
- `exercise` -- Physical activity, walking, sports, fitness
- `development` -- Programming, system configuration, technical learning
- `creative` -- Music, art, writing, creative pursuits
- `entertainment` -- TV, movies, games, passive consumption
- `health` -- Medical, mental health, therapy, health tracking
- `learning` -- Reading, studying, courses (non-technical)
- `admin` -- Life administration, errands, household tasks

### Multi-Tag Examples

**Professional Contact**:
```yaml
---
tags:
- social
- professional
---
# alex.chen

## Journal Entries
### 2025-11-15
- 45min discussion about AI safety research
```

**Family Entertainment**:
```yaml
---
tags:
- social
- family
- entertainment
---
# partner.tv

## Journal Entries
### 2025-11-09
- Rye Lane 1hr 10min funny clever unusual
```

**Solo Creative Work**:
```yaml
---
tags:
- creative
---
# piano.cl

## Journal Entries
### 2025-11-10
- 45min JSBach-WTC-I-Prelude-C
```

### Tag Selection Rules

- 1-3 tags maximum (avoid over-tagging)
- Most specific applicable tags (e.g., if social + professional applies, include both)
- Semantic analysis (reads actual content, not just filename)
- Conservative approach (prefers clear categories over speculation)

---

## Mention Collection

### What Are Mentions?

In addition to explicit activity entries (with `::`), the system also collects **mentions** -- prose references to people in daily notes.

**Explicit entry** (quantitative interaction):
```markdown
alex.chen:: 45min discussion about AI safety research
```

**Mention** (qualitative reference):
```markdown
Later I mentioned alex.chen to Sarah as potential collaborator
```

Both are collected to `alex.chen.md` in chronological order.

### How Mention Detection Works

**Pattern matching:**
- Detects lowercase dot-separated patterns: `[a-z][a-z0-9.]+`
- Must NOT be followed by `::` (that would be an explicit entry)
- Example matches: `alex.chen`, `mary.anne.jones`

**Social filtering:**
- Only collects mentions for files tagged with `social`
- If file does not exist yet or is not social-tagged, mention is skipped

**Why this works:**
- People names naturally use dots: `alex.chen`, `mary.anne.jones`
- Activity prose uses natural language: "piano practice", "walking" (no dots)
- Activity dot notation (`piano.jz::`) is only used with `::` for explicit tracking

### Mention Format

Mentions are prefixed with `"- Mention: "` to distinguish from quantitative entries:

```markdown
## Journal Entries

### 2025-11-16
- 45min discussion about AI safety research
- Mention: Later I mentioned alex.chen to Sarah as potential collaborator
- 30min follow-up call
- Mention: Suggested alex.chen for review panel
```

Quantitative aggregation scripts filter out mention lines to avoid skewing duration averages.

---

## Sub-Activity List Generation

### The Problem

Activity files often have hierarchical relationships:
- `partner.md` (parent) has sub-activities `partner.tv.md` and `partner.conv.md` (children)
- `piano.md` (parent) has sub-activities `piano.jz.md` and `piano.tech.md` (children)
- `travel.md` (parent) has sub-activities `travel.train.md`, `travel.plane.md`, `travel.bus.md` (children)

### The Solution

Automatic bidirectional relationship management:
1. **Child files** declare their parent using `**Parent**: [[parent-name]]`
2. **Parent files** use `{Auto-generated}` placeholder in their `## Sub-activities` section
3. **System automatically** scans all files, builds parent-to-children mappings, and populates placeholders

### How It Works

**In child files** (e.g., `partner.tv.md`):
```markdown
---
tags:
- social
- entertainment
---
# partner.tv

**Parent**: [[partner]]

## Journal Entries
### 2025-11-10
- 45min watched documentary together
```

**In parent files** (e.g., `partner.md`):
```markdown
## Sub-activities

{Auto-generated}
```

After `collect-entries` runs, the placeholder is replaced:
```markdown
## Sub-activities

- [[partner.conv]]
- [[partner.tv]]
```

### Design Properties

- **Idempotent**: Running multiple times produces the same result
- **Graceful degradation**: Skips files without the placeholder; never crashes the pipeline
- **Single source of truth**: Child files are authoritative for parent declarations
- **Alphabetical sorting**: Sub-activities are always sorted consistently

---

## Discovery and Querying

### Basic Commands

```nushell
# List all social interactions
social-list

# Recent social interactions (last 7 days by default)
social-recent 7
social-recent 30

# Search social interaction contents
social-search "AI safety"
social-search "conference"
```

### Advanced Querying

```nushell
# All professional social contacts
ls ~/notes/logs/*.md
| each { |f| open $f }
| where { |c| ($c | str contains "- social") and ($c | str contains "- professional") }
```

---

## Cost Analysis

### Per-File Cost

- Model: GPT-4o-mini
- Input: ~500 tokens (file content + prompt)
- Output: ~50 tokens (JSON response)
- Cost: ~$0.001 per file
- Only new files incur costs; already-tagged files are free

### Real-World Scenarios

| Scenario | New Files | Cost |
|----------|-----------|------|
| 3-day conference | ~35 | $0.035 |
| Typical year | ~150 | $0.15 |
| Heavy usage year | ~300 | $0.30 |

---

## Race Condition Prevention

### The Problem

If both linux-desktop and Mac run classification:

```
linux-desktop: Updates alex.chen.md (adds journal entry)
Mac:           Updates alex.chen.md (adds tags to frontmatter)
               -> Git sync -> MERGE CONFLICT
```

### The Solution: Single Source of Truth

**Only linux-desktop classifies:**

```
linux-desktop:
  collect-entries -> auto-tag-activities -> git push
  (Sequential, atomic operation)

Mac:
  git pull <- receives fully classified files
  (Read-only for tags)
```

Enforcement mechanisms:
1. Classification is integrated into `collect-entries` on linux-desktop
2. No manual intervention on Mac needed
3. OPENAI_API_KEY is only set on linux-desktop
4. Mac is documented as read-only for tags

---

## Classification Prompt

The system sends this prompt to OpenAI:

```
Analyze this activity journal entry and assign appropriate tags.
Return 1-3 single-word tags.

Available tags:
- social: Any interaction with people
- professional: Work-related, academic, career networking
- family: Spouse, children, relatives, family gatherings
- friends: Personal friendships, social gatherings
- exercise: Physical activity, walking, sports, fitness
- development: Programming, system configuration, technical learning
- creative: Music, art, writing, creative pursuits
- entertainment: TV, movies, games, passive consumption
- health: Medical, mental health, therapy, health tracking
- learning: Reading, studying, courses
- admin: Life administration, errands, household tasks

[file content]

Return ONLY a JSON object:
{"tags": ["tag1", "tag2"], "confidence": 0.95, "reasoning": "brief explanation"}

Rules:
- Use 1-3 tags maximum
- Tags must be from the available list above
- Be specific: if social AND professional, include both
- Be concise in reasoning
```

### API Response Example

```json
{
  "tags": ["social", "professional"],
  "confidence": 0.95,
  "reasoning": "Professional networking conversation about research at academic institution"
}
```

### Frontmatter Handling

Scenarios handled:
1. **No frontmatter**: Create new frontmatter with tags
2. **Frontmatter exists, no tags**: Add tags section
3. **Frontmatter with tags**: Merge new tags (deduplicate, sort)
4. **Multiple formats**: Handle both list and inline formats, normalize to canonical YAML

---

## Conference Use Case

### The Problem

At academic/professional conferences:
- Meet 20-50 new people over 2-3 days
- Multiple conversations per day
- Need to track who, when, what was discussed, follow-up needed
- Want to review all contacts afterward

### The AI-Powered Approach

**During conference -- just capture naturally:**
```markdown
# 2025-11-15.md

alex.chen:: 45min discussion about AI safety research, follow up re collaboration

priya.kumar:: coffee chat about implementation intentions, invited to visit lab

dinner.group:: 2hr panel discussion with James Morton and 5 others on self-perspective taking

networking.lunch:: brief intro to 10+ people, exchanged cards with Sarah Li (university NLP lab)
```

**Automatic processing (linux-desktop, background):**
1. `collect-entries` creates individual files
2. `auto-tag-activities` classifies each as `[social, professional]`
3. Files sync back to Mac with tags

**Post-conference review:**
```nushell
# Who did I meet?
social-recent 3

# Find the person who mentioned AI safety
social-search "AI safety"

# All professional contacts from conference
social-list | where { ... modified in last 3 days ... }
```

**Cost for entire conference**: $0.03 (30 files x $0.001)

---

## Integration with Existing Systems

### Quantified Tracking

The AI classification complements structured quantified tracking:

- **Quantified tracking**: Parses durations, aggregates time, tracks activity subtypes
- **AI classification**: Adds semantic categories (social, professional, creative) for discovery

They work together: quantified tracking answers "how much time on piano?", classification answers "show me all creative activities".

### Bidirectional Sync

Classification integrates with existing sync infrastructure:
- Auto-pull watchers on Mac pull classified files from GitHub
- Auto-push watchers on linux-desktop push classified files to GitHub
- Dotter sync watcher deploys script updates across machines
- No new sync mechanisms needed

---

## Troubleshooting

### Classification Not Running

**Symptom**: New files created but no tags added

**Check**:
```bash
# Is OPENAI_API_KEY set? (on linux-desktop)
echo $OPENAI_API_KEY

# Is classification integrated?
grep "auto-tag-activities" ~/dotfiles/scripts/collect-entries
```

**Solutions**:
1. Ensure OPENAI_API_KEY is set in Nushell environment on linux-desktop
2. Verify script is deployed: `ls -la ~/.local/bin/auto-tag-activities`
3. Run manually: `collect-entries --verbose`

### Wrong Tags Assigned

Edit frontmatter manually, or remove the tags section and re-run `auto-tag-activities` to re-classify.

### Discovery Commands Not Found

Verify functions are defined in Nushell config and that the config has been sourced:
```bash
which social-list
grep "social-list" ~/.config/nushell/config.nu
```
