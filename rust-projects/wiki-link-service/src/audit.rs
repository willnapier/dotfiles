//! `audit` — read-only report of what the corrected rules would change.
//! Uses only `Index::content` / `Index::backlink_names` / `resolve::transform`;
//! there is no write path in this module.

use crate::resolve;
use crate::wiki::{self, Index};
use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionChange {
    pub path: PathBuf,
    pub added: usize,
    pub removed: usize,
    /// "added", "removed" or "rewritten" (section present before and after).
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkerChange {
    pub path: PathBuf,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuditReport {
    pub roots: Vec<PathBuf>,
    pub notes: usize,
    pub sections: Vec<SectionChange>,
    pub entries_added: usize,
    pub entries_removed: usize,
    pub markers_added: Vec<MarkerChange>,
    pub markers_added_total: usize,
    pub markers_removed: Vec<MarkerChange>,
    pub markers_removed_total: usize,
}

/// Examples per category; `WLS_AUDIT_EXAMPLES=all` (or a number) overrides for a full listing.
pub fn examples() -> usize {
    match std::env::var("WLS_AUDIT_EXAMPLES").ok().as_deref() {
        Some("all") => usize::MAX,
        Some(n) => n.parse().unwrap_or(20),
        None => 20,
    }
}

pub fn audit(roots: &[PathBuf]) -> AuditReport {
    let index = Index::build(roots);
    let mut r = AuditReport { roots: roots.to_vec(), notes: index.files().len(), ..Default::default() };
    for i in 0..index.files().len() {
        let Some(content) = index.content(i) else { continue };
        let path = index.files()[i].clone();

        let desired = index.backlink_names(i);
        if content.len() >= 10 && wiki::with_section(&content, &desired) != *content {
            let want: BTreeSet<String> = desired.iter().map(|n| wiki::name_key(n)).collect();
            let have: BTreeSet<String> = wiki::section_entries(&content).iter().map(|n| wiki::name_key(n)).collect();
            let added = want.difference(&have).count();
            let removed = have.difference(&want).count();
            let had = wiki::find_section(&content).is_some();
            let kind = match (had, desired.is_empty()) {
                (false, _) => "added",
                (true, true) => "removed",
                (true, false) => "rewritten",
            };
            r.entries_added += added;
            r.entries_removed += removed;
            r.sections.push(SectionChange { path: path.clone(), added, removed, kind });
        }

        let t = resolve::transform(&content, &index);
        if t.added > 0 {
            r.markers_added_total += t.added;
            r.markers_added.push(MarkerChange { path: path.clone(), count: t.added });
        }
        if t.removed > 0 {
            r.markers_removed_total += t.removed;
            r.markers_removed.push(MarkerChange { path, count: t.removed });
        }
    }
    r
}

impl AuditReport {
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str("🔎 wiki-link-service audit (read-only)\n");
        for root in &self.roots {
            s.push_str(&format!("   root: {}{}\n", root.display(), if root.exists() { "" } else { " (missing)" }));
        }
        s.push_str(&format!("   notes scanned: {}\n\n", self.notes));

        let [added, removed, rewritten] = ["added", "removed", "rewritten"].map(|k| self.sections.iter().filter(|c| c.kind == k).count());
        s.push_str(&format!(
            "## Backlinks sections that would change: {} (entries +{} / -{}; sections added {added}, removed {removed}, rewritten {rewritten})\n",
            self.sections.len(),
            self.entries_added,
            self.entries_removed
        ));
        for c in self.sections.iter().take(examples()) {
            s.push_str(&format!("   {} (+{} -{}, {})\n", c.path.display(), c.added, c.removed, c.kind));
        }
        if self.sections.len() > examples() {
            s.push_str(&format!("   … and {} more\n", self.sections.len() - examples()));
        }

        for (title, total, list) in [("?[[ markers that would be added", self.markers_added_total, &self.markers_added), ("?[[ markers that would be removed", self.markers_removed_total, &self.markers_removed)] {
            s.push_str(&format!("\n{title}: {total} (in {} notes)\n", list.len()));
            for m in list.iter().take(examples()) {
                s.push_str(&format!("   {} ({})\n", m.path.display(), m.count));
            }
            if list.len() > examples() {
                s.push_str(&format!("   … and {} more\n", list.len() - examples()));
            }
        }
        s
    }
}
