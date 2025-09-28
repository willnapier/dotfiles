#!/usr/bin/env nu
# Open today's daily note directly in Helix - Native Nushell implementation

def main [--print-path] {
    let vault_dir = $"($env.HOME)/Forge"
    let daily_dir = $"($vault_dir)/NapierianLogs/DayPages"

    # Generate today's filename
    let today = (date now | format date "%Y-%m-%d")
    let daily_file = $"($daily_dir)/($today).md"

    # Template processing
    let template_file = $"($vault_dir)/Areas/PKMStrategies/Templates/DayPage.md"

    # Find cursor line using native Nushell (no grep/cut needed)
    let cursor_line = if ($template_file | path exists) {
        try {
            let content = (open $template_file --raw | lines | enumerate)
            let cursor_match = ($content | where item =~ "<cursor>" | first)
            if ($cursor_match | is-not-empty) {
                $cursor_match.index + 1  # Convert to 1-based line numbering
            } else {
                3  # Default fallback
            }
        } catch {
            3  # Error fallback
        }
    } else {
        3  # Template doesn't exist fallback
    }

    # Create the file if it doesn't exist
    if not ($daily_file | path exists) {
        # Create directory if needed
        mkdir ($daily_file | path dirname)

        let current_time = (date now | format date "%H:%M")

        # Get human readable date
        let human_date = (date now | format date "%A, %B %d, %Y")

        # Process template with native Nushell string replacement
        if ($template_file | path exists) {
            let date_pattern = "{" + "{date}" + "}"
            let time_pattern = "{" + "{time24}" + "}"
            let hdate_pattern = "{" + "{hdate}" + "}"

            let processed = (
                open $template_file --raw
                | str replace --all $date_pattern $today
                | str replace --all $time_pattern $current_time
                | str replace --all $hdate_pattern $human_date
                | str replace --all "<cursor>" ""
            )
            $processed | save --force $daily_file
            print $"📝 Created new daily note: ($today).md"
        } else {
            # Fallback template if original doesn't exist
            let fallback_content = $"# ($today) - ($human_date)

## Tasks
- [ ]

## Notes

## Journal

"
            $fallback_content | save --force $daily_file
            print $"📝 Created new daily note with fallback template: ($today).md"
        }
    }

    # Handle --print-path option
    if $print_path {
        print $daily_file
        return
    }

    # Debug information using structured Nushell data
    let debug_info = {
        cursor_line: $cursor_line,
        daily_file: $daily_file,
        file_exists: ($daily_file | path exists),
        line_count: (if ($daily_file | path exists) {
            (open $daily_file --raw | lines | length)
        } else {
            0
        })
    }

    print $"DEBUG: (($debug_info | to json))"

    # Open in Helix with proper positioning
    let position_spec = if ($env.ZELLIJ? | is-not-empty) {
        # Zellij mode
        print "DEBUG: Inside Zellij - using Zellij-compatible positioning"
        $"($daily_file):($cursor_line):1"
    } else {
        # Regular terminal
        print "DEBUG: Regular terminal - using standard positioning"
        $"($daily_file):($cursor_line):0"
    }

    # Use universal hx command (wraps hx-auto for theme detection)
    print $"DEBUG: Executing: hx ($position_spec)"
    run-external hx $position_spec
}