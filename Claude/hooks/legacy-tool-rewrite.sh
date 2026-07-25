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
changes = []

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
        elif c in ('n', 'i', 'l', 'c', 'v', 'w', 'o', 'h', 'H'):
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

# Rewrite grep → rg
# Match grep preceded by: start of line, pipe, semicolon, &&, ||, $(, or backtick
# But NOT pgrep, egrep, fgrep, zgrep, xargs grep (handle xargs separately)
# Word boundary: grep must be preceded by non-alphanumeric or start of string
cmd = re.sub(
    r'((?:^|[|;&`]\s*|\$\(\s*))grep\s+((?:-[a-zA-Z]+\s+)*)(.*?)(?=\s*(?:[|;&]|$))',
    rewrite_grep_match,
    cmd,
    flags=re.MULTILINE
)

# Rewrite find → fd
# Match: find <path> -name "pattern" or find <path> -name 'pattern' or find <path> -name pattern
# Optionally with -type f or -type d
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

cmd = re.sub(
    r'((?:^|[|;&`]\s*|\$\(\s*))find\s+(\S+)\s+-name\s+(["\']?)([^"\'\s]+)\3(\s+-type\s+[fd])?',
    rewrite_find_match,
    cmd,
    flags=re.MULTILINE
)

# Rewrite cat → bat (standalone only)
# Match: cat <file> at start of command or after ; or &&
# Do NOT rewrite: cat file | cmd (piped — bat adds formatting that breaks pipes)
# Do NOT rewrite: cat << EOF (heredoc)
# Do NOT rewrite: zcat, tac (compound commands)
def rewrite_cat_match(m):
    prefix = m.group(1)
    args = m.group(2)
    return f'{prefix}bat {args}'

# Only match cat when NOT followed by a pipe on the same segment
# Strategy: split on pipes first, only rewrite cat in the LAST segment
# (if cat output is piped, leave it alone)
# Simpler: match cat <file> only when followed by end-of-string or semicolon, not pipe
cmd = re.sub(
    r'((?:^|[;&]\s*))cat\s+([^|;&\n]+?)(?=\s*(?:[;&]|$))',
    rewrite_cat_match,
    cmd,
    flags=re.MULTILINE
)

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
