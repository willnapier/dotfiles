#!/usr/bin/env nu

def main [assistant?: string] {
    let assistant = ($assistant | default "")

    if ($assistant | str trim | is-empty) {
        print "Usage: ai-brief.nu <assistant-name>"
        exit 1
    }

    let normalized = ($assistant | str downcase)
    let briefings = [
        { name: "codex" path: ($"($env.HOME)/Assistants/briefings/codex.md") }
        { name: "claude-code" path: ($"($env.HOME)/Assistants/briefings/claude-code.md") }
    ]

    let entry = ($briefings | where name == $normalized | get 0?)

    if $entry == null {
        print $"⚠️ ai-brief: no briefing configured for '($assistant)'"
        exit 2
    }

    let briefing_path = $entry.path

    if not ($briefing_path | path exists) {
        print $"⚠️ ai-brief: briefing file missing at ($briefing_path)"
        exit 3
    }

    let content = (open --raw $briefing_path | decode utf-8)
    print $content
    print ""
    print "Supplemental docs: Assistants/index.md · claude.md · Claude/CLAUDE-TOOLCHAIN-PREFERENCES.md · Claude/CLAUDE-DEBUGGING-PATTERNS.md · Claude/STONE-IN-SHOE-DEBUGGING-PHILOSOPHY.md · Claude/NUSHELL-KNOWLEDGE-TOOLS-README.md"
}
