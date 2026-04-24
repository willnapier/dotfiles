//! One-off Gmail-label cleanup for tags that leaked from notmuch.
//!
//! Context: `lieer`'s `gmi push` had been pushing local notmuch tags
//! to Gmail as labels for years with `ignore_tags: []`. Two classes of
//! tag leaked:
//!
//! - **`new`** — a notmuch `[new]` convention tag, supposed to be
//!   stripped transiently by the post-new hook. The hook never did,
//!   so `tag:new` grew monotonically (165,774 locally) and lieer
//!   pushed the ones it could, resulting in a `new` Gmail label
//!   covering 40,275 messages. The fix: the post-new hook now strips
//!   `new` at the end; lieer's config has `new` in `ignore_tags`;
//!   push-tags's `LOCAL_ONLY_TAGS` includes it; and this module
//!   removes the label from Gmail entirely.
//! - **`curator-*-seen`** — mail-curator's bookkeeping tags ("this
//!   sender's policy has been applied to this message"). Pure local
//!   state with no user-facing Gmail value. Same treatment — strip
//!   from every message, delete the Gmail label.
//!
//! Labels intentionally kept on Gmail because the user uses them in
//! the web UI: `billing`, `Expenses`, `receipts`.
//!
//! Usage: `practiceforge email gmail-cleanup-leaked-labels [--execute]`.
//! Default mode is dry-run (report counts, no writes). Pass
//! `--execute` to do the real modifications.

use anyhow::Result;

use super::GmailApi;

/// Hard-coded list of labels known to be safe to strip. The single
/// literal `new` plus any label matching the closure. The list is
/// intentionally small and manually vetted — this module is a one-
/// off cleanup, not a general-purpose tool.
fn should_strip_label(name: &str) -> bool {
    if name == "new" {
        return true;
    }
    // mail-curator bookkeeping tags — always end in `-seen` and start
    // with `curator-`. The fixed `curator-*` prefix lets us tolerate
    // new sender policies without updating this file.
    if name.starts_with("curator-") && name.ends_with("-seen") {
        return true;
    }
    false
}

/// Run the cleanup. When `execute=false`, only reports counts.
pub fn run(execute: bool) -> Result<()> {
    let api = GmailApi::new()?;

    eprintln!("Fetching full label list…");
    let all_labels = api.list_all_labels()?;
    eprintln!(
        "Got {} labels total; filtering for leaked tags…",
        all_labels.len()
    );

    let targets: Vec<_> = all_labels
        .iter()
        .filter(|l| should_strip_label(&l.name))
        .cloned()
        .collect();

    if targets.is_empty() {
        eprintln!("No leaked labels present on Gmail. Nothing to clean.");
        return Ok(());
    }

    eprintln!("\nTargets ({}):", targets.len());
    for l in &targets {
        eprintln!("  - {} (id={})", l.name, l.id);
    }

    if !execute {
        eprintln!("\nDry-run mode — computing counts without modification.\n");
    } else {
        eprintln!("\nExecuting: stripping each label from its messages, then deleting the label.\n");
    }

    let mut total_stripped = 0usize;
    let mut labels_deleted = 0usize;

    for label in &targets {
        eprintln!("=== {} (id={}) ===", label.name, label.id);

        // Stream pages: for each page, immediately either count it
        // (dry-run) or strip it (execute). This avoids allocating
        // all 60k+ ids up front — and gives natural resumability:
        // on a mid-label network timeout, already-stripped pages
        // stay stripped, and a re-run finds only what remains.
        let mut label_stripped = 0usize;
        let mut page_token: Option<String> = None;
        let mut page_index = 0usize;
        loop {
            let (ids, next) = api.list_message_ids_by_label_page(&label.id, page_token.as_deref())?;
            page_index += 1;
            if ids.is_empty() && page_token.is_none() {
                eprintln!("  (label is empty)");
                break;
            }
            if !ids.is_empty() {
                if execute {
                    let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
                    let label_ref = label.id.as_str();
                    api.batch_modify(&id_refs, &[], &[label_ref])?;
                    label_stripped += ids.len();
                    eprintln!(
                        "  page {page_index}: stripped {} (cumulative for this label: {label_stripped})",
                        ids.len()
                    );
                } else {
                    label_stripped += ids.len();
                    eprintln!(
                        "  page {page_index}: [dry-run] would strip {} (cumulative: {label_stripped})",
                        ids.len()
                    );
                }
            }
            match next {
                Some(t) => page_token = Some(t),
                None => break,
            }
        }

        if execute {
            total_stripped += label_stripped;
            eprintln!(
                "  → stripped {label_stripped} message(s) from {} total.",
                label.name
            );
        } else {
            eprintln!(
                "  → {} message(s) would be stripped from {}.",
                label_stripped, label.name
            );
        }

        if !execute {
            eprintln!("  [dry-run] would DELETE label {} (id={}).", label.name, label.id);
        } else {
            api.delete_label(&label.id)?;
            labels_deleted += 1;
            eprintln!("  deleted label {}.", label.name);
        }

        eprintln!();
    }

    if execute {
        eprintln!(
            "Done. Stripped labels from {total_stripped} message(s); deleted {labels_deleted} label(s)."
        );
    } else {
        eprintln!("Dry-run complete. Re-run with `--execute` to apply changes.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_strip_new() {
        assert!(should_strip_label("new"));
    }

    #[test]
    fn should_strip_curator_seen() {
        assert!(should_strip_label("curator-amazon-shipping-seen"));
        assert!(should_strip_label("curator-nicabm-seen"));
        assert!(should_strip_label("curator-uber-trips-seen"));
    }

    #[test]
    fn should_not_strip_legitimate_labels() {
        assert!(!should_strip_label("billing"));
        assert!(!should_strip_label("Expenses"));
        assert!(!should_strip_label("receipts"));
        assert!(!should_strip_label("INBOX"));
        assert!(!should_strip_label("UNREAD"));
        assert!(!should_strip_label("STARRED"));
    }

    #[test]
    fn should_not_strip_unrelated_prefixes() {
        assert!(!should_strip_label("curator-something-else"));    // no -seen
        assert!(!should_strip_label("foo-curator-seen"));          // no curator- prefix
        assert!(!should_strip_label("seen"));                      // bare seen
    }
}
