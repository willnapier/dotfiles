// Content extraction: per-policy extractors that pull structured fields
// from messages and append them to JSONL files in ~/.local/share/mailcurator/.
//
// Design principle (from the project memory): extract and destroy. Once a
// message's information is captured to plain-text JSONL, the email envelope
// is disposable. Lifecycle's `delete_after_days` is gated on the
// `curator-<policy>-extracted` tag, so messages we haven't yet captured
// from won't be destroyed.
//
// Field rule kinds:
//   - literal     hard-coded value (e.g. carrier name)
//   - header      pull from a named RFC822 header (case-insensitive)
//   - body_regex  regex on decoded, HTML-stripped body; first capture group
//                 is the value (or whole match if no groups)
//   - subject_regex  same idea against the Subject header
//
// Bodies are decoded via mailparse (handles MIME multipart + transfer
// encodings). HTML parts are crudely stripped (regex `<[^>]+>` → space)
// before regexes apply — sufficient for the structured-data patterns in
// transactional emails (tracking numbers, dates, amounts).

use anyhow::{Context, Result};
use mailparse::{parse_mail, MailHeaderMap};
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::process::Command;

use crate::extractors;
use crate::llm;
use crate::llm_cache::Cache;
use crate::policy::{FieldRule, Policy};
use crate::store;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

/// Process-wide LLM fallback budget. Bumped per LLM call; once it hits the
/// cap (set via `set_llm_budget`), further fallbacks short-circuit.
/// Default cap of 100 catches both runaway loops (a deterministic
/// regression that suddenly leaves every message missing fields would
/// burn through the cap and stop, surfacing the issue) and steady-state
/// quotas (legitimate fallback for ~10–20 messages/run).
static LLM_BUDGET: OnceLock<AtomicUsize> = OnceLock::new();
static LLM_USED: AtomicUsize = AtomicUsize::new(0);
static LLM_DISABLED: AtomicUsize = AtomicUsize::new(0); // 1 = disabled

pub fn set_llm_budget(cap: usize) {
    let _ = LLM_BUDGET.set(AtomicUsize::new(cap));
}

pub fn disable_llm_fallback() {
    LLM_DISABLED.store(1, Ordering::Relaxed);
}

fn llm_budget_remaining() -> usize {
    let cap = LLM_BUDGET
        .get_or_init(|| AtomicUsize::new(100))
        .load(Ordering::Relaxed);
    let used = LLM_USED.load(Ordering::Relaxed);
    cap.saturating_sub(used)
}

fn llm_enabled() -> bool {
    LLM_DISABLED.load(Ordering::Relaxed) == 0
}

pub fn llm_calls_made() -> usize {
    LLM_USED.load(Ordering::Relaxed)
}

/// Reset the per-run counters — call from the run subcommand before each
/// invocation. (Currently unused: the process exits after one run, so
/// the AtomicUsize starts at 0 anyway.)
#[allow(dead_code)]
pub fn reset_llm_counters() {
    LLM_USED.store(0, Ordering::Relaxed);
    LLM_DISABLED.store(0, Ordering::Relaxed);
}

