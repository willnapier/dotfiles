# Codex session tailing utilities

# Tail the latest codex session file
export def main [] {
    let sessions_dir = $"($env.HOME)/.codex/sessions"

    if not ($sessions_dir | path exists) {
        print "Error: Codex sessions directory not found"
        return
    }

    # Find the most recently modified .jsonl file
    let files = (
        glob $"($sessions_dir)/**/*.jsonl"
        | each { |file|
            {
                name: $file
                modified: (ls $file | get modified.0)
                size: (ls $file | get size.0)
                type: "file"
            }
        }
    )

    if ($files | is-empty) {
        print "No codex session files found"
        return
    }

    let latest = ($files | sort-by modified -r | first)

    print $"Tailing: ($latest.name)"
    print $"Modified: ($latest.modified)"
    print "─────────────────────────────────────────"

    # Use tail -f with filtering to show only conversation content
    ^tail -f $latest.name | lines | each { |line|
        try {
            let json = ($line | from json)

            # Only process response_item messages
            if $json.type == "response_item" {
                let payload = $json.payload

                # Check if it's a user or assistant message
                if $payload.type == "message" and ($payload.role == "user" or $payload.role == "assistant") {
                    # Extract text from content array
                    let content = ($payload.content | each { |item|
                        if $item.type == "input_text" or $item.type == "output_text" or $item.type == "text" {
                            $item.text
                        }
                    } | where $it != null | str join "\n")

                    # Skip environment context messages
                    if not ($content | str contains "<environment_context>") {
                        let role_display = if $payload.role == "user" { "USER" } else { "ASSISTANT" }
                        print $"(ansi cyan)┌─ ($role_display)(ansi reset)"
                        print $content
                        print $"(ansi cyan)└─────────────────────────────────────────(ansi reset)\n"
                    }
                }
            }
        }
    }
}

# Tail today's latest codex session
export def "today" [] {
    let today = (date now | format date "%Y/%m/%d")
    let today_dir = $"($env.HOME)/.codex/sessions/($today)"

    if not ($today_dir | path exists) {
        print $"No sessions found for today \(($today))"
        print "Falling back to latest session overall..."
        main
        return
    }

    let files = (
        glob $"($today_dir)/*.jsonl"
        | each { |file|
            {
                name: $file
                modified: (ls $file | get modified.0)
                size: (ls $file | get size.0)
                type: "file"
            }
        }
    )

    if ($files | is-empty) {
        print "No session files found for today"
        return
    }

    let latest = ($files | sort-by modified -r | first)

    print $"Tailing today's session: ($latest.name)"
    print $"Modified: ($latest.modified)"
    print "─────────────────────────────────────────"

    # Use tail -f with filtering to show only conversation content
    ^tail -f $latest.name | lines | each { |line|
        try {
            let json = ($line | from json)

            # Only process response_item messages
            if $json.type == "response_item" {
                let payload = $json.payload

                # Check if it's a user or assistant message
                if $payload.type == "message" and ($payload.role == "user" or $payload.role == "assistant") {
                    # Extract text from content array
                    let content = ($payload.content | each { |item|
                        if $item.type == "input_text" or $item.type == "output_text" or $item.type == "text" {
                            $item.text
                        }
                    } | where $it != null | str join "\n")

                    # Skip environment context messages
                    if not ($content | str contains "<environment_context>") {
                        let role_display = if $payload.role == "user" { "USER" } else { "ASSISTANT" }
                        print $"(ansi cyan)┌─ ($role_display)(ansi reset)"
                        print $content
                        print $"(ansi cyan)└─────────────────────────────────────────(ansi reset)\n"
                    }
                }
            }
        }
    }
}

# List all codex sessions for today
export def "sessions" [] {
    let today = (date now | format date "%Y/%m/%d")
    let today_dir = $"($env.HOME)/.codex/sessions/($today)"

    if not ($today_dir | path exists) {
        print $"No sessions found for today \(($today))"
        return
    }

    ls $"($today_dir)/*.jsonl"
    | select name modified size
    | sort-by modified -r
}
