#!/bin/bash
# PreToolUse hook: rewrite SSH commands to wrap compound operators in bash -c.
#
# Problem: Claude consistently generates `ssh host "cmd1 || cmd2"` which fails
# on nushell hosts. This hook intercepts Bash tool calls containing SSH commands
# and rewrites them to use `bash -c` wrappers when compound operators are detected.
#
# Only rewrites when ALL of these are true:
# 1. Command starts with `ssh`
# 2. The remote command portion contains && || ; or $( (compound operators)
# 3. The remote command is NOT already wrapped in bash -c or sh -c
#
# Input: JSON on stdin with .tool_input.command
# Output: JSON on stdout with updatedInput if rewritten, or exit 0 to pass through

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

# Only process ssh commands
if [[ ! "$COMMAND" =~ ^ssh[[:space:]] ]]; then
  exit 0
fi

# Extract the remote command portion.
# SSH commands look like: ssh [options] host "remote command"
# or: ssh [options] host 'remote command'
# We need to find the quoted remote command and check for compound operators.

# Check if the command contains compound operators anywhere after the host
# but is NOT already using bash -c or sh -c
if echo "$COMMAND" | grep -qP '(&&|\|\||;\s|\$\()' && \
   ! echo "$COMMAND" | grep -qP 'bash\s+-[cl]|sh\s+-c|nu\s+-c'; then

  # Rewrite: find the remote command (the quoted string at the end) and wrap it.
  # Strategy: use sed to replace the final quoted argument with a bash -c wrapped version.
  #
  # Match patterns:
  #   ssh host "remote cmd"  ->  ssh host "bash -c 'remote cmd'"
  #   ssh host 'remote cmd'  ->  ssh host "bash -c 'remote cmd'"

  # Extract host and remote command parts
  # Handle: ssh [flags] user@host "command" or ssh [flags] user@host 'command'
  REWRITTEN=$(echo "$COMMAND" | python3 -c "
import sys, re, json

cmd = sys.stdin.read().strip()

# Find the last quoted string (single or double) which is the remote command
# Pattern: everything up to the last quote-delimited argument
m = re.match(r'''(ssh\s+(?:[^'\"]*?\s+)?)(['\"])(.*)\2\s*$''', cmd, re.DOTALL)
if not m:
    # No quoted remote command found, pass through
    sys.exit(1)

prefix = m.group(1)  # ssh [flags] host
quote = m.group(2)
remote_cmd = m.group(3)

# Escape single quotes in the remote command for bash -c wrapping
escaped = remote_cmd.replace(\"'\", \"'\\\\\\\"'\\\\\\\"'\")

rewritten = prefix + '\"bash -c \\'' + escaped + '\\'\"'
print(rewritten)
")

  if [ $? -eq 0 ] && [ -n "$REWRITTEN" ]; then
    jq -n --arg cmd "$REWRITTEN" '{
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "allow",
        permissionDecisionReason: "SSH command rewritten: compound operators wrapped in bash -c",
        updatedInput: {
          command: $cmd
        }
      }
    }'
    exit 0
  fi
fi

# No rewrite needed — pass through
exit 0
