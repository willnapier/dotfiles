//! Byte-for-byte parity with the Nushell oracles.
//!
//! ## Oracle harness
//! Each scenario builds the SAME fixture tree twice under a tempdir
//! (`<tmp>/oracle/{Forge,Admin}` and `<tmp>/rust/{Forge,Admin}`), applies the
//! same user edit to both, then feeds the same event to
//! * the oracle: `nu -n -c "source '<patched script>'; handle_change '<op>' '<path>' '<new>' ['<roots>']"`
//!   with `HOME=<tmp>/oracle` — the script is a copy of `scripts/wiki-*` with
//!   its `MARKER_FILE` const pointed inside the tempdir (so the real
//!   `/tmp/wiki-watcher-writing` is never touched) and
//!   `RIPGREP_CONFIG_PATH` set to a file containing `--sort=path`, because the
//!   oracle's `rg -l` order is otherwise nondeterministic (parallel walker);
//! * the port: `Which::handle(&ctx, op, path, new_path)` — the same function
//!   the watcher loop calls.
//!
//! Then every file under both trees is compared byte-for-byte. Both homes'
//! marker files are removed before each step (simulating > 5 s between
//! events) unless the scenario is about the marker itself. Nothing here ever
//! touches a real directory: HOME is overridden for the oracle and the port
//! is given explicit roots.
//!
//! Requires `nu`, `rg`, `fd`, `sd` on PATH; skips (with a message) otherwise.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use wiki_link_service::logger::Logger;
use wiki_link_service::watch::Which;
use wiki_link_service::wiki::{Ctx, Outcome};

type Snap = BTreeMap<String, Vec<u8>>;

const CODE_MD: &str = "# Code\n\nEscapes: `\\\\` and [[Alpha]] and ?[[Target]] and [[Target]].\n";

// ── harness ─────────────────────────────────────────────────────────
fn tools_available() -> bool {
    ["nu", "rg", "fd", "sd"].iter().all(|t| Command::new("which").arg(t).output().map(|o| o.status.success()).unwrap_or(false))
}

