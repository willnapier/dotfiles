#!/bin/bash
# Interactive content search with live preview and wikilink output
# Similar to forge-linker but for content search

# Terminal setup
export TERM="xterm"
export RUST_BACKTRACE=0

VAULT="/Users/williamnapier/Obsidian.nosync/Forge"

echo "🔍 Content Search with Wikilink Output"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "💡 Type to search file contents interactively"
echo ""

# Preview function showing matched content
preview_content() {
    local file="$1"
    local full_path="$VAULT/$file"
    
    if [[ -f "$full_path" && "$full_path" == *.md ]]; then
        echo "📄 $(basename "$file")"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo ""
        
        # Use Nushell word-wrapping for intelligent word-level wrapping
        if command -v nu >/dev/null 2>&1; then
            nu /Users/williamnapier/.local/bin/word-wrap-preview.nu "$full_path" 80 2>/dev/null
        elif command -v bat >/dev/null 2>&1; then
            # Fallback to bat with character wrapping
            bat --style=plain --color=always --line-range=:30 --wrap=character --terminal-width=80 "$full_path" 2>/dev/null
        else
            head -30 "$full_path" 2>/dev/null
        fi
    else
        echo "📄 File: $file"
        echo "📁 Path: $full_path"
    fi
}

# Export preview function for skim
export -f preview_content
export SHELL="/bin/bash"
export VAULT

# Interactive search - skim with live ripgrep
cd "$VAULT"

# Build ripgrep command based on case sensitivity setting
if [[ -n "$SEARCH_CASE_INSENSITIVE" ]]; then
    rg_cmd='rg --type md --files-with-matches --color=always --ignore-case {}'
    prompt="🔍 Search content (case insensitive): "
    header="Type to search (case insensitive) • Enter=select • Esc=cancel"
else
    rg_cmd='rg --type md --files-with-matches --color=always {}'
    prompt="🔍 Search content (case sensitive): "
    header="Type to search (case sensitive) • Enter=select • Esc=cancel"
fi

# Use skim's interactive mode with ripgrep
selected=$(sk --ansi \
    --interactive \
    --cmd "$rg_cmd" \
    --prompt="$prompt" \
    --preview="preview_content {}" \
    --preview-window="right:60%" \
    --header="$header" \
    --bind="enter:accept")

if [[ -z "$selected" ]]; then
    echo ""
    echo "❌ No selection made"
    exit 0
fi

# Extract filename and create wikilink
filename=$(basename "$selected" .md)
wikilink="[[$filename]]"

echo ""
echo "$wikilink"

# Copy to clipboard
echo -n "$wikilink" | pbcopy

echo "📋 Copied to clipboard! Paste with Cmd+V"
sleep 1.5