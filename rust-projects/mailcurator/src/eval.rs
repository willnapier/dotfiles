// Karpathy-loop support: labelling, scoring, and proposer logic.
//
// Three public entry points:
//   - label(n, window): sample n messages, classify via Claude, persist labels
//   - score(): per-policy precision/recall against the labelled corpus
//   - improve(policy_name, rounds): run the proposer/test/keep-or-revert loop
//
// All eval state lives under ~/.local/share/mailcurator/eval/:
//   - labels.jsonl       label records (one per classified message)
//   - tried_edits.jsonl  proposer history (so reverted edits aren't re-tried)
//
// Label categories (fixed set, MUST match the prompt in `classify_batch`):
//   auth-code, newsletter, delivery, shipping-receipt, invoice,
//   calendar-invite, correspondence, marketing, notification, other

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir_all, read_to_string, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use crate::config;
use crate::llm;
use crate::notmuch;
use crate::policy::Policy;
use crate::store;

const CATEGORIES: &[&str] = &[
    "auth-code",
    "newsletter",
    "delivery",
    "shipping-receipt",
    "invoice",
    "calendar-invite",
    "correspondence",
    "marketing",
    "notification",
    "other",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub message_id: String,
    pub category: String,
    pub subject: String,
    pub from: String,
    pub labelled_at: String,
}

fn eval_dir() -> Result<PathBuf> {
    let base = dirs::data_local_dir().context("no XDG_DATA_HOME equivalent")?;
    let dir = base.join("mailcurator").join("eval");
    create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

fn labels_path() -> Result<PathBuf> {
    Ok(eval_dir()?.join("labels.jsonl"))
}

fn tried_edits_path() -> Result<PathBuf> {
    Ok(eval_dir()?.join("tried_edits.jsonl"))
}

/// Load all labels from disk.
pub fn load_labels() -> Result<Vec<Label>> {
    let path = labels_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let l: Label = serde_json::from_str(line)
            .with_context(|| format!("parsing line {} of {}", i + 1, path.display()))?;
        out.push(l);
    }
    Ok(out)
}

fn append_label(label: &Label) -> Result<()> {
    let path = labels_path()?;
    let line = serde_json::to_string(label)?;
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

// ============================================================================
// LABEL: sample messages + classify via Claude
// ============================================================================

/// Sample messages from the inbox-plus-recent-archive pool and classify.
/// Skips messages that are already labelled. Writes new labels to disk.
pub fn label(sample_size: usize, window: &str) -> Result<()> {
    llm::probe()?;

    // Collect already-labelled message IDs to avoid duplicate work.
    let existing: HashSet<String> = load_labels()?
        .into_iter()
        .map(|l| l.message_id)
        .collect();

    // Sample: messages in inbox OR recently-archived, within window.
    let query = format!(
        "(tag:inbox or (not tag:inbox and not tag:trash and not tag:spam)) and date:{window}.. and not tag:trash"
    );
    let mut candidates = store::list_messages(&query)?;
    candidates.retain(|m| !existing.contains(&m.message_id));

    if candidates.is_empty() {
        println!("no candidates to label (all sampled messages already labelled)");
        return Ok(());
    }

    // Reservoir-sample down to the requested size.
    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    let chosen: Vec<_> = candidates.choose_multiple(&mut rng, sample_size).cloned().collect();

    println!("classifying {} messages via Claude...", chosen.len());

    // Batch in chunks of 25 to keep prompts reasonable.
    const BATCH: usize = 25;
    let ts = chrono::Utc::now().to_rfc3339();
    let mut labelled = 0;
    for chunk in chosen.chunks(BATCH) {
        let categories = classify_batch(chunk)?;
        for (msg, cat) in chunk.iter().zip(categories.iter()) {
            let label = Label {
                message_id: msg.message_id.clone(),
                category: cat.clone(),
                subject: msg.subject.clone(),
                from: msg.from.clone(),
                labelled_at: ts.clone(),
            };
            append_label(&label)?;
            labelled += 1;
        }
        println!("  labelled {labelled}/{} so far", chosen.len());
    }
    println!("done — {labelled} new labels written");
    Ok(())
}

fn classify_batch(messages: &[store::MessageSummary]) -> Result<Vec<String>> {
    let mut prompt = String::new();
    prompt.push_str(
        "You are classifying email messages for a curation tool. For each message below, \
         choose exactly ONE category from this list:\n\n\
         - auth-code         verification codes, password resets, OTP, sign-in codes\n\
         - newsletter        regular informational mailings with no retention value\n\
         - delivery          parcel tracking, delivery notifications (on its way, out for delivery, delivered)\n\
         - shipping-receipt  shipping/order receipts with potential retention value\n\
         - invoice           invoices, payment receipts, financial transactions\n\
         - calendar-invite   meeting invites, appointment confirmations\n\
         - correspondence    personal or professional correspondence from identifiable individuals\n\
         - marketing         promotional emails with commercial intent\n\
         - notification      service notifications (sign-in alerts, account changes, delivered-confirmation)\n\
         - other             anything not fitting the above\n\n\
         Respond with one category per message, one per line, in the same order as given. \
         Output ONLY the category name on each line — no numbering, no prose, no formatting.\n\n\
         Messages:\n",
    );
    for (i, m) in messages.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. From: {} | Subject: {} | Date: {}\n",
            i + 1,
            truncate(&m.from, 80),
            truncate(&m.subject, 200),
            m.date,
        ));
    }

    let response = llm::ask(&prompt)?;
    let valid: HashSet<&str> = CATEGORIES.iter().copied().collect();

    let mut out = Vec::with_capacity(messages.len());
    for line in response.lines() {
        let t = line.trim().trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
        if t.is_empty() {
            continue;
        }
        if valid.contains(t) {
            out.push(t.to_string());
        } else {
            // Claude returned something unexpected — store as "other".
            out.push("other".to_string());
        }
    }

    // If response length doesn't match input, pad or truncate.
    while out.len() < messages.len() {
        out.push("other".to_string());
    }
    out.truncate(messages.len());
    Ok(out)
}

