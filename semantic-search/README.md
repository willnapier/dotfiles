# Semantic Search System

OpenAI-powered semantic search over the Forge knowledge base. Indexes
markdown notes with `text-embedding-3-large`, stores vectors in a local
FAISS index, queries via the `f*me`/`f*mv`/`f*ml` family of nushell
commands.

This directory is dotter-managed and symlinked to
`~/.local/share/semantic-search/`.

## Architecture

```
fsme (nushell function in config.nu)
   ↓
~/.local/bin/semantic-query        ← thin bash wrapper that activates the venv
   ↓
~/.local/share/semantic-search/venv/bin/python3 semantic_query.py
   ↓
OpenAI text-embedding-3-large (query embedding)
   ↓
~/Literature/db/faiss_index.bin    ← FAISS vector store (vault embeddings)
~/Literature/db/file_metadata.json ← per-file metadata
```

The indexer pipeline mirrors this with `semantic-indexer` →
`semantic_indexer.py`. Auto-update is driven by cron, calling the wrapper
described below.

## Where things live

| | Path |
|---|---|
| Source (dotter-managed) | `~/dotfiles/semantic-search/` |
| Deployed (runtime) | `~/.local/share/semantic-search/` |
| Wrappers in PATH | `~/.local/bin/semantic-{query,indexer,auto-update,cron-wrapper}` |
| FAISS index | `~/Literature/db/faiss_index.bin` |
| Metadata | `~/Literature/db/file_metadata.json` |
| Logs | `~/.local/share/semantic-search/logs/{semantic,auto-update}.log` |
| Config | `~/.local/share/semantic-search/config.yaml` |
| Indexed corpus | `~/Forge/` — `.md` files only, with exclusions in `config.yaml` |

## Daily usage (interactive)

These are nushell functions in `~/dotfiles/nushell/config.nu` and are the
intended user-facing interface:

```nushell
fsme            # Forge semantic search → open match in editor
fsmv            # Forge semantic search → preview match (read-only)
fsml            # Forge semantic search → wiki link to clipboard
```

Each prompts for a query, embeds it via OpenAI, retrieves the top
matches from the FAISS index, and pipes them through `sk` for selection.

## Setup requirements

### macOS Keychain entry for the API key

The cron wrapper and the interactive nushell startup both pull
`OPENAI_API_KEY` from the macOS Keychain. To set it:

```bash
security add-generic-password \
  -s "openai-api-key" \
  -a "semantic-search" \
  -w "sk-..."
```

This is the same convention used for the Gemini key. The interactive
nushell shell pulls it in `env.nu`; the cron job pulls it in
`scripts/semantic-cron-wrapper`. Neither uses `~/.zshrc` (the user runs
nushell, not zsh).

### First-time / rebuild

```bash
cd ~/.local/share/semantic-search
./setup.sh        # creates venv, installs deps
~/.local/bin/semantic-indexer --rebuild
```

A full rebuild against ~6,000 Forge notes costs roughly $1–2 in OpenAI
credits at current `text-embedding-3-large` rates.

## Auto-update (cron)

A user crontab on the Mac runs the indexer every 12 hours:

```cron
0 8  * * * /Users/williamnapier/dotfiles/scripts/semantic-cron-wrapper
0 20 * * * /Users/williamnapier/dotfiles/scripts/semantic-cron-wrapper
```

The wrapper:

1. Pulls `OPENAI_API_KEY` from Keychain (`security find-generic-password
   -s "openai-api-key" -a "semantic-search" -w`)
2. Sets `PATH` explicitly (cron's environment is minimal)
3. Execs `~/.local/bin/semantic-auto-update`, which delegates to
   `semantic_indexer.py --update` (incremental, processes only changed
   files)

Failures and progress are appended to
`~/.local/share/semantic-search/logs/auto-update.log`.

## Known gotchas (history)

Both of the following were diagnosed and fixed on **2026-04-08**:

1. **Wrapper deleted by an auto-commit (2025-10-05).** Commit `7980a10`
   ("Auto-commit: 26 files changed on macOS") wiped 26 scripts including
   `semantic-cron-wrapper`, leaving the cron job pointing at a missing
   file. The original wrapper sourced `~/.zshrc` for the API key, which
   never worked anyway because the user runs nushell — so the wrapper
   was already silently failing before it was deleted. Recreated from
   scratch using Keychain retrieval (commit `e8602dd`).
2. **Tilde expansion bug in `vault.path`.** Commit `e46f96d` (2025-11-07,
   "Linux fixes") changed the vault path from an absolute Mac path to
   `~/Forge/` for cross-platform reuse, but `Path("~/Forge/")` in Python
   does not expand the tilde — every other path in the same file already
   uses `os.path.expanduser()`. Result: `_find_markdown_files` returned
   zero files for ~5 months. Fix: one missing `os.path.expanduser()` call
   (commit `0a77f3d`).

The two bugs masked each other: the wrapper was never running, so the
path bug never had a chance to surface.

## Maintenance

All edits go in `~/dotfiles/semantic-search/`. Dotter symlinks them into
`~/.local/share/semantic-search/`, but be aware that **the deployed
Python source is also a symlink** — you can edit either side, and a
manual `cp` is **not** needed after a dotfiles edit. (This was unclear
during the 2026-04-08 cleanup; verified by `diff` afterwards.)

After any change, verify with:

```bash
~/dotfiles/scripts/semantic-cron-wrapper       # full pipeline test
tail -20 ~/.local/share/semantic-search/logs/auto-update.log
```

A healthy run logs `Found N markdown files` (where N is the current
Forge count, ~6,000) and either `Update completed successfully` or per-
file processing entries.
