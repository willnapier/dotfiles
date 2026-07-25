#!/bin/bash
# PreToolUse hook: block Write (overwrite) on clinical client files.
#
# Client files contain session history. Write overwrites the entire file.
# Edit appends to it. Using Write on a client file would destroy all
# previous session notes.
#
# Blocks: Write tool on ~/Clinical/clients/*/*.md
# Allows: Edit tool (not matched by this hook's matcher)
#
# Input: JSON on stdin with .tool_input.file_path or .tool_input.filePath
# Output: JSON on stdout with block decision, or exit 0 to pass through

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.filePath // empty')

[ -z "$FILE_PATH" ] && exit 0

if [[ "$FILE_PATH" == */Clinical/clients/*/*.md ]]; then
  jq -n '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "block",
      permissionDecisionReason: "BLOCKED: Do not use Write on client files — it overwrites the entire file, destroying session history. Use Edit to append the new session note instead."
    }
  }'
  exit 0
fi

exit 0
