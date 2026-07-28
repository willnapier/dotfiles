#!/usr/bin/env bash
# SessionStart:compact hook — restore persona context after a compaction,
# but ONLY in persona sessions.
#
# WHY THIS EXISTS
# ---------------
# Compaction discards the context a persona skill's preamble loaded at session
# start. The persona skill does not re-run, so a compacted Seneca session loses
# Jenny, the friendship philosophy, and the intellectual positions exactly when
# it most needs them. This hook puts them back.
#
# WHY IT IS GATED
# ---------------
# The predecessor of this hook was an unconditional `cat` wired straight into
# settings.json. It fired on EVERY compaction in EVERY project, so a compacted
# practiceforge Rust session was paying ~11.7k tokens for William's marriage
# history and clarinet story. Gating on the working directory does NOT work:
# senior-dev and life-optimiser both run from ~ (measured 2026-07-28 — 10
# senior-dev invocations from /Users/williamnapier in a 15-day window). The only
# reliable signal is which persona the session actually invoked, which is
# readable from the transcript.
#
# Reads SessionStart hook input from stdin (JSON with .transcript_path).
# Emits nothing at all unless a persona skill was invoked. Always exits 0 —
# a context restorer must never be able to break session start.

set -uo pipefail

# Personas whose sessions want the biographical/philosophical/social context.
# senior-dev is deliberately ABSENT — dev sessions have no use for it.
PERSONAS='life-optimiser|philosophy-tutor|music-scr|diana'

# Vendor-neutral context files. These live in ~/Assistants/shared/ and are read
# by any harness; nothing here is Claude-specific except the hook mechanism.
CONTEXT_FILES=(
  "$HOME/Assistants/shared/WILLIAM-PHILOSOPHICAL.md"
  "$HOME/Assistants/shared/WILLIAM-SOCIAL.md"
)
# WILLIAM-BIOGRAPHICAL.md is deliberately omitted: it is the most static of the
# three and the least likely to change what the persona would say next. Add it
# back here if a compacted session is observed missing life-history detail.

INPUT=$(cat)

TRANSCRIPT=$(echo "$INPUT" | jq -r '.transcript_path // empty' 2>/dev/null)
[ -n "$TRANSCRIPT" ] || exit 0
[ -f "$TRANSCRIPT" ] || exit 0

# Match both invocation forms: the slash-command entry and the Skill tool call.
if ! grep -qE "(<command-name>/?($PERSONAS)</command-name>|\"skill\":\"($PERSONAS)\")" "$TRANSCRIPT" 2>/dev/null; then
  exit 0
fi

for f in "${CONTEXT_FILES[@]}"; do
  [ -f "$f" ] && cat "$f"
done

exit 0
