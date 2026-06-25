#!/usr/bin/env nu

# rust-learn-resume — start, attach, or resurrect the rust-learning Zellij session
#
# Behaviour:
#   - If a running `rust-learning` session exists: attach to it
#   - If a stale (EXITED) `rust-learning` session exists: delete it, then start fresh from the layout
#   - If no `rust-learning` session exists: start fresh from the layout
#
# Layout: ~/rust-learning/unified/learning-layout.kdl
#   → 2 panes: claude (CC) | workshop (course-system loaded).
#     The Rust book is read in a BROWSER (3-panel design deprecated):
#       `rust-learn book`       → online Brown book (rust-book.cs.brown.edu)
#       `rust-learn book-serve` → offline: mdbook serves book-brown on :3000

const SESSION_NAME = "rust-learning"
const LAYOUT_PATH = "~/rust-learning/unified/learning-layout.kdl"

def main [] {
    let layout = ($LAYOUT_PATH | path expand)

    if not ($layout | path exists) {
        print -e $"(ansi red_bold)error:(ansi reset) layout not found at ($layout)"
        exit 1
    }

    # Inside an existing Zellij session, Zellij refuses to start/attach a
    # nested session, so the create/attach path below silently no-ops.
    # Open the rust-learning workspace as a new TAB in the current session
    # instead (the layout names the tab "rust-learning").
    if ($env.ZELLIJ? | is-not-empty) {
        print $"(ansi cyan)Inside a Zellij session — opening rust-learning as a new tab...(ansi reset)"
        zellij action new-tab --layout $layout
        return
    }

    let sessions = (
        try { zellij list-sessions | ansi strip | lines }
        catch { [] }
    )

    let matches = ($sessions | where { |line| $line | str contains $SESSION_NAME })
    let alive = ($matches | where { |line| not ($line | str contains "EXITED") })
    let exited = ($matches | where { |line| $line | str contains "EXITED" })

    if ($alive | length) > 0 {
        print $"(ansi green)Attaching to running ($SESSION_NAME) session...(ansi reset)"
        zellij attach $SESSION_NAME
    } else if ($exited | length) > 0 {
        print $"(ansi yellow)Found stale ($SESSION_NAME) session — deleting + starting fresh from layout...(ansi reset)"
        zellij delete-session $SESSION_NAME
        zellij --session $SESSION_NAME --new-session-with-layout $layout
    } else {
        print $"(ansi cyan)Starting fresh ($SESSION_NAME) session from layout...(ansi reset)"
        zellij --session $SESSION_NAME --new-session-with-layout $layout
    }
}