// ============================================================================
// SCORE: per-policy precision/recall against labelled corpus
// ============================================================================

#[derive(Debug, Clone)]
pub struct PolicyScore {
    pub policy_name: String,
    pub intended_categories: Vec<String>,
    pub tp: usize,
    pub fp: usize,
    pub fn_count: usize,
    pub tn: usize,
    pub fp_examples: Vec<Label>,
    pub fn_examples: Vec<Label>,
}

impl PolicyScore {
    pub fn precision(&self) -> f64 {
        let denom = self.tp + self.fp;
        if denom == 0 { 1.0 } else { self.tp as f64 / denom as f64 }
    }
    pub fn recall(&self) -> f64 {
        let denom = self.tp + self.fn_count;
        if denom == 0 { 1.0 } else { self.tp as f64 / denom as f64 }
    }
}

pub fn score_all(policies: &[Policy], labels: &[Label]) -> Result<Vec<PolicyScore>> {
    let mut out = Vec::new();
    for pol in policies {
        if pol.intended_categories.is_empty() {
            continue; // skip policies not yet opted into the eval loop
        }
        out.push(score_one(pol, labels)?);
    }
    Ok(out)
}

pub fn score_one(pol: &Policy, labels: &[Label]) -> Result<PolicyScore> {
    let intended: HashSet<&str> = pol.intended_categories.iter().map(|s| s.as_str()).collect();
    let base_query = pol.base_query();

    // Which labelled messages does this policy match?
    // We check each by running `notmuch search '(base_query) and id:<message_id>'`.
    // Batch via a single big OR query for efficiency.
    let matched = matched_ids_for_query(&base_query, labels)?;

    let mut tp = 0;
    let mut fp = 0;
    let mut fn_count = 0;
    let mut tn = 0;
    let mut fp_examples = Vec::new();
    let mut fn_examples = Vec::new();

    for lab in labels {
        let should_match = intended.contains(lab.category.as_str());
        let did_match = matched.contains(&lab.message_id);
        match (should_match, did_match) {
            (true, true) => tp += 1,
            (false, true) => {
                fp += 1;
                if fp_examples.len() < 10 {
                    fp_examples.push(lab.clone());
                }
            }
            (true, false) => {
                fn_count += 1;
                if fn_examples.len() < 10 {
                    fn_examples.push(lab.clone());
                }
            }
            (false, false) => tn += 1,
        }
    }

    Ok(PolicyScore {
        policy_name: pol.name.clone(),
        intended_categories: pol.intended_categories.clone(),
        tp,
        fp,
        fn_count,
        tn,
        fp_examples,
        fn_examples,
    })
}

