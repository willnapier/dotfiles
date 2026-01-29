# Universal Tool Architecture -- Editor-Neutral Design

## Overview

This document describes the universal tool architecture: 12 Nushell functions organized into three domains (Forge, Global, Citations) with consistent naming conventions and cross-platform compatibility.

Functions work identically across different editors (Helix, Neovim, Vim, Emacs, VS Code), different platforms (macOS, Linux), and different contexts (local, SSH, remote).

---

## Architecture

### Design Philosophy

**Split Paradigm Approach**:
- **Forge functions** (knowledge base): Use `e` suffix = opens in `$env.EDITOR`
- **Global functions** (any directory): Use `o` suffix = opens with system-appropriate application
- **Link functions** (clipboard): Use `l` suffix = editor-neutral, copies wiki links

This split recognizes that:
- Knowledge base files are always markdown -- editing is the primary use case
- General files can be any type (PDF, Excel, images) -- system viewer is appropriate

### Naming Convention: `[scope][search-type][output]`

**Scope Prefixes**:
- `f` = **Forge** (searches knowledge base at `$env.FORGE`)
- `g` = **Global** (searches current directory/context)
- `c` = **Citations** (academic literature workflow)

**Search Types**:
- `s` = **file Search** (by filename)
- `c` = **Content search** (by file contents with ripgrep)
- `sm` = **SeMantic search** (AI-powered relevance via OpenAI embeddings)
- `i` = **cItation** (academic references)
- `iz` = **cItation + Zotero** (PDF/reference manager integration)

**Output Destinations**:
- `l` = **Link** (copies `[[wikilink]]` to clipboard)
- `e` = **Editor** (opens in `$env.EDITOR`, fallback to `vi`)
- `o` = **Open** (platform-aware: `open` on macOS, `xdg-open` on Linux)
- `z` = **Zotero** (opens PDF in Zotero or system PDF viewer)

---

## Complete Function Reference

### Global Functions (2)

Work from any directory, search current context, open with system-appropriate application.

#### `gso` -- Global file Search, Open

Find files by name in current directory, open with system default application.

```nushell
def gso [] {
    let file = (fd . --type f --hidden --exclude .git --exclude Library/CloudStorage/Dropbox | sk ...)
    if (sys | get host.name) == "Darwin" {
        open $file  # macOS
    } else {
        xdg-open $file  # Linux
    }
}
```

#### `gco` -- Global Content search, Open

Find files by content in current directory, open with system default application.

```nushell
def gco [] {
    let query = (input "Search content: ")
    let results = (rg -i -l $query . --glob '!Library/CloudStorage/Dropbox/**' | lines)
    let selected = ($results | sk --preview "rg --color=always -i -C 3 '$query' {}")
    if (sys | get host.name) == "Darwin" {
        open $selected
    } else {
        xdg-open $selected
    }
}
```

---

### Forge Functions (6)

Work anywhere (universal), search knowledge base, behavior depends on suffix.

#### File Search Functions

**`fse` -- Forge file Search, Editor**

Find markdown files by name in Forge, open in configured editor.

```nushell
def fse [] {
    let file = (fd . $env.FORGE --type f --hidden --exclude .git | sk ...)
    let editor = (if ($env.EDITOR? | is-empty) { "vi" } else { $env.EDITOR })
    ^$editor $file
}
```

Editor compatibility:
- `export EDITOR=hx` (Helix)
- `export EDITOR=nvim` (Neovim)
- `export EDITOR=vim` (Vim)
- `export EDITOR=emacs` (Emacs)
- `export EDITOR="code --wait"` (VS Code)
- Fallback: `vi` if `$env.EDITOR` not set

**`fsl` -- Forge file Search, Link**

Find markdown files by name in Forge, copy wiki link to clipboard.

```nushell
def fsl [] {
    let file = (fd . $env.FORGE --type f --extension md | sk ...)
    let filename = ($file | path basename | str replace ".md" "")
    let wikilink = $"[[($filename)]]"
    $wikilink | pbcopy
}
```

#### Content Search Functions

**`fce` -- Forge Content search, Editor**

Search file contents in Forge with ripgrep, open match in editor.

```nushell
def fce [] {
    let query = (input "Search content: ")
    let results = (rg -i --type md -l $query $env.FORGE | lines)
    let selected = ($results | sk --preview "rg --color=always -i -C 3 '$query' {}")
    let editor = (if ($env.EDITOR? | is-empty) { "vi" } else { $env.EDITOR })
    ^$editor $selected
}
```

