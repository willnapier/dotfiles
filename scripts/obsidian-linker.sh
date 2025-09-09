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
echo "🔍 Detecting Obsidian vault..."
VAULT_PATH=$(nu -c "source ~/.config/nushell/scripts/project-root-detection.nu; find-obsidian-vault" 2>/dev/null)

# If not found from current location, check known vault locations
if [[ -z "$VAULT_PATH" ]]; then
    # Check common vault locations
    for possible_vault in "/Users/williamnapier/Obsidian.nosync/Forge" "/Users/williamnapier/Obsidian/Forge" "$HOME/Documents/Obsidian/Forge"; do
        if [[ -d "$possible_vault/.obsidian" ]]; then
            VAULT_PATH="$possible_vault"
            echo "📍 Using default vault location"
            break
        fi
    done
fi

if [[ -z "$VAULT_PATH" ]]; then
    echo "❌ No Obsidian vault found"
    echo "📝 Searched from current location and checked default vaults"
    echo "💡 Create a .obsidian folder to mark a vault root"
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
notes=$(fd -L -e md . "$VAULT_PATH" \
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
    notes_relative=$(fd -L -e md . \
        --exclude ".obsidian" \
        --exclude "linked_media" \
        --exclude ".trash" \
        --exclude "Templates" \
        2>/dev/null | sort)
    selected=$(printf '%s\n' "$notes_relative" | sk \
        --prompt="📝 Select note(s) [Tab=toggle, Ctrl+A=all, Ctrl+N=new]: " \
        --multi \
        --reverse \
        --bind="tab:toggle+down" \
        --bind="ctrl-n:print-query+accept" \
        --preview="nu $HOME/.local/bin/word-wrap-preview.nu {} 80 2>/dev/null || $HOME/.local/bin/simple-file-preview {}" \
        --preview-window="right:60%" \
        --header="Tab=toggle selection, Ctrl+N=create new note, arrows=navigate, Enter=confirm")
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
            # Check if this is an existing file or a new note name
            if [[ -f "$VAULT_PATH/$file" ]]; then
                # Existing file - extract just the filename without path or extension
                filename=$(basename "$file")
                filename="${filename%.md}"
                link_prefix=""
            else
                # New note (from Ctrl+N) - use as-is, add ? prefix for unresolved link
                filename="$file"
                filename="${filename%.md}"  # Remove .md if present
                link_prefix="?"
            fi
            
            # Add to wiki links collection
            if [[ -z "$wiki_links" ]]; then
                wiki_links="${link_prefix}[[$filename]]"
            else
                wiki_links="$wiki_links ${link_prefix}[[$filename]]"
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