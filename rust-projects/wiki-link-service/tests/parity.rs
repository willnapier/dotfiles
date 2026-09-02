//! Parity with the Nushell oracles where 0.2.0 left behaviour untouched
//! (`parity_*`), and the corrected behaviour per Will's spec where it
//! deliberately diverges (`spec_*`), plus the read-only `audit`.
//!
//! ## Oracle harness (parity_* tests)
//! Each scenario builds the SAME fixture tree twice under a tempdir
//! (`<tmp>/oracle/{Forge,Admin}` and `<tmp>/rust/{Forge,Admin}`), applies the
//! same user edit to both, then feeds the same event to
//! * the oracle: `nu -n -c "source '<patched script>'; handle_change '<op>' '<path>' '<new>' ['<roots>']"`
//!   with `HOME=<tmp>/oracle` — the script is a copy of `scripts/wiki-*` with
//!   its `MARKER_FILE` const pointed inside the tempdir (so the real
//!   `/tmp/wiki-watcher-writing` is never touched) and `RIPGREP_CONFIG_PATH`
//!   set to a file containing `--sort=path` (the oracle's `rg -l` order is
//!   otherwise nondeterministic);
//! * the port: `Which::handle(&ctx, op, path, new_path)`.
//!
//! Then every file under both trees is compared byte-for-byte. Nothing here
//! ever touches a real directory. Requires `nu`, `rg`, `fd`, `sd` on PATH;
//! parity tests skip (with a message) otherwise. `spec_*` tests run the port
//! only, against hand-derived expectations.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use wiki_link_service::audit;
use wiki_link_service::logger::Logger;
use wiki_link_service::watch::Which;
use wiki_link_service::wiki::{Ctx, Outcome};

type Snap = BTreeMap<String, Vec<u8>>;

const ALPHA: &str = "# Alpha\n\nLinks to [[Beta]] and [[Gamma|alias]] and ?[[Missing]].\n\n## Backlinks\n\n- [[Beta]]\n- [[Delta]]\n";
const BETA: &str = "# Beta\n\nSee [[Alpha]] and [[Delta]] and [[Gamma]].\n\n## Backlinks\n\n- [[Alpha]]\n";
const GAMMA: &str = "# Gamma\n\nPlain body without links.\n";
const LATER: &str = "# Later\n\nBody [[Alpha]].\n\n## Backlinks\n\n- [[Nobody]]\n\n## Afterwards\n\nThis section follows the backlinks.\n";
const CODE_MD: &str = "# Code\n\nEscapes: `\\\\` and [[Alpha]] and ?[[Target]] and [[Target]].\n";
const NOTES: &str = "Admin note linking [[Alpha]] and [[Plain]].\n";

// ── fixture ─────────────────────────────────────────────────────────
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

/// Notes with links, marks, existing sections, a subdirectory, a second
/// root, a section followed by another section, and a literal `\\`.
fn fixture(home: &Path) {
    write(home, "Forge/Alpha.md", ALPHA);
    write(home, "Forge/Beta.md", BETA);
    write(home, "Forge/sub/Delta.md", "# Delta\n\nRefers to [[Alpha]] and ?[[Beta]].\n");
    write(home, "Forge/Gamma.md", GAMMA);
    write(home, "Forge/Plain.md", "# Plain\n\nNo wiki links here at all.\n");
    write(home, "Forge/Later.md", LATER);
    write(home, "Forge/Code.md", CODE_MD);
    write(home, "Admin/Notes.md", NOTES);
}