**`fcl` -- Forge Content search, Link**

Search file contents in Forge, copy wiki link to match.

```nushell
def fcl [] {
    let query = (input "Search content: ")
    let results = (rg -i --type md -l $query $env.FORGE | lines)
    let selected = ($results | sk --preview "rg --color=always -i -C 3 '$query' {}")
    let filename = ($selected | path basename | str replace ".md" "")
    let wikilink = $"[[($filename)]]"
    $wikilink | pbcopy
}
```

#### Semantic Search Functions

**`fsme` -- Forge SeMantic search, Editor**

AI-powered relevance search in Forge, open most relevant note in editor.

Requirements:
- `semantic-indexer` configured
- `$env.OPENAI_API_KEY` set
- Vault indexed with `semantic-indexer`

```nushell
def fsme [] {
    let query = (input "Search concept: ")
    let results = (semantic-query --text $query --limit 20 | lines)
    let selected = ($results | sk ...)
    let filename = # extract from semantic result
    let filepath = (fd -t f --full-path "$filename.md" $env.FORGE | head -1)
    let editor = (if ($env.EDITOR? | is-empty) { "vi" } else { $env.EDITOR })
    ^$editor $filepath
}
```

**`fsml` -- Forge SeMantic search, Link**

AI-powered relevance search, copy wiki link to most relevant note.

---

### Citation Functions (4)

Academic research workflow with BibTeX/Zotero integration and literature note management.

#### `cit` -- CITation plain text

Select citation from bibliography, copy plain text key to clipboard.

Citation format: `AuthorYear Title` (e.g., `Zamoyski2009 Poland: A History`)

#### `cil` -- CItation, Literature note Link

Select citation, copy wiki link to literature note file.

#### `ciz` -- CItation, Zotero/PDF

Select citation, open PDF in Zotero or system PDF viewer.

```nushell
def ciz [] {
    let selected = # citation picker
    let zotero_key = # extract from BibTeX library.bib
    let pdf_path = # extract file path from BibTeX

    if (sys | get host.name) == "Darwin" {
        open "zotero://select/items/@$zotero_key"
    } else {
        xdg-open "zotero://select/items/@$zotero_key"
    }
}
```

#### `cizl` -- CItation Zotero, Link

Select citation, copy markdown link with Zotero protocol.

Output format: `[Poland: A History](zotero://select/items/@1_ABC123XYZ)`

---

## Technical Implementation Details

### Editor Neutrality Pattern

All Forge `*e` functions use this pattern:

```nushell
let editor = (if ($env.EDITOR? | is-empty) { "vi" } else { $env.EDITOR })
^$editor $filepath
```

Rationale:
- Respects Unix `$EDITOR` convention
- Graceful fallback ensures it always works
- No hardcoded editor dependencies
- Single configuration point for users

### Platform Detection Pattern

All Global `*o` functions use this pattern:

```nushell
if (sys | get host.name) == "Darwin" {
    open $file  # macOS native command
} else {
    xdg-open $file  # Linux freedesktop standard
}
```

Behavior by file type:
- `.pdf` -- PDF viewer (Preview, Evince, Okular)
- `.xlsx` -- Spreadsheet app (Excel, LibreOffice Calc)
- `.jpg` -- Image viewer (Photos, Eye of GNOME, Gwenview)
- `.mp4` -- Video player (QuickTime, VLC, mpv)
- `.md` -- Default text editor (TextEdit, gedit, Kate)

### Clipboard Operations Pattern

All `*l` functions use cross-platform clipboard:

```nushell
# macOS
$wikilink | pbcopy

# Linux (future)
# $wikilink | xclip -selection clipboard   # X11
# $wikilink | wl-copy                      # Wayland
```

---

## Dependencies and Requirements

### Core Requirements

- **Nushell** 0.80+
- **`$env.FORGE`**: Path to knowledge base (required for `f*` functions)
- **`$env.EDITOR`**: Preferred text editor (optional, defaults to `vi`)

### Search Tools

- **fd**: Fast file discovery (`brew install fd` / `apt install fd-find`)
- **ripgrep**: Fast content searching (`brew install ripgrep` / `apt install ripgrep`)
- **skim**: Interactive fuzzy finder with preview (`brew install sk` / `cargo install skim`)

