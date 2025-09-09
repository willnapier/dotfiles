#!/bin/bash
# Content search for Zellij - uses bash for compatibility

# Set terminal for skim
export TERM="xterm"

echo "🔍 Content Search with Wikilink Output"
echo ""
read -p "Enter search terms: " query

if [[ -z "$query" ]]; then
    echo "No search provided"
    exit 0
fi

VAULT="/Users/williamnapier/Obsidian.nosync/Forge"

echo "Searching for: $query"

# Function to preview file content with search context
preview_cmd="rg --type md --color=always --context 3 '$query' '$VAULT/{}' 2>/dev/null | head -30 || bat --style=plain --color=always '$VAULT/{}' 2>/dev/null || head -30 '$VAULT/{}'"

# Search with ripgrep and let user select with preview
selected=$(rg --type md --files-with-matches --follow "$query" "$VAULT" \
    | sed "s|$VAULT/||" \
    | sk --prompt="📝 Select file: " \
         --preview="$preview_cmd" \
         --preview-window="right:60%" \
         --header="Tab=toggle, Enter=select, Esc=cancel")

if [[ -z "$selected" ]]; then
    echo "No selection made"
    exit 0
fi

# Extract filename and create wikilink
filename=$(basename "$selected" .md)
wikilink="[[$filename]]"

echo "$wikilink"

# Copy to clipboard
echo -n "$wikilink" | pbcopy

echo "📋 Copied to clipboard!"
sleep 1.5