/// Roots handed to both sides: Forge, Admin, and an Archives that does not exist.
fn roots(home: &Path) -> Vec<PathBuf> {
    vec![home.join("Forge"), home.join("Admin"), home.join("Archives")]
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

// ── oracle harness ──────────────────────────────────────────────────
fn tools_available() -> bool {
    ["nu", "rg", "fd", "sd"].iter().all(|t| Command::new("which").arg(t).output().map(|o| o.status.success()).unwrap_or(false))
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

type Step<'a> = (&'a str, &'a str, Option<&'a str>);

struct Run {
    before: Snap,
    after: Vec<Snap>,
    outcomes: Vec<Outcome>,
    oracle_out: Vec<String>,
}

impl Run {
    fn last(&self) -> &Snap {
        self.after.last().unwrap()
    }
}

/// Run `steps` against oracle and port on identical fixtures; assert
/// byte-for-byte equality of the whole tree after every step.
fn parity(w: Which, setup: &dyn Fn(&Path), steps: &[Step]) -> Option<Run> {
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
    let ctx = Ctx::new(roots(&r), Logger::silent());
    let (mut after, mut outcomes, mut oracle_out) = (Vec::new(), Vec::new(), Vec::new());
    for (op, rel, new_rel) in steps {
        let _ = fs::remove_file(o.join("marker")); // > 5 s between oracle events
        let new_o = new_rel.map(|n| o.join(n));
        let new_r = new_rel.map(|n| r.join(n));
        oracle_out.push(run_oracle(&o, w, op, &o.join(rel), new_o.as_deref()));
        outcomes.push(w.handle(&ctx, op, &r.join(rel), new_r.as_deref()));
        let (so, sr) = (snapshot(&o), snapshot(&r));
        assert!(so == sr, "{} {op} {rel}: port differs from oracle\n{}\noracle said:\n{}", w.sub(), diff(&so, &sr), oracle_out.last().unwrap());
        after.push(sr);
    }
    Some(Run { before, after, outcomes, oracle_out })
}

// ── spec runner (port only, one Ctx across steps) ───────────────────
enum SpecStep<'a> {
    Ev(&'a str, &'a str, Option<&'a str>),
    /// An external edit between events (no event of its own).
    Edit(&'a dyn Fn(&Path)),
}
use SpecStep::{Edit, Ev};

struct SpecRun {
    before: Snap,
    /// One snapshot per `Ev`.
    after: Vec<Snap>,
    outcomes: Vec<Outcome>,
    log: String,
}

impl SpecRun {
    fn last(&self) -> &Snap {
        self.after.last().unwrap()
    }
}

fn spec(w: Which, setup: &dyn Fn(&Path), steps: &[SpecStep]) -> SpecRun {
    let tmp = tempfile::tempdir().unwrap();
    let h = tmp.path().join("rust");
    fixture(&h);
    setup(&h);
    let before = snapshot(&h);
    let log = h.join("log");
    let ctx = Ctx::new(roots(&h), Logger { file: Some(log.clone()), tag: None, quiet: true });
    let (mut after, mut outcomes) = (Vec::new(), Vec::new());
    for step in steps {
        match step {
            Ev(op, rel, new_rel) => {
                let new = new_rel.map(|n| h.join(n));
                outcomes.push(w.handle(&ctx, op, &h.join(rel), new.as_deref()));
                after.push(snapshot(&h));
            }
            Edit(f) => f(&h),
        }
    }
    SpecRun { before, after, outcomes, log: fs::read_to_string(&log).unwrap_or_default() }
}

// ═══ parity (unchanged behaviour) ═══════════════════════════════════
#[test]
fn parity_backlinks_file_without_links_is_untouched() {
    let Some(run) = parity(Which::Backlinks, &|_| {}, &[("Write", "Forge/Plain.md", None), ("Write", "Forge/Gamma.md", None)]) else { return };
    assert_eq!(run.before, *run.last());
    assert!(run.outcomes.iter().all(|o| o.actions == 0));
    assert!(run.oracle_out[0].contains("No wiki links in file, skipping"));
}

#[test]
fn parity_backlinks_remove_of_unlisted_note_is_a_noop() {
    let Some(run) = parity(Which::Backlinks, &|h| fs::remove_file(h.join("Forge/Gamma.md")).unwrap(), &[("Remove", "Forge/Gamma.md", None)]) else { return };
    assert_eq!(run.before, *run.last());
}

#[test]
fn parity_backlinks_skips_large_and_link_heavy_files() {
    let Some(run) = parity(
        Which::Backlinks,
        &|h| {
            write(h, "Forge/Big.md", &format!("# Big\n\n[[Plain]]\n{}\n", "x".repeat(600_000)));
            let many: String = (1..=101).map(|i| format!("[[l{i}]] ")).collect();
            write(h, "Forge/Many.md", &format!("# Many\n\n{many}[[Plain]]\n"));
        },
        &[("Write", "Forge/Big.md", None), ("Write", "Forge/Many.md", None)],
    ) else {
        return;
    };
    assert_eq!(run.before, *run.last());
    assert!(run.oracle_out[0].contains("Skipping large file"));
    assert!(run.oracle_out[1].contains("Skipping file with 102 links"));
}

#[test]
fn parity_resolve_write_all_resolved_is_untouched() {
    let Some(run) = parity(Which::ResolveMark, &|_| {}, &[("Write", "Forge/Beta.md", None), ("Write", "Forge/Alpha.md", None), ("Write", "Forge/Plain.md", None)]) else { return };
    assert_eq!(run.before, *run.last());
    // Alpha's only unresolved link is already ?[[Missing]]: the oracle re-saved identical bytes; the port does not write at all.
    assert!(run.oracle_out[1].contains("Marked 1 new unresolved links"));
    assert!(run.outcomes.iter().all(|o| o.actions == 0));
}

#[test]
fn parity_resolve_skips_large_and_link_heavy_files() {
    let Some(run) = parity(
        Which::ResolveMark,
        &|h| {
            write(h, "Forge/Big.md", &format!("# Big\n\n[[Gone]]\n{}\n", "x".repeat(600_000)));
            let many: String = (1..=101).map(|i| format!("[[l{i}]] ")).collect();
            write(h, "Forge/Many.md", &format!("# Many\n\n{many}\n"));
        },
        &[("Write", "Forge/Big.md", None), ("Write", "Forge/Many.md", None)],
    ) else {
        return;
    };
    assert_eq!(run.before, *run.last());
}

// ═══ spec: backlinks ════════════════════════════════════════════════
#[test]
fn spec_backlinks_add_link_appends_entry_and_is_idempotent() {
    // one blank line before the heading (bug 5 format); hidden files still ignored
    let run = spec(
        Which::Backlinks,
        &|h| {
            append(h, "Forge/Gamma.md", "\nNow also [[Plain]].\n");
            write(h, "Forge/.hidden/H.md", "[[Plain]]\n");
        },
        &[Ev("Write", "Forge/Gamma.md", None), Ev("Write", "Forge/Gamma.md", None)],
    );
    assert_eq!(read(&run.after[0], "Forge/Plain.md"), "# Plain\n\nNo wiki links here at all.\n\n## Backlinks\n\n- [[Gamma]]\n- [[Notes]]\n");
    assert_eq!(changed(&run.before, &run.after[0]), vec!["Forge/Plain.md"]);
    assert_eq!(run.outcomes[0].actions, 1);
    assert_eq!(run.after[0], run.after[1]);
    assert_eq!(run.outcomes[1].actions, 0);
}

#[test]
fn spec_backlinks_removed_link_drops_entry_without_cascade() {
    // bug 4: Beta's "- [[Alpha]]" goes when Alpha stops linking Beta.
    // bug 3: Alpha's own section still lists Beta and Delta but that is not a link.
    let run = spec(
        Which::Backlinks,
        &|h| write(h, "Forge/Alpha.md", "# Alpha\n\nLinks to Beta and [[Gamma|alias]] and ?[[Missing]].\n\n## Backlinks\n\n- [[Beta]]\n- [[Delta]]\n"),
        &[Ev("Write", "Forge/Alpha.md", None), Ev("Write", "Forge/Alpha.md", None)],
    );
    assert_eq!(read(&run.after[0], "Forge/Beta.md"), "# Beta\n\nSee [[Alpha]] and [[Delta]] and [[Gamma]].\n\n## Backlinks\n\n- [[Delta]]\n");
    // bug 6: [[Gamma|alias]] is a link to Gamma
    assert_eq!(read(&run.after[0], "Forge/Gamma.md"), "# Gamma\n\nPlain body without links.\n\n## Backlinks\n\n- [[Alpha]]\n- [[Beta]]\n");
    assert_eq!(changed(&run.before, &run.after[0]), vec!["Forge/Beta.md", "Forge/Gamma.md"]);
    assert_eq!(run.after[0], run.after[1], "no cascade: second identical event changes nothing");
}

#[test]
fn spec_backlinks_section_replacement_preserves_everything_else() {
    let run = spec(
        Which::Backlinks,
        &|h| {
            write(h, "Forge/Indent.md", "\n\n   # Indent\n\nbody\n\n## Backlinks\n\n- [[X]]\n");
            write(h, "Forge/Tiny.md", "tiny\n");
            write(h, "Forge/Solo.md", "# Solo\n\nbody text\n");
            write(h, "Forge/NoNl.md", "# NoNl\n\nbody");
            append(h, "Forge/Gamma.md", "\n[[Later]] [[Indent]] [[Tiny]] [[Solo|s]] [[NoNl]]\n");
        },
        &[Ev("Write", "Forge/Gamma.md", None), Edit(&|h| write(h, "Forge/Gamma.md", GAMMA)), Ev("Write", "Forge/Gamma.md", None)],
    );
    let s = &run.after[0];
    // bug 5: the section after "## Backlinks" is replaced; "## Afterwards" survives; leading whitespace kept
    assert_eq!(read(s, "Forge/Later.md"), "# Later\n\nBody [[Alpha]].\n\n## Backlinks\n\n- [[Gamma]]\n\n## Afterwards\n\nThis section follows the backlinks.\n");
    assert_eq!(read(s, "Forge/Indent.md"), "\n\n   # Indent\n\nbody\n\n## Backlinks\n\n- [[Gamma]]\n");
    assert_eq!(read(s, "Forge/Tiny.md"), "tiny\n");
    // bug 6: alias-only target gets a real entry
    assert_eq!(read(s, "Forge/Solo.md"), "# Solo\n\nbody text\n\n## Backlinks\n\n- [[Gamma]]\n");
    // bug 2: trailing-newline state preserved
    assert_eq!(read(s, "Forge/NoNl.md"), "# NoNl\n\nbody\n\n## Backlinks\n\n- [[Gamma]]");
    // links gone again → sections removed (bug 4), rest byte-for-byte
    let t = &run.after[1];
    assert_eq!(read(t, "Forge/Later.md"), "# Later\n\nBody [[Alpha]].\n\n## Afterwards\n\nThis section follows the backlinks.\n");
    assert_eq!(read(t, "Forge/Indent.md"), "\n\n   # Indent\n\nbody\n");
    assert_eq!(read(t, "Forge/Solo.md"), "# Solo\n\nbody text\n");
    assert_eq!(read(t, "Forge/NoNl.md"), "# NoNl\n\nbody");
}

#[test]
fn spec_backlinks_rename_rewrites_all_forms_and_rebuilds_section() {
    let run = spec(
        Which::Backlinks,
        &|h| {
            append(h, "Forge/Code.md", "See [[gamma#Sec]].\n");
            fs::rename(h.join("Forge/Gamma.md"), h.join("Forge/Gamma2.md")).unwrap();
        },
        &[Ev("Rename", "Forge/Gamma.md", Some("Forge/Gamma2.md"))],
    );
    let s = run.last();
    assert_eq!(read(s, "Forge/Alpha.md"), ALPHA.replace("[[Gamma|alias]]", "[[Gamma2|alias]]"));
    assert_eq!(read(s, "Forge/Beta.md"), BETA.replace("[[Gamma]]", "[[Gamma2]]"));
    assert_eq!(read(s, "Forge/Code.md"), format!("{CODE_MD}See [[Gamma2#Sec]].\n"));
    assert_eq!(read(s, "Forge/Gamma2.md"), "# Gamma\n\nPlain body without links.\n\n## Backlinks\n\n- [[Alpha]]\n- [[Beta]]\n- [[Code]]\n");
    assert_eq!(changed(&run.before, s), vec!["Forge/Alpha.md", "Forge/Beta.md", "Forge/Code.md", "Forge/Gamma2.md"]);
}

#[test]
fn spec_backlinks_case_insensitive_and_regex_safe_names() {
    let run = spec(
        Which::Backlinks,
        &|h| {
            write(h, "Forge/Foo (bar).md", "# Foo\n\nbody\n");
            write(h, "Forge/C++.md", "# C++\n\nbody\n");
            append(h, "Forge/Gamma.md", "\n[[foo (bar)]] [[c++]]\n");
        },
        &[Ev("Write", "Forge/Gamma.md", None)],
    );
    assert_eq!(read(run.last(), "Forge/Foo (bar).md"), "# Foo\n\nbody\n\n## Backlinks\n\n- [[Gamma]]\n");
    assert_eq!(read(run.last(), "Forge/C++.md"), "# C++\n\nbody\n\n## Backlinks\n\n- [[Gamma]]\n");
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/C++.md", "Forge/Foo (bar).md"]);
}

#[test]
fn spec_backlinks_deleted_note_leaves_no_entries() {
    let run = spec(Which::Backlinks, &|h| fs::remove_file(h.join("Forge/Beta.md")).unwrap(), &[Ev("Remove", "Forge/Beta.md", None)]);
    assert_eq!(read(run.last(), "Forge/Alpha.md"), "# Alpha\n\nLinks to [[Beta]] and [[Gamma|alias]] and ?[[Missing]].\n\n## Backlinks\n\n- [[Code]]\n- [[Delta]]\n- [[Later]]\n- [[Notes]]\n");
    assert_eq!(changed(&run.before, run.last()), vec!["Forge/Alpha.md"]);
}

#[test]
fn spec_self_writes_are_suppressed_until_the_file_really_changes() {
    let run = spec(
        Which::Backlinks,
        &|h| {
            write(h, "Forge/Solo.md", "# Solo\n\nbody text\n");
            append(h, "Forge/Gamma.md", "\n[[Solo]]\n");
        },
        &[
            Ev("Write", "Forge/Gamma.md", None),
            Ev("Write", "Forge/Solo.md", None), // our own write, bytes unchanged → skipped
            Ev("Write", "Forge/Gamma.md", None), // never written by us → processed
            Edit(&|h| append(h, "Forge/Solo.md", "\nmore\n")),
            Ev("Write", "Forge/Solo.md", None), // changed since → processed
        ],
    );
    assert_eq!(read(&run.after[0], "Forge/Solo.md"), "# Solo\n\nbody text\n\n## Backlinks\n\n- [[Gamma]]\n");
    assert_eq!(run.outcomes[1].actions, 0);
    assert_eq!(run.log.matches("Modified: Solo.md").count(), 1, "{}", run.log);
    assert_eq!(run.log.matches("Modified: Gamma.md").count(), 2, "{}", run.log);
}

// ═══ spec: resolve-mark ═════════════════════════════════════════════
const MIX_IN: &str = "# Mix\n\n[[Gone]] [[Gone|al]] [[Gone#h]] ?[[Gone]] [[Alpha]] [[beta]] ![[img.png]] >[[Inbox]] [[https://x.y]] [[a]] [[Foo (bar)]] [[Note: x]] [[deadbeefdeadbeefdeadbeefdeadbeef]] [[Alpha|a]] ![[Alpha]] [[delta]] !?[[img2.png]] >?[[Inbox2]] ??[[Gone]] ?[[Alpha]]\n";
const MIX_OUT: &str = "# Mix\n\n?[[Gone]] ?[[Gone|al]] ?[[Gone#h]] ?[[Gone]] [[Alpha]] [[beta]] ![[img.png]] >[[Inbox]] [[https://x.y]] [[a]] [[Foo (bar)]] [[Note: x]] [[deadbeefdeadbeefdeadbeefdeadbeef]] [[Alpha|a]] ![[Alpha]] [[delta]] ![[img2.png]] >[[Inbox2]] ?[[Gone]] [[Alpha]]\n";

fn mix(h: &Path) {
    write(h, "Forge/Mix.md", MIX_IN);
    write(h, "Forge/Foo (bar).md", "# Foo\n");
}

#[test]
fn spec_resolve_write_marks_missing_unmarks_existing_never_embeds() {
    let run = spec(Which::ResolveMark, &mix, &[Ev("Write", "Forge/Mix.md", None), Ev("Write", "Forge/Mix.md", None)]);
    assert_eq!(read(&run.after[0], "Forge/Mix.md"), MIX_OUT);
    assert_eq!(changed(&run.before, &run.after[0]), vec!["Forge/Mix.md"]);
    assert_eq!(run.outcomes[0].actions, 1);
    assert_eq!(run.after[0], run.after[1]);
    assert_eq!(run.outcomes[1].actions, 0);
}

#[test]
fn spec_resolve_create_unmarks_all_forms_everywhere() {
    let run = spec(
        Which::ResolveMark,
        &|h| {
            append(h, "Admin/Notes.md", "See ?[[missing|al]] and ?[[Missing#h]].\n");
            write(h, "Forge/Missing.md", "# Missing\n");
        },
        &[Ev("Create", "Forge/Missing.md", None)],
    );
    let s = run.last();
    assert_eq!(read(s, "Forge/Alpha.md"), ALPHA.replace("?[[Missing]]", "[[Missing]]"));
    assert_eq!(read(s, "Admin/Notes.md"), format!("{NOTES}See [[missing|al]] and [[Missing#h]].\n"));
    assert_eq!(read(s, "Forge/Code.md"), CODE_MD, "unrelated note untouched, final newline intact");
    assert_eq!(changed(&run.before, s), vec!["Admin/Notes.md", "Forge/Alpha.md"]);
}

#[test]
fn spec_resolve_remove_marks_all_forms_everywhere() {
    let run = spec(Which::ResolveMark, &|h| fs::remove_file(h.join("Forge/Gamma.md")).unwrap(), &[Ev("Remove", "Forge/Gamma.md", None)]);
    let s = run.last();
    assert_eq!(read(s, "Forge/Alpha.md"), ALPHA.replace("[[Gamma|alias]]", "?[[Gamma|alias]]"));
    assert_eq!(read(s, "Forge/Beta.md"), BETA.replace("[[Gamma]]", "?[[Gamma]]"));
    assert_eq!(changed(&run.before, s), vec!["Forge/Alpha.md", "Forge/Beta.md"]);
}

#[test]
fn spec_resolve_rename_is_remove_then_create() {
    let run = spec(
        Which::ResolveMark,
        &|h| fs::rename(h.join("Forge/Gamma.md"), h.join("Forge/Gamma2.md")).unwrap(),
        &[
            Ev("Rename", "Forge/Gamma.md", Some("Forge/Gamma2.md")),
            // the backlinks watcher rewrites the link; the resulting Write unmarks it
            Edit(&|h| write(h, "Forge/Alpha.md", &ALPHA.replace("[[Gamma|alias]]", "?[[Gamma2|alias]]"))),
            Ev("Write", "Forge/Alpha.md", None),
        ],
    );
    assert_eq!(read(&run.after[0], "Forge/Alpha.md"), ALPHA.replace("[[Gamma|alias]]", "?[[Gamma|alias]]"));
    assert_eq!(read(&run.after[0], "Forge/Beta.md"), BETA.replace("[[Gamma]]", "?[[Gamma]]"));
    assert_eq!(read(&run.after[1], "Forge/Alpha.md"), ALPHA.replace("[[Gamma|alias]]", "[[Gamma2|alias]]"));
}

#[test]
fn spec_resolve_preserves_line_endings() {
    let run = spec(
        Which::ResolveMark,
        &|h| {
            write(h, "Forge/NoNl.md", "x [[Gone]]");
            write(h, "Forge/Crlf.md", "x [[Gone]]\r\n\r\n");
        },
        &[Ev("Write", "Forge/NoNl.md", None), Ev("Write", "Forge/Crlf.md", None)],
    );
    assert_eq!(read(run.last(), "Forge/NoNl.md"), "x ?[[Gone]]");
    assert_eq!(read(run.last(), "Forge/Crlf.md"), "x ?[[Gone]]\r\n\r\n");
}

// ═══ audit ══════════════════════════════════════════════════════════
#[test]
fn audit_reports_blast_radius_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let h = tmp.path().join("home");
    fixture(&h);
    write(&h, "Forge/Mix.md", "# Mix\n\n[[Gone]] [[Gone|al]] [[Gone#h]] ?[[Gone]] [[Alpha]] [[beta]] ![[img.png]] >[[Inbox]] [[https://x.y]] [[a]] [[Foo (bar)]] [[Note: x]] [[deadbeefdeadbeefdeadbeefdeadbeef]] [[Alpha|a]] ![[Alpha]] [[delta]]\n");
    write(&h, "Forge/Foo (bar).md", "# Foo\n");
    let before = snapshot(&h);

    let r = audit::audit(&roots(&h));
    assert_eq!(snapshot(&h), before, "audit must never write");
    assert_eq!(r.notes, 10);

    let rel = |p: &Path| p.strip_prefix(&h).unwrap().display().to_string();
    let sections: Vec<(String, usize, usize, &str)> = r.sections.iter().map(|c| (rel(&c.path), c.added, c.removed, c.kind)).collect();
    assert_eq!(
        sections,
        vec![
            ("Forge/Alpha.md".to_string(), 4, 0, "rewritten"),
            ("Forge/Beta.md".to_string(), 2, 0, "rewritten"),
            ("Forge/Foo (bar).md".to_string(), 1, 0, "added"),
            ("Forge/Gamma.md".to_string(), 2, 0, "added"),
            ("Forge/Later.md".to_string(), 0, 1, "removed"),
            ("Forge/Plain.md".to_string(), 1, 0, "added"),
            ("Forge/sub/Delta.md".to_string(), 2, 0, "added"),
        ]
    );
    assert_eq!((r.entries_added, r.entries_removed), (12, 1));
    let added: Vec<(String, usize)> = r.markers_added.iter().map(|m| (rel(&m.path), m.count)).collect();
    assert_eq!(added, vec![("Forge/Code.md".to_string(), 1), ("Forge/Mix.md".to_string(), 3)]);
    assert_eq!(r.markers_added_total, 4);
    let removed: Vec<(String, usize)> = r.markers_removed.iter().map(|m| (rel(&m.path), m.count)).collect();
    assert_eq!(removed, vec![("Forge/sub/Delta.md".to_string(), 1)]);
    assert_eq!(r.markers_removed_total, 1);

    let text = r.render();
    assert!(text.contains("notes scanned: 10"), "{text}");
    assert!(text.contains("## Backlinks sections that would change: 7 (entries +12 / -1; sections added 4, removed 1, rewritten 2)"), "{text}");
    assert!(text.contains("?[[ markers that would be added: 4 (in 2 notes)"), "{text}");
    assert!(text.contains("?[[ markers that would be removed: 1 (in 1 notes)"), "{text}");
    assert!(text.contains("(missing)"), "absent Archives root flagged: {text}");
    assert_eq!(snapshot(&h), before);
}