### Optional Dependencies

- **Semantic Search** (`fsm*`): `semantic-indexer`, `semantic-query`, `OPENAI_API_KEY`
- **Citation Management** (`c*`): Zotero, Better BibTeX plugin, `library.bib`, `citations.md`
- **Platform Commands**: `open`/`pbcopy` (macOS built-in), `xdg-open`/`xclip`/`wl-copy` (Linux)

---

## Cross-Platform Compatibility

### macOS

- File opening: `open` command (built-in, respects file associations)
- Clipboard: `pbcopy` / `pbpaste` (built-in)

### Linux

- File opening: `xdg-open` (freedesktop.org standard, respects desktop environment defaults)
- Clipboard: `xclip -selection clipboard` (X11) or `wl-copy` (Wayland)

### SSH and Remote Usage

All functions work identically over SSH provided:
- Nushell installed on remote system
- `$env.FORGE` set on remote system
- Same dependencies (fd, rg, sk) installed

Clipboard behavior over SSH copies to the **remote** clipboard. Use `*e` functions to edit directly instead of `*l` functions when remote.

---

## Migration from v1.0

### Renamed Functions

| Old Name | New Name | Description |
|----------|----------|-------------|
| `fsh` | `fse` | Forge Search, Editor |
| `fcsh` | `fce` | Forge Content, Editor |
| `fsel` | `fsml` | Forge SeMantic, Link |
| `fseh` | `fsme` | Forge SeMantic, Editor |
| `gsh` | `gso` | Global Search, Open |
| `gch` | `gco` | Global Content, Open |

Citation functions (`cit`, `cil`, `ciz`, `cizl`) are unchanged.

### Breaking Changes

1. **Editor dependency removed**: Hardcoded `hx` replaced with `$env.EDITOR`. Set `export EDITOR=hx` to maintain Helix behavior.
2. **Global functions behavior change**: `gso`/`gco` open with system defaults (not editor). Use `fse`/`fce` for editor-opening behavior.

---

## Performance

### Search Speed

- **File search** (`fse`, `fsl`, `gso`): Uses `fd` (Rust-based, parallel). ~10-100x faster than `find`. Handles 10,000+ files instantly.
- **Content search** (`fce`, `fcl`, `gco`): Uses `ripgrep` (Rust-based, parallel). ~5-50x faster than `grep`.
- **Semantic search** (`fsme`, `fsml`): Depends on OpenAI API latency (~1-2 seconds). Pre-indexed vault for faster results.

### Scalability

Tested with 6,400 files. All functions work instantly. Content search across entire vault completes in under 1 second.

---

## Error Handling

### Missing Dependencies

Functions detect missing tools and provide platform-specific install commands:
```
fd and sk are required. Install with: brew install fd sk
```

### Missing Environment Variables

- `$env.FORGE` not set: Error message displayed
- `$env.EDITOR` not set: Graceful fallback to `vi`
- `$env.OPENAI_API_KEY` not set: Error for semantic search only

### Empty Results

Clear error messages for no matches, no citations, or file-not-found conditions.

---

## Troubleshooting

| Symptom | Solution |
|---------|----------|
| "Command not found: fse" | `source ~/.config/nushell/config.nu` or restart terminal |
| "FORGE not set" | `export FORGE=/path/to/vault` in env.nu or shell rc |
| "No matches found" (but files exist) | Verify query, check `$env.FORGE` path, test `fd . $env.FORGE` |
| Semantic search fails | Check `$env.OPENAI_API_KEY`, verify `semantic-indexer` installed |
| Wrong editor opens | Check `$env.EDITOR`, update with `export EDITOR=preferred_editor` |

---

## Design Comparison

| Approach | Pros | Cons |
|----------|------|------|
| GUI apps (Obsidian, Notion) | Visual, WYSIWYG | Requires specific app, SSH-incompatible |
| Keybinding automation | Fast muscle memory | Machine-specific, doesn't work over SSH |
| Editor plugins | Deep integration | Editor-specific, separate plugin per editor |
| **Universal Functions** | Works everywhere, editor-agnostic, SSH-compatible | Requires command-line comfort |

Keybindings and universal functions combine well:
```bash
# Keybindings that call functions
bind Alt+c = "fse"
bind Alt+Shift+c = "cil"
bind Alt+s = "fsml"
```

This gives muscle memory locally and universal functions over SSH, with no logic duplication.
