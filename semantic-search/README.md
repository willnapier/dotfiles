# Semantic Search System - Dotter Managed

This directory is managed by Dotter and symlinked to `~/.local/share/semantic-search/`.

## First-Time Setup

After Dotter deployment:

```bash
cd ~/.local/share/semantic-search
./setup.sh
```

## Usage

All functionality is available through Nushell commands:

```bash
semantic-rebuild     # First-time index building
semantic-update      # Daily maintenance  
semantic "query"     # Search by concept
related file.md      # Find similar notes
semantic-status      # System status
```

## File Structure

- `semantic_indexer.py` - Background indexer with token truncation
- `semantic_query.py` - Query interface
- `config.yaml` - System configuration
- `setup.sh` - First-time setup script
- `db/` - FAISS vector database (created on first run)
- `logs/` - System logs
- `cache/` - Temporary files
- `venv/` - Python virtual environment (created by setup.sh)

## Integration

This system integrates with your existing workflow:
- Nushell commands for daily use
- Zellij floating panes for contextual discovery (Alt+r planned)
- Proper Dotter management prevents configuration drift

## Maintenance

The system is completely managed through dotfiles. Any changes should be made in:
`~/dotfiles/semantic-search/`

Changes are automatically deployed via Dotter.