/// Given a base query and a set of labels, return the subset of message IDs
/// that match the base query. Done efficiently via a single notmuch call.
fn matched_ids_for_query(base_query: &str, labels: &[Label]) -> Result<HashSet<String>> {
    if labels.is_empty() {
        return Ok(HashSet::new());
    }
    // notmuch `thread:` id syntax works for what list_messages returns.
    // We use the thread id as message_id throughout.
    // The cheapest path: compute the full match set and intersect with labels.
    let matches = store::list_messages(base_query)?;
    let match_set: HashSet<String> = matches.into_iter().map(|m| m.message_id).collect();
    let label_set: HashSet<String> = labels.iter().map(|l| l.message_id.clone()).collect();
    Ok(match_set.intersection(&label_set).cloned().collect())
}

// Drop unused warning; used via CLI wrappers that may or may not exercise every path.
#[allow(dead_code)]
fn avoid_unused_notmuch_warning() {
    let _ = notmuch::count("*");
    let _ = config::load;
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

// ============================================================================
// IMPROVE: the proposer/test/keep-or-revert loop
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriedEdit {
    pub ts: String,
    pub policy: String,
    pub proposal_toml: String,
    pub outcome: String, // "kept" | "reverted"
    pub before: Metrics,
    pub after: Metrics,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub precision: f64,
    pub recall: f64,
    pub tp: usize,
    pub fp: usize,
    pub fn_count: usize,
}

impl From<&PolicyScore> for Metrics {
    fn from(s: &PolicyScore) -> Self {
        Self {
            precision: s.precision(),
            recall: s.recall(),
            tp: s.tp,
            fp: s.fp,
            fn_count: s.fn_count,
        }
    }
}

fn append_tried_edit(edit: &TriedEdit) -> Result<()> {
    let path = tried_edits_path()?;
    let line = serde_json::to_string(edit)?;
    let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

pub fn load_tried_edits(policy_name: &str) -> Result<Vec<TriedEdit>> {
    let path = tried_edits_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = read_to_string(&path)?;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(e) = serde_json::from_str::<TriedEdit>(line) {
            if e.policy == policy_name {
                out.push(e);
            }
        }
    }
    Ok(out)
}

/// Generate one candidate edit for the named policy via Claude, applied to the
/// score's observed FPs and FNs.
pub fn propose_edit(pol: &Policy, score: &PolicyScore, history: &[TriedEdit]) -> Result<String> {
    let mut prompt = String::new();
    prompt.push_str(
        "You are optimising an email-curation policy expressed as TOML. Your job is to \
         propose ONE edit that improves precision and recall against a labelled corpus.\n\n",
    );

    prompt.push_str("Current policy:\n```toml\n");
    prompt.push_str(&policy_to_toml(pol));
    prompt.push_str("```\n\n");

    prompt.push_str(&format!(
        "This policy intends to match these categories: {}\n\
         Current metrics:\n  precision = {:.3}  (TP={} / (TP+FP)={})\n  recall    = {:.3}  (TP={} / (TP+FN)={})\n\n",
        pol.intended_categories.join(", "),
        score.precision(), score.tp, score.tp + score.fp,
        score.recall(), score.tp, score.tp + score.fn_count,
    ));

    if !score.fp_examples.is_empty() {
        prompt.push_str("Observed false positives (policy matched but shouldn't have):\n");
        for fp in score.fp_examples.iter().take(10) {
            prompt.push_str(&format!("  - [{}] From: {} | Subject: {}\n",
                fp.category, truncate(&fp.from, 60), truncate(&fp.subject, 100)));
        }
        prompt.push('\n');
    }

    if !score.fn_examples.is_empty() {
        prompt.push_str("Observed false negatives (policy didn't match but should have):\n");
        for fne in score.fn_examples.iter().take(10) {
            prompt.push_str(&format!("  - [{}] From: {} | Subject: {}\n",
                fne.category, truncate(&fne.from, 60), truncate(&fne.subject, 100)));
        }
        prompt.push('\n');
    }

    if !history.is_empty() {
        prompt.push_str("Previously tried and reverted edits (do NOT re-propose these):\n");
        for h in history.iter().filter(|e| e.outcome == "reverted").take(20) {
            prompt.push_str(&format!("  - {}: {}\n",
                truncate(&h.proposal_toml, 200), h.reason));
        }
        prompt.push('\n');
    }

    prompt.push_str(
        "Available matchers:\n\
         - from: single notmuch-style from-fragment (e.g., '@domain.com' or 'noreply@host.com')\n\
         - subject_contains: single phrase the subject must contain (token match)\n\
         - subject_contains_any: array of phrases; any match qualifies\n\
         - subject_not_contains: phrase excluded from match\n\n\
         Propose a REPLACEMENT for the policy's match fields (from / subject_contains / \
         subject_contains_any / subject_not_contains). Output ONLY the replacement TOML \
         fields (no policy name, no lifecycle fields, no markdown), like:\n\n\
         from = \"@example.com\"\n\
         subject_contains_any = [\"verify\", \"your code\"]\n\n\
         If you cannot suggest a useful edit, output the single line: NO_CHANGE\n"
    );

    let response = llm::ask(&prompt)?;
    Ok(response)
}

/// Basic TOML rendering for a policy (for embedding in the proposer prompt).
fn policy_to_toml(p: &Policy) -> String {
    let mut s = format!("[[policy]]\nname = \"{}\"\n", p.name);
    if let Some(f) = &p.r#match.from {
        s.push_str(&format!("from = \"{}\"\n", f));
    }
    if let Some(sc) = &p.r#match.subject_contains {
        s.push_str(&format!("subject_contains = \"{}\"\n", sc));
    }
    if let Some(sca) = &p.r#match.subject_contains_any {
        let quoted: Vec<String> = sca.iter().map(|t| format!("\"{t}\"")).collect();
        s.push_str(&format!("subject_contains_any = [{}]\n", quoted.join(", ")));
    }
    if let Some(snc) = &p.r#match.subject_not_contains {
        s.push_str(&format!("subject_not_contains = \"{}\"\n", snc));
    }
    if let Some(d) = p.archive_after_days {
        s.push_str(&format!("archive_after_days = {d}\n"));
    }
    if let Some(d) = p.delete_after_days {
        s.push_str(&format!("delete_after_days = {d}\n"));
    }
    if !p.intended_categories.is_empty() {
        let quoted: Vec<String> = p.intended_categories.iter().map(|c| format!("\"{c}\"")).collect();
        s.push_str(&format!("intended_categories = [{}]\n", quoted.join(", ")));
    }
    s
}

