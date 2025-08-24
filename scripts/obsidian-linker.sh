#!/bin/bash
# obsidian-linker.sh - Floating pane link picker for Helix + Zellij
# Solves the terminal access issue by running in a dedicated floating pane

# Terminal compatibility fixes for skim - use most basic but reliable setup
export TERM="xterm"           # Use most basic, widely supported terminal type
unset TERMINFO                # Clear any conflicting terminfo
export COLORTERM=""           # Clear color term to avoid conflicts
export RUST_BACKTRACE=0       # Suppress Rust panic backtraces for cleaner output

# Ensure we have a proper terminal size
export LINES=$(tput lines 2>/dev/null || echo 24)
export COLUMNS=$(tput cols 2>/dev/null || echo 80)

# Get the Obsidian vault path - hardcode since env vars don't pass to floating panes
VAULT_PATH="/Users/williamnapier/Obsidian.nosync/Forge"

# Verify vault exists
if [[ ! -d "$VAULT_PATH" ]]; then
    echo "❌ Obsidian vault not found at: $VAULT_PATH"
    echo "Set OBSIDIAN_VAULT environment variable to your vault path"
    exit 1
fi

# Function to preview note content
preview_note() {
    local file="$1"
    if [[ -f "$file" && "$file" == *.md ]]; then
        # Show first 20 lines with syntax highlighting if bat is available
        if command -v bat >/dev/null 2>&1; then
            bat --style=plain --color=always --line-range=:20 "$file" 2>/dev/null
        else
            head -20 "$file" 2>/dev/null
        fi
    else
        echo "📄 File: $(basename "$file")"
        echo "📁 Path: $file"
    fi
}

# Export the preview function for skim - make it available to subshells
export -f preview_note
export SHELL="/bin/bash"  # Ensure bash is used for preview commands

# Build list of markdown files - exclude common directories that slow things down
echo "🔍 Scanning vault for markdown files..."
notes=$(fd -e md . "$VAULT_PATH" \
    --exclude ".obsidian" \
    --exclude "linked_media" \
    --exclude "Trash" \
    --exclude "node_modules" \
    --exclude ".*" \
    | sort)

if [[ -z "$notes" ]]; then
    echo "❌ No markdown files found in vault"
    exit 1
fi

echo "📝 Found $(echo "$notes" | wc -l) notes"
echo "🎯 Select a note to insert as wiki link..."
echo ""

# Check if we have a TTY (interactive terminal)
if [ -t 0 ] && [ -t 1 ] && [ -t 2 ]; then
    # Interactive mode - run skim with full interface
    cd "$VAULT_PATH"
    selected=$(printf '%s\n' "$notes" | sk \
        --prompt="📝 Select note(s) [Tab=toggle, Ctrl+A=all]: " \
        --multi \
        --reverse \
        --bind="tab:toggle+down" \
        --preview="head -20 {}" \
        --preview-window="right:50%" \
        --header="Tab=toggle selection, arrows=navigate, Enter=confirm")
else
    # Non-interactive mode - return most recent note as fallback
    # This handles the case when run from Helix :insert-output
    selected=$(printf '%s\n' $notes | head -1)
fi

# Process selection(s)
if [[ -n "$selected" ]]; then
    # Handle multiple selections (one per line)
    wiki_links=""
    while IFS= read -r file; do
        if [[ -n "$file" ]]; then
            # Extract just the filename without path or extension
            filename=$(basename "$file")
            filename="${filename%.md}"
            
            # Add to wiki links collection
            if [[ -z "$wiki_links" ]]; then
                wiki_links="[[$filename]]"
            else
                wiki_links="$wiki_links [[$filename]]"
            fi
        fi
    done <<< "$selected"
    
    # Output the wiki link(s) directly for Helix insertion
    echo "$wiki_links"
    
    # Also copy to clipboard for Alt+l floating pane workflow
    echo -n "$wiki_links" | pbcopy 2>/dev/null
else
    echo ""
    echo "❌ No selection made"
    sleep 1
fi