fn write(home: &Path, rel: &str, content: &str) {
    let p = home.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn append(home: &Path, rel: &str, extra: &str) {
    let p = home.join(rel);
    let mut s = fs::read_to_string(&p).unwrap();
    s.push_str(extra);
    fs::write(p, s).unwrap();
}

fn read(snap: &Snap, rel: &str) -> String {
    String::from_utf8(snap.get(rel).unwrap_or_else(|| panic!("{rel} missing from snapshot")).clone()).unwrap()
}

/// The standard fixture: notes with links, marks, existing sections, a
/// subdirectory, a second root, a section followed by another section, and
/// a note containing a literal `\\` (which the resolve-mark oracle's broken
/// rg pattern happens to match).
fn fixture(home: &Path) {
    write(home, "Forge/Alpha.md", "# Alpha\n\nLinks to [[Beta]] and [[Gamma|alias]] and ?[[Missing]].\n\n## Backlinks\n\n- [[Beta]]\n- [[Delta]]\n");
    write(home, "Forge/Beta.md", "# Beta\n\nSee [[Alpha]] and [[Delta]] and [[Gamma]].\n\n## Backlinks\n\n- [[Alpha]]\n");
    write(home, "Forge/sub/Delta.md", "# Delta\n\nRefers to [[Alpha]] and ?[[Beta]].\n");
    write(home, "Forge/Gamma.md", "# Gamma\n\nPlain body without links.\n");
    write(home, "Forge/Plain.md", "# Plain\n\nNo wiki links here at all.\n");
    write(home, "Forge/Later.md", "# Later\n\nBody [[Alpha]].\n\n## Backlinks\n\n- [[Nobody]]\n\n## Afterwards\n\nThis section follows the backlinks.\n");
    write(home, "Forge/Code.md", CODE_MD);
    write(home, "Admin/Notes.md", "Admin note linking [[Alpha]] and [[Plain]].\n");
}

/// Roots handed to both sides: Forge, Admin, and an Archives that does not exist.
fn roots(home: &Path) -> Vec<PathBuf> {
    vec![home.join("Forge"), home.join("Admin"), home.join("Archives")]
}

fn oracle_script(home: &Path, w: Which) -> PathBuf {
    let name = match w {
        Which::Backlinks => "wiki-backlinks",
        Which::ResolveMark => "wiki-resolve-mark",
    };
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts").join(name);
    let s = fs::read_to_string(&src).unwrap_or_else(|e| panic!("reading oracle {}: {e}", src.display()));
    let needle = "const MARKER_FILE = \"/tmp/wiki-watcher-writing\"";
    assert_eq!(s.matches(needle).count(), 1, "MARKER_FILE const not found exactly once in {}", src.display());
    let patched = s.replace(needle, &format!("const MARKER_FILE = \"{}\"", home.join("marker").display()));
    let dst = home.join(format!("oracle-{}.nu", w.sub()));
    fs::write(&dst, patched).unwrap();
    dst
}

fn run_oracle(home: &Path, w: Which, op: &str, path: &Path, new_path: Option<&Path>) -> String {
    let script = oracle_script(home, w);
    let rgconf = home.join("rgconfig");
    fs::write(&rgconf, "--sort=path\n").unwrap();
    let roots: Vec<String> = roots(home).iter().map(|p| format!("'{}'", p.display())).collect();
    let cmd = format!(
        "source '{}'; handle_change '{}' '{}' '{}' [{}]",
        script.display(),
        op,
        path.display(),
        new_path.map(|p| p.display().to_string()).unwrap_or_default(),
        roots.join(" ")
    );
    let out = Command::new("nu").args(["-n", "-c", &cmd]).env("HOME", home).env("RIPGREP_CONFIG_PATH", &rgconf).output().expect("running nu");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success(), "oracle failed:\n{cmd}\n{text}");
    text
}

fn run_rust(home: &Path, w: Which, op: &str, path: &Path, new_path: Option<&Path>) -> Outcome {
    let ctx = Ctx { roots: roots(home), marker: home.join("marker"), logger: Logger::silent() };
    w.handle(&ctx, op, path, new_path)
}

fn collect(dir: &Path, base: &Path, into: &mut Snap) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, base, into);
        } else {
            into.insert(p.strip_prefix(base).unwrap().display().to_string(), fs::read(&p).unwrap());
        }
    }
}

fn snapshot(home: &Path) -> Snap {
    let mut s = Snap::new();
    for d in ["Forge", "Admin", "Archives"] {
        collect(&home.join(d), home, &mut s);
    }
    s
}

fn diff(a: &Snap, b: &Snap) -> String {
    let mut out = String::new();
    for k in a.keys().chain(b.keys()).collect::<std::collections::BTreeSet<_>>() {
        match (a.get(k), b.get(k)) {
            (Some(x), Some(y)) if x == y => {}
            (x, y) => out.push_str(&format!("--- {k}\n<<< {:?}\n>>> {:?}\n", x.map(|v| String::from_utf8_lossy(v)), y.map(|v| String::from_utf8_lossy(v)))),
        }
    }
    out
}

fn changed(a: &Snap, b: &Snap) -> Vec<String> {
    a.keys().chain(b.keys()).filter(|k| a.get(*k) != b.get(*k)).cloned().collect::<std::collections::BTreeSet<_>>().into_iter().collect()
}

struct Run {
    before: Snap,
    /// Snapshot (of the port's tree, which equals the oracle's) after each step.
    after: Vec<Snap>,
    outcomes: Vec<Outcome>,
    oracle_out: Vec<String>,
}

impl Run {
    fn last(&self) -> &Snap {
        self.after.last().unwrap()
    }
}

type Step<'a> = (&'a str, &'a str, Option<&'a str>);

