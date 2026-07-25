#!/usr/bin/env bash
# Stop hook: detect hollow acknowledgments ("Noted", "Understood", etc.)
# without a corresponding Write/Edit to a memory or documentation file.
#
# Reads Stop hook input from stdin (JSON with last_assistant_message and transcript_path).
# If the assistant said "Noted"/"Understood"/etc. but didn't write to memory,
# outputs a warning that gets injected into the next turn.

set -euo pipefail

INPUT=$(cat)

LAST_MSG=$(echo "$INPUT" | jq -r '.last_assistant_message // empty')

if [ -z "$LAST_MSG" ]; then
  exit 0
fi

# Check for hollow acknowledgment phrases (case-insensitive)
if echo "$LAST_MSG" | grep -qiE '\b(noted|understood|I understand|I will note|I.ll (make a |take )?note|I.ve noted|I.ve made a note|will keep that in mind|good to know)\b'; then
  # Check if a Write or Edit to a memory file also happened
  # We look for evidence of memory/doc writes in the response itself
  if echo "$LAST_MSG" | grep -qiE '(memory|MEMORY\.md|saved to|written to|persisted|saved.*memory|wrote.*memory|updated.*memory)'; then
    # Acknowledgment accompanied by a memory write — OK
    exit 0
  fi

  # Check transcript for Write/Edit tool calls in this turn
  TRANSCRIPT=$(echo "$INPUT" | jq -r '.transcript_path // empty')
  if [ -n "$TRANSCRIPT" ] && [ -f "$TRANSCRIPT" ]; then
    # Look for recent Write/Edit to memory files in the last few entries
    if tail -20 "$TRANSCRIPT" 2>/dev/null | grep -qE '(memory/|MEMORY\.md|feedback_|project_|user_|reference_)'; then
      exit 0
    fi
  fi

  # Hollow acknowledgment detected
  echo '{"error": "You just used an acknowledgment phrase (Noted/Understood/etc.) without writing the information to memory or a file. Per the No Hollow Acknowledgments rule: if it is worth acknowledging, it is worth writing down. Please write it down now, or retract the claim that you have noted it."}'
  exit 0
fi

exit 0
