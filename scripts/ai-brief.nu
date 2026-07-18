#!/usr/bin/env nu

const ORIENTATION_SCHEMA = 1
const DEFAULT_BUDGET = 18000
const HARNESSES = [codex claude-code grok-build api]

def required-text [path: path] {
    if not ($path | path exists) {
        error make { msg: $"required orientation source is missing: ($path)" }
    }
    open --raw $path
}

def resolve-host [requested: string] {
    if not ($requested | str trim | is-empty) {
        let normalized = ($requested | str downcase)
        if $normalized not-in [macos nimbini] {
            error make { msg: $"unknown host layer: ($requested); expected macos or nimbini" }
        }
        return $normalized
    }

    let host = (sys host)
    if $host.name == "Darwin" {
        "macos"
    } else if ($host.hostname | str downcase | str contains "nimbini") {
        "nimbini"
    } else {
        error make { msg: $"cannot map host ($host.hostname) / ($host.name) to an orientation machine layer" }
    }
}

def messageboard-head [path: path] {
    let raw = (required-text $path)
    let sections = ($raw | split row "\n### " | skip 1)
    if ($sections | is-empty) {
        return "No current Messageboard entries."
    }
    $"### (($sections | first | str trim))"
}

def forum-open-summary [path: path] {
    let lines = (required-text $path | lines)
    mut inside = false
    mut rows = []
    for line in $lines {
        if $line == "## Open" {
            $inside = true
        } else if $inside and ($line | str starts-with "## ") {
            $inside = false
        } else if $inside and ($line | str starts-with "| `") {
            $rows = ($rows | append $line)
        }
    }
    if ($rows | is-empty) {
        "No open forum threads."
    } else {
        $rows | str join "\n"
    }
}

def forum-inbox-summary [] {
    if (which forum | is-empty) {
        return "Forum CLI unavailable; inspect design-forum/INDEX.md directly."
    }
    try {
        let summary = (^forum inbox --format brief | complete)
        if $summary.exit_code == 0 and not ($summary.stdout | str trim | is-empty) {
            $summary.stdout | str trim
        } else {
            "No unread forum completions, or this forum version predates inbox support."
        }
    } catch {
        "No unread forum completions, or this forum version predates inbox support."
    }
}

def render-contract [harness: string host: string budget: int] {
    if $harness not-in $HARNESSES {
        error make { msg: $"unknown harness: ($harness); expected ($HARNESSES | str join ', ')" }
    }
    if $budget <= 0 {
        error make { msg: "budget must be greater than zero" }
    }

    let home = $env.HOME
    let kernel_path = ($home | path join "Assistants/shared/ORIENTATION.md")
    let machine_path = ($home | path join $"Assistants/context/machines/($host).md")
    let adapter_path = ($home | path join $"Assistants/context/briefings/($harness).md")
    let messageboard_path = ($home | path join "Assistants/shared/MESSAGEBOARD.md")
    let index_path = ($home | path join "Assistants/shared/design-forum/INDEX.md")

    let kernel = (required-text $kernel_path)
    let declared_schema = ($kernel | lines | where {|line| $line | str starts-with "orientation_schema:" } | first | split row ":" | last | str trim | into int)
    if $declared_schema != $ORIENTATION_SCHEMA {
        error make { msg: $"renderer schema ($ORIENTATION_SCHEMA) does not match ORIENTATION.md schema ($declared_schema)" }
    }
    let declared_budget = ($kernel | lines | where {|line| $line | str starts-with "render_budget_bytes:" } | first | split row ":" | last | str trim | into int)
    if $budget > $declared_budget {
        error make { msg: $"requested budget ($budget) exceeds ORIENTATION.md hard limit ($declared_budget)" }
    }

    let body = ([
        "## Vendor-neutral kernel"
        ($kernel | str trim)
        $"## Machine layer: ($host)"
        (required-text $machine_path | str trim)
        $"## Harness adapter: ($harness)"
        (required-text $adapter_path | str trim)
        "## Messageboard head (transient)"
        (messageboard-head $messageboard_path)
        "## Open forum summary (discovery only)"
        (forum-open-summary $index_path)
        "## Forum inbox"
        (forum-inbox-summary)
    ] | str join "\n\n")
    let content_hash = ($body | hash sha256)
    mut total = 0
    mut payload = ""
    for _ in 1..3 {
        let header = $"# Effective Assistant Startup Contract\n\norientation-schema: ($ORIENTATION_SCHEMA)\nharness: ($harness)\nhost: ($host)\ncontent-sha256: ($content_hash)\npayload-bytes: ($total)\nbudget-bytes: ($budget)\n"
        $payload = $"($header)\n($body)\n"
        $total = ($payload | str length --utf-8-bytes)
    }
    if $total > $budget {
        error make { msg: $"startup payload is ($total) bytes, exceeding the hard budget of ($budget)" }
    }
    $payload
}

