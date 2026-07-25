#!/bin/bash
# PreToolUse hook: nudge prior-art research before creating new Rust projects.
#
# Blocks `cargo new` / `cargo init` unless a prior-art report exists
# in ~/Forge/research/ from the last 4 hours. After running
# `prior-art "description" --save`, the report exists and retry succeeds.
#
# Input: JSON on stdin with .tool_input.command
# Output: JSON block decision, or exit 0 to pass through

INPUT=$(cat)
COMMAND=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

[ -z "$COMMAND" ] && exit 0

# Only trigger on cargo new/init
if echo "$COMMAND" | grep -qE 'cargo\s+(new|init)\s+'; then
    # Extract project name
    PROJECT=$(echo "$COMMAND" | grep -oE 'cargo\s+(new|init)\s+[^ ]+' | awk '{print $NF}' | sed 's|.*/||')

    # Check for any prior-art report created in last 4 hours
    RESEARCH_DIR="$HOME/Forge/research"
    if [ -d "$RESEARCH_DIR" ]; then
        RECENT=$(fd --changed-within 4h -e md . "$RESEARCH_DIR" 2>/dev/null | head -1)
    else
        RECENT=""
    fi

    if [ -z "$RECENT" ]; then
        jq -n --arg project "$PROJECT" '{
            hookSpecificOutput: {
                hookEventName: "PreToolUse",
                permissionDecision: "block",
                permissionDecisionReason: ("New project \"" + $project + "\" — no recent prior-art research found in ~/Forge/research/.\nRun: prior-art \"description of " + $project + "\" --save\nThen retry. The report unblocks project creation (4h window).")
            }
        }'
        exit 0
    fi
fi

# Pass through
exit 0
