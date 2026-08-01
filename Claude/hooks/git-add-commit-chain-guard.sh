#!/bin/bash
# PreToolUse hook: refuse `git add ... ; git commit ...` — require `&&` between them.
#
# The failure this prevents, observed 2026-07-31:
#
#   git rm -q scripts/forge-metadata-backup      # stages the deletion
#   ...
#   git add .dotter/global.toml src/main.rs scripts/forge-metadata-backup; git commit -m "..."
#
# `git add` is ATOMIC across its pathspecs: it validates them all first, and if any
# one fails, NOTHING is added. Here `scripts/forge-metadata-backup` no longer existed
# in the working tree (git rm had removed it), so the add died with
# `fatal: pathspec ... did not match any files` and .dotter/global.toml was never staged.
#
# The `;` then ran the commit anyway. Because *other* things happened to be staged, the
# commit SUCCEEDED — quietly capturing a subset of the intended change. `git push`
# reported success too. The result was a half-landed migration: the nu script was
# retired on both machines while its dotter mapping stayed in git, so the next
# `dotter deploy` anywhere would have silently undone the work.
#
# That is the whole trap: the add fails LOUDLY, the commit succeeds QUIETLY, and the
# push confirms a lie. With `&&`, the non-zero exit from `git add` stops the chain and
# nothing lands at all — a visible failure instead of a plausible-looking partial commit.
#
# Scope: only fires when a single command contains a staging verb AND `git commit`
# separated by `;`. Newline-separated commands are left alone (a script is not a chain).
#
# NB the nushell house rule does not apply here — this hook governs Claude's own Bash
# tool calls, which really do run in bash. Commands written FOR William to paste into
# his terminal must still use nushell syntax.
#
# Input: JSON on stdin with .tool_input.command
# Output: JSON block decision, or exit 0 to pass through

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

[ -z "$COMMAND" ] && exit 0

# Only look at git work.
echo "$COMMAND" | grep -q 'git commit' || exit 0

# Collapse to a single line so a multi-line heredoc commit message does not hide the
# chain, but drop real newline separators first — those are sequential statements, not
# a `;` chain, and carry no false expectation of fail-fast.
SINGLE_LINE=$(echo "$COMMAND" | tr '\n' ' ')

# Look for a staging verb followed by `;` ... `git commit`, with no `&&` between them.
# `git add`/`git rm`/`git mv` all stage, and all can fail on a bad pathspec.
BETWEEN=$(echo "$SINGLE_LINE" | sed -n 's/.*git \(add\|rm\|mv\)\(.*\)git commit.*/\2/p')

if [ -n "$BETWEEN" ]; then
  # A `&&` anywhere between the staging verb and the commit means the chain already
  # fails fast. Nothing to complain about.
  if echo "$BETWEEN" | grep -q '&&'; then
    exit 0
  fi

  if echo "$BETWEEN" | grep -q ';'; then
    jq -n '{
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "block",
        permissionDecisionReason: "BLOCKED: use `&&` between `git add`/`git rm` and `git commit`, not `;`.\n\n`git add` is atomic across its pathspecs — if ONE path is bad (e.g. it was already `git rm`d, so it no longer exists in the working tree), NOTHING is staged. With `;` the commit then runs anyway, succeeds on whatever else happened to be staged, and you get a partial commit that looks like a success. `git push` will confirm the lie.\n\nWith `&&` the failed add stops the chain and nothing lands.\n\nThen VERIFY: `git show --stat HEAD` and check the file list against what you intended to commit.\n\n(This governs your own bash tool calls. Commands you write for William to paste still need nushell `;` syntax.)"
      }
    }'
    exit 0
  fi
fi

exit 0
