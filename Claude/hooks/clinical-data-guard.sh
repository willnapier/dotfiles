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
#   Clinical dir            — the Dropbox-backed client tree: client files,
#                             notes.md, identity.yaml, tm3-client-map.toml,
#                             comparisons.jsonl
#   tm3-appointments dir    — TM3 diary captures (full names)
#   practiceforge/session-N — persisted clinic session JSONs (full-name rosters)
#
# MAIL (added 2026-08-21). COHS and Napier mail are practice-bearing stores:
# client mail, GP correspondence, appointment traffic. They were NOT covered by
# the original pattern, so a `notmuch search` on a Max session emitted real
# names into a transcript — observed, not theoretical.
#
# Mail is guarded by EMISSION, not by mention. A blanket block on "notmuch" or
# the maildir root would also block PHI-free mail-infrastructure work (sync
# health, folder counts, file mtimes, timer state), which is the bulk of what
# mail debugging actually is. Over-blocking teaches working-around, so:
#
#   BLOCKED   notmuch show|search|reply|address|dump   (emit subjects/senders/bodies)
#             cat|bat|rg|head|sed|... targeting the cohs or personal maildirs
#             Read/Grep/Glob with a path field inside those maildirs
#   ALLOWED   notmuch count|new|tag|config             (emit integers/nothing)
#             ls|fd|stat|wc on the same paths          (maildir filenames are opaque)
#             systemctl, mbsync logs, timer state
#
# The gmail-rs store is deliberately NOT covered: it is newsletters/orders and is
# the working corpus for mailcurator dev, which is PHI-free. Revisit if practice
# mail ever lands there.
#
# Limitation (honest): catches the known data paths via Read/Grep/Glob inputs and
# Bash command strings. It cannot intercept every conceivable read (a custom
# tool, a shell alias, PHI in an unexpected location), and it does NOT scan
# Write/Edit *content*, so it cannot stop PHI being written out. It is a strong
# backstop, not airtight.
#
# Input: JSON on stdin. Output: a PreToolUse "block" decision, or exit 0 to pass.

# On the DPA/API path → allow everything (clinical reads are fine there).
[ -n "$ANTHROPIC_API_KEY" ] && exit 0

INPUT=$(cat)

# Path fields and command strings are kept SEPARATE. A path field (Read/Grep/
# Glob) always means "emit this file"; a command string may be a PHI-free
# operation on the same directory, so it needs a second discriminator.
PATHS=$(echo "$INPUT" | jq -r '
  [.tool_input.file_path, .tool_input.filePath, .tool_input.path]
  | map(select(. != null)) | join(" ")' 2>/dev/null)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // ""' 2>/dev/null)

TARGET="$PATHS $CMD"
[ -z "${TARGET// /}" ] && exit 0

CLINICAL_RE='/Clinical/|/tm3-appointments/|practiceforge/session-[0-9]'
MAIL_RE='/Mail/(cohs|personal)(/|$|["'"'"'[:space:]])'
# Utilities that print file contents into the transcript.
READER_RE='(^|[^[:alnum:]_-])(cat|bat|head|tail|less|more|rg|grep|egrep|fgrep|awk|sed|strings|xxd|od|hexdump|nl|tac)([^[:alnum:]_-]|$)'
# notmuch subcommands that emit subjects, senders or bodies. Tolerates global
# flags, e.g. `notmuch --config=/x search ...`.
NOTMUCH_RE='(^|[^[:alnum:]_-])notmuch([[:space:]]+--[^[:space:]]+)*[[:space:]]+(show|search|reply|address|dump)([^[:alnum:]_-]|$)'

deny() {
  jq -n --arg why "$1" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "deny",
      permissionDecisionReason: $why
    }
  }'
  exit 0
}

BASE="This session has no ANTHROPIC_API_KEY, so it is the no-DPA / may-train subscription path, and special-category health data must not transit it. Read clinical data only in a \`cc-clinical\` session (DPA + no-training API key); PHI-free work stays here on Max."

if echo "$TARGET" | grep -Eq "$CLINICAL_RE"; then
  deny "BLOCKED — clinical PHI data on a consumer (Max) session. $BASE If this is a false positive (a path that merely mentions a clinical directory), rephrase to avoid the clinical-data path."
fi

if echo "$CMD" | grep -Eq "$NOTMUCH_RE"; then
  deny "BLOCKED — this notmuch subcommand emits mail content (subjects, senders, bodies) and the index spans practice-bearing stores. $BASE For PHI-free mail-infrastructure work use \`notmuch count\`, file mtimes under the maildir, or systemd/mbsync state — those are not blocked."
fi

if echo "$PATHS" | grep -Eq "$MAIL_RE"; then
  deny "BLOCKED — reading a file inside a practice-bearing maildir on a consumer (Max) session. $BASE To check sync health without content, use \`ls\`/\`fd\`/\`stat\` on the maildir or \`notmuch count\` — those are not blocked."
fi

if echo "$CMD" | grep -Eq "$MAIL_RE" && echo "$CMD" | grep -Eq "$READER_RE"; then
  deny "BLOCKED — this command prints the contents of practice-bearing mail into the transcript. $BASE Directory-level checks (\`ls\`, \`fd\`, \`stat\`, \`wc\`) on the same paths are not blocked, and maildir filenames are opaque."
fi

exit 0
