#!/usr/bin/env bash
# PreToolUse hook on Bash: catch memory files being propagated to the wrong
# platform.
#
# Mac-only memories (Karabiner, launchd, codesign, keychain, …) shouldn't
# land on nimbini's memory index. Linux-only memories (systemd, pacman,
# niri, kanata, …) shouldn't land on Mac's. The hook inspects scp/ssh
# commands that touch memory/*.md and warns if content + destination
# don't match.
#
# Soft enforcement — emits an error message (not a block). The model can
# override with a one-line justification or rewrite the memory to be
# cross-platform before retrying.

set -euo pipefail

INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name // empty')

if [[ "$TOOL_NAME" != "Bash" ]]; then
  exit 0
fi

CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only inspect commands that mention nimbini AND memory AND .md
if [[ "$CMD" != *nimbini* ]] || [[ "$CMD" != *memory* ]] || [[ "$CMD" != *.md* ]]; then
  exit 0
fi

# Identify a local memory file path appearing in the command.
MEM_FILE=$(echo "$CMD" | grep -oE '/[^ ]*memory/[^ ]*\.md' | head -1)
if [ -z "$MEM_FILE" ] || [ ! -f "$MEM_FILE" ]; then
  exit 0
fi

# Determine direction: TO nimbini (Mac → Linux) or FROM nimbini (Linux → Mac).
# Conservative: only flag Mac→Linux when the source path is on Mac, and vice versa.
TO_NIMBINI=false
FROM_NIMBINI=false
if [[ "$CMD" =~ scp[[:space:]]+[^[:space:]]*[[:space:]]+will@nimbini: ]]; then
  TO_NIMBINI=true
elif [[ "$CMD" =~ scp[[:space:]]+will@nimbini:[^[:space:]]+[[:space:]]+ ]]; then
  FROM_NIMBINI=true
elif [[ "$CMD" == *"ssh will@nimbini"*">"*"memory"* ]]; then
  TO_NIMBINI=true
fi

if [ "$TO_NIMBINI" = false ] && [ "$FROM_NIMBINI" = false ]; then
  exit 0
fi

CONTENT=$(cat "$MEM_FILE")

# Platform-specific keyword sets (case-insensitive).
MAC_KEYWORDS='karabiner|hammerspoon|launchd|applescript|homebrew|brew install|codesign|keychain|defaults (read|write)|system extension|touch id|magic keyboard|preview\.app|/Library/|/Applications/|MacOS|cfprefsd|securityd'
LINUX_KEYWORDS='\bniri\b|\bsway\b|\bwayland\b|\bsystemctl\b|\bjournalctl\b|\bpacman\b|\byay\b|kanata|keyd|/etc/systemd|libinput|/dev/input|arch linux|\bxkb\b'

HAS_MAC=$(echo "$CONTENT" | grep -iE "$MAC_KEYWORDS" | head -1 || true)
HAS_LINUX=$(echo "$CONTENT" | grep -iE "$LINUX_KEYWORDS" | head -1 || true)

if [ "$TO_NIMBINI" = true ] && [ -n "$HAS_MAC" ] && [ -z "$HAS_LINUX" ]; then
  cat <<EOF
{"error": "MEMORY PLATFORM CHECK: $MEM_FILE contains Mac-only content (matched line: \"$(echo "$HAS_MAC" | head -c 100 | tr -d '\"')\") and is being propagated to nimbini (Linux). Either (1) skip nimbini for this memory, (2) rewrite it to be platform-neutral or to explicitly scope to Mac, or (3) state why it's still appropriate to propagate. Then retry."}
EOF
  exit 0
fi

if [ "$FROM_NIMBINI" = true ] && [ -n "$HAS_LINUX" ] && [ -z "$HAS_MAC" ]; then
  cat <<EOF
{"error": "MEMORY PLATFORM CHECK: $MEM_FILE contains Linux-only content (matched line: \"$(echo "$HAS_LINUX" | head -c 100 | tr -d '\"')\") and is being propagated from nimbini to Mac. Either (1) skip Mac for this memory, (2) rewrite it, or (3) state why it's still appropriate to propagate. Then retry."}
EOF
  exit 0
fi

exit 0
