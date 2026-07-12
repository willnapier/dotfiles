# fd-budget — Fable adversarial review, 2026-07-12

**Model:** Claude Fable 5 (subscription window), 6 parallel adversarial reviewers, one per concern.
**Verifier:** Opus 4.8 — top/HIGH findings re-read against the code (marked ✅ VERIFIED); the rest carry each reviewer's own CONFIRMED/PLAUSIBLE label.
**Scope:** `~/dotfiles/rust-projects/fd-budget` @ `13e5f54` (~11k LOC). Read-only; no code changed.
**Baseline:** run `cargo test` before applying any fix — findings are against a (presumed) green tree; confirm it.

## STATUS — 2026-07-12 (Opus fix pass, deployed + migrated)

**All 8 HIGH findings FIXED, tested (170 lib + 8 + 3 e2e green), deployed to `~/.local/bin/fd-budget` (rollback `.prev`), and the real store migrated (2747 rows re-keyed, 28 collision groups un-collided, totals byte-identical; backup `~/fd-budget-migrate-backup-2026-07-12-163623`).**

| Finding | Status | Note |
|---|---|---|
| H1 store-load laundering | ✅ FIXED | `TryFrom` fail-closed + refuse-to-rewrite; +7 tests |
| H2 mid-file import truncation | ✅ FIXED | date is the sole footer discriminator; +3 tests |
| H3 dedup collision | ✅ FIXED | occurrence-indexed id + `migrate-ids` migration; +4 tests |
| H4 FX rate-direction | ✅ FIXED (properly) | root cause confirmed vs a real export: PayPal puts the rate on the GBP conversion row, not the foreign leg — amount-link now reads it there, direction-agnostic (real data is `amount/rate`); 39 real recoveries went medium→**high** (amount-verified) |
| H5 FX amount-blind fallback | ✅ FIXED | tight time-gate + MEDIUM confidence; +2 tests |
| H6/H7 subscriptions grouping | ✅ FIXED | amount-clustering within a merchant; +3 tests |
| H8/H9 coverage | ✅ FIXED | end-truncation caveat + month-presence recoverability; +2 tests |
| H10 smoothing window-drift | ✅ FIXED | annual-double counted once + notes; +3 tests |
| M14 tx_type erased on rewrite | ✅ FIXED | folded into H1 (`from_code` accepts `as_str` names) |

**ALL MEDIUM findings FIXED (2026-07-12, second pass — 5 parallel worktree agents, integrated + full-suite-verified, 188 lib + 10 + 3 green, deployed):** M1 (pessimistic buffer ordering), M2 (honest retrospective-sizing doc), M3 (short-history coverage note), M4 (median representative), M5 (same-day amounts kept), M6 (lower-median + max-gap guard), M7 (directional price-change flag), M8 (exclusion-line precedence chain), M9 (window-length annualisation), M10 (case-insensitive buckets), M11 (counterparty nonspend/card exclusion), M12 (reapply re-applies the card `transfer` tag), M13 (whole-token + stopword name match), M15 (sidecar schema-migration on append), M16 (amount-scaled FX tolerance), M17 (skipped-row count surfaced). Spend floor unchanged (£153,598.36); `--by-category` still reconciles exactly. **LOW findings (L1–L15) remain OPEN** — the natural next pass if wanted. **H4/H5 sub-question RESOLVED (2026-07-12):** a fresh real export (`~/Downloads/Download.CSV`) showed the `Exchange Rate` column IS present and populated — but on the GBP `General Currency Conversion` row (e.g. `1.265008669165684`), not the foreign payment leg (blank). The importer captured it correctly; the recovery algorithm read it from the wrong row. Fixed properly (see H4 above): pass-1 now sources the rate from the conversion row, so the amount-link fires on real data and all 39 FX recoveries are amount-verified (High).

---

## TL;DR — two spines run through almost every finding

