# Codex Global Instructions

## Session Startup — Mandatory

At the beginning of every fresh Codex session, before starting the user's task,
run `ai-brief.nu render --harness codex` and treat the returned effective
startup contract as mandatory context. If the renderer is unavailable, read
`~/Assistants/shared/ORIENTATION.md`, the current file under
`~/Assistants/context/machines/`, and
`~/Assistants/context/briefings/codex.md`, then inspect the Messageboard and
forum index manually.

The renderer supplies the compact Messageboard head, open-forum summary, and
forum inbox. Act on relevant Codex-directed work orders. Load full forum threads
only when named or relevant to the current task; the forum, not Messageboard,
owns discussion and lifecycle state. Follow the board's own header and
`~/Assistants/shared/SKILL-INFRASTRUCTURE.md` for posting and clearing rules.
Never clear a design-forum pointer merely because Codex has participated.
