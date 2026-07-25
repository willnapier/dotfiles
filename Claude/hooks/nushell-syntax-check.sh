#!/usr/bin/env bash
# Stop hook: detect bash syntax in commands suggested to the user.
#
# William's terminal is nushell. Commands with bash-only syntax will fail.
# This hook scans the assistant's last message for common bash patterns
# that appear in code blocks or inline code, and rejects the response
# if any are found.
#
# Patterns checked:
#   - echo -n (nushell echo has no -n flag)
#   - && or || outside bash/sh code fences
#   - 2>&1 or 2>/dev/null (should be out+err> or err> /dev/null)
#   - export VAR= (should be $env.VAR =)
#   - VAR=value cmd (inline env, should be with-env)
#   - $VAR (should be $env.VAR for env vars in nushell)

set -euo pipefail

INPUT=$(cat)

LAST_MSG=$(echo "$INPUT" | jq -r '.last_assistant_message // empty')

if [ -z "$LAST_MSG" ]; then
  exit 0
fi

# Extract code blocks and inline code that the user would run.
# We check:
# 1. Fenced code blocks NOT marked as bash/sh/rust/python/toml/json/yaml/sql/html/css/js
#    (bash-marked blocks are intentionally bash — e.g., instructions for Mac)
# 2. Inline backtick code
# 3. Lines starting with ``` with no language or with ```nushell/```nu

VIOLATIONS=""

# --- Check for bash patterns in unmarked or nushell code blocks ---
# Extract content of code blocks that are unmarked or marked as nushell/nu
# Skip blocks explicitly marked as bash, sh, rust, python, toml, json, yaml, sql, html, css, javascript, typescript
BLOCKS=$(echo "$LAST_MSG" | awk '
  /^```(nushell|nu)?[[:space:]]*$/ { capture=1; next }
  /^```(bash|sh|rust|python|toml|json|yaml|sql|html|css|javascript|typescript|markdown|md|diff)/ { capture=0; next }
  /^```/ { capture=0; next }
  capture { print }
')

# Also extract inline code (single backticks) — these are usually nushell commands
INLINE=$(echo "$LAST_MSG" | grep -oE '`[^`]+`' | sed 's/^`//;s/`$//' || true)

# Combine for checking
CHECK_TEXT=$(printf '%s\n%s' "$BLOCKS" "$INLINE")

if [ -z "$CHECK_TEXT" ]; then
  exit 0
fi

# Pattern 1: && (bash chaining)
if echo "$CHECK_TEXT" | grep -qE '[^&]&&[^&]|^&&|&&$'; then
  VIOLATIONS="${VIOLATIONS}  - '&&' found (use ';' in nushell)\n"
fi

# Pattern 2: || (bash or)
if echo "$CHECK_TEXT" | grep -qE '[^|]\|\|[^|]|^\|\||^\|\|$'; then
  VIOLATIONS="${VIOLATIONS}  - '||' found (not valid in nushell)\n"
fi

# Pattern 3: 2>&1 or 2>/dev/null
if echo "$CHECK_TEXT" | grep -qE '2>&1|2>/dev/null'; then
  VIOLATIONS="${VIOLATIONS}  - '2>&1' or '2>/dev/null' found (use 'out+err>' or 'err> /dev/null' in nushell)\n"
fi

# Pattern 4: export VAR=
if echo "$CHECK_TEXT" | grep -qE '^[[:space:]]*export [A-Z_]+='; then
  VIOLATIONS="${VIOLATIONS}  - 'export VAR=' found (use '\$env.VAR =' in nushell)\n"
fi

# Pattern 5: echo -n
if echo "$CHECK_TEXT" | grep -qE 'echo -n '; then
  VIOLATIONS="${VIOLATIONS}  - 'echo -n' found (nushell echo has no -n flag; use 'print -n' or pipe the string directly)\n"
fi

# Pattern 6: VAR=value command (inline env assignment before command)
# Match: ALLCAPS_VAR=something command (but not inside strings or assignments)
if echo "$CHECK_TEXT" | grep -qE '^[A-Z_]+=[^ ]+ [a-z]'; then
  VIOLATIONS="${VIOLATIONS}  - Inline 'VAR=value cmd' found (use 'with-env { VAR: \"value\" } { cmd }' in nushell)\n"
fi

if [ -n "$VIOLATIONS" ]; then
  VIOLATIONS_ESCAPED=$(printf '%s' "$VIOLATIONS" | sed 's/"/\\"/g' | tr '\n' ' ')
  echo "{\"error\": \"NUSHELL SYNTAX VIOLATION: Your response contains bash-only syntax in commands meant for the user. William runs nushell — these commands will fail. Fix before sending:\\n${VIOLATIONS_ESCAPED}\\nRewrite the offending commands in nushell syntax.\"}"
  exit 0
fi

exit 0
