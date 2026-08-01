#!/bin/bash
# PreToolUse hook: block reads of clinical PHI *data* when the session is NOT on
# the DPA'd Anthropic API path.
#
# Policy (Will, controller, 2026-06-15): bulk PHI-FREE dev runs on the cheap Max
# subscription (just `claude`). Reading real clinical DATA — special-category
# health data — must only transit Anthropic on the commercial **DPA + no-training
# API** path, i.e. a `cc-clinical` session (which sets ANTHROPIC_API_KEY).
#
# This hook fires ONLY when ANTHROPIC_API_KEY is unset (= the consumer Max
# subscription, no DPA, may train). On a `cc-clinical` / API session it passes
# everything through. It targets DATA paths only — code under ~/Code is never
# touched, so normal development is unaffected.
#
# Data paths (require path context so we don't block code that merely mentions
# the word "clinical"):
#   /Clinical/              — ~/Clinical (→ Dropbox/Clinical): client files,
#                             notes.md, identity.yaml, tm3-client-map.toml,
#                             comparisons.jsonl
#   /tm3-appointments/      — TM3 diary captures (full names)
#   practiceforge/session-N — persisted clinic session JSONs (full-name rosters)
#
# Limitation (honest): catches the known data paths via Read/Grep/Glob inputs and
# Bash command strings. It cannot intercept every conceivable read (a custom
# tool, or PHI in an unexpected location). It is a strong backstop, not airtight.
#
# Input: JSON on stdin. Output: a PreToolUse "block" decision, or exit 0 to pass.

# On the DPA/API path → allow everything (clinical reads are fine there).
[ -n "$ANTHROPIC_API_KEY" ] && exit 0

INPUT=$(cat)

# Collect the path/command fields across Read (file_path), Grep/Glob (path),
# and Bash (command).
TARGET=$(echo "$INPUT" | jq -r '
  [.tool_input.file_path, .tool_input.filePath, .tool_input.path, .tool_input.command]
  | map(select(. != null)) | join(" ")' 2>/dev/null)

[ -z "$TARGET" ] && exit 0

if echo "$TARGET" | grep -Eq '/Clinical/|/tm3-appointments/|practiceforge/session-[0-9]'; then
  jq -n '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: "BLOCKED — clinical PHI data on a consumer (Max) session. This session has no ANTHROPIC_API_KEY, so it is the no-DPA / may-train subscription path, and special-category health data must not transit it. Read clinical data only in a `cc-clinical` session (DPA + no-training API key); PHI-free code work stays here on Max. If this is a false positive (a path that merely mentions a clinical directory), rephrase to avoid the clinical-data path."
    }
  }'
  exit 0
fi

exit 0