/// Run `steps` against oracle and port on identical fixtures; assert
/// byte-for-byte equality of the whole tree after every step.
fn parity(w: Which, setup: &dyn Fn(&Path), steps: &[Step], clear_marker: bool) -> Option<Run> {
    if !tools_available() {
        eprintln!("SKIPPED: nu/rg/fd/sd not all on PATH");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    let o = tmp.path().join("oracle");
    let r = tmp.path().join("rust");
    for h in [&o, &r] {
        fixture(h);
        setup(h);
    }
    assert_eq!(snapshot(&o), snapshot(&r), "fixtures diverged before any event");
    let before = snapshot(&r);
    let (mut after, mut outcomes, mut oracle_out) = (Vec::new(), Vec::new(), Vec::new());
    for (op, rel, new_rel) in steps {
        if clear_marker {
            let _ = fs::remove_file(o.join("marker"));
            let _ = fs::remove_file(r.join("marker"));
        }
        let new_o = new_rel.map(|n| o.join(n));
        let new_r = new_rel.map(|n| r.join(n));
        oracle_out.push(run_oracle(&o, w, op, &o.join(rel), new_o.as_deref()));
        outcomes.push(run_rust(&r, w, op, &r.join(rel), new_r.as_deref()));
        let (so, sr) = (snapshot(&o), snapshot(&r));
        assert!(so == sr, "{} {op} {rel}: port differs from oracle\n{}\noracle said:\n{}", w.sub(), diff(&so, &sr), oracle_out.last().unwrap());
        after.push(sr);
    }
    Some(Run { before, after, outcomes, oracle_out })
}

// ── backlinks scenarios ─────────────────────────────────────────────
#[test]
fn backlinks_add_link_appends_backlink() {
    // Gamma gains [[Plain]]; a hidden file also links Plain but rg/fd skip hidden files.
    let Some(run) = parity(
        Which::Backlinks,
        &|h| {
            append(h, "Forge/Gamma.md", "\nNow also [[Plain]].\n");
            write(h, "Forge/.hidden/H.md", "[[Plain]]\n");
        },
        &[("Write", "Forge/Gamma.md", None)],
        true,
    ) else {
        return;
    };
    assert_eq!(read(run.last(), "Forge/Plain.md"), "# Plain\n\nNo wiki links here at all.\n\n\n## Backlinks\n\n- [[Gamma]]\n- [[Notes]]\n");
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/Plain.md"]);
    assert_eq!(run.outcomes[0].actions, 1);
}

#[test]
fn backlinks_remove_link_is_stale_until_target_is_refreshed() {
    // ORACLE BUG: dropping [[Beta]] from Alpha leaves "- [[Alpha]]" in Beta's
    // section; it is only rebuilt when another file linking Beta is written.
    let Some(run) = parity(
        Which::Backlinks,
        &|h| {
            let p = h.join("Forge/Alpha.md");
            let s = fs::read_to_string(&p).unwrap().replace("[[Beta]]", "Beta");
            fs::write(p, s).unwrap();
        },
        &[("Write", "Forge/Alpha.md", None), ("Write", "Forge/sub/Delta.md", None)],
        true,
    ) else {
        return;
    };
    // step 1: Alpha's remaining links are [[Gamma|alias]], ?[[Missing]] and — because the
    // oracle also reads links out of the ## Backlinks section — [[Delta]]. Gamma and Delta
    // are refreshed; Beta keeps the stale "- [[Alpha]]".
    assert_eq!(read(&run.after[0], "Forge/Beta.md"), read(&run.before, "Forge/Beta.md"));
    assert_eq!(read(&run.after[0], "Forge/Gamma.md"), "# Gamma\n\nPlain body without links.\n\n\n## Backlinks\n\n- [[Beta]]\n");
    assert_eq!(changed(&run.before, &run.after[0]), vec!["Forge/Gamma.md", "Forge/sub/Delta.md"]);
    // step 2: Delta links Alpha and ?[[Beta]] → both rebuilt; cross-dir + subdir ordering.
    // ORACLE BUG (cascade): Alpha is refreshed first and its new section contains
    // "- [[Beta]]", and Gamma's section from step 1 contains "- [[Beta]]", so Beta's
    // backlinks become Alpha, Gamma, Delta — the removed body link never drops out.
    assert_eq!(read(&run.after[1], "Forge/Beta.md"), "# Beta\n\nSee [[Alpha]] and [[Delta]] and [[Gamma]].\n\n## Backlinks\n\n- [[Alpha]]\n- [[Gamma]]\n- [[Delta]]\n");
    assert_eq!(
        read(&run.after[1], "Forge/Alpha.md"),
        "# Alpha\n\nLinks to Beta and [[Gamma|alias]] and ?[[Missing]].\n\n## Backlinks\n\n- [[Beta]]\n- [[Code]]\n- [[Later]]\n- [[Delta]]\n- [[Notes]]\n"
    );
}

#[test]
fn backlinks_file_without_links_is_untouched() {
    let Some(run) = parity(Which::Backlinks, &|_| {}, &[("Write", "Forge/Plain.md", None), ("Write", "Forge/Gamma.md", None)], true) else { return };
    assert_eq!(run.before, *run.last());
    assert!(run.outcomes.iter().all(|o| o.actions == 0));
    assert!(run.oracle_out[0].contains("No wiki links in file, skipping"));
}

#[test]
fn backlinks_idempotent_after_cascade_settles() {
    // ORACLE BUG: links inside ## Backlinks sections are extracted like any
    // other, so a section written in round 1 becomes a backlink in round 2
    // (backlinks-init explicitly excludes the section to prevent this). The
    // same event is therefore NOT idempotent until the cascade settles; it
    // does settle, and from then on repeats change nothing.
    let step = ("Write", "Forge/Alpha.md", None);
    let Some(run) = parity(Which::Backlinks, &|_| {}, &[step, step, step], true) else { return };
    assert_ne!(run.before, run.after[0], "first run should have rebuilt Beta/Gamma/Delta sections");
    assert_ne!(run.after[0], run.after[1], "cascade: Gamma's new section adds Gamma to Beta's backlinks");
    assert_eq!(read(&run.after[1], "Forge/Beta.md"), "# Beta\n\nSee [[Alpha]] and [[Delta]] and [[Gamma]].\n\n## Backlinks\n\n- [[Alpha]]\n- [[Gamma]]\n- [[Delta]]\n");
    assert_eq!(run.after[1], run.after[2], "settled: third run is a no-op");
}

#[test]
fn backlinks_create_rebuilds_targets_across_roots_and_subdirs() {
    let Some(run) = parity(Which::Backlinks, &|h| write(h, "Forge/New.md", "# New\n\nHello [[Alpha]] and [[Plain]] and [[nope]].\n"), &[("Create", "Forge/New.md", None)], true) else {
        return;
    };
    assert_eq!(
        read(run.last(), "Forge/Alpha.md"),
        "# Alpha\n\nLinks to [[Beta]] and [[Gamma|alias]] and ?[[Missing]].\n\n## Backlinks\n\n- [[Beta]]\n- [[Code]]\n- [[Later]]\n- [[New]]\n- [[Delta]]\n- [[Notes]]\n"
    );
    assert_eq!(read(run.last(), "Forge/Plain.md"), "# Plain\n\nNo wiki links here at all.\n\n\n## Backlinks\n\n- [[New]]\n- [[Notes]]\n");
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/Alpha.md", "Forge/Plain.md"]);
}

#[test]
fn backlinks_section_edge_cases() {
    // Later: long tail after ## Backlinks → half-size guard refuses (unchanged).
    // Short: short tail → the tail is DISCARDED (ORACLE BUG).
    // Indent: leading blank lines/indent trimmed (ORACLE BUG). Tiny: < 10 bytes skipped.
    // Solo: linked only as [[Solo|s]] → gets an EMPTY section.
    let Some(run) = parity(
        Which::Backlinks,
        &|h| {
            write(h, "Forge/Short.md", "# Short\n\nBody.\n\n## Backlinks\n\n- [[Nobody]]\n\n## After\n\nkept?\n");
            write(h, "Forge/Indent.md", "\n\n   # Indent\n\nbody\n\n## Backlinks\n\n- [[X]]\n");
            write(h, "Forge/Tiny.md", "tiny\n");
            write(h, "Forge/Solo.md", "# Solo\n\nbody text\n");
            write(h, "Forge/Triple.md", "# Triple\n\nbody\n\n### Backlinks\n\n- [[X]]\n");
            append(h, "Forge/Gamma.md", "\n[[Later]] [[Short]] [[Indent]] [[Tiny]] [[Solo|s]] [[Triple]]\n");
        },
        &[("Write", "Forge/Gamma.md", None)],
        true,
    ) else {
        return;
    };
    let s = run.last();
    assert_eq!(read(s, "Forge/Later.md"), read(&run.before, "Forge/Later.md"));
    assert_eq!(read(s, "Forge/Short.md"), "# Short\n\nBody.\n\n## Backlinks\n\n- [[Gamma]]\n");
    assert_eq!(read(s, "Forge/Indent.md"), "# Indent\n\nbody\n\n## Backlinks\n\n- [[Gamma]]\n");
    assert_eq!(read(s, "Forge/Tiny.md"), "tiny\n");
    assert_eq!(read(s, "Forge/Solo.md"), "# Solo\n\nbody text\n\n\n## Backlinks\n");
    assert_eq!(read(s, "Forge/Triple.md"), read(&run.before, "Forge/Triple.md"));
    assert!(run.oracle_out[0].contains("Refusing to write potentially corrupted content to: Later.md"));
    assert!(run.oracle_out[0].contains("Backlinks section detection failed for: Triple.md"));
}

#[test]
fn backlinks_rename_rewrites_plain_links_only() {
    let Some(run) = parity(
        Which::Backlinks,
        &|h| fs::rename(h.join("Forge/Gamma.md"), h.join("Forge/Gamma2.md")).unwrap(),
        &[("Rename", "Forge/Gamma.md", Some("Forge/Gamma2.md"))],
        true,
    ) else {
        return;
    };
    let s = run.last();
    assert_eq!(read(s, "Forge/Beta.md"), "# Beta\n\nSee [[Alpha]] and [[Delta]] and [[Gamma2]].\n\n## Backlinks\n\n- [[Alpha]]\n");
    assert_eq!(read(s, "Forge/Gamma2.md"), "# Gamma\n\nPlain body without links.\n\n\n## Backlinks\n\n- [[Beta]]\n");
    // ORACLE BUG: [[Gamma|alias]] in Alpha is not renamed
    assert_eq!(read(s, "Forge/Alpha.md"), read(&run.before, "Forge/Alpha.md"));
}

#[test]
fn backlinks_remove_is_a_noop() {
    let Some(run) = parity(Which::Backlinks, &|h| fs::remove_file(h.join("Forge/Gamma.md")).unwrap(), &[("Remove", "Forge/Gamma.md", None)], true) else { return };
    assert_eq!(run.before, *run.last());
}

#[test]
fn backlinks_skips_large_and_link_heavy_files() {
    let Some(run) = parity(
        Which::Backlinks,
        &|h| {
            write(h, "Forge/Big.md", &format!("# Big\n\n[[Plain]]\n{}\n", "x".repeat(600_000)));
            let many: String = (1..=101).map(|i| format!("[[l{i}]] ")).collect();
            write(h, "Forge/Many.md", &format!("# Many\n\n{many}[[Plain]]\n"));
        },
        &[("Write", "Forge/Big.md", None), ("Write", "Forge/Many.md", None)],
        true,
    ) else {
        return;
    };
    assert_eq!(run.before, *run.last());
    assert!(run.oracle_out[0].contains("Skipping large file"));
    assert!(run.oracle_out[1].contains("Skipping file with 102 links"));
}

#[test]
fn backlinks_marker_skips_event() {
    let Some(run) = parity(
        Which::Backlinks,
        &|h| {
            append(h, "Forge/Gamma.md", "\n[[Plain]]\n");
            fs::write(h.join("marker"), "resolve-mark").unwrap();
        },
        &[("Write", "Forge/Gamma.md", None)],
        false,
    ) else {
        return;
    };
    assert_eq!(run.before, *run.last());
}

// ── resolve-mark scenarios ──────────────────────────────────────────
const MIX_IN: &str = "# Mix\n\n[[Gone]] [[Gone|al]] [[Gone#h]] ?[[Gone]] [[Alpha]] [[beta]] ![[img.png]] >[[Inbox]] [[https://x.y]] [[a]] [[Foo (bar)]] [[Note: x]] [[deadbeefdeadbeefdeadbeefdeadbeef]] [[Alpha|a]] ![[Alpha]] [[delta]]\n";
const MIX_OUT: &str = "# Mix\n\n?[[Gone]] ?[[Gone|al]] ?[[Gone#h]] ?[[Gone]] [[Alpha]] [[beta]] !?[[img.png]] >?[[Inbox]] [[https://x.y]] [[a]] ?[[Foo (bar)]] [[Note: x]] [[deadbeefdeadbeefdeadbeefdeadbeef]] [[Alpha|a]] ![[Alpha]] [[delta]]\n";

fn mix(h: &Path) {
    write(h, "Forge/Mix.md", MIX_IN);
    write(h, "Forge/Foo (bar).md", "# Foo\n");
}

#[test]
fn resolve_write_marks_missing_targets() {
    // Covers: plain / alias / heading forms marked; already-marked not doubled;
    // smart-case resolution ([[beta]], [[delta]] in a subdir); smart-filter
    // exclusions; ORACLE BUGS: embeds → `!?[[`, inbox → `>?[[`, and
    // `[[Foo (bar)]]` marked although "Foo (bar).md" exists (fd regex).
    let Some(run) = parity(Which::ResolveMark, &mix, &[("Write", "Forge/Mix.md", None)], true) else { return };
    assert_eq!(read(run.last(), "Forge/Mix.md"), MIX_OUT);
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/Mix.md"]);
    assert!(run.oracle_out[0].contains("Marked 6 new unresolved links"));
}

#[test]
fn resolve_write_all_resolved_is_untouched() {
    let Some(run) = parity(Which::ResolveMark, &|_| {}, &[("Write", "Forge/Beta.md", None), ("Write", "Forge/Alpha.md", None), ("Write", "Forge/Plain.md", None)], true) else { return };
    assert_eq!(run.before, *run.last());
    assert_eq!(run.outcomes[0].actions, 0);
    // Alpha's only unresolved link is already ?[[Missing]]: the oracle still re-saves identical bytes
    assert_eq!(run.outcomes[1].actions, 1);
    assert!(run.oracle_out[1].contains("Marked 1 new unresolved links"));
    assert_eq!(run.outcomes[2].actions, 0);
}

#[test]
fn resolve_idempotent() {
    let Some(run) = parity(Which::ResolveMark, &mix, &[("Write", "Forge/Mix.md", None), ("Write", "Forge/Mix.md", None)], true) else { return };
    assert_eq!(run.after[0], run.after[1]);
    assert_eq!(read(&run.after[1], "Forge/Mix.md"), MIX_OUT);
}

#[test]
fn resolve_create_target_does_not_unmark() {
    // ORACLE BUGS: the rg pattern is double-escaped, so creating Missing.md
    // leaves ?[[Missing]] in Alpha. The pattern DOES match a file containing
    // a literal `\\` (for ANY name), so both creates "clean" Code.md — the
    // first changes nothing but the sd round-trip drops its final newline;
    // the second turns ?[[Target]] into [[Target]].
    let Some(run) = parity(
        Which::ResolveMark,
        &|h| {
            write(h, "Forge/Missing.md", "# Missing\n");
            write(h, "Forge/Target.md", "# Target\n");
        },
        &[("Create", "Forge/Missing.md", None), ("Create", "Forge/Target.md", None)],
        true,
    ) else {
        return;
    };
    assert_eq!(read(&run.after[0], "Forge/Code.md"), CODE_MD.strip_suffix('\n').unwrap());
    assert_eq!(changed(&run.before, &run.after[0]), vec!["Forge/Code.md"]);
    assert_eq!(read(&run.after[1], "Forge/Alpha.md"), read(&run.before, "Forge/Alpha.md"));
    assert!(read(&run.after[1], "Forge/Alpha.md").contains("?[[Missing]]"));
    assert_eq!(read(&run.after[1], "Forge/Code.md"), "# Code\n\nEscapes: `\\\\` and [[Alpha]] and [[Target]] and [[Target]].");
    assert_eq!(changed(&run.before, &run.after[1]), vec!["Forge/Code.md"]);
    assert!(run.oracle_out[0].contains("Cleaning ?[[ markers in 1 files"));
    assert!(run.oracle_out[1].contains("Cleaning ?[[ markers in 1 files"));
}

#[test]
fn resolve_delete_target_marks_only_backslash_files() {
    // ORACLE BUG: same double-escaped pattern — deleting Alpha marks [[Alpha]]
    // only in Code.md (which contains `\\`); Beta, Delta, Later, Notes keep [[Alpha]].
    let Some(run) = parity(Which::ResolveMark, &|h| fs::remove_file(h.join("Forge/Alpha.md")).unwrap(), &[("Remove", "Forge/Alpha.md", None)], true) else { return };
    assert_eq!(read(run.last(), "Forge/Code.md"), "# Code\n\nEscapes: `\\\\` and ?[[Alpha]] and ?[[Target]] and [[Target]].\n");
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/Code.md"]);
    assert!(read(run.last(), "Forge/Beta.md").contains("[[Alpha]]"));
    assert!(!read(run.last(), "Forge/Beta.md").contains("?[[Alpha]]"));
}

#[test]
fn resolve_rename_rewrites_only_backslash_files() {
    let Some(run) = parity(
        Which::ResolveMark,
        &|h| fs::rename(h.join("Forge/Alpha.md"), h.join("Forge/Alpha2.md")).unwrap(),
        &[("Rename", "Forge/Alpha.md", Some("Forge/Alpha2.md"))],
        true,
    ) else {
        return;
    };
    assert_eq!(read(run.last(), "Forge/Code.md"), "# Code\n\nEscapes: `\\\\` and [[Alpha2]] and ?[[Target]] and [[Target]].\n");
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/Code.md"]);
}

#[test]
fn resolve_skips_large_link_heavy_and_marker() {
    let Some(run) = parity(
        Which::ResolveMark,
        &|h| {
            write(h, "Forge/Big.md", &format!("# Big\n\n[[Gone]]\n{}\n", "x".repeat(600_000)));
            let many: String = (1..=101).map(|i| format!("[[l{i}]] ")).collect();
            write(h, "Forge/Many.md", &format!("# Many\n\n{many}\n"));
        },
        &[("Write", "Forge/Big.md", None), ("Write", "Forge/Many.md", None)],
        true,
    ) else {
        return;
    };
    assert_eq!(run.before, *run.last());

    let Some(run) = parity(
        Which::ResolveMark,
        &|h| {
            mix(h);
            fs::write(h.join("marker"), "backlinks").unwrap();
        },
        &[("Write", "Forge/Mix.md", None)],
        false,
    ) else {
        return;
    };
    assert_eq!(run.before, *run.last());
}
