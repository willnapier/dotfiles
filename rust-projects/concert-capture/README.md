# concert-capture

Extract concert data from Wigmore Hall HTML snapshots and log them to DayPages.

## What It Does

1. **Parses** SingleFile HTML snapshots of Wigmore Hall concert pages
2. **Extracts** performers and programme (works with composers)
3. **Generates** standardized work notation (e.g., `Mozart-K465`, `Handel-HWV6`)
4. **Appends** a `concert::` entry to the appropriate DayPage
5. **Archives** the HTML to `~/Captures/concerts/`

## Installation

```bash
cd ~/dotfiles/rust-projects/concert-capture
cargo build --release
```

The binary is symlinked to `~/.local/bin/concert-capture` via dotter.

## Usage

```bash
# Process a specific HTML file
concert-capture ~/Downloads/wigmore-concert.html

# Find and process the latest concert HTML in Downloads or web-archives
concert-capture --latest

# Preview what would happen (no file changes)
concert-capture --dry-run ~/Downloads/wigmore-concert.html

# Skip Open Opus API lookup (offline mode)
concert-capture --no-api ~/Downloads/wigmore-concert.html

# Output only the wikilink (for editor integration)
concert-capture --link-only ~/Downloads/wigmore-concert.html

# List recent archives
concert-capture list
```

## Helix Integration

Press `Space+D` to:
1. Find the latest concert HTML in `~/Downloads/` **or** `~/Captures/web-archives/`
2. Archive it to `~/Captures/concerts/`
3. Insert the full `concert.{venue}:: …` line at the cursor

The keybinding calls `hx-concert-capture`, which runs `concert-capture --latest --entry-only`.

`ai-export-watcher` moves ordinary SingleFile saves from Downloads to `web-archives` within a second. `--latest` therefore searches both places, newest first, skips non-venue HTML, and ignores files older than **3 days** (so leftover venue pages in web-archives cannot steal the paste). After SingleFile, just `Space+D` — do not use `Space+C`.

## Output Format

The tool generates entries like:

```
concert:: JerusalemQuartet Mozart-K465 Ran-string-quartet-no-4 Ravel-string-quartet-in-f [[captures/concerts/2026-01-21-jerusalem-quartet.html]]
```

Format breakdown:
- `concert::` - Entry type marker
- `JerusalemQuartet` - Performers (PascalCase, roles stripped)
- `Mozart-K465` - Works in `Composer-Catalog` notation
- `[[captures/concerts/...]]` - Wikilink to archived HTML

## Work Notation

Follows the music-work-notation-system spec:

| Catalog Type | Example Input | Output |
|--------------|---------------|--------|
| Mozart K | "String Quartet K465" | `Mozart-K465` |
| Bach BWV | "Well-Tempered Clavier BWV 846" | `JSBach-BWV846` |
| Handel HWV | "Agrippina HWV6" | `Handel-HWV6` |
| Haydn Hob | "Piano Sonata HXVI/49" | `Haydn-HobXVI49` |
| Opus | "Sonata Op. 27 No. 2" | `Beethoven-Op27No2` |
| Vivaldi RV | "Gloria RV 589" | `Vivaldi-RV589` |
| Schubert D | "Quartet D 810" | `Schubert-D810` |
| Liszt S | "Années de pèlerinage S163" | `Liszt-S163` |

Works without catalog numbers fall back to slugified titles.

## How It Works

### HTML Parsing (`html.rs`)

Extracts data from Wigmore Hall's HTML structure:

- **Date**: From URL pattern `wigmore-hall.org.uk/whats-on/YYYYMMDDHHMM`
- **Performers**: From `.performance-title` elements, split on semicolons
- **Works**: From `.repertoire-work-item` containers
  - Composer: `<a href="/artists/...">` links
  - Title: `.rich-text.inline.bold` elements

### Notation Generation (`notation.rs`)

1. Converts composer names to tags (e.g., "Johann Sebastian Bach" → "JSBach")
2. Extracts catalog numbers from work titles using regex patterns
3. Falls back to slugified titles when no catalog found

### DayPage Integration (`daypage.rs`)

- Finds DayPage at `~/Forge/NapierianLogs/DayPages/YYYY-MM-DD.md`
- Inserts entry before `## Backlinks` section (or appends at end)
- Creates minimal DayPage if it doesn't exist

### Open Opus API (`api.rs`)

Optional lookup for canonical work information:
- Searches by composer surname
- Matches works by simplified title
- Extracts catalog/key info from API response

Use `--no-api` to skip (faster, works offline).

## File Locations

| Purpose | Path |
|---------|------|
| Source code | `~/dotfiles/rust-projects/concert-capture/` |
| Binary | `~/.local/bin/concert-capture` |
| Helix wrapper | `~/.local/bin/hx-concert-capture` |
| Archives | `~/Captures/concerts/` |
| DayPages | `~/Forge/NapierianLogs/DayPages/` |

## Workflow

1. Save the venue concert page with the SingleFile browser extension
2. File lands in `~/Downloads/` as `YYYY-MM-DD-Title.html`
3. `ai-export-watcher` may immediately move it to `~/Captures/web-archives/` (generic clip route)
4. In Helix, press `Space+D` — it finds the newest venue HTML in either folder, archives it to `~/Captures/concerts/YYYY-MM-DD-performer.html`, and pastes the `concert.{venue}::` line at the cursor
5. Optional: type a reaction under the line; `collect-entries` accrues it to `concert.wigmore.md` (etc.)

CLI `concert-capture --latest` (no `--entry-only`) instead queues the line onto the **concert date's** DayPage via `daypage-append` — flush with `Space+U`. That is not the Helix paste path.

## Dependencies

- `scraper` - HTML parsing
- `reqwest` - HTTP client for Open Opus API
- `clap` - CLI argument parsing
- `chrono` - Date handling
- `regex` - Pattern matching for catalog numbers
- `slug` - URL-safe string conversion