/// Run the improve loop for up to `rounds` iterations. Each iteration:
///   1. score the current policy
///   2. ask Claude for a candidate edit
///   3. apply to a clone of the policy, re-score
///   4. keep if precision AND recall non-regressing (one must strictly improve)
///   5. log the attempt to tried_edits.jsonl
pub fn improve(policy_name: &str, rounds: usize) -> Result<()> {
    llm::probe()?;
    let labels = load_labels()?;
    if labels.is_empty() {
        anyhow::bail!("no labels found — run `mailcurator label --sample N` first");
    }

    let config_path = dirs::home_dir().context("no home")?
        .join(".config").join("mailcurator").join("policies.toml");
    let raw_toml_before = read_to_string(&config_path)
        .with_context(|| format!("reading {}", config_path.display()))?;

    for round in 1..=rounds {
        println!("\n=== round {round}/{rounds} ===");
        let cfg = config::load(&config_path)?;
        let pol = cfg.policies.iter().find(|p| p.name == policy_name)
            .with_context(|| format!("no policy named '{policy_name}'"))?;
        if pol.intended_categories.is_empty() {
            anyhow::bail!(
                "policy '{}' has no intended_categories set — add it to the policy before improving",
                pol.name
            );
        }

        let current_score = score_one(pol, &labels)?;
        let before = Metrics::from(&current_score);
        println!("current: precision={:.3} recall={:.3} tp={} fp={} fn={}",
            before.precision, before.recall, before.tp, before.fp, before.fn_count);

        if before.fp == 0 && before.fn_count == 0 {
            println!("already perfect on labelled corpus — nothing to improve.");
            break;
        }

        let history = load_tried_edits(policy_name)?;
        let proposal = propose_edit(pol, &current_score, &history)?;
        println!("proposal:\n{proposal}\n");

        if proposal.trim() == "NO_CHANGE" {
            println!("claude declined to propose an edit. stopping.");
            break;
        }

        // Build a candidate config by replacing the match block of this
        // policy with the proposal's match fields.
        let candidate_toml = apply_proposal_to_toml(&raw_toml_before, policy_name, &proposal)?;
        let tmp_path = eval_dir()?.join(format!("_scratch-{policy_name}.toml"));
        std::fs::write(&tmp_path, &candidate_toml)?;

        // Load the candidate and re-score.
        let cand_cfg = match config::load(&tmp_path) {
            Ok(c) => c,
            Err(e) => {
                println!("candidate failed to parse: {e}. reverting.");
                append_tried_edit(&TriedEdit {
                    ts: chrono::Utc::now().to_rfc3339(),
                    policy: policy_name.into(),
                    proposal_toml: proposal.clone(),
                    outcome: "reverted".into(),
                    before: before.clone(),
                    after: before.clone(),
                    reason: format!("parse error: {e}"),
                })?;
                continue;
            }
        };
        let cand_pol = cand_cfg.policies.iter().find(|p| p.name == policy_name).unwrap();
        let new_score = score_one(cand_pol, &labels)?;
        let after = Metrics::from(&new_score);
        println!("candidate: precision={:.3} recall={:.3} tp={} fp={} fn={}",
            after.precision, after.recall, after.tp, after.fp, after.fn_count);

        // Accept if BOTH metrics non-regressing and at least one strictly improved.
        let improved = (after.precision >= before.precision && after.recall >= before.recall)
            && (after.precision > before.precision || after.recall > before.recall);

        let (outcome, reason) = if improved {
            // Commit to the live config file.
            std::fs::copy(&tmp_path, &config_path)?;
            ("kept".to_string(), "improved on labelled corpus".to_string())
        } else {
            (
                "reverted".to_string(),
                format!(
                    "no strict improvement (precision {:.3}→{:.3}, recall {:.3}→{:.3})",
                    before.precision, after.precision, before.recall, after.recall
                ),
            )
        };
        println!("{outcome}: {reason}");

        append_tried_edit(&TriedEdit {
            ts: chrono::Utc::now().to_rfc3339(),
            policy: policy_name.into(),
            proposal_toml: proposal.clone(),
            outcome,
            before,
            after,
            reason,
        })?;

        // Clean up scratch.
        let _ = std::fs::remove_file(&tmp_path);
    }

    Ok(())
}

