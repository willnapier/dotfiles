#!/usr/bin/env nu
# Open today's daily note directly in Helix - Native Nushell implementation

# Gather reminders for a specific date
def gather_reminders [reminders_dir: string, date: string, month_day: string]: nothing -> string {
    mut items = []

    # Check for one-off reminder file
    let reminder_file = $"($reminders_dir)/($date).md"
    if ($reminder_file | path exists) {
        let content = (open $reminder_file --raw | lines | where { |l| $l =~ "^- " })
        $items = ($items | append $content)
    }

    # Check for recurring reminders matching this month-day
    let recurring_file = $"($reminders_dir)/recurring.md"
    if ($recurring_file | path exists) {
        let matches = (
            open $recurring_file --raw
            | lines
            | where { |l| $l starts-with $month_day }
            | each { |l|
                let parts = ($l | split row ": " | skip 1)
                $"- [ ] ($parts | str join ': ') 🔄"
            }
        )
        $items = ($items | append $matches)
    }

    $items | str join "\n"
}

def main [
    --print-path      # Just print the file path, don't open
    --date: string    # Optional date (YYYY-MM-DD), defaults to today
] {
    # DEBUG LOG SETUP
    let log_file = $"($env.HOME)/.local/share/daily-note-debug.log"
    let timestamp = (date now | format date "%Y-%m-%d %H:%M:%S")

    # Log script invocation
    $"[($timestamp)] ===== DAILY-NOTE INVOKED =====\n" | save --append $log_file
    $"[($timestamp)] Print-path flag: ($print_path)\n" | save --append $log_file
    $"[($timestamp)] Date flag: ($date | default 'today')\n" | save --append $log_file
    $"[($timestamp)] SHELL: ($env.SHELL? | default 'unknown')\n" | save --append $log_file
    $"[($timestamp)] PWD: ($env.PWD)\n" | save --append $log_file

    let vault_dir = $"($env.HOME)/Forge"
    let daily_dir = $"($vault_dir)/NapierianLogs/DayPages"
    let reminders_dir = $"($vault_dir)/NapierianLogs/Reminders"

    # Parse target date - use provided date or default to today
    let target_date = if ($date | is-empty) {
        date now
    } else {
        $date | into datetime
    }

    # Generate filename from target date
    let today = ($target_date | format date "%Y-%m-%d")
    let today_month_day = ($target_date | format date "%m-%d")
    let daily_file = $"($daily_dir)/($today).md"

    # Template processing
    let template_file = $"($env.HOME)/Forge/Templates/DayPage-Minimal.md"

    # Log template status
    $"[($timestamp)] Template path: ($template_file)\n" | save --append $log_file
    $"[($timestamp)] Template exists: (($template_file | path exists))\n" | save --append $log_file

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

    # Check for existing sync conflicts and warn
    let conflict_files = (glob $"($daily_dir)/($today).sync-conflict-*.md")
    if ($conflict_files | length) > 0 {
        print $"⚠️  Warning: ($conflict_files | length) sync conflict\(s) for ($today) - check for lost edits"
        $"[($timestamp)] ⚠️  Found ($conflict_files | length) sync conflict files\n" | save --append $log_file
    }

    # Create the file if it doesn't exist (with sync-aware check)
    if not ($daily_file | path exists) {
        # Brief pause to let any in-progress Syncthing sync complete
        $"[($timestamp)] File not found, waiting 1.5s for potential sync...\n" | save --append $log_file
        sleep 1500ms

        # Re-check after pause
        if ($daily_file | path exists) {
            $"[($timestamp)] ✅ File appeared after sync wait - using synced version\n" | save --append $log_file
            print $"📥 Daily note synced from another machine"
        } else {
            $"[($timestamp)] File still missing after wait, creating: ($daily_file)\n" | save --append $log_file

            # Create directory if needed
            mkdir ($daily_file | path dirname)

        let current_time = (date now | format date "%H:%M")

        # Get human readable date
        let human_date = ($target_date | format date "%A, %B %d, %Y")

        # Calculate yesterday and tomorrow dates relative to target
        let yesterday = ($target_date - 1day | format date "%Y-%m-%d")
        let tomorrow = ($target_date + 1day | format date "%Y-%m-%d")

        # Process template with native Nushell string replacement
        if ($template_file | path exists) {
            $"[($timestamp)] ✅ Template file found - using template\n" | save --append $log_file
            let date_pattern = "{" + "{date}" + "}"
            let time_pattern = "{" + "{time24}" + "}"
            let hdate_pattern = "{" + "{hdate}" + "}"
            let date_minus_1_pattern = "{" + "{date-1}" + "}"
            let date_plus_1_pattern = "{" + "{date+1}" + "}"

            let processed = (
                open $template_file --raw
                | str replace --all $date_pattern $today
                | str replace --all $time_pattern $current_time
                | str replace --all $hdate_pattern $human_date
                | str replace --all $date_minus_1_pattern $yesterday
                | str replace --all $date_plus_1_pattern $tomorrow
                | str replace --all "<cursor>" ""
            )

            # Gather reminders for this date
            let reminders = (gather_reminders $reminders_dir $today $today_month_day)
            let final_content = if ($reminders | is-empty) {
                $processed
            } else {
                $"($processed)\n## Reminders\n\n($reminders)"
            }

            # Log template content for verification
            let template_preview = ($final_content | lines | first 5 | str join "\n")
            $"[($timestamp)] Template processed, first 5 lines:\n($template_preview)\n" | save --append $log_file

            $final_content | save --force $daily_file
            $"[($timestamp)] ✅ File saved using TEMPLATE\n" | save --append $log_file
            print $"📝 Created new daily note: ($today).md"
        } else {
            # Fallback template if original doesn't exist
            $"[($timestamp)] ⚠️  Template file NOT found - using FALLBACK\n" | save --append $log_file

            let fallback_content = $"# ($today) - ($human_date)

## Tasks
- [ ]

## Notes

## Journal

"
            $fallback_content | save --force $daily_file
            $"[($timestamp)] ⚠️  File saved using FALLBACK template\n" | save --append $log_file
            print $"📝 Created new daily note with fallback template: ($today).md"
        }
        }
    } else {
        $"[($timestamp)] Daily file already exists: ($daily_file)\n" | save --append $log_file
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