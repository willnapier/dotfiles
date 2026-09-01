//! dotter-drift-monitor — stateless "deployed vs intended" check for Dotter.
//!
//! Rust port (2026-09-01) of the Nushell rewrite made the same night. Same CLI,
//! same output lines, same exit codes, so the systemd timer, the launchd agent
//! and `system-health-check` need no change.
//!
//! What it answers: is every file that `~/dotfiles/.dotter/global.toml` says
//! should be deployed actually a symlink to its dotfiles source, right now?
//! Intent is re-read on every run. There is no baseline, no `--setup`, no
//! daemon — the mtime-baseline design it replaced could not fail (a missing
//! `$HOME` made the baseline empty so the loop ran zero times, and on the Mac a
//! stale baseline flagged every file forever, ~1.9 GB of false positives).
//!
//! Exit codes: 0 clean · 1 drift · 2 could not check. A zero denominator is an
//! error, never a pass. `--self-test` builds a fixture containing every drift
//! class and asserts the checker reports each — proof that it can fail. The
//! same fixture backs `cargo test`.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Dotter's `automatic` target type deploys a source as a rendered template
/// (regular file) if it contains this opener, otherwise as a symlink. This
/// source file is not itself dotter-managed, so the literal is safe here.
const HANDLEBARS_OPEN: &[u8] = b"{{";

