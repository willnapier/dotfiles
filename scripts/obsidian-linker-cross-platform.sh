#!/bin/bash
# obsidian-linker-cross-platform.sh - Cross-platform floating pane link picker
# Solves the terminal access issue by running in a dedicated floating pane

# Load cross-platform paths
source "$(dirname "$0")/cross-platform-paths"

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
VAULT_PATH=$("$NU_PATH" -c "source $CONFIG_PATH/nushell/scripts/project-root-detection.nu; find-obsidian-vault" 2>/dev/null)

# If not found from current location, check known vault locations
if [[ -z "$VAULT_PATH" ]]; then
    echo "💡 Checking known vault locations..."
    for possible_vault in "${OBSIDIAN_VAULT_PATHS[@]}"; do
        if [[ -d "$possible_vault/.obsidian" ]]; then
            VAULT_PATH="$possible_vault"
            echo "✅ Found vault: $VAULT_PATH"
            break
        fi
    done
fi

if [[ -z "$VAULT_PATH" ]] || [[ ! -d "$VAULT_PATH" ]]; then
    echo "❌ No Obsidian vault found"
    echo "Checked locations:"
    for path in "${OBSIDIAN_VAULT_PATHS[@]}"; do
        echo "  - $path"
    done
    echo ""
    echo "Press any key to exit..."
    read -n 1
    exit 1
fi

echo "📁 Using vault: $VAULT_PATH"
cd "$VAULT_PATH" || exit 1

# Create link picker using cross-platform clipboard
wiki_links=$(fd -e md . | \
    sk --ansi \
        --border \
        --height=80% \
        --preview="FILE_PATH={} $NU_PATH $CONFIG_PATH/yazi/scripts/obsidian-preview.nu" \
        --preview-window=right:60%:wrap \
        --bind="tab:down" \
        --header="📝 Pick notes to link (Tab=navigate, Enter=select, Esc=cancel)" \
        --multi | \
    sd '\.md$' '' | \
    sd '(.*)' '"[[$1]]"' | \
    paste -sd ' ' -)

if [[ -n "$wiki_links" ]]; then
    echo "📋 Selected links: $wiki_links"
    echo "💾 Copied to clipboard"
    
    # Cross-platform clipboard copy
    if command -v $CLIPBOARD_COPY >/dev/null 2>&1; then
        echo -n "$wiki_links" | $CLIPBOARD_COPY 2>/dev/null
    else
        echo "⚠️  Clipboard not available on this platform"
    fi
    
    echo ""
    echo "✅ Links ready to paste in Helix"
    echo "Press any key to continue..."
    read -n 1
else
    echo "❌ No links selected"
    echo "Press any key to exit..."
    read -n 1
fi