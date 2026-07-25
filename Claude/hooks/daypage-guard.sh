#!/bin/bash
# PreToolUse hook: block Edit/Write on DayPage files.
#
# DayPages are almost always open in Helix. Direct writes cause
# "file modified by external process" errors and risk data loss.
# The correct method is daypage-append (queues entries for Helix merge).
#
# Input: JSON on stdin with .tool_input.file_path or .tool_input.filePath
# Output: JSON on stdout with block decision, or exit 0 to pass through

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.filePath // empty')

[ -z "$FILE_PATH" ] && exit 0

if [[ "$FILE_PATH" == */DayPages/*.md ]]; then
  jq -n '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "block",
      permissionDecisionReason: "BLOCKED: Do not write to DayPage files directly — they are open in Helix. Use daypage-append \"tag:: content\" instead."
    }
  }'
  exit 0
fi

exit 0