/// Replace the match fields of a named policy in a raw TOML string with the
/// proposed replacement fields. Naive: finds the `[[policy]]` block whose
/// `name = "<policy_name>"` line exists, and rewrites its match-related lines.
///
/// Strategy: read line-by-line; when inside the target block, drop any
/// from/subject_*/contains_* lines; inject the new ones right after the name
/// line; stop dropping once we hit the next [[policy]] or a section header.
fn apply_proposal_to_toml(raw: &str, policy_name: &str, proposal: &str) -> Result<String> {
    let match_fields = ["from", "subject_contains", "subject_contains_any", "subject_not_contains"];
    let mut out = String::new();
    let mut in_target = false;
    let mut injected = false;

    for line in raw.lines() {
        let trimmed = line.trim_start();

        // Detect block boundaries.
        if trimmed.starts_with("[[policy]]") || trimmed.starts_with('[') {
            in_target = false; // leaving whatever block we were in
            injected = false;
        }

        // Identify target by the `name = "..."` line.
        if trimmed.starts_with("name") && trimmed.contains(&format!("\"{}\"", policy_name)) {
            in_target = true;
            out.push_str(line);
            out.push('\n');
            // Inject the proposal's match fields right after the name line.
            for pline in proposal.lines() {
                out.push_str(pline);
                out.push('\n');
            }
            injected = true;
            continue;
        }

        // Inside the target block, drop old match-field lines.
        if in_target && injected {
            let is_match_field = match_fields.iter().any(|f| {
                trimmed.starts_with(&format!("{f} ")) || trimmed.starts_with(&format!("{f}="))
            });
            if is_match_field {
                continue;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    Ok(out)
}
