#!/bin/bash
# PreToolUse hook: rewrite legacy Unix tools to modern Rust equivalents.
#
# Rewrites:
#   grep [flags] → rg [translated flags]  (not pgrep/egrep/fgrep)
#   find <path> -name "pattern" → fd "pattern" <path>
#   cat <file> → bat <file>  (standalone only, not when piped to another command)
#
# Does NOT rewrite (too context-dependent):
#   sed  (different syntax from sd — correct fix is Edit tool or sd)
#
# 🚨 DOES NOT REWRITE INSIDE HEREDOC BODIES (added 2026-08-27).
#   A heredoc body is DATA being written to a file, not a command being run.
#   Rewriting it silently corrupts file content. Real case: writing a doc that
#   quoted `pgrep -af helix | grep "filename"` produced a file containing
#   `... | rg "filename"` instead, so a later search-and-replace against that
#   file found nothing and the edit failed twice before the cause was spotted.
#   The corruption is silent: the command succeeds, the file is just wrong.
#
# 🚨 DOES NOT REWRITE ssh COMMANDS (added 2026-08-27).
#   Text inside an ssh command string runs on ANOTHER machine, so we cannot
#   assume rg/fd/bat exist there. Worse, the rewrite is not semantics-preserving:
#   grep takes BRE/ERE, rg takes Rust regex. Real case:
#   `ssh mac "ioreg ... | grep '+-o'"` became `rg '+-o'`, which fails with
#   "regex parse error: repetition operator missing expression".
#
# KNOWN LIMIT, not fixed: the same regex-dialect mismatch exists for LOCAL
#   commands. grep patterns valid as BRE may be invalid or mean something else
#   in rg. Rewriting the tool does not rewrite the pattern. Accepted because
#   local rg is house policy; if a local rewrite ever mangles a pattern, that is
#   this limitation, not a new bug.
#
# Input: JSON on stdin with .tool_input.command
# Output: JSON on stdout with updatedInput if rewritten, or exit 0 to pass through

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

[ -z "$COMMAND" ] && exit 0

REWRITTEN=$(HOOK_COMMAND="$COMMAND" python3 << 'PYEOF'
import sys, re, os

cmd = os.environ.get("HOOK_COMMAND", "")
if not cmd:
    sys.exit(1)

original = cmd

# --- Guard 1: never touch a command that reaches another machine -------------
# The remote host's tooling is not ours to assume, and grep→rg is not
# semantics-preserving across regex dialects.
if re.search(r'(?:^|[|;&(]\s*)ssh\b', cmd):
    sys.exit(1)


def rewrite_grep_match(m):
    """Rewrite a single grep invocation to rg."""
    prefix = m.group(1)  # pipe, semicolon, or start
    flags = m.group(2).strip() if m.group(2) else ""
    rest = m.group(3)

    # Translate flags
    new_flags = []
    i = 0
    while i < len(flags):
        c = flags[i]
        if c == '-':
            i += 1
            continue
        elif c in ('r', 'R'):
            pass  # rg is recursive by default
        elif c == 'E':
            pass  # rg uses extended regex by default
        elif c == 'P':
            new_flags.append('-P')
        elif c in ('n', 'i', 'l', 'c', 'v', 'w', 'o', 'h', 'H', 'F'):
            new_flags.append(f'-{c}')
        elif c == ' ':
            pass
        else:
            # Unknown flag — keep it, rg may support it
            new_flags.append(f'-{c}')
        i += 1

    flag_str = ' '.join(new_flags)
    if flag_str:
        flag_str = ' ' + flag_str
    return f'{prefix}rg{flag_str} {rest}'


def rewrite_find_match(m):
    prefix = m.group(1)
    path = m.group(2)
    quote = m.group(3) or ''
    pattern = m.group(4)
    type_match = m.group(5) or ''

    fd_type = ''
    if type_match:
        t = type_match.strip().split()[-1] if type_match.strip() else ''
        if t == 'f':
            fd_type = ' -t f'
        elif t == 'd':
            fd_type = ' -t d'

    return f'{prefix}fd {quote}{pattern}{quote} {path}{fd_type}'


def rewrite_cat_match(m):
    prefix = m.group(1)
    args = m.group(2)
    return f'{prefix}bat {args}'


def apply_rewrites(text):
    """The three rewrites. Applied ONLY to command text, never heredoc bodies."""
    # grep → rg. Match grep preceded by start of line, pipe, semicolon, &&, ||,
    # $( or backtick. Not pgrep/egrep/fgrep/zgrep.
    text = re.sub(
        r'((?:^|[|;&`]\s*|\$\(\s*))grep\s+((?:-[a-zA-Z]+\s+)*)(.*?)(?=\s*(?:[|;&]|$))',
        rewrite_grep_match,
        text,
        flags=re.MULTILINE
    )
    # find <path> -name "pattern" [-type f|d] → fd
    text = re.sub(
        r'((?:^|[|;&`]\s*|\$\(\s*))find\s+(\S+)\s+-name\s+(["\']?)([^"\'\s]+)\3(\s+-type\s+[fd])?',
        rewrite_find_match,
        text,
        flags=re.MULTILINE
    )
    # cat <file> → bat, standalone only (not piped, bat's formatting breaks pipes)
    text = re.sub(
        r'((?:^|[;&]\s*))cat\s+([^|;&\n]+?)(?=\s*(?:[;&]|$))',
        rewrite_cat_match,
        text,
        flags=re.MULTILINE
    )
    return text


# --- Guard 2: mask heredoc bodies -------------------------------------------
# A line is protected if it lies strictly between a heredoc introducer and its
# terminator. The introducer line and the terminator line are themselves command
# text and stay rewritable. Handles <<EOF, <<-EOF, <<'EOF', <<"EOF", and several
# heredocs in one command.
HEREDOC_START = re.compile(r'<<-?\s*(["\']?)([A-Za-z_][A-Za-z0-9_]*)\1')


def heredoc_mask(text):
    lines = text.split('\n')
    mask = [False] * len(lines)
    delim = None
    for i, line in enumerate(lines):
        if delim is None:
            m = HEREDOC_START.search(line)
            if m:
                delim = m.group(2)
        else:
            if line.strip() == delim:
                delim = None      # terminator: command text again
            else:
                mask[i] = True    # body: data, do not touch
    return lines, mask


lines, mask = heredoc_mask(cmd)

out_lines = []
run = []
run_protected = mask[0] if mask else False
for line, protected in zip(lines, mask):
    if protected == run_protected:
        run.append(line)
    else:
        chunk = '\n'.join(run)
        out_lines.append(chunk if run_protected else apply_rewrites(chunk))
        run = [line]
        run_protected = protected
if run:
    chunk = '\n'.join(run)
    out_lines.append(chunk if run_protected else apply_rewrites(chunk))

cmd = '\n'.join(out_lines)

if cmd != original:
    print(cmd)
else:
    sys.exit(1)
PYEOF
)

if [ $? -eq 0 ] && [ -n "$REWRITTEN" ]; then
  jq -n --arg cmd "$REWRITTEN" --arg orig "$COMMAND" '{
    hookSpecificOutput: {
      hookEventName: "PreToolUse",
      permissionDecision: "allow",
      permissionDecisionReason: ("Legacy tool rewritten to modern equivalent: " + $cmd),
      updatedInput: {
        command: $cmd
      }
    }
  }'
  exit 0
fi

# No rewrite needed — pass through
exit 0