/// Run a policy's extractors over every matching message not yet tagged
/// `curator-<name>-extracted`. For each:
///   1. read the message file from disk
///   2. parse + decode body (mailparse handles MIME + transfer encoding)
///   3. apply each FieldRule
///   4. append the resulting record(s) to <category>.jsonl
///   5. tag the message with the extracted-tag (idempotency)
///
/// Returns the number of messages actually extracted.
pub fn run_extractors(pol: &Policy, dry_run: bool) -> Result<u64> {
    if pol.extractors.is_empty() {
        return Ok(0);
    }

    let base = pol.base_query();
    let extracted_tag = pol.extracted_tag();
    let query = format!("({base}) and not tag:{extracted_tag}");

    // Get message file paths
    let files = list_files(&query)?;
    if files.is_empty() {
        return Ok(0);
    }

    if dry_run {
        return Ok(files.len() as u64);
    }

    // Build the LLM cache once per policy run, then thread it through
    // each extraction. A miss + actual LLM call writes through to disk
    // immediately so a crash mid-run preserves earlier work.
    let mut cache = Cache::load()?;

    // Deduplicate by Message-ID. notmuch's --output=files returns one
    // line per maildir file, but Gmail's label model produces multiple
    // files per logical message (e.g. INBOX + Sent both contain the
    // same outbound message). Without dedup, every duplicate file
    // generated a duplicate jsonl row + a duplicate LLM call. The
    // extracted-tag is applied per message-id (notmuch's `id:"..."`
    // query matches all files for that message), so processing one
    // file is sufficient to mark the message done — the second file
    // would just be wasted work.
    use std::collections::HashSet;
    let mut seen_message_ids: HashSet<String> = HashSet::new();

    let mut count = 0u64;
    for path in &files {
        match extract_one(pol, path, &mut cache, &mut seen_message_ids) {
            Ok(true) => count += 1,
            Ok(false) => {} // duplicate message-id, skipped
            Err(e) => {
                eprintln!(
                    "  [{}] extract failed for {}: {}",
                    pol.name,
                    path.split('/').next_back().unwrap_or(path),
                    e
                );
                // Continue: one bad message shouldn't stop the batch.
            }
        }
    }

    // Tag all successfully-extracted messages with the extracted-tag in one
    // notmuch call. We re-query for the same set; any that had errors won't
    // appear because we don't apply this tag to them. Simpler approach:
    // only tag the IDs we actually extracted (use msg-id from the files).
    // For now, rely on the per-message tag application inside extract_one.
    Ok(count)
}

fn is_populated(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        _ => true,
    }
}

fn pretty_vendor_label(module_name: &str) -> &'static str {
    match module_name {
        "amazon_orders" => "Amazon order",
        "trainline_journeys" => "Trainline journey",
        "airbnb_bookings" => "Airbnb booking",
        "tesla" => "Tesla",
        _ => "vendor",
    }
}

