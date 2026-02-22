#!/usr/bin/env nu
# remind - Create reminders via DayPage r:: entries
#
# One-off reminders are written to the DayPage via daypage-append.
# collect-entries parses r:: entries and routes them to ~/Forge/NapierianLogs/Reminders/.
#
# Usage:
#   remind "Call dentist" in 3 days
#   remind "Jenny birthday" on March 17
#   remind "Check insurance" next month
#   remind list                          # Show upcoming reminders
#   remind view [months]                 # Calendar view (default: 3 months)
#   remind recurring                     # Show recurring annual events
#   remind recurring add "Jenny birthday" on March 17

def main [
    message?: string,      # The reminder message (or subcommand: list, view, recurring)
    ...when: string        # Natural language date: "in 3 days", "on March 17", "next month"
] {
    let reminders_dir = $"($env.HOME)/Forge/NapierianLogs/Reminders"
    let recurring_file = $"($reminders_dir)/recurring.md"

    # Ensure reminders directory exists
    mkdir $reminders_dir

    # Handle subcommands
    if $message == "list" {
        list_reminders $reminders_dir
        return
    }

    if $message == "view" {
        let months = if ($when | length) > 0 {
            try { $when | first | into int } catch { 3 }
        } else {
            3
        }
        view_calendar $reminders_dir $months
        return
    }

    if $message == "recurring" {
        if ($when | length) == 0 {
            show_recurring $recurring_file
        } else if ($when | first) == "add" {
            let rest = ($when | skip 1)
            add_recurring $recurring_file $rest
        } else {
            print "Usage: remind recurring [add \"message\" on Month Day]"
        }
        return
    }

    # Regular reminder — validate then queue via daypage-append
    if $message == null or ($when | length) == 0 {
        print "Usage: remind \"message\" <when>"
        print ""
        print "Examples:"
        print "  remind \"Call dentist\" in 3 days"
        print "  remind \"Check insurance\" next month"
        print "  remind \"Meeting prep\" on January 15"
        print ""
        print "Subcommands:"
        print "  remind list                              # Show upcoming reminders"
        print "  remind view [months]                     # Calendar view (default: 3 months)"
        print "  remind recurring                         # Show recurring annual events"
        print "  remind recurring add \"Birthday\" on Mar 17"
        return
    }

    let date_str = ($when | str join " ")

    # Validate the date parses before queuing — fail fast
    let target_date = try {
        $date_str | date from-human | format date "%Y-%m-%d"
    } catch {
        print $"Could not parse date: '($date_str)'"
        print "Try formats like: 'in 3 days', 'next Tuesday', 'on March 15'"
        return
    }

    # Queue as r:: entry in today's DayPage
    let entry = $"r:: ($date_str): ($message)"
    daypage-append $entry

    print $"Reminder set for ($target_date): ($message)"
    print $"Queued in DayPage: ($entry)"
}

def list_reminders [reminders_dir: string] {
    let today = (date now | format date "%Y-%m-%d")

    let files = try {
        glob $"($reminders_dir)/*.md"
        | where { |f| not ($f | str contains "recurring") }
        | each { |f| $f | path basename | str replace ".md" "" }
        | where { |d| $d >= $today }
        | sort
    } catch {
        []
    }

    if ($files | length) == 0 {
        print "No upcoming reminders."
        return
    }

    print "Upcoming reminders:\n"

    for date in $files {
        let file = $"($reminders_dir)/($date).md"
        let content = open $file --raw | lines | where { |l| $l =~ "^- " }
        print $"($date):"
        for line in $content {
            print $"  ($line)"
        }
    }
}

def show_recurring [recurring_file: string] {
    if not ($recurring_file | path exists) {
        print "No recurring reminders set."
        print "Add one with: remind recurring add \"Birthday\" on Mar 17"
        return
    }

    print "Recurring annual reminders:\n"
    open $recurring_file --raw | print
}

def view_calendar [reminders_dir: string, months: int] {
    let today = (date now)
    let end_date = ($today + ($months * 30day))

    mut rows = []

    let reminder_files = try {
        glob $"($reminders_dir)/*.md"
        | where { |f| not ($f | str contains "recurring") }
    } catch {
        []
    }

    for file in $reminder_files {
        let date_str = ($file | path basename | str replace ".md" "")
        let file_date = try { $date_str | into datetime } catch { continue }

        if $file_date >= $today and $file_date <= $end_date {
            let items = (open $file --raw | lines | where { |l| $l =~ "^- " } | each { |l| $l | str replace --regex "^- \\[.\\] " "" })
            for item in $items {
                $rows = ($rows | append { date: $date_str, reminder: $item })
            }
        }
    }

    let recurring_file = $"($reminders_dir)/recurring.md"
    if ($recurring_file | path exists) {
        let recurring = (
            open $recurring_file --raw
            | lines
            | where { |l| $l =~ "^\\d{2}-\\d{2}:" }
            | each { |l|
                let parts = ($l | split row ": ")
                { month_day: ($parts | first), text: ($parts | skip 1 | str join ": ") }
            }
        )

        mut current = $today
        while $current <= $end_date {
            let current_md = ($current | format date "%m-%d")
            for rec in $recurring {
                if $rec.month_day == $current_md {
                    $rows = ($rows | append {
                        date: ($current | format date "%Y-%m-%d"),
                        reminder: $"($rec.text) (recurring)"
                    })
                }
            }
            $current = ($current + 1day)
        }
    }

    if ($rows | length) == 0 {
        print $"No reminders in the next ($months) months."
        return
    }

    let sorted = ($rows | sort-by date)

    print $"Reminders for next ($months) months:\n"
    print "| Date | Reminder |"
    print "|------|----------|"
    for row in $sorted {
        print $"| ($row.date) | ($row.reminder) |"
    }
}

def add_recurring [recurring_file: string, args: list<string>] {
    let on_idx = try {
        $args | enumerate | where { |x| $x.item == "on" } | first | get index
    } catch {
        print "Usage: remind recurring add \"message\" on Month Day"
        print "Example: remind recurring add \"Jenny birthday\" on 17 March"
        return
    }

    let message = ($args | take $on_idx | str join " ")
    let date_part = ($args | skip ($on_idx + 1) | str join " ")

    let parsed = try {
        $"($date_part) 2025" | date from-human
    } catch {
        let parts = ($date_part | split row " ")
        if ($parts | length) == 2 {
            let reversed = $"($parts | get 1) ($parts | get 0) 2025"
            try {
                $reversed | date from-human
            } catch {
                print $"Could not parse date: '($date_part)'"
                print "Try: '17 March' or 'March 17'"
                return
            }
        } else {
            print $"Could not parse date: '($date_part)'"
            print "Try: '17 March' or 'March 17'"
            return
        }
    }

    let month_day = ($parsed | format date "%m-%d")
    let display_date = ($parsed | format date "%B %d")

    if not ($recurring_file | path exists) {
        "# Recurring Annual Reminders\n# Format: MM-DD: Description\n\n" | save $recurring_file
    }

    $"($month_day): ($message)\n" | save --append $recurring_file

    print $"Recurring reminder added: ($message) on ($display_date) every year"
}
