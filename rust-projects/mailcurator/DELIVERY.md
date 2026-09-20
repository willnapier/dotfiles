# Verified destination delivery

Information-bearing mail is automatically archived only after verified delivery,
not merely extraction or visibility in MailForge. Manual archive is unchanged.
Automatic trash remains disabled; this feature never deletes source or destination.

## First adapter and scope

`[verified_delivery]` configures a prospective, personal-account-only `local_file`
booking adapter. Paths are relative to HOME for macOS/Linux portability:

```toml
[verified_delivery]
root = "Forge/Householding/Bookings"
writer = "nimbini"
not_before = "2026-09-20T00:00:00Z"
```

Only the configured writer runs this pipeline, independently of recent MailForge
activity. No fallback writer is elected. Missing configuration means no delivery;
information mail remains held. The account must be explicitly selected with
`NOTMUCH_CONFIG=$HOME/Mail/.notmuch-config`. The adapter never exports CoHS or
custom-index mail. Financial/other destinations are not inferred from this one.

One immutable, human-readable Markdown file per account/policy/source identity
contains the entire structured extraction, schema/item id, evidence date and a
local MailForge source link. It is a replaceable capture destination in Forge,
not a calendar or a complete/current reservation inventory. It does not infer
payment or cancellation state. Keep human notes in a separate linked note; editing
the managed file deliberately invalidates the receipt instead of being overwritten.
The filename is an opaque stable digest, not a subject, property or client name.

The v1 capture contract requires property, booking reference and explicit-year
check-in/out dates with a valid future interval relative to the source date.
Ambiguous slash dates (such as 03/04/2027) also require review.
All additional extracted fields are included too. The original full email remains
retained as evidence; this contract does NOT claim to understand every sentence.
Missing/inferred dates, LLM-derived fields, cancellation/amendment/reminder subjects,
attachments (including embedded binary parts), old sources, conflicting references
or overlapping stays in existing receipts/evidence are held, not merged.

No historical backfill occurs merely by configuring delivery. Only newly processed
sources at/after `not_before` are eligible; old `-extracted` tags and legacy ledger
rows never mint delivery receipts. Failed delivery gets a per-policy retry tag;
retries do not repeatedly append extraction ledger rows.

## Receipt and archive gate

Order: exclusive single-process run lock; deterministic extraction; no-clobber
file publication with file/directory sync; read-back; durable receipt publication;
fresh verification; only then remove Inbox. A crash between file and receipt adopts
the identical file on retry. Different or removed acknowledged files are never
overwritten or resurrected. Symlinks, unsafe paths, excessive reads and ambiguous
source identities fail closed.

Receipts in `~/.local/share/mailcurator/delivery-receipts/` bind account-qualified
identity, match/extractor/vendor-contract fingerprint, destination-relative path, SHA-256 of
the destination and source bytes, and minimal dates/dependency hashes. They contain
no body, subject or property name. Receipts are writer-local operational state;
Forge files sync normally. Losing the receipt store causes holds, not guessed proof.
They are not disposal tombstones, and no lifetime/backup-erasure promise is made.
Schedule or tag edits do not invalidate receipts; changing the capture contract
does. There is no automatic re-attestation of changed contracts or edited records.

Every matching extraction policy must be satisfied; an unsatisfied expense claim
blocks filing even when the booking file exists. Financial labels are held until
a financial destination is implemented. All policies' declared information and
financial tag additions are claims before they run, independent of policy order
or `--only`. Booking-like subject/provider cues and
existing evidence/retention tags also hold unknown senders against broad noise
policies. This conservative cue net is not proof of complete recognition.
Unknown mail no longer disappears solely because it is six months old.

The gate applies to overlapping policies, `--only`, `--now`, and dry runs. It never
trusts a verification tag. On-arrival actions cannot remove Inbox/protection tags.
The personal hook no longer has an independent age-filing operation. Both hooks
require the `delivery-status` capability before running curation; missing/older
binaries skip curation without stopping mail sync. Outgoing/Sent classification
remains separate from automatic inbound filing.
The trash path independently excludes the entire information net and recorded
source identities, even if legacy automatic-trash opt-in is enabled. Successful
delivery is never destruction authority. CoHS also keeps information-like matches
in Inbox, but never exports them to this adapter or reads personal receipts;
each index has a separate process lock.

`mailcurator delivery-status --json` is a read-only, sensitive user-facing report
of information awaiting delivery and existing receipts (including archived mail).
It gives source ids, verified destination paths and held reasons, but no bodies or
LLM calls. An edited/missing destination remains an exception; it neither restores
Inbox nor deletes anything. This is not an assistant-safe or deidentified API.
`curator-delivery-pending` is an Inbox review marker, not authority to archive.
Run this report on the designated writer; other hosts do not own its receipts.
The report names its account and refuses an unset/unrecognised `NOTMUCH_CONFIG`.
Before migrating the writer, copy and validate its receipt store along with the
destination files; otherwise archived deliveries fall out of monitoring.
Manual archive remains the deliberate human release path for held messages; it
does not mint a delivery receipt or claim verified capture.
Recognised receipt, destination and source failures have distinct reason codes.

## Operational limits

Live source/destination files can still be edited outside MailCurator. Read-back
verification is fresh immediately before filing, not a distributed transaction
against concurrent manual edits. The follow-up status check detects later drift.
The writer-local lock serialises MailCurator runs, not unrelated applications.

Forge replication and backup/version-history copies remain subject to their own
retention. Before activation, inspect these rather than assuming a file deletion
erases every copy. No expiry or coordinated destruction is implemented here.
Later cancellation correspondence remains visible in Inbox; records explicitly
state their evidence date and require checking later mail. There is no automatic
amendment reconciliation or claim of a complete trip register.
Sender authenticity is not verified: the receipt attests local capture integrity,
not that a sender or a claimed reservation is genuine. Conservative MIME/subject
checks may hold confirmations with inline logos or "free cancellation" wording.
Malformed/oversized receipt or legacy evidence inventories fail closed and need
operator repair; they are not silently discarded. Searches are scoped to Inbox
plus already-receipted sources, not the entire historical mailbox.
The combined source-id query can hit the OS argument-length limit before the
inventory cap; this fails closed. Chunked queries and static capture-rejection
reason codes are tracked follow-ups before broadening destination adapters.

## Deployment order

1. Deploy both hosts' hooks first and check no old six-month prune remains.
   With an old binary, the new hook safely skips curation.
2. Build/test the candidate on both hosts. Install the writer's binary before
   enabling its private destination configuration or booking archive thresholds.
3. Preserve a signed Mac binary if the signing identity is unavailable. Do not
   install unsigned over it or enable new archive thresholds with the old binary.
   The Mac hook skips curation until its signed upgrade; nimbini owns processing.
4. Verify installed hashes, synthetic positive/negative workflows, live read-only
   held volume, and a bounded live run. Zero eligible deliveries is not proof of
   real booking capture; report it separately from non-empty synthetic tests.
   Inventory other invokers (MailForge/manual Sweep is separate from hooks),
   verify the writer identity in the sync service environment, check multiple
   source copies, and establish which operational tags can reach each server.

## Verification

`cargo test --locked` exercises real notmuch against synthetic mail only. Tests
cover verified release, routine filing, missing destination, overlap/only/now,
forged tags, multi-policy/financial claims, broad unknown-provider holds,
uncertain dates, attachments/cancellations, manual edit/removal, changed source,
crash adoption, symlinks, wrong writer, duplicate-free retries, and the hook's
no-age-only-filing behaviour. `DELIVERY_TEST_BIN` can run the positive workflow
test against the previous deployed binary as a known-red control.
