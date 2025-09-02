#!/bin/bash
# Semantic Search Setup Script - Dotter Managed Version

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "🚀 Setting up AI-Powered Semantic Note Association System"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Create required directories
mkdir -p "$SCRIPT_DIR/db" "$SCRIPT_DIR/logs" "$SCRIPT_DIR/cache"

# Create Python virtual environment if it doesn't exist
VENV_DIR="$SCRIPT_DIR/venv"
if [[ ! -d "$VENV_DIR" ]]; then
    echo "📦 Creating Python virtual environment..."
    python3 -m venv "$VENV_DIR"
fi

# Install/update dependencies
echo "📚 Installing Python dependencies..."
"$VENV_DIR/bin/pip" install --upgrade pip
"$VENV_DIR/bin/pip" install openai faiss-cpu numpy pandas pyyaml tqdm watchdog

# Make scripts executable
chmod +x "$SCRIPT_DIR/semantic_indexer.py"
chmod +x "$SCRIPT_DIR/semantic_query.py"

# Check for OpenAI API key
if [[ -z "$OPENAI_API_KEY" ]]; then
    echo "⚠️  OpenAI API key not found in environment"
    echo "Set with: export OPENAI_API_KEY='your-api-key-here'"
    echo "Add to ~/.config/nushell/env.nu to make permanent"
    echo ""
else
    echo "✅ OpenAI API key found"
fi

echo "✅ Setup complete!"
echo ""
echo "Next steps:"
echo "1. Set OPENAI_API_KEY if not already set"
echo "2. Run: semantic-rebuild (in Nushell)"
echo "3. Test: semantic \"your query\""