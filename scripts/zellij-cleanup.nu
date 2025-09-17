#!/usr/bin/env nu
# Clean up zombie Zellij sessions - Native Nushell implementation

def main [] {
    print "🧹 Zellij Session Cleanup"
    print "─────────────────────────"

    # Show current sessions
    print "📊 Current sessions:"
    try {
        zellij list-sessions --no-formatting
    } catch {
        print "No sessions found"
    }
    print ""

    # Count running Zellij processes using native Nushell
    let process_count = (ps | where name =~ "zellij" | length)
    print $"🔍 Running Zellij processes: ($process_count)"

    if $process_count > 3 {
        print "⚠️  Too many Zellij processes detected!"
        print ""

        # Show old/exited sessions using Nushell structured data
        print "🗑️  Cleaning up old sessions:"
        let exited_sessions = (
            try {
                zellij list-sessions --no-formatting
                | lines
                | where $it =~ "EXITED"
                | each { |line| $line | split row ' ' | get 0 }
            } catch {
                []
            }
        )

        if ($exited_sessions | length) > 0 {
            print $"Found ($exited_sessions | length) exited sessions to clean up:"

            for session in $exited_sessions {
                print $"  Deleting session: ($session)"
                try {
                    zellij delete-session $session
                } catch {
                    print $"    ❌ Failed to delete ($session)"
                }
            }
            print ""
        } else {
            print "No exited sessions found"
            print ""
        }

        # Recount processes after cleanup
        let new_process_count = (ps | where name =~ "zellij" | length)
        print $"✅ Cleanup complete. Processes now: ($new_process_count)"

        if $new_process_count < $process_count {
            let cleaned = $process_count - $new_process_count
            print $"🎯 Reduced Zellij processes by ($cleaned)"
        }
    } else {
        print "✅ Zellij process count looks healthy"
    }

    print ""
    print "🔍 Current process overview:"
    ps | where name =~ "zellij" | select name pid cpu mem | table
}