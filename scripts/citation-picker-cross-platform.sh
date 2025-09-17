#!/bin/bash
# citation-picker-cross-platform.sh - Cross-platform citation key picker
# Picks citation keys from citations.md and copies to clipboard

# Load cross-platform paths
source "$(dirname "$0")/cross-platform-paths"

echo "🔍 Looking for citations.md..."

# Check for citations.md in known vault locations
CITATIONS_FILE=""
for possible_vault in "${OBSIDIAN_VAULT_PATHS[@]}"; do
    if [[ -f "$possible_vault/citations.md" ]]; then
        CITATIONS_FILE="$possible_vault/citations.md"
        echo "✅ Found citations: $CITATIONS_FILE"
        break
    fi
done

if [[ -z "$CITATIONS_FILE" ]] || [[ ! -f "$CITATIONS_FILE" ]]; then
    echo "❌ citations.md not found in any vault location"
    echo "Checked:"
    for path in "${OBSIDIAN_VAULT_PATHS[@]}"; do
        echo "  - $path/citations.md"
    done
    exit 1
fi

echo "📚 Picking citation from: $CITATIONS_FILE"

# Extract citation keys and let user pick - using modern tools
citation_key=$(rg -o '@[a-zA-Z0-9_-]*' "$CITATIONS_FILE" | \
    sort -u | \
    sk --ansi \
        --border \
        --height=60% \
        --header="📖 Pick citation key (Enter=select, Esc=cancel)" \
        --preview="rg -A 5 -B 1 '{}' '$CITATIONS_FILE'" \
        --preview-window=right:60%:wrap)

if [[ -n "$citation_key" ]]; then
    echo "📋 Selected: $citation_key"
    echo "💾 Copying to clipboard..."
    
    # Format as wiki link
    citation_link="[[$citation_key]]"
    
    # Cross-platform clipboard copy
    if command -v $CLIPBOARD_COPY >/dev/null 2>&1; then
        echo -n "$citation_link" | $CLIPBOARD_COPY 2>/dev/null
        echo "✅ Copied: $citation_link"
    else
        echo "⚠️  Clipboard not available - manually copy: $citation_link"
    fi
else
    echo "❌ No citation selected"
fi