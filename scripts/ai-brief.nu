#!/usr/bin/env nu

const ORIENTATION_SCHEMA = 1
const DEFAULT_BUDGET = 18000
const HARNESSES = [codex claude-code grok-build api]
const MESSAGEBOARD_BUDGET = 4500
const FORUM_INDEX_BUDGET = 3500
const FORUM_INBOX_BUDGET = 2000

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

def cap-component [text: string budget: int label: string] {
    if ($text | str length --utf-8-bytes) <= $budget {
        return $text
    }
    let notice = $"\n… [($label) truncated at startup; load the source on demand]"
    let content_budget = $budget - ($notice | str length --utf-8-bytes)
    mut kept = []
    mut used = 0
    for line in ($text | lines) {
        let addition = if ($kept | is-empty) { $line } else { $"\n($line)" }
        let bytes = ($addition | str length --utf-8-bytes)
        if ($used + $bytes) > $content_budget {
            break
        }
        $kept = ($kept | append $line)
        $used = $used + $bytes
    }
    let joined = ($kept | str join "\n")
    $"($joined)($notice)"
}

def messageboard-head [path: path] {
    let raw = (required-text $path)
    let sections = ($raw | split row "\n### " | skip 1)
    if ($sections | is-empty) {
        return "No current Messageboard entries."
    }
    let head = $"### (($sections | first | str trim | str replace --regex '\n---$' ''))"
    cap-component $head $MESSAGEBOARD_BUDGET "Messageboard head"
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
        cap-component ($rows | str join "\n") $FORUM_INDEX_BUDGET "forum index summary"
    }
}

def forum-inbox-summary [] {
    if (which forum | is-empty) {
        return "WARNING: forum CLI unavailable; unread completion state could not be checked."
    }
    try {
        let summary = (^forum inbox --format brief | complete)
        if $summary.exit_code == 0 and not ($summary.stdout | str trim | is-empty) {
            cap-component ($summary.stdout | str trim) $FORUM_INBOX_BUDGET "forum inbox"
        } else if $summary.exit_code == 0 {
            "WARNING: forum inbox returned empty output; unread completion state is unknown."
        } else {
            $"WARNING: forum inbox failed with exit ($summary.exit_code): ($summary.stderr | str trim)"
        }
    } catch {|error|
        $"WARNING: forum inbox check failed: ($error.msg)"
    }
}

def assemble-payload [harness: string host: string budget: int body: string] {
    let content_hash = ($body | hash sha256)
    let placeholder = "00000000"
    let header = $"# Effective Assistant Startup Contract\n\norientation-schema: ($ORIENTATION_SCHEMA)\nharness: ($harness)\nhost: ($host)\ncontent-sha256: ($content_hash)\npayload-bytes: ($placeholder)\nbudget-bytes: ($budget)\n"
    let template = $"($header)\n($body)\n"
    let total = ($template | str length --utf-8-bytes)
    if $total > 99999999 {
        error make { msg: "startup payload exceeds the fixed eight-digit byte field" }
    }
    if $total > $budget {
        error make { msg: $"startup payload is ($total) bytes, exceeding the hard budget of ($budget)" }
    }
    let byte_field = ($total | into string | fill --alignment right --character "0" --width 8)
    $template | str replace $placeholder $byte_field
}

def metadata-value [payload: string key: string] {
    let prefix = $"($key):"
    $payload | lines | where {|line| $line | str starts-with $prefix } | first | str replace $prefix "" | str trim
}

def verify-payload [payload: string expected_harness: string expected_host: string] {
    let marker = "## Vendor-neutral kernel"
    let split = ($payload | split row $marker)
    if ($split | length) != 2 {
        error make { msg: "effective payload lacks a unique vendor-neutral body marker" }
    }
    let body = $"($marker)(($split | last | str trim --right))"
    let actual_hash = ($body | hash sha256)
    let claimed_hash = (metadata-value $payload "content-sha256")
    let actual_bytes = ($payload | str length --utf-8-bytes)
    let claimed_bytes = (metadata-value $payload "payload-bytes" | into int)
    let budget = (metadata-value $payload "budget-bytes" | into int)
    let schema = (metadata-value $payload "orientation-schema" | into int)
    let harness = (metadata-value $payload "harness")
    let host = (metadata-value $payload "host")

    if $actual_hash != $claimed_hash {
        error make { msg: $"content hash mismatch: claimed ($claimed_hash), actual ($actual_hash)" }
    }
    if $actual_bytes != $claimed_bytes {
        error make { msg: $"payload byte mismatch: claimed ($claimed_bytes), actual ($actual_bytes)" }
    }
    if $actual_bytes > $budget {
        error make { msg: $"payload is ($actual_bytes) bytes, exceeding budget ($budget)" }
    }
    if $schema != $ORIENTATION_SCHEMA or $harness != $expected_harness or $host != $expected_host {
        error make { msg: "payload schema, harness, or host metadata does not match the render request" }
    }
    { bytes: $actual_bytes hash: $actual_hash }
}

def claude-fallback [host: string error_message: string] {
    $"# Orientation renderer fallback\n\nThe full startup contract could not be assembled: ($error_message)\n\nBefore beginning the task, read `~/Assistants/shared/ORIENTATION.md`, `~/Assistants/context/machines/($host).md`, and `~/Assistants/context/briefings/claude-code.md`; then inspect the current Messageboard head, `design-forum/INDEX.md`, and `forum inbox`. Treat those sources as mandatory context."
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
    assemble-payload $harness $host $budget $body
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
            let verified = (verify-payload $payload $harness $host)
            { harness: $harness status: "ok" detail: $"verified ($verified.bytes) bytes; sha256: ($verified.hash)" }
        } catch {|error|
            { harness: $harness status: "FAIL" detail: $error.msg }
        }
        $results = ($results | append $result)
    }

    let boundary = try {
        let filler = ("" | fill --width 9760 --character "x")
        let body = $"## Vendor-neutral kernel\n\n($filler)"
        let payload = (assemble-payload "boundary-test" $host 18000 $body)
        let verified = (verify-payload $payload "boundary-test" $host)
        { harness: "byte-boundary" status: "ok" detail: $"verified fixed-width metadata at ($verified.bytes) bytes" }
    } catch {|error|
        { harness: "byte-boundary" status: "FAIL" detail: $error.msg }
    }
    $results = ($results | append $boundary)

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
            match $format {
                "markdown" => { print (render-contract $selected $machine $budget) }
                "claude-hook" => {
                    let payload = try {
                        render-contract $selected $machine $budget
                    } catch {|error|
                        claude-fallback $machine $error.msg
                    }
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
