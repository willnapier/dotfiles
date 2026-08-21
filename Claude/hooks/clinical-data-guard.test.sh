#!/bin/bash
# Test suite for clinical-data-guard.sh.
#
# Run:  bash ~/.claude/hooks/clinical-data-guard.test.sh
#
# NOT a hook — never wire this into settings.json. It lives beside the hook so
# the guard's behaviour is checked rather than assumed.
#
# Why this exists: the segment-scoping rule shipped once with a `while read`
# that dropped the final unterminated line, so a bare `cat <message-file>` —
# the commonest case there is — passed straight through. The greps all looked
# right. Only running the cases caught it.
#
# Each case feeds the hook a synthetic PreToolUse payload with ANTHROPIC_API_KEY
# unset (the Max / no-DPA path, where the guard is live) and asserts DENY/ALLOW.

HOOK="$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")/clinical-data-guard.sh"
PASS=0; FAIL=0

probe() {
  local expect="$1" label="$2" json="$3" out got
  out=$(printf '%s' "$json" | env -u ANTHROPIC_API_KEY bash "$HOOK" 2>/dev/null)
  if printf '%s' "$out" | grep -q '"deny"'; then got=DENY; else got=ALLOW; fi
  if [ "$got" = "$expect" ]; then
    PASS=$((PASS+1)); printf '  ok   %-6s %s\n' "$got" "$label"
  else
    FAIL=$((FAIL+1)); printf '  FAIL want=%s got=%s  %s\n' "$expect" "$got" "$label"
  fi
}
cmd() { jq -n --arg c "$1" '{tool_input:{command:$c}}'; }
rd()  { jq -n --arg p "$1" '{tool_input:{file_path:$p}}'; }
gp()  { jq -n --arg p "$1" '{tool_input:{path:$p}}'; }

M=/home/will/Mail

echo "=== clinical data rules (original three; must not regress) ==="
probe DENY  "client notes file"                   "$(rd /home/will/Clinical/clients/x/notes.md)"
probe DENY  "tm3 diary capture"                   "$(cmd 'bat /home/will/.local/share/tm3-appointments/2026-08-21.json')"
probe DENY  "persisted clinic session roster"     "$(cmd 'cat ~/.config/practiceforge/session-4471.json')"
probe ALLOW "code that merely says clinical"      "$(rd /home/will/Code/practiceforge/src/clinical_note.rs)"

echo "=== notmuch: content subcommands blocked ==="
probe DENY  "search"                              "$(cmd 'notmuch search --limit=3 "path:cohs/**"')"
probe DENY  "show"                                "$(cmd 'notmuch show thread:0001')"
probe DENY  "reply"                               "$(cmd 'notmuch reply id:abc@x')"
probe DENY  "address"                             "$(cmd 'notmuch address --output=sender "*"')"
probe DENY  "dump"                                "$(cmd 'notmuch dump --output=/tmp/t')"
probe DENY  "global-flag evasion"                 "$(cmd 'notmuch --config=/tmp/nc search foo')"
probe DENY  "buried mid-command"                  "$(cmd 'echo hi; notmuch search foo | head -3')"

echo "=== notmuch: PHI-free subcommands still work ==="
probe ALLOW "count"                               "$(cmd 'notmuch count "path:cohs/** and date:7d.."')"
probe ALLOW "new"                                 "$(cmd 'notmuch new')"
probe ALLOW "tag"                                 "$(cmd 'notmuch tag +seen -- tag:inbox')"
probe ALLOW "config get"                          "$(cmd 'notmuch config get database.path')"

echo "=== maildir: content reads blocked ==="
probe DENY  "Read tool, cohs"                     "$(rd "$M/cohs/INBOX/cur/1786099531.1217261_1.nimbini,U=1515:2,")"
probe DENY  "Read tool, personal"                 "$(rd "$M/personal/INBOX/cur/abc:2,S")"
probe DENY  "Grep path field"                     "$(gp "$M/cohs/INBOX")"
probe DENY  "cat a message"                       "$(cmd "cat $M/cohs/INBOX/cur/abc:2,S")"
probe DENY  "rg across the maildir"               "$(cmd "rg -i 'subject:' $M/cohs/INBOX")"
probe DENY  "grep -r personal"                    "$(cmd "grep -r From $M/personal")"
probe DENY  "head a message"                      "$(cmd "head -50 $M/cohs/INBOX/cur/abc")"

echo "=== maildir: PHI-free infrastructure checks still work ==="
probe ALLOW "ls the maildir"                      "$(cmd "ls -la $M/cohs")"
probe ALLOW "fd file listing"                     "$(cmd "fd . $M/cohs -t f")"
probe ALLOW "stat for mtimes"                     "$(cmd "stat -c %y $M/cohs/INBOX/cur/abc")"
probe ALLOW "wc a file count"                     "$(cmd "ls $M/cohs/INBOX/cur | wc -l")"
probe ALLOW "mbsync service state"                "$(cmd 'systemctl --user status mbsync-cohs.service')"
probe ALLOW "mbsyncrc is config, not mail"        "$(cmd 'rg -n Patterns /home/will/.mbsyncrc')"
probe ALLOW "gmail-rs deliberately uncovered"     "$(cmd "rg orders $M/gmail-rs")"
probe ALLOW "mailforge source"                    "$(rd /home/will/Code/mailforge/src/mail/mod.rs)"

echo "=== segment scoping: reader must share a segment with the path ==="
probe ALLOW "ls maildir | head"                   "$(cmd "ls $M/cohs | head -3")"
probe ALLOW "fd maildir | head"                   "$(cmd "fd . $M/cohs -t f | head -5")"
probe ALLOW "count; then ls | head"               "$(cmd "notmuch count 'path:cohs/**'; ls $M/cohs | head -3")"
probe ALLOW "ls maildir; cat unrelated file"      "$(cmd "ls $M/cohs; cat /etc/hostname")"
probe DENY  "cat a message | head"                "$(cmd "cat $M/cohs/INBOX/cur/abc | head -20")"
probe DENY  "ls elsewhere; rg the maildir"        "$(cmd "ls /tmp; rg From $M/cohs")"
probe DENY  "reader in 2nd segment WITH path"     "$(cmd "echo hi && head -5 $M/personal/INBOX/cur/x")"
probe DENY  "xargs pipeline evasion"              "$(cmd "ls $M/cohs/INBOX/cur | head -1 | xargs cat")"
probe DENY  "|| chained reader"                   "$(cmd "false || bat $M/cohs/INBOX/cur/x")"

echo "=== the DPA path passes everything ==="
out=$(printf '%s' "$(rd /home/will/Clinical/clients/x/notes.md)" | ANTHROPIC_API_KEY=sk-fake bash "$HOOK" 2>/dev/null)
if [ -z "$out" ]; then PASS=$((PASS+1)); echo "  ok   ALLOW  cc-clinical session bypasses the guard"
else FAIL=$((FAIL+1)); echo "  FAIL cc-clinical session was blocked"; fi

echo
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
