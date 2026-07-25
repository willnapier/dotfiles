#!/bin/bash
# PreToolUse hook: block Edit/Write on the shared MESSAGEBOARD.md.
#
# Raw Edit/Write to the messageboard is hazardous: concurrent sessions do
# read-modify-write and silently clobber each other's entries (lost update),
# and a hand-edit bypasses the auto-stamped "### date — device" header + the
# Syncthing sync trigger. It also invites confusion with the sister file
# ASSISTANT-HANDOFF.md — a session "posts to the messageboard" but actually
# edits the handoff (or vice-versa), with no receipt to tell them apart.
#
# Correct method: `messageboard-edit insert "message"` (atomic, stamps the
# header, triggers sync, prints a receipt naming the file) and
# `messageboard-edit remove-containing "unique text"`.
#
# The hook matches only the Edit/Write TOOLS, so messageboard-edit (a Bash
# script using nu `save`) passes through unaffected.
#
# Input: JSON on stdin with .tool_input.file_path or .tool_input.filePath
# Output: JSON on stdout with block decision, or exit 0 to pass through

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // .tool_input.filePath // empty')

[ -z "$FILE_PATH" ] && exit 0

if [[ "$FILE_PATH" == */MESSAGEBOARD.md ]]; then
  jq -n '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "block",
      permissionDecisionReason: "BLOCKED: Do not Edit/Write MESSAGEBOARD.md directly — raw edits clobber other sessions'\'' entries and skip the stamped header + sync. Post with: messageboard-edit insert \"your message\" (it stamps a ### date — device header, inserts atomically, triggers sync, and prints a receipt). Remove an item with: messageboard-edit remove-containing \"unique text\". NOTE: this is the MESSAGEBOARD (actionable inbox). For a narrative status broadcast, edit ASSISTANT-HANDOFF.md instead — and when you tell Will you posted, name the exact file."
    }
  }'
  exit 0
fi

exit 0
