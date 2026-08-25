#!/bin/bash
# Tests for grok-forum-waiter-gate.
# Run: bash ~/dotfiles/grok/hooks/grok-forum-waiter-gate.test.sh
set -euo pipefail

GATE="${GATE:-$HOME/dotfiles/scripts/grok-forum-waiter-gate}"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

pass=0
fail=0

run_case() {
  local name="$1" chat="$2" jobs="$3" expect="$4"
  local out
  out=$(GROK_FORUM_WAITER_CHAT_HISTORY="$chat" GROK_FORUM_WAITER_JOBS="$jobs" \
    python3 "$GATE" <<<"{\"reason\":\"end_turn\",\"sessionId\":\"test\"}" || true)
  if [[ "$expect" == "block" ]]; then
    if printf '%s' "$out" | grep -q '"decision": "block"'; then
      echo "  ok   $name"
      pass=$((pass + 1))
    else
      echo "  FAIL $name (expected block, got: $out)"
      fail=$((fail + 1))
    fi
  else
    if [[ -z "$out" ]]; then
      echo "  ok   $name"
      pass=$((pass + 1))
    else
      echo "  FAIL $name (expected allow/empty, got: $out)"
      fail=$((fail + 1))
    fi
  fi
}

CHAT_UNWATCHED="$TMP/unwatched.jsonl"
cat >"$CHAT_UNWATCHED" <<'EOF'
{"type":"assistant","tool_calls":[{"id":"1","name":"run_terminal_command","arguments":"{\"command\":\"forum convene meta-x --caller grok-build --background\"}"}]}
{"type":"tool_result","tool_call_id":"1","content":"Queued forum job: meta-x-r2-1\n"}
EOF

CHAT_WATCHED="$TMP/watched.jsonl"
cat >"$CHAT_WATCHED" <<'EOF'
{"type":"assistant","tool_calls":[{"id":"1","name":"run_terminal_command","arguments":"{\"command\":\"forum convene meta-x --caller grok-build --background\"}"}]}
{"type":"tool_result","tool_call_id":"1","content":"Queued forum job: meta-x-r2-1\n"}
{"type":"assistant","tool_calls":[{"id":"2","name":"monitor","arguments":"{\"command\":\"job='meta-x-r2-1'\\nwhile true; do forum inbox; done\"}"}]}
EOF

CHAT_EMPTY="$TMP/empty.jsonl"
: >"$CHAT_EMPTY"

CHAT_HELP="$TMP/help.jsonl"
cat >"$CHAT_HELP" <<'EOF'
{"type":"assistant","tool_calls":[{"id":"1","name":"run_terminal_command","arguments":"{\"command\":\"forum convene --help\"}"}]}
{"type":"tool_result","tool_call_id":"1","content":"Usage: forum convene\n"}
EOF

echo "grok-forum-waiter-gate tests"
run_case "unwatched active job blocks" "$CHAT_UNWATCHED" "queuedmeta-x-r2-1thread=meta-x" block
run_case "monitor on job id allows" "$CHAT_WATCHED" "queuedmeta-x-r2-1thread=meta-x" allow
run_case "completed unwatched job allows" "$CHAT_UNWATCHED" "completedmeta-x-r2-1thread=meta-x" allow
run_case "no jobs allows" "$CHAT_EMPTY" "" allow
run_case "convene --help allows" "$CHAT_HELP" "queuedmeta-x-r2-1thread=meta-x" allow

# session-end Stop must not gate
out=$(GROK_FORUM_WAITER_CHAT_HISTORY="$CHAT_UNWATCHED" GROK_FORUM_WAITER_JOBS="queuedmeta-x-r2-1" \
  python3 "$GATE" <<<"{\"reason\":\"shutdown\",\"sessionId\":\"test\"}" || true)
if [[ -z "$out" ]]; then
  echo "  ok   session-end Stop ignored"
  pass=$((pass + 1))
else
  echo "  FAIL session-end Stop ignored (got: $out)"
  fail=$((fail + 1))
fi

echo "$pass passed, $fail failed"
if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
