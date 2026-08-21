# Activity logs (dotfiles)

Current behaviour lives in the scripts and in `~/Assistants/shared/QUANTIFIED-TRACKING.md` plus `~/Assistants/shared/AI-ACTIVITY-CLASSIFICATION-SYSTEM.md` (short current-state page; the 2025 manifesto was replaced 2026-08-21).

**Pipeline:** `collect-entries` → optional `auto-tag-activities` (root `NapierianLogs/*.md` only, OpenAI, skip if `tags:` present) → `generate-subactivity-lists` (hub `## Sub-activities` from dotted filenames in root + `Social/`).

**Do not** treat this as a “no Social/ directory, tags instead of folders” design. `Social/` is how person keys route. Parent files are hubs, not rolled-up child journals.

| Script | Path |
|--------|------|
| collect-entries | `scripts/collect-entries` |
| generate-subactivity-lists | `scripts/generate-subactivity-lists` |
| auto-tag-activities | `scripts/auto-tag-activities` |
| classify-activity | `scripts/classify-activity` |