1. **The load path launders corruption, and rewrites persist it.** `csv_store.rs` (and the PayPal sidecar) turn any unparseable cell into a *silent* default — amount → `£0.00`, date → `1970-01-01`, account → `Current` — then `tag reapply/set/rename` truncate-rewrite the store from those laundered rows, **destroying the original text irreversibly**. Zeroed money is invisible to the "reconciles exactly ✓" check. Found **independently by 3 of 6 agents**.

2. **Absence reads as completeness.** Spans, coverage %, subscription cadence, and the smoothing window are all derived from *whatever data happens to be present*, with no cross-check against an expected window — so missing months, truncated imports, dropped duplicates, and unbound FX legs all read as "fine." The coverage module — whose entire purpose is to prevent this — has the bug in its own verdict line.

**Correction to prior assumptions:** money math is `rust_decimal::Decimal` end-to-end. There is **no float-drift bug** anywhere; the "reconciles exactly" invariant itself is sound (each spend row → exactly one bucket, truncation tail folded back, rollup catches all). The bugs live at **ingest and the edges**, not the core arithmetic.

---

## Priority fix sequence

| # | Fix | Files | Why first |
|---|-----|-------|-----------|
| 1 | **Make the store load fallible** — `TryFrom<StoredTransaction>`, no `unwrap_or_default`/1970/`Current`; refuse to `rewrite()` a store that didn't round-trip | `store/csv_store.rs:162-186` | Cheap; closes the single most dangerous, **irreversible** corruption path; 3-agent confirmed |
| 2 | **Fail closed on mid-file import anomalies** — only `break` on true footer (empty first field / known footer text); else warn-with-line-number and skip or abort | `import/midata.rs:38,44,55,65` | Silent import truncation loses months with a "success" message |
| 3 | **Fix the dedup key** — add an occurrence counter (n-th identical row that day) and/or `balance`; dedup within-batch | `dedup.rs:7-15` | Silent drop of distinct duplicates **and** PayPal merchant-collapse; 3-agent confirmed |
| 4 | **PayPal FX: make the amount link direction-agnostic** (`amt*rate` **or** `amt/rate` within tolerance), require an amount corridor in the pass-2 fallback (no amount-blind `High`), fix the two contradictory tests | `paypal/recover.rs:499,517-525` | The flagship "leg-binding fix" is likely **inert in production** |
| 5 | Subscriptions: cluster by amount-band within a merchant before cadence; require amount-similarity for 2-occurrence annual | `subscriptions/mod.rs:363-388,293` | Two subs under one descriptor vanish; seasonal shopping → phantom annual sub |
| 6 | Coverage: cross-check each source span vs the filter/union window; recoverability by **month-presence** and **£-value**; always emit the under-count caveat | `coverage/mod.rs:152-201,298-315` | The module's own reason to exist is defeated |
| 7 | Smoothing: cadence-dedup the window (cap one lump per tag per ~year), coverage-guard the actuals basis, pessimistic buffer ordering | `smooth/mod.rs:339-364,384-390` | Annual bill → 0×/2× drip; buffer understated |
| 8 | Stats/UX correctness batch (below) | `query/`, `main.rs` | Individually smaller; several are silent mis-reports |

---

## HIGH findings

### H1 ✅ VERIFIED — Store load launders corrupt rows to £0/1970, and every tag command rewrites the corruption in permanently
`store/csv_store.rs:170-177` (load), `:113-129` (`rewrite` = `File::create` truncate + reserialize). Found by the reconciliation, coverage, and enrich agents independently.
- **Bug:** `NaiveDate…unwrap_or_else(→1970)`, `Decimal::from_str…unwrap_or_default(→ZERO)`, `account.parse().unwrap_or(Current)` — all silent. `ZERO.is_sign_positive()` is true, so a zeroed debit is counted as a £0 credit and `--by-category` still says "reconciles exactly ✓".
- **Scenario:** hand-edited `-£12.50` (with £ sign) in transactions.csv → loads as £0.00 → totals under-count by £12.50, no warning; then a routine `tag reapply` does `load_all → rewrite`, writing `0.00` over the original string — the /tmp snapshot is now the only copy of the real amount.
- **Fix:** fallible `TryFrom`; abort load (or refuse to rewrite) on any non-round-tripping row.

