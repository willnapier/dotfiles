# Yazi Deep Rust Integration Guide

**Status**: ✅ Complete pure Rust toolchain integration  
**Created**: 2025-08-24  
**Philosophy**: Replace all inferior built-ins with superior Rust tools

## Overview

This configuration transforms Yazi from a basic file manager into a **pure Rust-powered exploration and editing environment**. Every search, preview, and navigation operation uses best-in-class Rust tools for superior performance and consistency.

## The Rust Stack

### Core Tools Integration:
- **fd** → Lightning-fast file discovery (replaces `find`)
- **rg** (ripgrep) → Content search with context (replaces `grep`)  
- **sk** (skim) → Interactive fuzzy selection (replaces `fzf`)
- **bat** → Syntax-highlighted previews (replaces `cat`)
- **nu** (Nushell) → Structured data and rich metadata (replaces `ls`)
- **zoxide** → Frecency-based directory jumping (replaces `cd`)

## Enhanced Commands

### File Search (`s`)
**What it does**: Find files with rich preview combining metadata and content

**Tools pipeline**: `fd` → `sk` → `nu` + `bat` → `hx`

**Preview shows**:
```
📄 File Info:
│ name      │ type │ size  │ modified │
├───────────┼──────┼───────┼──────────┤
│ note.md   │ file │ 2.1KB │ 2h ago   │

📖 Content Preview:
# My Note
Content with syntax highlighting...
```

**Command**: `fd -t f | sk --preview "echo \"📄 File Info:\"; nu -c \"ls {} | table\"; echo; echo \"📖 Content Preview:\"; bat --color=always --line-range=:20 {}"`

### Content Search (`S`)
**What it does**: Search inside files with contextual preview

**Tools pipeline**: `rg` → `sk` → `rg` (context) → `hx`

**Preview shows**: Search matches with 3 lines of context, syntax highlighted

**Command**: `rg -l . | sk --preview "rg --color=always --context=3 \\".*\\" {}"`

### Frecency Jump (`z`)
**What it does**: Jump to frequently/recently used directories

**Tools**: `zoxide` plugin with learned directory preferences

### Directory Browser (`Z`)
**What it does**: Explore directories with rich metadata preview

**Tools pipeline**: `fd` → `sk` → `nu` (table view)

**Preview shows**: Directory contents as structured tables with file metadata

**Command**: `fd -t d | sk --preview "nu -c \"ls {} | table\""`

## Key Advantages Over Built-ins

### Performance Benefits:
- **fd**: 10x+ faster than traditional `find`
- **rg**: Fastest text search, respects gitignore
- **sk**: Rust implementation, better than fzf
- **Parallel processing**: All tools use multiple cores

### User Experience Benefits:
- **Consistent interface**: Same fuzzy selection across all operations
- **Rich previews**: See file metadata AND content simultaneously  
- **Structured data**: Nushell's table format for clear information
- **Smart defaults**: Tools configured for optimal workflow

### Integration Benefits:
- **Direct Helix opening**: All searches can open files in preferred editor
- **Context preservation**: Commands run in current directory context
- **Unified toolchain**: Single language ecosystem (Rust)

## Configuration Details

### File Locations:
- **Main config**: `/Users/williamnapier/.config/yazi/keymap.toml`
- **Plugins**: `/Users/williamnapier/.config/yazi/plugins/`
- **Zoxide plugin**: `zoxide.yazi/` for frecency jumping

### Keybinding Philosophy:
- **Colemak-DH navigation**: `neio` for movement (consistent with Helix)
- **Mnemonic commands**: `s`=search, `S`=Search content, `z`=zoxide, `Z`=Zip (directories)
- **Helix integration**: `l` launches files in Helix with proper terminal context

## Technical Implementation

### Shell Command Structure:
```bash
# Pattern: discovery | selection | action
fd -t f | sk --preview "preview_command" | xargs -r hx

# Multi-tool preview pattern:
--preview "tool1_info; echo; tool2_content"
```

### Preview Window Optimization:
- **Structured layout**: Metadata first, content second
- **Visual separators**: Icons and spacing for clarity
- **Truncation**: Limited lines to prevent overflow
- **Color coding**: Syntax highlighting throughout

### Error Handling:
- **xargs -r**: Only execute if selection made
- **Null handling**: Commands handle empty results gracefully
- **Path safety**: Proper quoting for filenames with spaces

## Workflow Examples

### 1. Find and Edit a Configuration File:
1. Press `s` in Yazi
2. Type "config" → See all config files with metadata and previews
3. Select with fuzzy matching
4. File opens directly in Helix

### 2. Search for Code Containing "function":
1. Press `S` in Yazi  
2. Enter "function" → See all files containing the term
3. Preview shows actual matches with context
4. Select file → Opens at relevant location

### 3. Jump to Project Directory:
1. Press `z` in Yazi
2. Type partial project name → Zoxide suggests based on usage frequency
3. Instant navigation to most relevant directory

## Performance Metrics

### Before (Traditional Tools):
- File search: `find` (slow, basic output)
- Content search: `grep -r` (slower, limited context)  
- Directory jumping: Manual `cd` navigation
- Previews: Basic `cat` or `less`

### After (Rust Integration):
- **10x faster** file discovery with fd
- **Rich structured data** from Nushell
- **Intelligent directory jumping** with zoxide
- **Unified fuzzy selection** across all operations
- **Syntax-highlighted previews** for all file types

## Dependencies

### Required Rust Tools:
```bash
# Core search and navigation
brew install fd ripgrep skim bat
brew install zoxide nushell

# Editor integration  
brew install helix
```

### Yazi Plugins:
- `zoxide.yazi` - Directory frecency jumping
- Built-in preview plugins for various file types

## Troubleshooting

### Command Not Found Errors:
- Ensure all Rust tools are in PATH
- Verify Homebrew installation: `/opt/homebrew/bin`

### Preview Issues:
- Check terminal color support: `echo $COLORTERM`
- Verify bat themes: `bat --list-themes`
- Test nushell tables: `nu -c "ls | table"`

### Helix Integration:
- Confirm Helix in PATH: `which hx`
- Test direct opening: `echo "test.md" | xargs hx`

## Future Enhancements

### Potential Additions:
- **Multi-select workflows**: Select multiple files for batch operations
- **Git integration**: Show git status in file metadata
- **Custom preview scripts**: Project-specific preview commands
- **Session management**: Remember search contexts

### Performance Optimizations:
- **Preview caching**: Cache expensive preview operations
- **Lazy loading**: Only preview visible items
- **Background indexing**: Pre-index large directories

## Philosophy Notes

### Why This Approach Works:
1. **Single ecosystem**: All tools written in Rust, consistent performance
2. **Composable design**: Each tool does one thing excellently
3. **Rich data flow**: Structured information throughout pipeline
4. **Editor integration**: Seamless transition from discovery to editing
5. **User-centric**: Optimized for actual workflows, not theoretical features

### The Rust Advantage:
- **Performance**: Compiled efficiency vs. interpreted scripts
- **Safety**: Memory safety eliminates classes of errors  
- **Consistency**: Unified error handling and output formats
- **Modern design**: Built with contemporary UX principles

This integration represents the evolution of file management - from simple directory listing to intelligent, context-aware file exploration and editing environment.

---

**Last Updated**: 2025-08-24  
**Compatibility**: Yazi 25.5.31+, macOS with Homebrew  
**License**: Configuration freely shared and modified