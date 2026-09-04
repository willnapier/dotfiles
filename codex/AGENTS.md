# Codex Global Instructions

## Session Startup — Mandatory

At the beginning of every fresh Codex session, before starting the user's task,
run `ai-brief render --harness codex` and treat the returned effective
startup contract as mandatory context. If the renderer is unavailable, read
`~/Assistants/shared/ORIENTATION.md`, the current file under
`~/Assistants/context/machines/`, and
`~/Assistants/context/briefings/codex.md`, then inspect the Messageboard and
forum index and run `forum inbox` manually.

The renderer supplies the compact Messageboard head, open-forum summary, and
forum inbox. Act on relevant Codex-directed work orders. Load full forum threads
only when named or relevant to the current task; the forum, not Messageboard,
owns discussion and lifecycle state. Follow the board's own header and
`~/Assistants/shared/SKILL-INFRASTRUCTURE.md` for posting and clearing rules.
Never clear a design-forum pointer merely because Codex has participated.

## Tier routing — when to delegate to `luna-implement`

A custom subagent `luna-implement` (gpt-5.6-luna) exists for bounded mechanical
work. It draws the shared Codex allowance at roughly a twentieth of Sol's rate.
Delegate to it only when ALL four hold: the spec is crisp; correctness is checked
by the compiler and existing tests, not by judgement; the blast radius is
bounded; the change is reversible. Typical: post-decision follow-through after a
forum thread is decided (rebuilds, plumbing, renames, doc updates, CI fixes).

Never delegate: any path listed in the repository's `.frontier-only` file, any
test file, and design or review work. Default is Sol; Luna needs an affirmative
case. Before accepting a Luna result, read the diff yourself and re-run the tests.
If a forum work order names a tier, follow it. Design record and the frontier-only
list rationale: `~/Assistants/shared/CODEX-TIER-ROUTING.md`.
