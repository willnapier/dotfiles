#!/usr/bin/env bash
# Stop hook: D2 diagram impact gate enforcement
#
# After each assistant response, checks whether files were modified that
# could affect a flow described in ~/Forge/diagrams/*.d2. If a "diagram-
# relevant" source changed but no .d2 diagram was updated in the same turn,
# injects a reminder requiring the assistant to either update the diagram
# or explain why no update is needed.
#
# Companion to doc-gate-check.sh — same pattern, different concern.
# Mechanises the Diagram Impact Gate previously documented as instruction.

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

# Categorise: which are diagram-relevant changes vs diagram updates
HAS_DIAGRAM_RELEVANT=false
HAS_DIAGRAM_UPDATE=false

while IFS= read -r filepath; do
  case "$filepath" in
    # D2 diagram source/output (either counts as "diagram updated")
    */Forge/diagrams/*.d2 | */Forge/diagrams/*.svg)
      HAS_DIAGRAM_UPDATE=true
      ;;
    # PracticeForge Rust workspace — drives clinical-{pipeline,session,letters,privacy,auth,responsibilities}.d2.
    # Source moved from ~/dotfiles/rust-projects/clinical/crates/* to ~/Code/practiceforge/* in the
    # 2026-04-30 repo extraction; this gate watched the dead (non-existent) path until 2026-06-29.
    # Three workspace crates: clinical-core (dephi/privacy), clinical-portal (letter delivery),
    # practiceforge (notes/auth/onboarding/UI). NB the clinical-notes skill was retired 2026-05-06;
    # note-generation prompts now live in ~/.config/practiceforge/prompts/ (edited under ~/dotfiles) —
    # the note-gen *code* is covered by the practiceforge crate below.
    */Code/practiceforge/clinical-core/src/* | */Code/practiceforge/clinical-portal/src/* | */Code/practiceforge/practiceforge/src/*)
      HAS_DIAGRAM_RELEVANT=true
      ;;
    # Bequest source (drives bequest-system.d2)
    */dotfiles/rust-projects/bequest/src/*)
      HAS_DIAGRAM_RELEVANT=true
      ;;
    # Clinical scripts in dotfiles
    */dotfiles/scripts/clinic-* | */dotfiles/scripts/clinical-*)
      HAS_DIAGRAM_RELEVANT=true
      ;;
    # DayPage entries are not diagram-relevant
    */DayPages/*.md)
      ;;
    # Ignore temp files
    /tmp/* | *.log)
      ;;
  esac
done <<< "$MODIFIED_FILES"

if [ "$HAS_DIAGRAM_RELEVANT" = true ] && [ "$HAS_DIAGRAM_UPDATE" = false ]; then
  echo '{"error": "DIAGRAM IMPACT GATE: You modified PracticeForge/bequest source or clinic scripts in this response but did not update any D2 diagram in ~/Forge/diagrams/. Check whether the change alters a flow described in clinical-{session,pipeline,letters,auth,privacy,responsibilities}.d2 or bequest-system.d2. If yes, update the .d2 source, regenerate the SVG (`clinical-diagrams-render` for the batch, or `d2 --layout elk <file>.d2 <file>.svg`), and run `diagram-lint`. If genuinely no diagram is affected, state which diagrams you considered and why none needed updating."}'
fi

exit 0
