# Subscription monitoring — JSONL schema + module contract

Append-only event log at `~/.local/share/mailcurator/subscriptions.jsonl`.

## Event types

| `event` value | When emitted | Source |
|---|---|---|
| `renewal_reminder` | Vendor announced upcoming auto-renewal (the gold) | Track B extractors |
| `charged` | A billing event happened (receipt confirming charge) | Track B extractors |
| `subscription_started` | First-time subscription onboarding email | Track B extractors |
| `cancellation_confirmed` | Vendor acknowledged cancellation | Track B extractors |
| `candidate` | Heuristic flagged a possible subscription, awaiting human review | Track A discovery |

## Schema

Every line is one event. Required fields apply to all event types; optional fields apply where the event type and source allow.

### Required fields

| Field | Type | Notes |
|---|---|---|
| `ts` | string (RFC 3339 UTC) | When the event was logged |
| `event` | string | One of the event types above |
| `service` | string | Normalised service identifier — e.g. `apple.com`, `drop.app`, `stripe:nicabm`. Lowercase, dot-separated. Use `vendor:subbrand` when one billing vendor handles multiple SaaS subscriptions (Stripe is the prototypical case). |
| `source` | string | Message-ID or thread-id of the email this came from |

### Optional fields

| Field | Type | Notes |
|---|---|---|
| `next_renewal` | string (ISO date `YYYY-MM-DD`) | When the subscription will next renew |
| `amount` | string | Billed amount — preserve original formatting including currency symbol (`£49.99`, `$9.99`) |
| `currency` | string (ISO 4217) | If parseable separately; otherwise just leave it inside `amount` |
| `frequency` | string | `monthly` / `annual` / `quarterly` / `weekly` / `unknown` |
| `cancellation_notice_days` | integer | Days of notice required before renewal to cancel cleanly |
| `subject` | string | Original email subject (context for review) |
| `from` | string | Original sender (context for review) |
| `extracted_at` | string (RFC 3339 UTC) | When extraction ran (parallels `extract.rs` records) |

### Candidate-only fields (event = `candidate`)

| Field | Type | Notes |
|---|---|---|
| `confidence` | string | `low` / `medium` / `high` |
| `reason` | string | What heuristic flagged it (e.g. "subject matched /renew|renewal/") |

## Example records

```jsonl
{"ts":"2026-04-28T15:50:00Z","event":"renewal_reminder","service":"apple.com","source":"<msg-id-1>","next_renewal":"2026-05-15","amount":"£8.99","frequency":"monthly","subject":"Your Subscription Renews Soon","from":"no_reply@email.apple.com","extracted_at":"2026-04-28T15:50:00Z"}
{"ts":"2026-04-28T15:50:00Z","event":"charged","service":"apple.com","source":"<msg-id-2>","amount":"£0.99","frequency":"monthly","subject":"Your receipt from Apple","from":"no_reply@email.apple.com","extracted_at":"2026-04-28T15:50:00Z"}
{"ts":"2026-04-28T15:50:00Z","event":"candidate","service":"unknown:drop.app","source":"<msg-id-3>","confidence":"high","reason":"subject matched /will renew/i AND from matched /drop.app/","subject":"Your Drop subscription will renew on May 15","from":"hello@drop.app"}
```

## Service-name normalisation

`service` is the load-bearing key for grouping events into a single subscription's history. Conventions:

- Lowercase, ASCII only.
- Use the **billing domain** for direct subscriptions: `apple.com`, `drop.app`, `nicabm.com`.
- For Stripe-mediated SaaS (one Stripe sender, many subscriptions): `stripe:<brand>` where `<brand>` is the customer-facing service name — e.g. `stripe:notion`, `stripe:figma`. The Track B extractor for Stripe should parse the customer-facing name from the receipt body to populate this.
- Special case: Apple App Store subscriptions for individual apps can use the form `apple:<app-bundle-or-name>` if the Apple extractor can parse out the underlying app — otherwise group all under `apple.com`.
- Don't put display names (capitalisation, punctuation) in `service`. That information belongs in `subject` or `from`.

## Reading patterns

The state-model code in `subscriptions.rs` exposes a synthesis function that scans `subscriptions.jsonl` and produces one record per `service` representing current state:

- `last_seen` = most recent `ts` for that service
- `status` = `active` if any non-`cancellation_confirmed` event in the last 90 days; `cancelled` if last event was `cancellation_confirmed`; `dormant` otherwise
- `next_renewal` = most recent `next_renewal` value across `renewal_reminder` events
- `amount`, `frequency` = most recent populated values
- `events` = full event list for that service (chronological)

This synthesis is computed on-demand, not cached. The log is the source of truth.

## Module contract for parallel agents

Three independent work packages (each on its own worktree branch):

### Agent A — `subscriptions.rs` state model + reading subcommands

Owns: `src/subscriptions.rs`, plus the CLI dispatch for the three read-side subcommands.

- Define `SubscriptionEvent` struct mirroring the schema above (use `#[serde(rename_all = "snake_case")]`, `#[serde(skip_serializing_if = "Option::is_none")]` for optionals).
- Define `SubscriptionStatus` synthesised state struct.
- Implement `load_events() -> Result<Vec<SubscriptionEvent>>`, `synthesise(&[SubscriptionEvent]) -> Vec<SubscriptionStatus>`.
- Implement subcommand handlers:
  - `list` — print all known subscriptions sorted by next_renewal asc
  - `check [--alert]` — flag subscriptions where today is approaching the cancellation-notice window (default buffer: 7 days extra). With `--alert`, write a `subs::` entry to today's DayPage via `daypage-append`.
  - `report [--period 30d]` — group by service, totals by frequency, services not seen in N days
- Use existing `store::store_dir()` to find the JSONL.
- Public function `append_event(&SubscriptionEvent) -> Result<()>` that writes to `subscriptions.jsonl`. Both Agent B and Agent C will call this.

### Agent B — Track A heuristic discovery

Owns: a new `discover` subcommand under `subscriptions`, possibly a small new module `subscriptions_discover.rs`.

- Implement `subscriptions discover [--commit] [--window 6M]`:
  - Scans inbox over the window for subjects matching subscription-pattern regex: `(?i)(subscription|renew|renewal|will renew|will be charged|recurring|auto.?renew|membership)`.
  - Excludes already-known services (those that appear in subscriptions.jsonl with non-candidate events).
  - Without `--commit`: print candidates, do not write.
  - With `--commit`: write `event = "candidate"` records to subscriptions.jsonl via Agent A's `append_event`.
- Can use `claude-cli` (see `llm.rs`) for confidence classification, but not required for v1 — string heuristic is enough.

### Agent C — Track B Apple subscriptions extractor

Owns: the extractor side. Two pieces:

1. Edit `~/.config/mailcurator/policies.toml`: add an `[[policy.extractor]]` block (or new sibling policy) for Apple subscription-renewal emails. The extractor should write to the `subscriptions` category (i.e. `subscriptions.jsonl`).
2. Verify `extract.rs` correctly handles records destined for `subscriptions.jsonl` — the existing append_record path is generic, but the Apple extractor needs to populate the schema fields (event, service, frequency, etc.) that the existing Royal Mail extractor doesn't have. Consider whether new field rule kinds are needed (e.g. an `event = "renewal_reminder"` literal field is fine — `literal` already exists).

Apple sends two relevant email shapes:

- "Your Subscription Renews Soon" — yields `renewal_reminder` events with next_renewal date and amount
- "Your receipt from Apple" containing subscription line items — yields `charged` events

Test against any `from:no_reply@email.apple.com` messages in the user's notmuch DB. Document discovered patterns in this file as you go.