#[derive(Parser, Debug)]
#[command(name = "dotter-drift-monitor", version, about = "Stateless deployed-vs-intended check for Dotter")]
struct Cli {
    /// Accepted for compatibility with the systemd unit; the check is the default
    #[arg(short, long)]
    check: bool,
    /// Emit the result record as JSON (after the human-readable lines)
    #[arg(short, long)]
    json: bool,
    /// Print only problems and the summary line
    #[arg(short, long)]
    quiet: bool,
    /// Prove the checker can fail, then exit (0 = proof held)
    #[arg(long)]
    self_test: bool,
    /// Override the dotfiles root (tests)
    #[arg(long)]
    dotfiles: Option<PathBuf>,
    /// Override home for `~` expansion (tests)
    #[arg(long)]
    home: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Verdict {
    #[serde(rename = "OK")]
    Ok,
    #[serde(rename = "NOT-SYMLINK")]
    NotSymlink,
    #[serde(rename = "WRONG-TARGET")]
    WrongTarget,
    #[serde(rename = "BROKEN-LINK")]
    BrokenLink,
    #[serde(rename = "TGT-MISSING")]
    TgtMissing,
    #[serde(rename = "SRC-MISSING")]
    SrcMissing,
    #[serde(rename = "NOT-FILE")]
    NotFile,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Verdict::Ok => "OK",
            Verdict::NotSymlink => "NOT-SYMLINK",
            Verdict::WrongTarget => "WRONG-TARGET",
            Verdict::BrokenLink => "BROKEN-LINK",
            Verdict::TgtMissing => "TGT-MISSING",
            Verdict::SrcMissing => "SRC-MISSING",
            Verdict::NotFile => "NOT-FILE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Auto,
    Symbolic,
    Template,
    Other(String),
}

impl Kind {
    fn from_toml(s: &str) -> Kind {
        match s {
            "symbolic" => Kind::Symbolic,
            "template" => Kind::Template,
            other => Kind::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone)]
struct Mapping {
    pkg: String,
    /// The `global.toml` key this row came from (a directory entry yields many rows)
    entry: String,
    src: PathBuf,
    tgt: PathBuf,
    kind: Kind,
}

#[derive(Debug, Clone, Serialize)]
struct Row {
    pkg: String,
    entry: String,
    src: PathBuf,
    tgt: PathBuf,
    verdict: Verdict,
    detail: String,
}

#[derive(Debug, Serialize)]
struct Report {
    exit_code: u8,
    evaluated: usize,
    ok: usize,
    drift: Vec<Row>,
    packages: Vec<String>,
}

impl Report {
    fn cannot_check(packages: Vec<String>) -> Report {
        Report { exit_code: 2, evaluated: 0, ok: 0, drift: vec![], packages }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    if cli.self_test {
        return ExitCode::from(run_self_test());
    }

    let home = cli
        .home
        .clone()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let live = cli.dotfiles.is_none();
    let df = cli.dotfiles.clone().unwrap_or_else(|| home.join("dotfiles"));

    let report = run_check(&df, &home, cli.quiet);

    if live && !report.drift.is_empty() {
        notify(&format!("{} dotter-managed files have drifted (see dotter-drift-monitor)", report.drift.len()));
    }
    if live {
        side_checks(&df, &home);
    }
    if cli.json {
        match serde_json::to_string_pretty(&report) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("could not serialise report: {e}"),
        }
    }
    ExitCode::from(report.exit_code)
}

// ---------------------------------------------------------------------------
// Core check
// ---------------------------------------------------------------------------

fn run_check(df: &Path, home: &Path, quiet: bool) -> Report {
    let config_path = df.join(".dotter/global.toml");
    let local_path = df.join(".dotter/local.toml");

    let config = match load_toml_table(&config_path) {
        Ok(t) => t,
        Err(e) => {
            println!("❌ CANNOT CHECK: {}: {e:#}", config_path.display());
            return Report::cannot_check(vec![]);
        }
    };

    let local_packages: Vec<String> = match load_toml_table(&local_path) {
        Ok(t) => t
            .get("packages")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        Err(_) => vec![],
    };

    let packages = resolve_packages(&config, &local_packages);
    let auto_template = config
        .get("settings")
        .and_then(|s| s.get("default_target_type"))
        .and_then(|v| v.as_str())
        .unwrap_or("automatic")
        == "automatic";

    let mappings = collect_mappings(&config, &packages, df, home);
    let rows: Vec<Row> = mappings.into_iter().map(|m| evaluate(m, auto_template)).collect();

    let evaluated = rows.len();
    let drift: Vec<Row> = rows.into_iter().filter(|r| r.verdict != Verdict::Ok).collect();
    let ok = evaluated - drift.len();

    if evaluated == 0 {
        println!(
            "❌ CANNOT CHECK: zero mappings evaluated for packages {} — refusing to report clean",
            packages.join(", ")
        );
        return Report::cannot_check(packages);
    }

    if !quiet {
        println!("🔍 dotter-drift-monitor — packages: {}", packages.join(", "));
    }
    for row in &drift {
        println!(
            "❌ {:<12}  {}  ({}){}",
            row.verdict.label(),
            display_with_tilde(&row.tgt, home),
            row.entry,
            row.detail
        );
    }
    let exit_code: u8 = if drift.is_empty() { 0 } else { 1 };
    let status = if exit_code == 0 { "✅ clean".to_string() } else { format!("🚨 {} drifted", drift.len()) };
    println!("{status} — checked {evaluated} deployed files, {ok} OK");

    Report { exit_code, evaluated, ok, drift, packages }
}

fn load_toml_table(path: &Path) -> Result<toml::Table> {
    let text = fs::read_to_string(path).with_context(|| "not found or unreadable")?;
    let value: toml::Value = text.parse().with_context(|| "does not parse as TOML")?;
    match value {
        toml::Value::Table(t) => Ok(t),
        _ => Err(anyhow!("top level is not a table")),
    }
}

/// Packages to evaluate: `shared` plus local.toml's packages plus their
/// transitive `depends`. Only packages that carry a `files` table count.
fn resolve_packages(config: &toml::Table, local_packages: &[String]) -> Vec<String> {
    let mut wanted: Vec<String> = vec!["shared".to_string()];
    for p in local_packages {
        if !wanted.contains(p) {
            wanted.push(p.clone());
        }
    }
    let mut frontier = wanted.clone();
    loop {
        let mut fresh: Vec<String> = vec![];
        for p in &frontier {
            let deps = config
                .get(p)
                .and_then(|v| v.get("depends"))
                .and_then(|d| d.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            for d in deps {
                if d != "default" && !wanted.iter().any(|w| w == d) && !fresh.iter().any(|f| f == d) {
                    fresh.push(d.to_string());
                }
            }
        }
        if fresh.is_empty() {
            break;
        }
        wanted.extend(fresh.iter().cloned());
        frontier = fresh;
    }
    wanted
        .into_iter()
        .filter(|p| config.get(p).and_then(|v| v.get("files")).and_then(|f| f.as_table()).is_some())
        .collect()
}

/// Expand every `[<pkg>.files]` entry into one row per deployed FILE. A
/// directory source is deployed by dotter as one link per file underneath it,
/// so it is walked here the same way.
fn collect_mappings(config: &toml::Table, packages: &[String], df: &Path, home: &Path) -> Vec<Mapping> {
    let mut out = vec![];
    for pkg in packages {
        let Some(files) = config.get(pkg).and_then(|v| v.get("files")).and_then(|f| f.as_table()) else {
            continue;
        };
        for (src_rel, val) in files {
            let (tgt_raw, kind) = match val {
                toml::Value::String(s) => (s.clone(), Kind::Auto),
                toml::Value::Table(t) => (
                    t.get("target").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    t.get("type").and_then(|v| v.as_str()).map(Kind::from_toml).unwrap_or(Kind::Auto),
                ),
                _ => continue,
            };
            let src_abs = df.join(src_rel);
            let tgt_abs = expand_tilde(&tgt_raw, home);

            // lstat, to match how dotter (and the nushell oracle) see a
            // symlinked source: as a single entry, not a directory to walk.
            let is_dir = fs::symlink_metadata(&src_abs).map(|m| m.is_dir()).unwrap_or(false);
            if is_dir {
                let mut files_under = vec![];
                walk_files(&src_abs, &mut files_under);
                for f in files_under {
                    let rel = f.strip_prefix(&src_abs).unwrap_or(&f).to_path_buf();
                    out.push(Mapping {
                        pkg: pkg.clone(),
                        entry: src_rel.clone(),
                        src: f.clone(),
                        tgt: tgt_abs.join(rel),
                        kind: kind.clone(),
                    });
                }
            } else {
                out.push(Mapping { pkg: pkg.clone(), entry: src_rel.clone(), src: src_abs, tgt: tgt_abs, kind });
            }
        }
    }
    out
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        let is_dir = fs::symlink_metadata(&p).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            walk_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(raw)
    }
}

fn display_with_tilde(p: &Path, home: &Path) -> String {
    match p.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn evaluate(m: Mapping, auto_template: bool) -> Row {
    let row = |verdict: Verdict, detail: String| Row {
        pkg: m.pkg.clone(),
        entry: m.entry.clone(),
        src: m.src.clone(),
        tgt: m.tgt.clone(),
        verdict,
        detail,
    };

    // `exists()` follows symlinks: a source that is itself a link is fine as
    // long as it resolves.
    if !m.src.exists() {
        return row(Verdict::SrcMissing, String::new());
    }

    let kind = match &m.kind {
        Kind::Auto => {
            if auto_template && is_template(&m.src) {
                Kind::Template
            } else {
                Kind::Symbolic
            }
        }
        k => k.clone(),
    };

    let meta = fs::symlink_metadata(&m.tgt);

    if kind == Kind::Template {
        return match meta {
            Err(_) => row(Verdict::TgtMissing, " [template]".into()),
            Ok(md) if md.is_file() => row(Verdict::Ok, " [template]".into()),
            Ok(_) => row(Verdict::NotFile, " [template]".into()),
        };
    }

    match meta {
        Err(_) => row(Verdict::TgtMissing, String::new()),
        Ok(md) if !md.file_type().is_symlink() => {
            let what = if md.is_dir() { "dir" } else { "file" };
            row(Verdict::NotSymlink, format!(" [{what}]"))
        }
        Ok(_) => {
            let link = match fs::read_link(&m.tgt) {
                Ok(l) => l,
                Err(e) => return row(Verdict::BrokenLink, format!(" → <unreadable: {e}>")),
            };
            let resolved = if link.is_absolute() {
                link
            } else {
                m.tgt.parent().map(|p| p.join(&link)).unwrap_or(link)
            };
            match fs::canonicalize(&resolved) {
                Err(_) => row(Verdict::BrokenLink, format!(" → {}", resolved.display())),
                Ok(rc) => match fs::canonicalize(&m.src) {
                    Ok(sc) if sc == rc => row(Verdict::Ok, String::new()),
                    _ => row(Verdict::WrongTarget, format!(" → {}", resolved.display())),
                },
            }
        }
    }
}

fn is_template(src: &Path) -> bool {
    match fs::read(src) {
        Ok(bytes) => bytes.windows(HANDLEBARS_OPEN.len()).any(|w| w == HANDLEBARS_OPEN),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Side checks (warnings only; never affect the exit code)
// ---------------------------------------------------------------------------

fn side_checks(df: &Path, home: &Path) {
    if let Ok(out) = Command::new("git").arg("-C").arg(df).args(["status", "--porcelain"]).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if !s.trim().is_empty() {
            println!("⚠️  Uncommitted changes in ~/dotfiles — commit or lose them on next pull");
            print!("{s}");
            notify("Uncommitted changes in ~/dotfiles");
        }
    }
    let helix_check = home.join(".local/bin/helix-config-sync-check");
    if helix_check.exists() {
        if let Ok(out) = Command::new("nu").arg(&helix_check).output() {
            if !out.status.success() {
                println!("⚠️  Helix configs are out of sync!");
                print!("{}", String::from_utf8_lossy(&out.stdout));
            }
        }
    }
}

fn notify(msg: &str) {
    if which("notify-send") {
        let _ = Command::new("notify-send").args(["-u", "critical", "Dotter drift", msg]).status();
    } else if which("osascript") {
        let script = format!("display notification \"{}\" with title \"Dotter drift\"", msg.replace('"', "'"));
        let _ = Command::new("osascript").args(["-e", &script]).status();
    }
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Self-test: the checker must be able to fail
// ---------------------------------------------------------------------------

/// Builds a dotfiles + home fixture containing every drift class.
/// Returns (dotfiles, home).
fn build_fixture(root: &Path) -> Result<(PathBuf, PathBuf)> {
    use std::os::unix::fs::symlink;

    let df = root.join("dotfiles");
    let hm = root.join("home");
    fs::create_dir_all(df.join(".dotter"))?;
    fs::create_dir_all(df.join("hooks"))?;
    fs::create_dir_all(hm.join(".config/hooks"))?;

    fs::write(df.join("good.txt"), "ok")?;
    fs::write(df.join("bad.txt"), "drift")?;
    fs::write(df.join("hooks/h1.sh"), "hook")?;
    fs::write(df.join("gone.txt"), "gone")?;
    fs::write(df.join("wrong.txt"), "wrong")?;
    fs::write(df.join("broken.txt"), "broken")?;
    fs::write(df.join("mac.txt"), "mac")?;
    fs::write(df.join("tpl.conf"), "name = {{ name }}")?;

    let global = r#"[shared.files]
"good.txt" = "~/.config/good.txt"
"bad.txt" = "~/.config/bad.txt"
"hooks" = "~/.config/hooks"
"gone.txt" = "~/.config/gone.txt"
"wrong.txt" = { target = "~/.config/wrong.txt", type = "symbolic" }
"broken.txt" = "~/.config/broken.txt"
"ghost.txt" = "~/.config/ghost.txt"
"tpl.conf" = "~/.config/tpl.conf"
[macos.files]
"mac.txt" = "~/.config/mac.txt"
[settings]
default_target_type = "automatic"
"#;
    fs::write(df.join(".dotter/global.toml"), global)?;
    fs::write(df.join(".dotter/local.toml"), "packages = []\n")?;

    symlink(df.join("good.txt"), hm.join(".config/good.txt"))?;
    fs::copy(df.join("bad.txt"), hm.join(".config/bad.txt"))?;
    symlink(df.join("hooks/h1.sh"), hm.join(".config/hooks/h1.sh"))?;
    symlink(df.join("bad.txt"), hm.join(".config/wrong.txt"))?;
    symlink(root.join("nowhere"), hm.join(".config/broken.txt"))?;
    fs::write(hm.join(".config/tpl.conf"), "rendered")?;

    Ok((df, hm))
}

struct Assertion {
    name: &'static str,
    pass: bool,
}

fn fixture_assertions(root: &Path) -> Result<Vec<Assertion>> {
    let (df, hm) = build_fixture(root)?;

    println!("🧪 self-test fixture with deliberate drift");
    let r = run_check(&df, &hm, true);
    let verdict_of = |entry: &str| r.drift.iter().find(|row| row.entry == entry).map(|row| row.verdict);

    let mut checks = vec![
        Assertion { name: "exit code is 1 on drift", pass: r.exit_code == 1 },
        Assertion { name: "evaluated 8 files (macos excluded)", pass: r.evaluated == 8 },
        Assertion { name: "good symlink is OK (not in drift)", pass: verdict_of("good.txt").is_none() },
        Assertion { name: "dir entry file is OK", pass: verdict_of("hooks").is_none() },
        Assertion { name: "template file is OK", pass: verdict_of("tpl.conf").is_none() },
        Assertion { name: "regular file → NOT-SYMLINK", pass: verdict_of("bad.txt") == Some(Verdict::NotSymlink) },
        Assertion { name: "missing target → TGT-MISSING", pass: verdict_of("gone.txt") == Some(Verdict::TgtMissing) },
        Assertion { name: "wrong link → WRONG-TARGET", pass: verdict_of("wrong.txt") == Some(Verdict::WrongTarget) },
        Assertion { name: "dangling link → BROKEN-LINK", pass: verdict_of("broken.txt") == Some(Verdict::BrokenLink) },
        Assertion { name: "absent source → SRC-MISSING", pass: verdict_of("ghost.txt") == Some(Verdict::SrcMissing) },
    ];

    println!("🧪 self-test fixture with nothing to check");
    fs::write(df.join(".dotter/global.toml"), "[shared.files]\n")?;
    let r2 = run_check(&df, &hm, true);
    checks.push(Assertion { name: "zero mappings → exit 2, never clean", pass: r2.exit_code == 2 });

    Ok(checks)
}

fn run_self_test() -> u8 {
    let root = std::env::temp_dir().join(format!("dotter-drift-selftest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    if let Err(e) = fs::create_dir_all(&root) {
        println!("❌ self-test could not create fixture dir: {e}");
        return 1;
    }
    let result = fixture_assertions(&root);
    let _ = fs::remove_dir_all(&root);

    let checks = match result {
        Ok(c) => c,
        Err(e) => {
            println!("❌ self-test could not build fixture: {e:#}");
            return 1;
        }
    };
    for c in &checks {
        println!("{} {}", if c.pass { "✅" } else { "❌" }, c.name);
    }
    let failed = checks.iter().filter(|c| !c.pass).count();
    if failed == 0 {
        println!("✅ self-test passed: the checker can fail ({} assertions)", checks.len());
        0
    } else {
        println!("❌ self-test FAILED: {failed} of {} assertions", checks.len());
        1
    }
}

// ---------------------------------------------------------------------------
// cargo test — the same fixture, asserted the Rust way
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dotter-drift-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn fixture_reports_every_drift_class_and_the_empty_config() {
        let root = temp_root("classes");
        let checks = fixture_assertions(&root).unwrap();
        let _ = fs::remove_dir_all(&root);
        let failed: Vec<&str> = checks.iter().filter(|c| !c.pass).map(|c| c.name).collect();
        assert!(failed.is_empty(), "failed assertions: {failed:?}");
        assert_eq!(checks.len(), 11);
    }

    #[test]
    fn resolve_packages_follows_depends_and_ignores_default() {
        let config: toml::Table = r#"
[default]
depends = ["macos"]
[shared]
[shared.files]
"a" = "~/a"
[macos]
depends = ["shared", "extra"]
[macos.files]
"b" = "~/b"
[extra]
depends = ["default"]
[extra.files]
"c" = "~/c"
[nofiles]
depends = []
"#
        .parse()
        .unwrap();
        let pkgs = resolve_packages(&config, &["macos".to_string(), "nofiles".to_string()]);
        assert_eq!(pkgs, vec!["shared", "macos", "extra"]);
    }

    #[test]
    fn missing_config_is_exit_2() {
        let root = temp_root("noconfig");
        let r = run_check(&root.join("dotfiles"), &root.join("home"), true);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(r.exit_code, 2);
        assert_eq!(r.evaluated, 0);
    }

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/h");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/h"));
        assert_eq!(expand_tilde("~/x/y", home), PathBuf::from("/h/x/y"));
        assert_eq!(expand_tilde("/abs", home), PathBuf::from("/abs"));
    }

    #[test]
    fn symlinked_source_resolves_ok() {
        use std::os::unix::fs::symlink;
        let root = temp_root("srclink");
        let df = root.join("dotfiles");
        let hm = root.join("home");
        fs::create_dir_all(df.join(".dotter")).unwrap();
        fs::create_dir_all(&hm).unwrap();
        fs::write(df.join("real"), "x").unwrap();
        symlink(df.join("real"), df.join("alias")).unwrap();
        // dotter links the deployed path to the RESOLVED source
        symlink(df.join("real"), hm.join("alias")).unwrap();
        fs::write(df.join(".dotter/global.toml"), "[shared.files]\n\"alias\" = \"~/alias\"\n").unwrap();
        let r = run_check(&df, &hm, true);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(r.exit_code, 0, "drift: {:?}", r.drift);
    }
}
