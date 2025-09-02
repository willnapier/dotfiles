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

# Use intelligent vault detection - find .obsidian folder from current location
echo "🔍 Detecting Obsidian vault from current location..."
VAULT_PATH=$(nu -c "source ~/.config/nushell/scripts/project-root-detection.nu; find-obsidian-vault" 2>/dev/null)

if [[ -z "$VAULT_PATH" ]]; then
    echo "❌ No Obsidian vault found from current location"
    echo "📝 Make sure you're working within an Obsidian vault directory"
    echo "💡 Or create a .obsidian folder to mark a custom vault root"
    exit 1
fi

echo "✅ Found Obsidian vault: $VAULT_PATH"

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
    # Generate relative paths from current directory for cleaner display
    notes_relative=$(fd -e md . \
        --exclude ".obsidian" \
        --exclude "linked_media" \
        --exclude ".trash" \
        --exclude "Templates" \
        2>/dev/null | sort)
    selected=$(printf '%s\n' "$notes_relative" | sk \
        --prompt="📝 Select note(s) [Tab=toggle, Ctrl+A=all]: " \
        --multi \
        --reverse \
        --bind="tab:toggle+down" \
        --preview="FILE_PATH={} /Users/williamnapier/.config/yazi/scripts/obsidian-preview.nu" \
        --preview-window="right:60%" \
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
    
    # Also copy to clipboard for Alt+l floating pane workflow (cross-platform)
    # Use full path to avoid shell interception issues
    if command -v /usr/bin/pbcopy >/dev/null 2>&1; then
        echo -n "$wiki_links" | /usr/bin/pbcopy 2>/dev/null  # macOS
    elif command -v wl-copy >/dev/null 2>&1; then
        echo -n "$wiki_links" | wl-copy 2>/dev/null  # Wayland Linux
    elif command -v xclip >/dev/null 2>&1; then
        echo -n "$wiki_links" | xclip -selection clipboard 2>/dev/null  # X11 Linux
    fi
    
    # Brief pause for floating pane workflows to ensure clipboard copy completes
    # and user sees the result before pane closes
    if [[ -n "$ZELLIJ_SESSION_NAME" ]]; then
        echo "📋 Copied to clipboard! You can now paste with Cmd+V"
        sleep 1.5
    fi
else
    echo ""
    echo "❌ No selection made"
    sleep 1
fi