#!/usr/bin/env nu
# Convert continuum JSONL conversation to readable markdown

def main [jsonl_path: string] {
    if not ($jsonl_path | path exists) {
        print $"Error: File not found: ($jsonl_path)"
        exit 1
    }

    # Parse JSONL - each line is a JSON object
    let messages = (
        open $jsonl_path
        | lines
        | where $it != ""
        | each { |line| $line | from json }
    )

    if ($messages | is-empty) {
        print "Error: No messages found in conversation"
        exit 1
    }

    # Get metadata from first message
    let first_msg = ($messages | first)
    let date = if ($first_msg.timestamp? != null) {
        $first_msg.timestamp | into datetime | format date "%Y-%m-%d %H:%M"
    } else {
        "Unknown date"
    }

    let filename = ($jsonl_path | path basename)

    # Build markdown header
    mut output = [
        $"# Conversation - ($date)"
        ""
        $"Date: ($date)"
        $"Source: `($filename)`"
        ""
        "---"
        ""
    ]

    # Convert messages to markdown
    for msg in $messages {
        let role = $msg.role? | default "unknown"
        let content = $msg.content? | default ""

        # Skip system/command messages
        if ($content | str contains "<command-") or ($content | str contains "<local-command-") {
            continue
        }
        if ($content | str starts-with "Caveat: The messages below") {
            continue
        }

        # Format based on role
        if $role == "user" {
            $output = ($output | append ["## 👤 User" "" $content ""])
        } else if $role == "assistant" {
            $output = ($output | append ["## 🤖 Assistant" "" $content ""])
        }
    }

    # Output markdown
    $output | str join "\n"
}
