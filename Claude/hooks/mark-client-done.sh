#!/bin/bash
# PostToolUse hook: auto-mark clinic clients as done when their notes are edited.
# Fires on Edit/Write to ~/Clinical/clients/*/
# Extracts client ID from path and queues daypage-mark-done.

INPUT=$(cat)

FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Only process files under ~/Clinical/clients/
case "$FILE_PATH" in
  /Users/williamnapier/Clinical/clients/*)
    ;;
  "$HOME"/Clinical/clients/*)
    ;;
  *)
    exit 0
    ;;
esac

# Extract client ID: the directory name under clients/
# Handles XA01, XB+XC, XD, etc. (single code, joint pair, two-letter code)
CLIENT_ID=$(echo "$FILE_PATH" | sed -E 's|.*/Clinical/clients/([^/]+)/.*|\1|')

if [ -z "$CLIENT_ID" ]; then
  exit 0
fi

# Queue the checkbox toggle
daypage-mark-done "$CLIENT_ID"
exit 0