def verify-startup-surface [label: string path: path needle: string] {
    if not ($path | path exists) {
        return { harness: $label status: "FAIL" detail: $"missing ($path)" }
    }
    let raw = (open --raw $path)
    if ($raw | str contains $needle) {
        { harness: $label status: "ok" detail: $"startup surface ($path)" }
    } else {
        { harness: $label status: "FAIL" detail: $"startup surface does not reference renderer: ($path)" }
    }
}

def doctor [host: string budget: int] {
    mut results = []
    for harness in $HARNESSES {
        let result = try {
            let payload = (render-contract $harness $host $budget)
            let bytes = ($payload | str length --utf-8-bytes)
            let hash_line = ($payload | lines | where {|line| $line | str starts-with "content-sha256:" } | first)
            { harness: $harness status: "ok" detail: $"rendered ($bytes) bytes; ($hash_line)" }
        } catch {|error|
            { harness: $harness status: "FAIL" detail: $error.msg }
        }
        $results = ($results | append $result)
    }

    let home = $env.HOME
    $results = ($results | append (verify-startup-surface codex ($home | path join ".codex/AGENTS.md") "ai-brief.nu render --harness codex"))
    $results = ($results | append (verify-startup-surface claude-code ($home | path join ".claude/settings.json") "ai-brief.nu render --harness claude-code --format claude-hook"))
    $results = ($results | append (verify-startup-surface grok-build ($home | path join ".grok/AGENTS.md") "ai-brief.nu render --harness grok-build"))
    $results = ($results | append (verify-startup-surface api ($home | path join "Assistants/context/briefings/api.md") "ai-brief.nu render --harness api"))
    print ($results | table)
    if ($results | any {|row| $row.status == "FAIL" }) {
        error make { msg: "orientation doctor found failures" }
    }
}

def main [
    action: string = "render"
    assistant?: string
    --harness: string = ""
    --host: string = ""
    --budget: int = $DEFAULT_BUDGET
    --format: string = "markdown"
] {
    let compatibility_harness = if $action in $HARNESSES { $action } else { "" }
    let operation = if not ($compatibility_harness | is-empty) { "render" } else { $action }
    let selected = if not ($harness | str trim | is-empty) {
        $harness | str downcase
    } else if not ($compatibility_harness | is-empty) {
        $compatibility_harness
    } else {
        $assistant | default "" | str downcase
    }
    let machine = (resolve-host $host)

    match $operation {
        "render" => {
            if ($selected | str trim | is-empty) {
                error make { msg: "usage: ai-brief.nu render --harness <codex|claude-code|grok-build|api> [--host macos|nimbini]" }
            }
            let payload = (render-contract $selected $machine $budget)
            match $format {
                "markdown" => { print $payload }
                "claude-hook" => {
                    {
                        hookSpecificOutput: {
                            hookEventName: "SessionStart"
                            additionalContext: $payload
                        }
                    } | to json --raw | print
                }
                _ => { error make { msg: $"unknown format: ($format)" } }
            }
        }
        "doctor" => { doctor $machine $budget }
        _ => { error make { msg: $"unknown action: ($operation); expected render or doctor" } }
    }
}
