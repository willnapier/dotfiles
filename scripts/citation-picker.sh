#!/bin/bash
# citation-picker.sh - Floating pane citation key picker for Helix + Zellij
# Searches citations.md and extracts citation keys for journal notation

# Terminal compatibility fixes for skim - use most basic but reliable setup
export TERM="xterm"           # Use most basic, widely supported terminal type
unset TERMINFO                # Clear any conflicting terminfo
export COLORTERM=""           # Clear color term to avoid conflicts
export RUST_BACKTRACE=0       # Suppress Rust panic backtraces for cleaner output

# Ensure we have a proper terminal size
export LINES=$(tput lines 2>/dev/null || echo 24)
export COLUMNS=$(tput cols 2>/dev/null || echo 80)

# Use intelligent vault detection to find citations file
echo "🔍 Detecting Obsidian vault for citations..."
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
    exit 1
fi

# Look for citations file in standard locations within the vault
CITATIONS_FILE=""
for possible_path in "$VAULT_PATH/ZET/citations.md" "$VAULT_PATH/citations.md" "$VAULT_PATH/references/citations.md"; do
    if [[ -f "$possible_path" ]]; then
        CITATIONS_FILE="$possible_path"
        break
    fi
done

if [[ -z "$CITATIONS_FILE" ]]; then
    echo "❌ Citations file not found in vault: $VAULT_PATH"
    echo "📝 Looking for: citations.md in ZET/, root, or references/ folders"
    exit 1
fi

echo "✅ Found citations file: $CITATIONS_FILE"

# Function to preview citation details
preview_citation() {
    local line="$1"
    if [[ -n "$line" ]]; then
        # Extract citation key (first word) - using modern tools
        citation_key=$(echo "$line" | cut -d' ' -f1)
        echo "📖 Citation Key: $citation_key"
        echo ""
        echo "📝 Full Entry:"
        echo "$line"
        echo ""
        echo "💡 Usage in journal:"
        echo "r:: 30m $citation_key philosophy, neuroscience"
        echo "w:: 45m $citation_key academic-writing"
    fi
}

# Export the preview function for skim - make it available to subshells
export -f preview_citation
export SHELL="/bin/bash"  # Ensure bash is used for preview commands

# Read citations file and filter out header/empty lines - using ripgrep for better performance
echo "🔍 Loading citations database..."
citations=$(rg -v '^#' "$CITATIONS_FILE" | rg -v '^$' | rg -v '^Total entries:')

if [[ -z "$citations" ]]; then
    echo "❌ No citations found in database"
    exit 1
fi

citation_count=$(echo "$citations" | wc -l | tr -d ' ')
echo "📚 Found $citation_count citations"
echo "🎯 Select a citation to copy key to clipboard..."
echo ""

# Check if we have a TTY (interactive terminal)
if [ -t 0 ] && [ -t 1 ] && [ -t 2 ]; then
    # Interactive mode - run skim with full interface
    selected=$(printf '%s\n' "$citations" | sk \
        --prompt="📚 Select citation [arrows=navigate, Enter=confirm]: " \
        --reverse \
        --preview="preview_citation {}" \
        --preview-window="right:60%" \
        --header="Search by author, title, keywords, or citation key")
else
    # Non-interactive mode - return first citation as fallback
    selected=$(printf '%s\n' "$citations" | head -1)
fi

# Process selection
if [[ -n "$selected" ]]; then
    # Extract citation key (first word of the line) - using modern tools
    citation_key=$(echo "$selected" | cut -d' ' -f1)
    
    if [[ -n "$citation_key" ]]; then
        # Output the citation key as wikilink for Helix insertion
        echo "[[$citation_key]]"
        
        # Also copy to clipboard for Alt+c floating pane workflow (cross-platform)
        if command -v /usr/bin/pbcopy >/dev/null 2>&1; then
            echo -n "[[$citation_key]]" | /usr/bin/pbcopy 2>/dev/null  # macOS
        elif command -v wl-copy >/dev/null 2>&1; then
            echo -n "[[$citation_key]]" | wl-copy 2>/dev/null  # Wayland Linux
        elif command -v xclip >/dev/null 2>&1; then
            echo -n "[[$citation_key]]" | xclip -selection clipboard 2>/dev/null  # X11 Linux
        fi
        
        echo ""
        echo "✅ Citation wikilink copied to clipboard: [[$citation_key]]"
        echo "💡 Usage: r:: 30m [[$citation_key]] topic, keywords"
        sleep 1
    else
        echo "❌ Could not extract citation key from selection"
        sleep 1
    fi
else
    echo ""
    echo "❌ No selection made"
    sleep 1
fi