fn list_files(query: &str) -> Result<Vec<String>> {
    let output = Command::new("notmuch")
        .args(["search", "--output=files", query])
        .output()
        .with_context(|| format!("spawning notmuch search for files matching: {query}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "notmuch search --output=files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Process one maildir file. Returns Ok(true) if extraction happened,
/// Ok(false) if the file was a duplicate of a message already processed
/// in this run.
fn extract_one(
    pol: &Policy,
    path: &str,
    cache: &mut Cache,
    seen_message_ids: &mut std::collections::HashSet<String>,
) -> Result<bool> {
    let raw = fs::read(path).with_context(|| format!("reading {path}"))?;
    let parsed = parse_mail(&raw).with_context(|| format!("parsing RFC822 from {path}"))?;

    // Cache derived data so multi-rule extractors don't re-do work.
    let subject = parsed.headers.get_first_value("Subject").unwrap_or_default();
    let body_text = decode_body_to_text(&parsed)?;
    let html_body = decode_body_to_html(&parsed);
    let message_id = parsed.headers.get_first_value("Message-ID").unwrap_or_else(|| path.to_string());

    if !seen_message_ids.insert(message_id.clone()) {
        // Already processed in this run — Gmail label dupe.
        return Ok(false);
    }

    // Resolve vendor module once per message (Box<dyn> can't be cloned, so
    // we fetch a fresh instance per extractor in the inner loop below).
    let vendor_name = pol.vendor_module.as_deref();

    for ex in &pol.extractors {
        let mut record = Map::new();
        let now = chrono::Utc::now().to_rfc3339();

        // Schema-aware metadata: subscriptions.jsonl has a strict schema
        // (ts/event/service/source) defined in SUBSCRIPTIONS.md. Map our
        // generic extractor metadata to those keys and drop the redundant
        // `policy` and `message_id` slots. For all other categories,
        // retain the historical {message_id, policy, extracted_at} shape.
        if ex.category == "subscriptions" {
            // For subscription events, `ts` is the time the underlying event
            // happened — i.e. the email's Date header — not the extraction
            // run time. This matters for backfills: extracting 298 old Apple
            // receipts in one batch must NOT collapse to a single "most
            // recent" timestamp, or synthesis can't tell what's genuinely
            // latest. `extracted_at` records the run time separately.
            let ts = parsed
                .headers
                .get_first_value("Date")
                .and_then(|d| {
                    chrono::DateTime::parse_from_rfc2822(&d)
                        .ok()
                        .map(|dt| dt.with_timezone(&chrono::Utc).to_rfc3339())
                })
                .unwrap_or_else(|| now.clone());
            record.insert("ts".into(), Value::String(ts));
            record.insert("source".into(), Value::String(message_id.clone()));
            record.insert("extracted_at".into(), Value::String(now));
        } else {
            record.insert("message_id".into(), Value::String(message_id.clone()));
            record.insert("policy".into(), Value::String(pol.name.clone()));
            record.insert("extracted_at".into(), Value::String(now));
        }

        // Vendor module fields land first so that any FieldRule with the
        // same name overrides them — gives policies an escape hatch when
        // a vendor module gets a field wrong (set the FieldRule to the
        // correct literal/regex; module value is then displaced).
        let mut provenance: Map<String, Value> = Map::new();
        if let Some(name) = vendor_name {
            if let Some(extractor) = extractors::dispatch(name) {
                let mut det_fields: Map<String, Value> = match extractor.extract(&parsed, &html_body) {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!(
                            "  [{}] vendor module '{}' failed (continuing with FieldRules): {}",
                            pol.name, name, e
                        );
                        Map::new()
                    }
                };

                // Tier-2: LLM fallback for required fields the
                // deterministic pass missed. Cache per (msg_id, module);
                // budget-capped to prevent runaway loops; provenance
                // tracked so downstream queries can spot LLM-derived
                // fields.
                let required = extractor.required_fields();
                let missing: Vec<&str> = required
                    .iter()
                    .copied()
                    .filter(|f| !is_populated(det_fields.get(*f)))
                    .collect();

                if !missing.is_empty() && llm_enabled() {
                    if let Some(schema) = extractor.llm_schema() {
                        // Cache key includes the module name so a future
                        // module rename (or addition of a different
                        // module to the same vendor) doesn't read stale
                        // entries.
                        let llm_fields = if let Some(cached) = cache.get(&message_id, name) {
                            cached.clone()
                        } else if llm_budget_remaining() > 0 {
                            match llm::extract_structured(
                                &format!("{} email", pretty_vendor_label(name)),
                                schema,
                                &html_body,
                            ) {
                                Ok(f) => {
                                    LLM_USED.fetch_add(1, Ordering::Relaxed);
                                    let _ = cache.put(
                                        message_id.clone(),
                                        name.to_string(),
                                        f.clone(),
                                    );
                                    f
                                }
                                Err(e) => {
                                    eprintln!(
                                        "  [{}] LLM fallback failed: {}",
                                        pol.name, e
                                    );
                                    Map::new()
                                }
                            }
                        } else {
                            // Budget exhausted; log on first hit per
                            // invocation. Subsequent skips silent.
                            Map::new()
                        };

                        // Merge — only fields missing from det_fields,
                        // and only if the value passes per-field
                        // validation (catches hallucinations).
                        for (k, v) in &llm_fields {
                            if is_populated(det_fields.get(k)) {
                                continue;
                            }
                            if !extractor.validate_field(k, v) {
                                continue;
                            }
                            det_fields.insert(k.clone(), v.clone());
                            provenance.insert(k.clone(), Value::String("llm".into()));
                        }
                    }
                }

                // Mark deterministic fields' provenance for any field we
                // didn't already mark as llm.
                for k in det_fields.keys() {
                    provenance
                        .entry(k.clone())
                        .or_insert(Value::String("deterministic".into()));
                }

                for (k, v) in det_fields {
                    record.insert(k, v);
                }
            } else {
                eprintln!(
                    "  [{}] unknown vendor_module '{}' — skipping (known: {})",
                    pol.name,
                    name,
                    extractors::known_extractors().join(", ")
                );
            }
        }
        if !provenance.is_empty() {
            record.insert("_provenance".into(), Value::Object(provenance));
        }

        for f in &ex.fields {
            if let Some(v) = apply_rule(f, &parsed, &subject, &body_text)? {
                record.insert(f.name.clone(), Value::String(v));
            }
        }

        store::append_record(&ex.category, &Value::Object(record))?;
    }

    // Tag this single message as extracted (idempotent).
    crate::notmuch::apply_tag_changes(
        &format!("id:\"{}\"", message_id.trim_start_matches('<').trim_end_matches('>')),
        &[&pol.extracted_tag()],
        &[],
    )?;

    Ok(true)
}

fn apply_rule(
    f: &FieldRule,
    parsed: &mailparse::ParsedMail,
    subject: &str,
    body_text: &str,
) -> Result<Option<String>> {
    if let Some(lit) = &f.literal {
        return Ok(Some(lit.clone()));
    }
    if let Some(h) = &f.header {
        return Ok(parsed.headers.get_first_value(h));
    }
    if let Some(re) = &f.subject_regex {
        return Ok(apply_regex(re, subject)?);
    }
    if let Some(re) = &f.body_regex {
        return Ok(apply_regex(re, body_text)?);
    }
    Ok(None)
}

fn apply_regex(pattern: &str, text: &str) -> Result<Option<String>> {
    let re = Regex::new(pattern).with_context(|| format!("compiling regex {pattern}"))?;
    Ok(re.captures(text).map(|caps| {
        // Use first capture group if present, else whole match.
        caps.get(1).or_else(|| caps.get(0)).map(|m| m.as_str().to_string())
    }).flatten())
}

/// Decode message body to raw HTML if a text/html part exists; empty
/// string otherwise. Used by vendor extractor modules that need the DOM
/// structure (CSS selectors) rather than HTML-stripped plain text.
/// Quoted-printable / base64 transfer encodings are decoded by mailparse.
fn decode_body_to_html(parsed: &mailparse::ParsedMail) -> String {
    let mut bodies: HashMap<String, String> = HashMap::new();
    collect_bodies(parsed, &mut bodies);
    bodies.remove("text/html").unwrap_or_default()
}

/// Decode message body to plain text. For multipart messages, prefer
/// text/plain; fall back to HTML-stripped text/html. Walks all parts.
fn decode_body_to_text(parsed: &mailparse::ParsedMail) -> Result<String> {
    // Build a map of content-type → decoded body. Pick text/plain first,
    // then text/html (with HTML stripped).
    let mut bodies: HashMap<String, String> = HashMap::new();
    collect_bodies(parsed, &mut bodies);

    if let Some(plain) = bodies.get("text/plain") {
        return Ok(plain.clone());
    }
    if let Some(html) = bodies.get("text/html") {
        return Ok(strip_html(html));
    }
    // Last resort: any text we found.
    Ok(bodies.values().next().cloned().unwrap_or_default())
}

fn collect_bodies(part: &mailparse::ParsedMail, out: &mut HashMap<String, String>) {
    let ctype = part.ctype.mimetype.to_lowercase();
    if part.subparts.is_empty() {
        if ctype.starts_with("text/") {
            if let Ok(body) = part.get_body() {
                out.entry(ctype).or_insert(body);
            }
        }
        return;
    }
    for sub in &part.subparts {
        collect_bodies(sub, out);
    }
}

/// Crude HTML-to-text: replace tags with spaces, decode common entities,
/// collapse whitespace. Sufficient for regex extraction of structured tokens
/// like tracking numbers and dates from transactional email bodies.
fn strip_html(html: &str) -> String {
    let tagless = Regex::new(r"<[^>]+>").unwrap().replace_all(html, " ");
    let entities: &[(&str, &str)] = &[
        ("&nbsp;", " "),
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
    ];
    let mut s = tagless.into_owned();
    for (e, r) in entities {
        s = s.replace(e, r);
    }
    // Collapse whitespace runs to single spaces.
    let collapsed = Regex::new(r"\s+").unwrap().replace_all(&s, " ");
    collapsed.trim().to_string()
}