### H2 ✅ VERIFIED — Any mid-file parse anomaly silently truncates the rest of the import
`import/midata.rs:38,42-44,55,64` (same pattern at 114-133, 183-203).
- **Bug:** the "footer detection" heuristic `break`s the whole loop on a CSV error, `record.len()<5`, empty first field, unparseable date, **or** unparseable amount — anywhere, including mid-data.
- **Scenario:** 500-row export, row 200 has an oddly-rendered amount (`"-"`, `"0.00 CR"`, a quoting glitch) → rows 200-500 discarded, prints "Parsed 199 transactions" + success. A later re-import dedups the first 199 and re-drops the rest.
- **Fix:** only treat empty-first-field / known footer text as end-of-data; else warn-with-line-number and skip or abort (fail closed) + report a skipped-row count.

### H3 ✅ VERIFIED — dedup key collides distinct same-day/same-amount rows; PayPal rows collapse merchants
`dedup.rs:7-15`, `paypal/recover.rs:617-622` (RecoveryIndex). Found by enrich + PayPal agents; corroborates H1's store fragility.
- **Bug:** `import_id = sha256(date, amount, raw_description)` — no `balance`, no separators. Every bare PayPal row shares the description `"PAYPAL PAYMENT"`, so two same-amount same-day PayPal purchases hash identically; `deduplicate` also does no within-batch dedup.
- **Scenario:** two −£12.99 purchases on 2026-06-03. Export 1 has one, export 2 (overlapping) has both → the second is **never stored** (£12.99 vanishes). If both are stored, `RecoveryIndex` (HashMap) keeps one merchant → both rows show the same (one wrong) merchant. And `tag set <full-id>` is unresolvable (`Ambiguous`, "supply more characters" — impossible).
- **Fix:** occurrence counter and/or balance in the hash; delimit fields (`{date}|{amount:.2}|{desc}`); make `tag set` accept an occurrence disambiguator. (Delimiting needs a one-off id migration.)

