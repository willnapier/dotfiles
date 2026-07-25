#!/bin/bash
# PreToolUse hook: block Edit/Write on ~/.config/ paths, redirect to ~/dotfiles/.
#
# Problem: Claude often edits config files directly at ~/.config/ which are
# dotter-managed symlinks. The next dotter deploy will overwrite the change.
# The correct location is always ~/dotfiles/<relative-path>.
#
# Behaviour:
#   - Blocks any Edit/Write where file_path contains /.config/
#   - Returns an error message telling the agent to edit ~/dotfiles/ instead
#   - Exception: paths under /.config/systemd/ (systemd unit files are in dotfiles
#     under a different layout) — these still get blocked but with a more specific message
#
# Input: JSON on stdin with .tool_input.file_path or .tool_input.filePath
# Output: JSON on stdout with block decision, or exit 0 to pass through

INPUT=$(cat)

# Extract file path (Edit uses file_path, some tools use filePath)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.filePath // empty')

[ -z "$FILE_PATH" ] && exit 0

# Check if path contains /.config/
if [[ "$FILE_PATH" == *"/.config/"* ]]; then
  # Extract the relative path after .config/
  RELATIVE=${FILE_PATH#*/.config/}

  jq -n --arg rel "$RELATIVE" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "block",
      permissionDecisionReason: ("BLOCKED: Do not edit ~/.config/ directly — these are dotter-managed symlinks. Edit ~/dotfiles/" + $rel + " instead, then run dotter deploy.")
    }
  }'
  exit 0
fi

# Pass through
exit 0
