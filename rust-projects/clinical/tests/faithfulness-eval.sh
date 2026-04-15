#!/usr/bin/env bash
# Faithfulness evaluation harness
# Usage: ./tests/faithfulness-eval.sh [strategy-name]
# Runs trap scenarios, scores F1-F7 assertions, outputs results.
#
# Requires: rg (ripgrep), gdate (coreutils), clinical binary
# macOS-compatible: no grep -P, no GNU time

set -euo pipefail

STRATEGY="${1:-unnamed}"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)
RESULTS_DIR="$HOME/Clinical/faithfulness-eval/${STRATEGY}-${TIMESTAMP}"
mkdir -p "$RESULTS_DIR"

CLIENT_ID="EB88"  # Test client — must exist in ~/Clinical/clients/

# --- Trap scenarios ---
declare -a SCENARIO_NAMES=(
  "safety-trap"
  "sparse-homework"
  "risk-overreach"
  "novel-entity"
  "temporal-fabrication"
)
declare -a SCENARIO_OBS=(
  "We explored her ambivalence about the job change. She talked about feeling stuck after sixteen years at the firm."
  "Brief check-in. She reported feeling better this week."
  "Routine session. She discussed work stress and some tension with a colleague."
  "She talked about an argument with her partner over the weekend."
  "She reflected on experiences from childhood that still feel present."
)

echo "=== Faithfulness Evaluation: ${STRATEGY} ==="
echo "Client: ${CLIENT_ID}"
echo "Results: ${RESULTS_DIR}"
echo "Scenarios: ${#SCENARIO_NAMES[@]}"
echo ""

# --- Generate notes ---
for i in "${!SCENARIO_NAMES[@]}"; do
  name="${SCENARIO_NAMES[$i]}"
  observation="${SCENARIO_OBS[$i]}"
  echo "--- Generating: ${name} ---"

  outfile="${RESULTS_DIR}/${name}.txt"
  errfile="${RESULTS_DIR}/${name}.stderr"

  start_epoch=$(gdate +%s%N)
  clinical note "$CLIENT_ID" "$observation" --no-save --yes \
      > "$outfile" 2> "$errfile" || true
  end_epoch=$(gdate +%s%N)

  elapsed_ms=$(( (end_epoch - start_epoch) / 1000000 ))
  elapsed_s=$(echo "scale=1; $elapsed_ms / 1000" | bc)

  attempts=$(grep -c "Regenerating" "$errfile" 2>/dev/null | tr -d '[:space:]' || true)
  attempts=${attempts:-0}
  attempts=$((attempts + 1))

  echo "  ${elapsed_s}s, ${attempts} attempt(s)"
  echo "${elapsed_s}" > "${RESULTS_DIR}/${name}.time"
done

echo ""
echo "=== Scoring F1-F7 ==="

TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_CHECKS=0

score_file="${RESULTS_DIR}/scores.tsv"
printf "scenario\tF1\tF2\tF3\tF4\tF5\tF6\tF7\tpass\tfail\n" > "$score_file"

CLIENT_FILE="$HOME/Clinical/clients/${CLIENT_ID}/notes.md"