### H4 ✅ VERIFIED — The PayPal "leg-binding fix" is likely inert in production (wrong FX rate direction)
`paypal/recover.rs:499` (pass 1), `:517-525` (fallback), `store.rs:57-61` (doc), tests `:711-730` vs `:836-871`.
- **Bug:** pass 1 binds by `foreign_amount.abs() * exchange_rate ≈ bank_abs`, assuming rate = GBP-per-foreign. But the doc's own example and the `fx_chain` test use `1.1009` on a €299.40/£272.01 chain — that's foreign-per-GBP (299.40 **/** 1.1009 = £272.0). The two tests use **opposite** conventions: `fx_chain` passes only via the pass-2 fallback; `two_same_day…do_not_swap` uses inverted rates (0.909091) so pass 1 fires.
- **Scenario:** if real exports use the `1.1009` direction, pass 1 **never fires** → every FX bind degrades to timestamp-nearest → two same-day FX chains swap merchants by Transaction-ID tie-break (exactly what the fix claims to prevent).
- **Open question (needs one real `paypal.csv` FX row):** which direction does PayPal actually export? The doc example strongly implies foreign-per-GBP, i.e. the bug is real.
- **Fix:** accept `amt*rate` **or** `amt/rate` within tolerance (direction-agnostic); make both tests use the real direction so pass 1 is exercised.

### H5 ✅ VERIFIED — PayPal FX fallback binds ANY foreign leg with no amount check, recorded as `High` confidence
`paypal/recover.rs:517-525` (`is_foreign_payment_leg` only).
- **Bug:** when pass 1 finds nothing (per H4, possibly always), pass 2 picks the timestamp-nearest non-GBP leg within ±2 days with **zero amount constraint**, recorded `RecoveryConfidence::High`. (The doc claims the fallback runs "only when no leg carries an exchange rate" — the code falls back whenever pass 1 fails to *match*, rates present or not.)
- **Scenario:** bank −£272.01; true EUR leg missing (partial export / already consumed) → an unrelated USD −$5.00 leg two days away is bound: merchant = that $5 merchant, `bank_amount = -272.01`, confidence "high". The correct chain's deposit+conversion are consumed and starved.
- **Fix:** require a plausible amount corridor in pass 2, or emit `Medium`/skip instead of binding amount-blind.

### H6 ✅ VERIFIED — Subscriptions: two subs under one descriptor destroy each other's cadence and vanish
`subscriptions/mod.rs:363-388` (merchant-only grouping), `:289` (band).
- **Bug:** grouped by canonical merchant alone; interleaved billing dates push the median gap out of 25..=35 → `classify` returns `None` → nothing reported. `representative_amount` also uses one amount so the second sub's cost is never counted.
- **Scenario:** `APPLE.COM/BILL` £2.99 on the 5th + £9.99 on the 19th → merged gaps ~14/17 → median ~15 → **zero Apple subs reported**, ~£155/yr → nothing. All unrecovered `PAYPAL PAYMENT` rows share one key → same failure.
- **Fix:** within a merchant group, cluster by amount-band (or day-of-month) first, classify each cluster, sum annualised across clusters.

### H7 — Subscriptions: any merchant used twice ~a year apart becomes a phantom "annual subscription"
`subscriptions/mod.rs:293` (annual band), `:188` (`min_annual: 2`). Reviewer CONFIRMED.
- **Bug:** `min_annual=2` + amount dropped from the key + no amount-consistency check → two unrelated one-offs a year apart classify as annual.
- **Scenario:** `GARDEN CENTRE` £85 (2024-12-20), £12.50 (2025-12-18), gap 363 → "annual subscription", representative £12.50, spurious `WidePriceRange` note. Seasonal shopping (Christmas, MOT) systematically hits this. The old `(merchant, exact-amount)` key required identical amounts; nothing replaced that guard.
- **Fix:** for 2-occurrence annual, require amount similarity (≈10-15%) before classifying.

### H8 — Coverage: end-truncated sources report "no gaps" — the module's own motivating case passes silently
`coverage/mod.rs:152-157` (span = source's own min/max), `:197-201` (`gaps==0` → "no gaps." with **no** under-count caveat; the SPARSE warning lives only in the `gaps>0` branch, `:204-212`). ✅ VERIFIED.
- **Scenario:** Visa export itemises Jun–Dec 2025 only (7 contiguous months) → "covers 2025-06..2025-12 — 7 of 7 months present, no gaps." Cov%=100. Jan–May's ~£2k reads fully covered. Nothing compares the Visa span to the current-account span or the `--year` window.
- **Fix:** compare each source's span vs the filter window and/or the union of all sources; count leading/trailing missing months; always emit the "outside this span" caveat.

### H9 — Coverage: recoverability treats `min..max` of the export as solid — interior export gaps count as recoverable
`coverage/mod.rs:298-315`. ✅ VERIFIED (mechanism at H1's sibling load path).
- **Scenario:** import a Jan-2025 export + a Dec-2025 export (nothing between) → bank PAYPAL rows Feb–Nov all satisfy `earliest ≤ date ≤ latest` → "12 of 12 (100%) recoverable" — yet 10 months have zero export to join to and `paypal recover` fails on all. The exact silent-truncation the module exists to expose.
- **Fix:** classify "within" by month-presence of the export source (intersect with its `missing_months`), not span endpoints.

### H10 — Smoothing: actuals ÷12 samples a rolling window → an annual bill drifting across the edge yields 0× or 2× drip
`smooth/mod.rs:339-364` (window bucket `continue` on out-of-window rows), `:243-250` (anchor). Reviewer CONFIRMED; ✅ buffer-netting sibling verified.
- **Scenario:** insurance £1,200 paid 2024-08-05, renews 2025-07-28, latest data month = July 2025 → window Aug 2024–Jul 2025 contains **both** → annual_total £2,400, drip £200 (2×). Mirror case → neither in window → drip £0, obligation invisible. Printed as the standing order with no cadence/plausibility check.
- **Fix:** dedup by cadence (cap one lump per tag per ~12-month period; warn when matched payments are <2 or >13 months apart, or count ≠ last year's).

---

## MEDIUM findings

**Smoothing**
- **M1 — buffer understated by one drip** (`smooth/mod.rs:389-392`, ✅ VERIFIED): `running += drip - monthly_outflow[i]` credits drip and debits the lump in the same step, assuming the SO lands before the bill. If the DD fires first, the true trough is one drip lower. *Fix:* subtract outflow, record trough, then add drip (pessimistic).
- **M2 — the claimed "per-pot balance + won't-cover flag" does not exist** (`smooth/mod.rs`, `lib.rs:130`, ✅ VERIFIED): the module is one-shot retrospective *sizing* only; no pot ledger, no lookahead, no flag that can fire. A pot going negative in reality is never surfaced. *Fix:* build the forward layer, or correct the description.
- **M3 — short data history silently shrinks the drip** (`smooth/mod.rs:350`, `main.rs:600-613`): buckets with no coverage contribute £0, indistinguishable from lump-free months; `cmd_smooth` never consults `coverage`. *Fix:* warn/refuse actuals basis when the store doesn't span the full window.

**Subscriptions**
- **M4 — representative amount degenerates to the MINIMUM for FX subs** (`subscriptions/mod.rs:323-335`, ✅ VERIFIED via `max_by_key((count, Reverse(amount)))`): when every charge is distinct (normal for FX), all counts=1 → returns the min → `annualised()` = 12×min, systematically low. Also picks the OLD price after a rise. *Fix:* use median or most-recent charge.
- **M5 — same-day second charge dropped from amount stats** (`subscriptions/mod.rs:399-406`, ✅ VERIFIED): same-day collapse keeps only the first charge's amount (input-order dependent). *Fix:* collapse dates for cadence only; keep all amounts (or the day's sum).
- **M6 — even-gap-count median averages two central gaps** (`subscriptions/mod.rs:265-273`): out-of-band gaps can average into the band (false monthly); one doubled gap pulls a 2-gap median out (real monthly missed). Contradicts the "median absorbs a doubled gap" doc (true only for odd counts). *Fix:* lower-median + a regularity requirement (≥60% of gaps individually in-band).
- **M7 — price-change flag is amplitude-only** (`subscriptions/mod.rs:54,444-452`): `(max-min)/rep > 0.10` with no time structure — misses a permanent £10.99→£11.99 step (9.1%), cries wolf on 18 months of >10% FX drift. *Fix:* compare median of first-k vs last-k charges (sustained level shift).

**Reconciliation / stats**
- **M8 — business + one-off double-counted across exclusion lines** (`main.rs:1594-1617`): a row tagged both appears in full on both lines, breaking "Spend + Excluded == all debits" by its amount. *Fix:* precedence chain, or an explicit overlap line.
- **M9 — `--budget` treats any sub-year window's spend as annual** (`query/mod.rs:915,1074-1087`): `span_days.max(365)` clamps the divisor, so `--month` compares one month's spend directly to a £/yr target — everything shows green. *Fix:* annualise over the actual window length (with a short-window caveat).
- **M10 — category buckets are case-SENSITIVE, everything downstream case-insensitive** (`query/mod.rs:535-537` vs `:813-818,:882-884`): `Groceries` and `groceries` split into two buckets, each compared against the full budget → two "under budget" lines for a category that's over. *Fix:* lowercase the bucket key (keep first-seen casing for display).
- **M11 — `--by-counterparty` double-counts Visa payoffs when matches.jsonl is stale** (`query/mod.rs:334-336`): internal-transfer exclusion relies only on `confidence=="internal-transfer"`, ignoring the row's `transfer` tag / `is_card_payment()`; unmatched → `"none"`. Import-without-enrich → the full card bill counted on top of itemised purchases. *Fix:* also skip `is_nonspend()`/`is_card_payment()` rows.

**Enrich / import**
- **M12 — `tag reapply --reset` strips the auto-applied `transfer` tag → Visa payoffs enter the Spend floor** (`main.rs:1165-1203` vs `:1052-1063`): `--reset` rebuilds from rules + PayPal recovery but never re-runs `auto_tag_card_payments`, which isn't reproducible from rules. Spend floor inflated by the full card bill every month until `tag tag-transfers` is manually re-run. *Fix:* call `auto_tag_card_payments` in the reapply path (already idempotent).
- **M13 — enrich name-match: High confidence on any ≥3-char token substring, either direction** (`enrich/mod.rs:321-330`): generic tokens (`ltd`, `com`, `group`, `the`) match unrelated pairs. `GROUPON` ⊃ token "group" ⊂ "BT Group" bill at the same £45 same day → Groupon spend attributed to BT. *Fix:* whole-token equality + stopword list; never award High from the vendor⊇desc direction.
- **M14 — `tx_type` destroyed on the first store rewrite** (`store/csv_store.rs:151,173` vs `lib.rs:56-67`): writes `tx_type.as_str()` ("direct_debit") but reads via `from_code()` (accepts only bank codes "DD") → every load = `Unknown(0)`; the next rewrite persists "unknown" for the whole store. Unnoticed because no query consumes it yet. *Fix:* teach `from_code`/`from_str` to accept the `as_str` names.
- **M15 — PayPal append to a pre-upgrade sidecar breaks the store** (`paypal/store.rs:360-385`): a 9-column old header + appended 11-field rows → `load_all` (non-flexible reader) errors on the first new row; the entire PayPal store becomes unreadable. *Fix:* on append, read the existing header; if it differs from `STORE_HEADERS`, rewrite in the new schema.
- **M16 — PayPal fixed £0.01 tolerance vs rate quantisation** (`paypal/recover.rs:499,65`): reconstruction error grows with amount; a £2000 purchase with a 3-dp rate can be off £0.22 → true leg rejected in pass 1 → amount-blind fallback (H5). *Fix:* scale tolerance with amount, or compare the reconstruction rounded to 2dp.
- **M17 — PayPal parser fails open + d/m-vs-m/d hazard** (`paypal/store.rs:203-220`, `recover.rs:601`): malformed rows silently `continue` with no count; a US-locale export (`05/03/2026`) parses as 5 Mar not 3 May (day≤12) or vanishes (day>12), silently shifting dates. *Fix:* count/report skipped rows; reject a file where >N% skip.

---

## LOW findings (batch)

- **L1 — `tag set` accepts `|` and whitespace** (`main.rs:1368-1377`, `csv_store.rs:156,181`; 2-agent): `one|off` reloads as `one`+`off` (defeats one-off exclusion); `" transfer"` fails `eq_ignore_ascii_case("transfer")`. *Fix:* trim + reject `|`/empty at every tag entry point.
- **L2 — `--month` silently discards a contradictory `--year`** (`query/mod.rs:265-268`, `main.rs:85-90`): `(Some(m), _)` wins, no clap conflict. *Fix:* `conflicts_with = "year"`.
- **L3 — out-of-range `--year` becomes an unbounded filter** (`query/mod.rs:227-232`): `from_ymd_opt`→`None` leaves the filter as match-everything. *Fix:* make `year()` fallible.
- **L4 — smoothing drip rounding residual** (`smooth/mod.rs:384`): printed `{:.2}` drip vs unrounded internal → ≤~6p/yr signed drift. *Fix:* round the standing order up.
- **L5 — smoothing ignores same-tag refunds** (`smooth/mod.rs:343-345`): a cancelled-and-refunded premium still inflates the drip. *Design call* — netting invites abuse; `--detail` is the current mitigation.
- **L6 — smoothing accepts negative/zero `annual_budget`** (`smooth/mod.rs:332-338`): a `-6000` typo prints a negative drip. *Fix:* reject `<= 0` at load.
- **L7 — subscriptions false-merge on trailing digit token** (`subscriptions/mod.rs:74-79`): `CHANNEL 4` / `CHANNEL 5` → both `CHANNEL`. *Fix:* only strip digit-only tokens of length ≥3-4.
- **L8 — subscriptions false-split on alphanumeric per-txn refs / unicode masks** (`subscriptions/mod.rs:74-79`): `STREAMCO REF7A2B9C` unique each month → never detected. *Fix:* treat high-digit-ratio alphanumeric tails as noise; accept `•`/`·`.
- **L9 — enrich "sent" emails can match bank credits** (`enrich/mod.rs:406-413`): the credit-skip exempts `dir=="sent"`, contradicting its own comment → a £50 refund attributed to an outgoing £50 payment. *Fix:* drop the `&& dir_lower != "sent"` exemption.
- **L10 — dedup hash has no field separators + scale-sensitive amount** (`dedup.rs:8-11`): `-5.0`+`0X` ≡ `-5.00`+`X`; a bank export decimal-rendering change re-keys history. *Fix:* `{date}|{amount:.2}|{desc}` (needs id migration).
- **L11 — primary-category is rule-file-order + tagging-history dependent** (`tags/rules.rs:172-184`, `tags/apply.rs:7-16`): first tag wins the `--by-category` bucket; same data → different totals across runs/rule-reorders. *Fix:* explicit precedence (most-specific-pattern wins primary).
- **L12 — unknown account cell → `Current`** (`csv_store.rs:172`): a mangled account re-labels the row, extending the wrong source's span in coverage. *Fix:* propagate/skip (folds into H1's `TryFrom`).
- **L13 — PayPal store read-back fail-open £0/1970** (`paypal/store.rs:305-314`): same class as H1 on the sidecar. *Fix:* propagate/warn (fold into the H1 fix).
- **L14 — `parse_pp_amount` mangles comma-decimals 100×** (`paypal/store.rs:160-170`): `-299,40` → `-29940`. *Fix:* reject `,` + exactly 2 trailing digits with no dot, or parse locale-aware.
- **L15 — `is_bare_paypal_payment` is a bare `contains("paypal")`** (`paypal/recover.rs:131-137`; 2-agent): pulls in `PAYPAL *NETFLIX` (already has a merchant) and `TO PAYPAL` transfers → pollutes the recoverability metric and can starve genuine bare rows. *Fix:* anchor on the exact bare form + `Account::Current`.

---

## Test-quality note

Several tests give false confidence by exercising only the passing case:
- `dedup.rs` tests same-vs-different amount, never the same-day/same-amount **collision** (H3).
- The two FX tests use **opposite rate directions** and are mutually inconsistent (H4); `fx_chain` passes via the fallback, not the amount link it purports to test.
- Subscription tests avoid M4 only because a repeated amount happens to exist in the fixture.

Add adversarial fixtures for each when fixing.

---

## Checked and found sound (recorded so we don't re-litigate)

- **`--by-category` reconciliation invariant** — each spend row attributed to exactly one bucket (`primary_category`, single pass), limit-truncation tail folded back as `grand_total - shown_total`, rollup maps every bucket to a super (unmapped→`Other`), inclusive date filters with correct month-end arithmetic, transfer+descriptive row excluded once. No float anywhere — `rust_decimal` throughout.
- **PayPal idempotency** — `recover` recomputes over the full set and `write_recoveries` truncate-rewrites (`File::create`); re-running never appends/re-binds. Deterministic tie-breaks.
- **transactions.csv immutability** — the recovery path writes only `paypal.csv` / `paypal_matches.jsonl`; no transaction-store writes.
- **Smoothing** — `Window` month arithmetic (year-boundary `div/rem_euclid`) correct; empty/zero-lump/no-match paths return zeros without panicking; pooled-buffer `cum` walk correctly lets one category's peak fund another's trough (intended single-account design).
