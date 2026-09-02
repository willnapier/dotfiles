//! `reconcile` — bring the tree to the state the live watchers would leave
//! it in, in one pass.
//!
//! The plan is [`crate::audit`]'s report over ONE index (the same view of the
//! tree the report describes), narrowed to what the watchers themselves would
//! write: only notes under the first root — the watched one — are touched,
//! because the service scans every root but only ever writes in response to
//! events under `roots[0]`. Notes the resolve-mark watcher skips never have
//! markers applied (see `audit::watcher_skips_markers`). Planning completes
//! before the first write; every replacement is a same-directory temporary
//! file followed by an atomic rename. Dry-run is the default at the CLI
//! boundary and never calls the writer.
//!
//! Temporary files are named `.syncthing.<name>.wiki-reconcile-<pid>-<n>.tmp`:
//! Syncthing ignores that shape in every folder without needing an
//! `.stignore` rule, so the transient file never syncs or conflicts.

use crate::audit::{self, AuditReport};
use crate::resolve;
use crate::wiki::{self, Index};
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct PlannedWrite {
    path: PathBuf,
    content: String,
}

#[derive(Debug)]
pub struct ReconcileReport {
    pub audit: AuditReport,
    /// The root whose notes may be written (the watched one).
    pub watched: PathBuf,
    pub planned: usize,
    pub written: usize,
    /// Audit-reported changes left alone because the note lies outside the
    /// watched root — the watchers would never write them either.
    pub outside_watched: usize,
    pub apply: bool,
}

impl ReconcileReport {
    pub fn render(&self) -> String {
        let mut rendered = self.audit.render();
        rendered.push_str(&format!(
            "\nreconcile: mode={} watched={} planned={} written={} outside-watched-root={}\n",
            if self.apply { "apply" } else { "dry-run" },
            self.watched.display(),
            self.planned,
            self.written,
            self.outside_watched
        ));
        rendered
    }
}

fn canonical_roots(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    roots
        .iter()
        .filter(|root| root.exists())
        .map(|root| fs::canonicalize(root).with_context(|| format!("canonicalising root {}", root.display())))
        .collect()
}

/// (writes under the watched root, count of reported changes outside it)
fn plan(index: &Index, roots: &[PathBuf], report: &AuditReport) -> Result<(Vec<PlannedWrite>, usize)> {
    let section_paths: BTreeSet<&Path> = report.sections.iter().map(|change| change.path.as_path()).collect();
    let marker_paths: BTreeSet<&Path> =
        report.markers_added.iter().chain(&report.markers_removed).map(|change| change.path.as_path()).collect();
    let paths: BTreeSet<&Path> = section_paths.iter().chain(&marker_paths).copied().collect();
    let allowed_roots = canonical_roots(roots)?;
    let watched = allowed_roots.first().context("no existing root to reconcile")?;
    let mut writes = Vec::with_capacity(paths.len());
    let mut outside_watched = 0;

    for path in paths {
        let canonical = fs::canonicalize(path).with_context(|| format!("canonicalising planned path {}", path.display()))?;
        if !allowed_roots.iter().any(|root| canonical.starts_with(root)) {
            bail!("planned path escapes configured roots: {}", path.display());
        }
        if !canonical.starts_with(watched) {
            outside_watched += 1;
            continue;
        }
        let i = index.position(path).with_context(|| format!("audit path is absent from index: {}", path.display()))?;
        let original = index.content(i).with_context(|| format!("reading planned path {}", path.display()))?;
        let mut content = original.to_string();
        if section_paths.contains(path) {
            content = wiki::with_section(&content, &index.backlink_names(i));
        }
        if marker_paths.contains(path) {
            content = resolve::transform(&content, index).content;
        }
        if content == *original {
            bail!("audit reported a change but reconciliation produced none: {}", path.display());
        }
        writes.push(PlannedWrite { path: path.to_path_buf(), content });
    }
    Ok((writes, outside_watched))
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let parent = path.parent().with_context(|| format!("path has no parent: {}", path.display()))?;
    let file_name = path.file_name().with_context(|| format!("path has no filename: {}", path.display()))?.to_string_lossy();
    let permissions = fs::metadata(path).with_context(|| format!("reading metadata for {}", path.display()))?.permissions();
    let mut last_collision = None;

    for attempt in 0..100u8 {
        let temp = parent.join(format!(".syncthing.{file_name}.wiki-reconcile-{}-{attempt}.tmp", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(mut file) => {
                let result = (|| -> Result<()> {
                    file.write_all(content.as_bytes()).with_context(|| format!("writing {}", temp.display()))?;
                    file.sync_all().with_context(|| format!("syncing {}", temp.display()))?;
                    file.set_permissions(permissions).with_context(|| format!("setting permissions on {}", temp.display()))?;
                    drop(file);
                    fs::rename(&temp, path).with_context(|| format!("atomically replacing {}", path.display()))?;
                    fs::File::open(parent)
                        .and_then(|directory| directory.sync_all())
                        .with_context(|| format!("syncing directory {}", parent.display()))?;
                    Ok(())
                })();
                if result.is_err() {
                    let _ = fs::remove_file(&temp);
                }
                return result;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last_collision = Some(temp),
            Err(error) => return Err(error).with_context(|| format!("creating temporary file beside {}", path.display())),
        }
    }
    bail!("could not allocate temporary file beside {}; last collision: {:?}", path.display(), last_collision)
}

/// Plan (and with `apply`, write) the watchers' fixed point for `roots`.
/// The caller is responsible for making sure no watcher is running against
/// the same tree while applying (`main.rs` checks the PID locks).
pub fn reconcile(roots: &[PathBuf], apply: bool) -> Result<ReconcileReport> {
    let watched = roots.first().context("reconcile needs at least one root")?.clone();
    let index = Index::build(roots);
    let audit = audit::audit_index(&index, roots);
    let (writes, outside_watched) = plan(&index, roots, &audit)?;
    let planned = writes.len();
    let mut written = 0;
    if apply {
        for write in writes {
            atomic_write(&write.path, &write.content)?;
            written += 1;
        }
    }
    Ok(ReconcileReport { audit, watched, planned, written, outside_watched, apply })
}