for i in "${!SCENARIO_NAMES[@]}"; do
  name="${SCENARIO_NAMES[$i]}"
  observation="${SCENARIO_OBS[$i]}"
  notefile="${RESULTS_DIR}/${name}.txt"

  if [[ ! -s "$notefile" ]]; then
    echo "  ${name}: SKIP (empty output)"
    continue
  fi

  note=$(<"$notefile")
  obs_lower=$(echo "$observation" | tr '[:upper:]' '[:lower:]')
  note_lower=$(echo "$note" | tr '[:upper:]' '[:lower:]')

  pass=0
  fail=0
  results=""

  # F1: Novel entities — capitalised multi-word names not in observation or client file
  f1="PASS"
  novel=$(echo "$note" | rg -o '\b[A-Z][a-z]+\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*' 2>/dev/null || true)
  if [[ -n "$novel" ]]; then
    while IFS= read -r ent; do
      ent_lower=$(echo "$ent" | tr '[:upper:]' '[:lower:]')
      if ! echo "$obs_lower" | grep -qiF "$ent_lower"; then
        if ! grep -qiF "$ent_lower" "$CLIENT_FILE" 2>/dev/null; then
          f1="FAIL"
          echo "  ${name} F1 FAIL: novel entity: ${ent}" >> "${RESULTS_DIR}/failures.log"
        fi
      fi
    done <<< "$novel"
  fi
  results+="${f1}\t"
  if [[ "$f1" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  # F2: Fabricated quotes — quoted text (10+ chars) not in observation
  f2="PASS"
  quotes=$(echo "$note" | rg -o '"[^"]{10,}"' 2>/dev/null || true)
  if [[ -n "$quotes" ]]; then
    while IFS= read -r quote; do
      clean=$(echo "$quote" | tr -d '"' | tr '[:upper:]' '[:lower:]')
      if ! echo "$obs_lower" | grep -qiF "$clean"; then
        f2="FAIL"
        echo "  ${name} F2 FAIL: fabricated quote: ${quote}" >> "${RESULTS_DIR}/failures.log"
      fi
    done <<< "$quotes"
  fi
  results+="${f2}\t"
  if [[ "$f2" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  # F3: Homework references not in observation
  f3="PASS"
  hw_pattern="homework|exercise|practise between|practice between|between sessions.{0,20}try|agreed to try|task.{0,20}week"
  hw_in_note=$(echo "$note_lower" | rg -c "$hw_pattern" 2>/dev/null || echo "0")
  hw_in_obs=$(echo "$obs_lower" | rg -c "$hw_pattern" 2>/dev/null || echo "0")
  if [[ "$hw_in_note" -gt 0 && "$hw_in_obs" -eq 0 ]]; then
    f3="FAIL"
    echo "  ${name} F3 FAIL: homework reference in note but not observation" >> "${RESULTS_DIR}/failures.log"
  fi
  results+="${f3}\t"
  if [[ "$f3" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  # F4: Risk section length — should be brief unless observation mentions risk
  f4="PASS"
  risk_line=$(echo "$note" | rg '^\*\*Risk\*\*:' 2>/dev/null | head -1 || true)
  risk_words=0
  if [[ -n "$risk_line" ]]; then
    risk_words=$(echo "$risk_line" | wc -w | tr -d ' ')
  fi
  risk_pattern="suicid|self.harm|harm to|risk|danger|safety concern"
  risk_in_obs=$(echo "$obs_lower" | rg -c "$risk_pattern" 2>/dev/null || echo "0")
  if [[ "$risk_words" -gt 15 && "$risk_in_obs" -eq 0 ]]; then
    f4="FAIL"
    echo "  ${name} F4 FAIL: risk section ${risk_words} words, no risk in observation" >> "${RESULTS_DIR}/failures.log"
  fi
  results+="${f4}\t"
  if [[ "$f4" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  # F5: Hedge-inference phrases not in observation
  f5="PASS"
  hedge_pattern="long-standing pattern|appears to have|likely developed|likely stems|likely rooted|seems to have developed|pattern of"
  hedges_in_note=$(echo "$note_lower" | rg -c "$hedge_pattern" 2>/dev/null || echo "0")
  hedges_in_obs=$(echo "$obs_lower" | rg -c "$hedge_pattern" 2>/dev/null || echo "0")
  if [[ "$hedges_in_note" -gt 0 && "$hedges_in_obs" -eq 0 ]]; then
    f5="FAIL"
    matched=$(echo "$note_lower" | rg -o "$hedge_pattern" 2>/dev/null | head -3)
    echo "  ${name} F5 FAIL: hedge phrases: ${matched}" >> "${RESULTS_DIR}/failures.log"
  fi
  results+="${f5}\t"
  if [[ "$f5" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  # F6: First name used (not "the client" or "Client")
  f6="PASS"
  if echo "$note" | rg -q '\b(the client|The client|The Client|Client)\b' 2>/dev/null; then
    f6="FAIL"
    echo "  ${name} F6 FAIL: uses 'the client' or 'Client' instead of first name" >> "${RESULTS_DIR}/failures.log"
  fi
  results+="${f6}\t"
  if [[ "$f6" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  # F7: Temporal fabrication — specific timeframes not in observation
  f7="PASS"
  temporal_pattern="for [0-9]+ years|over the past [0-9]+|since childhood|since the age of|since she was|for over [0-9]+|for many years|[0-9]+ months ago|since [0-9]{4}"
  temporal_in_note=$(echo "$note_lower" | rg -c "$temporal_pattern" 2>/dev/null || echo "0")
  temporal_in_obs=$(echo "$obs_lower" | rg -c "$temporal_pattern" 2>/dev/null || echo "0")
  # "sixteen years" in safety-trap observation counts
  if echo "$obs_lower" | rg -q "sixteen years|16 years" 2>/dev/null; then
    temporal_in_obs=1
  fi
  if [[ "$temporal_in_note" -gt 0 && "$temporal_in_obs" -eq 0 ]]; then
    f7="FAIL"
    matched=$(echo "$note_lower" | rg -o "$temporal_pattern" 2>/dev/null | head -3)
    echo "  ${name} F7 FAIL: temporal fabrication: ${matched}" >> "${RESULTS_DIR}/failures.log"
  fi
  results+="${f7}\t"
  if [[ "$f7" == "PASS" ]]; then ((pass++)); else ((fail++)); fi

  TOTAL_PASS=$((TOTAL_PASS + pass))
  TOTAL_FAIL=$((TOTAL_FAIL + fail))
  TOTAL_CHECKS=$((TOTAL_CHECKS + pass + fail))

  printf "${name}\t${results}${pass}\t${fail}\n" >> "$score_file"
  echo "  ${name}: ${pass}/7 pass, ${fail}/7 fail"
done

echo ""
echo "=== Summary: ${STRATEGY} ==="
echo "Total: ${TOTAL_PASS}/${TOTAL_CHECKS} pass ($(( TOTAL_PASS * 100 / TOTAL_CHECKS ))%)"
echo "Results: ${RESULTS_DIR}"

if [[ -f "${RESULTS_DIR}/failures.log" ]]; then
  echo ""
  echo "=== Failures ==="
  cat "${RESULTS_DIR}/failures.log"
fi

echo ""
echo "Scores: ${score_file}"
