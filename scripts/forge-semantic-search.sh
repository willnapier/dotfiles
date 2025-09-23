#!/bin/bash
# Semantic search with wikilink output - Bash version for Zellij compatibility

# Terminal setup
export TERM="xterm"
export RUST_BACKTRACE=0

# Source environment for API key
if [[ -f ~/.zshrc ]]; then
    source ~/.zshrc
fi

# Try to get OpenAI key from keychain if not properly set in environment
if [[ -z "$OPENAI_API_KEY" || "$OPENAI_API_KEY" == *"REPLACE_"* ]]; then
    OPENAI_API_KEY=$(security find-generic-password -s "openai-api-key" -w 2>/dev/null || echo "")
    export OPENAI_API_KEY
fi

echo "🧠 Semantic Search with Wikilink Output"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Check if semantic search is available
SEMANTIC_DIR="$HOME/.local/share/semantic-search"
VENV_PYTHON="$SEMANTIC_DIR/venv/bin/python3"

if [[ ! -f "$VENV_PYTHON" ]]; then
    echo "❌ Semantic search not installed"
    echo "Run: semantic-indexer --rebuild"
    exit 1
fi

if [[ -z "$OPENAI_API_KEY" ]]; then
    echo "❌ OPENAI_API_KEY not set"
    echo "Export your OpenAI API key first"
    exit 1
fi

echo "✅ Semantic search system ready"
echo ""

# Get search query - accept as argument or use interactive input
if [[ $# -gt 0 ]]; then
    query="$*"
    echo "🧠 Using provided query: '$query'"
else
    # Use a simple prompt that works in floating panes
    echo "🧠 Enter conceptual search terms:"
    echo -n "Search: "
    read query
    if [[ -z "$query" ]]; then
        echo "No search provided"
        exit 0
    fi
fi

echo ""
echo "🧠 Searching semantically for: '$query'"
echo ""

# Run semantic query and capture output
TEMP_FILE=$(mktemp)
echo "🔄 Running semantic search..."
cd "$SEMANTIC_DIR"

# Show what we're about to run
echo "Command: $VENV_PYTHON semantic_query.py --text '$query'"
echo "Working directory: $(pwd)"
echo ""

"$VENV_PYTHON" semantic_query.py --text "$query" > "$TEMP_FILE" 2>&1
EXIT_CODE=$?

echo "🔍 Search completed with exit code: $EXIT_CODE"

if [[ $EXIT_CODE -ne 0 ]]; then
    echo "❌ Semantic search failed:"
    echo "Error output:"
    cat "$TEMP_FILE"
    echo ""
    echo "Press any key to continue..."
    read -n 1
    rm "$TEMP_FILE"
    exit 1
fi

echo "✅ Search successful, processing results..."

# Parse results for skim selection
# Extract lines that start with score like: "0.52  Filename" - using modern tools
results=$(rg "^[0-9]\.[0-9][0-9].*\.md|^[0-9]\.[0-9][0-9]  " "$TEMP_FILE" | head -10)

if [[ -z "$results" ]]; then
    echo "❌ No semantic matches found for '$query'"
    echo ""
    echo "💡 Try broader or different conceptual terms"
    rm "$TEMP_FILE"
    exit 0
fi

echo "📝 Found semantic matches:"
echo ""


# Preview function for semantic results
preview_semantic() {
    local line="$1"
    # Extract filename from the line (format: "0.52  Filename") 
    # Use sd (Rust-based) for safer trimming that handles special characters like apostrophes
    local filename=$(echo "$line" | cut -c6- | sd '^\s+' '' | sd '\s+$' '')
    
    # Find the actual .md file (could be anywhere in the vault)
    local vault_path="$HOME/Forge"
    local found_file=$(fd -t f "^${filename}\.md$" "$vault_path" | head -1)
    
    if [[ -n "$found_file" ]]; then
        local full_path="$found_file"
    else
        # Fallback to root level if not found
        local full_path="$vault_path/$filename.md"
    fi
    
    echo "📄 $filename"
    echo "📁 $full_path"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""
    
    if [[ -f "$full_path" ]]; then
        # Use Nushell word-wrapping for intelligent word-level wrapping
        if command -v nu >/dev/null 2>&1; then
            nu /.local/bin/word-wrap-preview.nu "$full_path" 80 2>/dev/null
        elif command -v bat >/dev/null 2>&1; then
            # Fallback to bat with character wrapping
            bat --style=plain --color=always --line-range=:25 --wrap=character --terminal-width=80 "$full_path" 2>/dev/null
        else
            # Basic word wrapping with fold
            head -25 "$full_path" 2>/dev/null | fold -s -w 80
        fi
    else
        echo "File not found: $full_path"
    fi
}

# Export preview function
export -f preview_semantic
export SHELL="/bin/bash"

# Interactive selection with skim
selected=$(echo "$results" | sk \
    --prompt="🧠 Select file: " \
    --preview="preview_semantic {}" \
    --preview-window="right:60%" \
    --header="Semantic search results - Enter=select and copy wikilink, Esc=cancel")

rm "$TEMP_FILE"

if [[ -z "$selected" ]]; then
    echo ""
    echo "❌ No selection made"
    exit 0
fi

# Extract filename from selection (format: "0.52  Filename") 
# Use sd (Rust-based) for safer trimming that handles special characters like apostrophes
filename=$(echo "$selected" | cut -c6- | sd '^\s+' '' | sd '\s+$' '')
wikilink="[[$filename]]"

echo ""
echo "$wikilink"

# Extract similarity for display (format: "0.52  Filename") - using modern tools
similarity=$(echo "$selected" | sd -E '^([0-9]\.[0-9][0-9]).*' '$1')
similarity_pct=$(echo "scale=0; $similarity * 100" | bc)

# Copy to clipboard
echo -n "$wikilink" | pbcopy

echo "📋 Copied to clipboard! Semantic similarity: ${similarity_pct}%"
echo "💡 You can now paste with Cmd+V"
sleep 1.5