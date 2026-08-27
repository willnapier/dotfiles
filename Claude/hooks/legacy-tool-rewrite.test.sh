#!/bin/bash
# Tests for legacy-tool-rewrite.sh
#
# Run: bash ~/.claude/hooks/legacy-tool-rewrite.test.sh
#
# Cases 1-8 are the pre-existing rewrite behaviour (must not regress).
# Cases 9-16 are the 2026-08-27 fixes: heredoc bodies and ssh are DATA, not
# local commands, and must pass through untouched.

HOOK="$(dirname "$0")/legacy-tool-rewrite.sh"
PASS=0
FAIL=0

# run <command> -> prints the rewritten command, or the original if no rewrite
run() {
  local out
  out=$(jq -n --arg c "$1" '{tool_input:{command:$c}}' | bash "$HOOK")
  if [ -z "$out" ]; then
    printf '%s' "$1"
  else
    printf '%s' "$out" | jq -r '.hookSpecificOutput.updatedInput.command'
  fi
}

# expect_contains <name> <command> <needle>
expect_contains() {
  local got; got=$(run "$2")
  if printf '%s' "$got" | grep -qF -- "$3"; then
    PASS=$((PASS+1))
  else
    FAIL=$((FAIL+1)); echo "FAIL: $1"; echo "  in:  $2"; echo "  got: $got"; echo "  want to contain: $3"
  fi
}

# expect_unchanged <name> <command>
expect_unchanged() {
  local got; got=$(run "$2")
  if [ "$got" = "$2" ]; then
    PASS=$((PASS+1))
  else
    FAIL=$((FAIL+1)); echo "FAIL: $1"; echo "  in:  $2"; echo "  got: $got"
  fi
}

# --- pre-existing behaviour: must not regress -------------------------------
expect_contains "grep → rg"            'grep foo file.txt'            'rg foo file.txt'
expect_contains "grep -r drops -r"     'grep -r foo .'                'rg foo .'
expect_contains "grep -n keeps -n"     'grep -n foo file.txt'         'rg -n foo'
expect_contains "piped grep"           'ls | grep foo'                '| rg foo'
expect_contains "find → fd"            'find . -name "*.rs"'          'fd "*.rs" .'
expect_contains "find -type f"         'find . -name "*.rs" -type f'  '-t f'
expect_contains "cat → bat"            'cat file.txt'                 'bat file.txt'
expect_unchanged "pgrep untouched"     'pgrep -af helix'

# --- 2026-08-27: heredoc bodies are DATA ------------------------------------
# The bug: writing a document that QUOTES a legacy command silently rewrote the
# document's content, so the file on disk did not match what was intended.
HD_Q=$'cat > f.md <<\'EOF\'\npgrep -af helix | grep "filename"\nEOF'
expect_contains "heredoc body preserved (quoted delim)" "$HD_Q" 'helix | grep "filename"'

HD_U=$'cat > f.md <<EOF\nrun grep foo bar\nEOF'
expect_contains "heredoc body preserved (bare delim)"   "$HD_U" 'run grep foo bar'

HD_DASH=$'cat > f.md <<-EOF\n\tgrep foo bar\n\tEOF'
expect_contains "heredoc body preserved (<<- form)"     "$HD_DASH" 'grep foo bar'

HD_CUSTOM=$'cat > f.md <<\'DOCEOF\'\nfind . -name "*.md"\nDOCEOF'
expect_contains "heredoc body preserved (custom delim)" "$HD_CUSTOM" 'find . -name "*.md"'

# Command text AROUND a heredoc is still rewritten.
HD_AFTER=$'cat > f.md <<\'EOF\'\ngrep inside\nEOF\ngrep outside f.md'
expect_contains "body untouched, trailing command rewritten" "$HD_AFTER" 'grep inside'
expect_contains "trailing command after heredoc IS rewritten" "$HD_AFTER" 'rg outside'

# Two heredocs in one command.
HD_TWO=$'cat > a <<\'EOF\'\ngrep one\nEOF\ncat > b <<\'EOF\'\ngrep two\nEOF'
expect_contains "second heredoc body preserved" "$HD_TWO" 'grep two'

# --- 2026-08-27: ssh runs elsewhere -----------------------------------------
# Remote tooling is not ours to assume, and grep BRE != rg regex.
expect_unchanged "ssh untouched"       'ssh mac "ioreg -l | grep '"'"'+-o'"'"'"'
expect_unchanged "ssh with cat"        'ssh mac "cat /etc/hosts"'
expect_unchanged "piped ssh untouched" 'echo x | ssh mac "grep foo"'

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
