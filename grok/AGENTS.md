# Grok Build Global Instructions

## Session Startup — Mandatory

At the beginning of every fresh Grok Build session, before starting the user's
task, run `ai-brief.nu render --harness grok-build` and treat the returned
effective startup contract as mandatory context. Grok SessionStart hook stdout
is ignored, so this global rule—not a printing hook—is the loading bridge.

If the renderer is unavailable, read `~/Assistants/shared/ORIENTATION.md`, the
current file under `~/Assistants/context/machines/`, and
`~/Assistants/context/briefings/grok-build.md`, then inspect the Messageboard,
forum index, and forum inbox manually.

Project and directory rules layer after this global contract. Load full forum
threads only when named or relevant to the task. Headless forum participation
is deliberation only and never authorizes implementation.

## Forum waiter (Stop hook)

A Grok `Stop` hook (`grok-forum-waiter-gate`) blocks ending a turn when this
session queued `forum convene --background` and did not attach the `monitor`
tool to that job id. That is a harness gate, not a skill. Session-end Stop
(`reason` other than `end_turn`) is ignored. Do not replace the hook with an
instruction.
