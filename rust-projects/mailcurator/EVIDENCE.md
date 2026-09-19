# Expense evidence and retention

## Booking safeguards (2026-09-19)

Automatic trash now defaults OFF at config load, including Sweep/`--now` and
newly proposed rules. Only an explicit top-level `allow_automatic_trash = true`
can enable eligible lifecycle deletion. The deployed private policies explicitly
set this false and have their old delete thresholds commented out, so even an
older binary cannot run those trash rules. Tag-only placeholders keep policies
with no remaining lifecycle action valid.

Every booking-extractor match is marked `booking` before policy selection.
All archive queries exclude booking matches and the booking tag, even across
overlapping rules and `--only --now`. The independent six-month personal Inbox
cleanup also excludes the booking tag and the supported booking-provider
domains. This is an interim hold, not a date-aware acknowledgement workflow.
Manual filing still works; no historical mail is automatically put in Inbox.

The bounded historical Airbnb recovery restored 50 audited messages out of
Gmail Trash and marked their copies on both hosts `booking` + `curator-retain`.
Private original-message backups and a pre-change manifest are under each
host's `Backups/mailcurator/2026-09-19-booking-safeguards` (full .eml copies on
nimbini). Never put recovery contents or private policies in Git.

MailForge's `/mail/bookings` is a deterministic, read-only register with
uncertain-date visibility and possible-overlap warnings. It never authorises
source deletion, asserts payment, or infers that missing evidence means no
booking exists. Future-booking lifecycle decisions remain deferred.

The normal `run` command is deterministic. The personal and CoHS post-new hooks
also explicitly pass `--llm-disable`. Every call to Claude requires the global
`--allow-llm` option, with `NOTMUCH_CONFIG` explicitly selecting
`$HOME/Mail/.notmuch-config`. CoHS and custom indexes cannot opt in. This is a
tool policy, not an OS sandbox or proof that personal mail contains no PHI.
LLM-assisted label/propose/improve operations also require this opt-in.
Hooks fail closed (skip curation, not mail sync) when an older binary lacks the
`evidence` command, including a rollback after a signing/deployment failure.

Before any lifecycle policy runs, all extractor-matching messages and existing
`billing`, `receipts` or `Expenses` tags receive `curator-retain`. All trash
queries exclude that tag and the current protection query, including `--only`,
`--now`, dry runs and destroy previews. Overlapping noise policies cannot trash
protected evidence. Non-booking archiving remains enabled. Previously trashed messages are
not restored; this is not a backup or a protection from manual/provider deletion.

`mailcurator evidence --json [--offset N] [--limit N] [--exceptions-only]`
is a read-only, version-1 projection of the local ledgers, not a mail operation.
It never invokes notmuch or an LLM, does not write coverage snapshots, and does
not approve expenses or post to Xero. It powers MailForge's Bills & exceptions
page. It unions legacy and host-sharded files present on this machine (it does
not itself sync them), consolidates duplicates by account/policy/message ID
using extraction timestamps, counts malformed rows, and flags missing amounts,
supplier/currency/date/account provenance and LLM-derived fields. Required-field
completeness is shown for vendor extractors; no records means unknown. Field
presence is not correctness. A 200 MB input cap prevents unbounded UI reads.

This output remains sensitive, practitioner-facing financial data, NOT an
assistant-safe/deidentified export. Old rows without account provenance have no
guessed source link. Newly extracted rows record the known canonical account.
Historical bad rows are displayed for review, not silently backfilled with AI.

`fd-budget enrich` now defaults to the ledger directory and reads legacy plus
all regular host shards, even if no legacy file exists. `--from FILE` still
selects exactly one file; `--from DIRECTORY` unions the shards. It does not
rewrite bank records or source evidence. Existing matches are refreshed only
when the operator runs enrichment; deployment does not rerun it automatically.

Verification: `cargo test --locked`; `tests/evidence_safety.rs` uses an isolated
synthetic notmuch database and a fake Claude capture program. It proves source
retention under overlapping `--only --now`, default zero LLM invocations,
explicit personal opt-in as a positive control, and CoHS opt-in refusal.
