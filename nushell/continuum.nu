# Continuum Nushell Functions
# Query plain-text conversation logs from multiple assistants

# Search across all conversations using ripgrep
export def continuum-search [
    query: string    # Search query (regex supported)
    --limit (-n): int = 20  # Limit number of results
] {
    rg $query ~/continuum-logs --json
    | lines
    | each { from json }
    | where type == "match"
    | get data
    | each { |match|
        let path_parts = ($match.path.text | split row '/')
        let assistant = ($path_parts | get 4)
        let date = ($path_parts | get 5)
        let session = ($path_parts | get 6)

        # Parse the message line (JSONL format)
        let message = ($match.lines.text | str trim | from json)

        {
            assistant: $assistant
            date: $date
            session: $session
            role: $message.role
            content: $message.content
            file: $match.path.text
            line: $match.line_number
        }
    }
    | first $limit
}

# Show timeline of all conversations for a specific date or date range
export def continuum-timeline [
    date?: string    # Date (YYYY-MM-DD) - defaults to today
    --from: string   # Start of date range
    --to: string     # End of date range
    --assistant: string  # Filter by assistant
] {
    let date_filter = if ($from != null and $to != null) {
        # Date range mode
        {from: $from, to: $to}
    } else if ($date != null) {
        # Single date mode
        {from: $date, to: $date}
    } else {
        # Default to today
        let today = (date now | format date "%Y-%m-%d")
        {from: $today, to: $today}
    }

    # Find all message files
    let all_messages = (
        ls ~/continuum-logs/**/messages.jsonl
        | each { |f|
            let path_parts = ($f.name | split row '/')
            let asst = ($path_parts | get 4)
            let date = ($path_parts | get 5)
            let session = ($path_parts | get 6)

            # Filter by date range
            if ($date >= $date_filter.from and $date <= $date_filter.to) {
                # Filter by assistant if specified
                if ($assistant == null or $asst == $assistant) {
                    # Read and parse all messages from this session
                    open $f.name
                    | lines
                    | each { |line|
                        let msg = ($line | from json)
                        $msg | insert assistant $asst | insert session $session | insert date $date
                    }
                } else {
                    []
                }
            } else {
                []
            }
        }
        | flatten
    )

    $all_messages
    | sort-by timestamp
}

# Show statistics about conversation logs
export def continuum-stats [] {
    ls ~/continuum-logs/**/messages.jsonl
    | each { |f|
        let path_parts = ($f.name | split row '/')
        let assistant = ($path_parts | get 4)
        let message_count = (open $f.name | lines | length)

        {
            assistant: $assistant
            session: ($path_parts | get 6)
            messages: $message_count
            size: $f.size
        }
    }
    | group-by assistant
    | transpose assistant data
    | each { |row|
        {
            assistant: $row.assistant
            sessions: ($row.data | length)
            total_messages: ($row.data | get messages | math sum)
            total_size: ($row.data | get size | math sum)
        }
    }
}

# Get context from a specific session
export def continuum-context [
    session: string   # Session ID (full or partial match)
    --limit: int = 50  # Number of recent messages to include
] {
    # Find session file by partial match
    let session_file = (
        ls ~/continuum-logs/**/messages.jsonl
        | where name =~ $session
        | first
    )

    if ($session_file | is-empty) {
        error make {msg: $"Session not found: ($session)"}
    }

    # Read messages
    open $session_file.name
    | lines
    | each { from json }
    | last $limit
}

# Export a session to markdown
export def continuum-export-md [
    session: string    # Session ID (full or partial match)
    output?: string    # Output file (defaults to session-id.md)
] {
    let session_file = (
        ls ~/continuum-logs/**/messages.jsonl
        | where name =~ $session
        | first
    )

    if ($session_file | is-empty) {
        error make {msg: $"Session not found: ($session)"}
    }

    let path_parts = ($session_file.name | split row '/')
    let assistant = ($path_parts | get 4)
    let date = ($path_parts | get 5)
    let session_id = ($path_parts | get 6)

    # Read session metadata
    let session_dir = ($session_file.name | path dirname)
    let metadata = (open $"($session_dir)/session.json")

    # Build markdown
    let md_content = (
        open $session_file.name
        | lines
        | each { |line|
            let msg = ($line | from json)
            $"### ($msg.role) \(($msg.timestamp)\)\n\n($msg.content)\n\n"
        }
        | str join
    )

    let header = $"# ($assistant) Session: ($session_id)\n\nDate: ($date)\nMessages: ($metadata.message_count)\n\n---\n\n"
    let full_content = $"($header)($md_content)"

    let output_file = if ($output != null) {
        $output
    } else {
        $"($session_id).md"
    }

    $full_content | save -f $output_file
    print $"Exported to ($output_file)"
}
