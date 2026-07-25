#!/usr/bin/env bash
# PostToolUse hook: regenerate meli-mine(1) when its markdown source changes.
#
# Fires after Edit/Write tool calls. If the modified file is
# ~/dotfiles/meli/meli-mine.md, runs pandoc to refresh the man page.
# Failure is silent — keeps the prior .1; the doc gate will catch staleness.

set -euo pipefail

INPUT=$(cat)

TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty')
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.filePath // empty')

if [[ "$TOOL_NAME" != "Edit" && "$TOOL_NAME" != "Write" ]]; then
  exit 0
fi

MD_SOURCE="$HOME/dotfiles/meli/meli-mine.md"
ROFF_OUTPUT="$HOME/dotfiles/meli/meli-mine.1"

if [[ "$FILE_PATH" != "$MD_SOURCE" ]]; then
  exit 0
fi

if command -v pandoc >/dev/null 2>&1; then
  pandoc -s -t man "$MD_SOURCE" -o "$ROFF_OUTPUT" 2>/dev/null || true
fi

exit 0
