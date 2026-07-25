#!/usr/bin/env bash
# Stop hook: documentation gate enforcement
#
# After each assistant response, checks whether files were modified.
# If substantive files changed (scripts, configs, Rust source, skill files)
# but no documentation was updated (~/Assistants/shared/, ARCHITECTURE.md,
# CLINICAL-PRIVACY-SYSTEM.md, etc.), injects a reminder.
#
# This is the mechanised documentation gate — not a suggestion.

set -euo pipefail

INPUT=$(cat)

TRANSCRIPT=$(echo "$INPUT" | jq -r '.transcript_path // empty')

if [ -z "$TRANSCRIPT" ] || [ ! -f "$TRANSCRIPT" ]; then
  exit 0
fi

# Look at recent tool calls (last 50 lines should cover the current turn)
RECENT=$(tail -50 "$TRANSCRIPT" 2>/dev/null || true)

# Extract file paths from Write/Edit tool calls
MODIFIED_FILES=$(echo "$RECENT" | grep -oE '"file_path"\s*:\s*"[^"]+"' | grep -oE '/[^"]+' || true)

if [ -z "$MODIFIED_FILES" ]; then
  exit 0
fi

# Categorise: which are substantive changes vs documentation
HAS_SUBSTANTIVE=false
HAS_DOC=false

while IFS= read -r filepath; do
  case "$filepath" in
    # Documentation files
    */Assistants/shared/*.md | */ARCHITECTURE.md | */CLINICAL-PRIVACY-SYSTEM.md | \
    */LEIGH-ONBOARDING-NOTES.md | */ASSISTANT-HANDOFF.md | */SKILL-INFRASTRUCTURE.md | \
    */.claude/projects/*/memory/*.md | */memory/MEMORY.md)
      HAS_DOC=true
      ;;
    # Skill files (these ARE documentation)
    */.claude/skills/*/SKILL.md | */.claude/skills/*.md)
      HAS_DOC=true
      ;;
    # Cheatsheets colocated with their tool's config (man page sources)
    */dotfiles/meli/meli-mine.md)
      HAS_DOC=true
      ;;
    # DayPage entries (not substantive changes)
    */DayPages/*.md)
      ;;
    # Ignore temp files and logs
    /tmp/* | *.log)
      ;;
    # Everything else is substantive
    *.rs | *.nu | *.sh | *.toml | *.yaml | *.json | *.py | *.d2)
      HAS_SUBSTANTIVE=true
      ;;
    */.local/bin/*)
      HAS_SUBSTANTIVE=true
      ;;
    */dotfiles/*)
      HAS_SUBSTANTIVE=true
      ;;
  esac
done <<< "$MODIFIED_FILES"

if [ "$HAS_SUBSTANTIVE" = true ] && [ "$HAS_DOC" = false ]; then
  echo '{"error": "DOCUMENTATION GATE: You modified code, scripts, or configuration in this response but did not update any documentation. Check: does any doc in ~/Assistants/shared/, ARCHITECTURE.md, skill files, or ASSISTANT-HANDOFF.md need updating to reflect this change? If so, update now. If genuinely no documentation is affected, state why."}'
fi

exit 0
