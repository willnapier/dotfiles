#!/bin/bash
# PreToolUse hook: block sudo commands.
#
# Claude Code's Bash tool has no TTY for password input, so sudo always
# fails. Block it immediately with a helpful message instead of wasting
# time and tokens on the inevitable failure.
#
# Input: JSON on stdin with .tool_input.command
# Output: JSON on stdout with block decision, or exit 0 to pass through

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

[ -z "$COMMAND" ] && exit 0

if [[ "$COMMAND" =~ (^|[;\|&])\ *sudo\  ]]; then
  jq -n '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "block",
      permissionDecisionReason: "BLOCKED: sudo requires an interactive terminal (TTY) which the Bash tool does not have. Give the user the command to run in their own terminal instead."
    }
  }'
  exit 0
fi

exit 0
