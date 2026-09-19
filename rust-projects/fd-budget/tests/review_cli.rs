//! End-to-end review safety checks. All financial data is synthetic and the
//! subprocess HOME/store/profile are isolated; no live bank data is touched.
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};
static SEQ: AtomicUsize = AtomicUsize::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "fd-review-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(root.join("store")).unwrap();
        fs::write(root.join("review.csv"),concat!(
            "date,account,amount,description,merchant,original_tags,analysis_tags,group,category,reason,evidence,import_id,allocated_personal_gbp,allocated_business_gbp,identified_product,receipt_message_id,receipt_date,receipt_bank_date_delta_days,allocated_joint_gbp,fd_budget_import_id,related_refund_import_id\n",
            "2026-01-02,current,-10.00,SHOP,Shop,shopping,shopping,Review,Shopping,,,r1,,,,,,,,b1,\n",
            "2026-01-03,current,-3.01,MIXED,Mixed,shopping,allocation-split,Split personal / business,Dropbox,,,r2,1.51,1.50,,,,,,b2,\n",
            "2026-01-04,current,5.00,REFUND,Refund,,,Credits,Matched purchase refund,,,r3,,,,,,,,b3,r4\n",
            "2026-01-04,current,-5.00,RETURNED,Returned,,,Outside living costs,Fully refunded purchases,,,r4,,,,,,,,b4,r3\n"
        )).unwrap();
        fs::write(
            root.join("store/transactions.csv"),
            concat!(
                "date,account,amount,description,tags,import_id\n",
                "2026-01-02,current,-10.00,SHOP UNMASKED,shopping,b1\n",
                "2026-01-03,current,-3.01,MIXED,allocation-split,b2\n",
                "2026-01-04,current,5.00,REFUND,refund,b3\n",
                "2026-01-04,current,-5.00,RETURNED,refunded,b4\n"
            ),
        )
        .unwrap();
        for f in [
            "rules.toml",
            "paypal.csv",
            "paypal_matches.jsonl",
            "matches.jsonl",
            "budgets.toml",
            "categories.toml",
            "smoothing.toml",
        ] {
            fs::write(root.join("store").join(f), "unchanged\n").unwrap();
        }
        fs::write(
            root.join("report.md"),
            "# Original report\n\nPreserve this discussion verbatim.\n",
        )
        .unwrap();
        fs::write(
            root.join("agenda.md"),
            "# Accountant\n\n## Accrued since 14 Sep — for the next send\n\nExisting items.\n",
        )
        .unwrap();
        Self(root)
    }
    fn cmd(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_fd-budget"))
            .env("HOME", &self.0)
            .env("REVIEW_TEST_BIN", env!("CARGO_BIN_EXE_fd-budget"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.0.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .arg("review")
            .arg("--profile")
            .arg(self.0.join("profile.json"))
            .args(args)
            .output()
            .unwrap()
    }
    fn init(&self) {
        let out = self.cmd(&[
            "init",
            "--csv",
            self.0.join("review.csv").to_str().unwrap(),
            "--report",
            self.0.join("report.md").to_str().unwrap(),
            "--agenda",
            self.0.join("agenda.md").to_str().unwrap(),
            "--store",
            self.0.join("store").to_str().unwrap(),
            "--apply",
        ]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn read(&self, name: &str) -> Vec<u8> {
        fs::read(self.0.join(name)).unwrap()
    }
    fn set(&self, extra: &[&str]) -> Output {
        let mut args = vec![
            "set",
            "r1",
            "--allocation",
            "business",
            "--category",
            "Research",
            "--reason",
            "Confirmed professional reading",
            "--tags",
            "research",
        ];
        args.extend(extra);
        self.cmd(&args)
    }
}

#[cfg(unix)]
#[test]
fn sync_failure_is_resumable_and_peer_divergence_blocks_writes() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = Fixture::new();
    f.init();
    fs::create_dir_all(f.0.join(".config")).unwrap();
    fs::remove_dir(f.0.join(".config/fd-budget")).unwrap();
    symlink(f.0.join("store"), f.0.join(".config/fd-budget")).unwrap();
    fs::create_dir(f.0.join("peer-store")).unwrap();
    for entry in fs::read_dir(f.0.join("store")).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), f.0.join("peer-store").join(entry.file_name())).unwrap();
    }
    fs::create_dir(f.0.join("bin")).unwrap();
    for (name, script) in [
        ("ssh", "#!/bin/sh\nexec \"$REVIEW_TEST_BIN\" review fingerprint --store \"$HOME/peer-store\"\n"),
        ("fd-budget-sync", "#!/bin/sh\nif test -e \"$HOME/fail-sync\"; then exit 42; fi\nfor file in \"$HOME/store/\"*; do cp \"$file\" \"$HOME/peer-store/\" || exit 1; done\n"),
    ] {
        let path = f.0.join("bin").join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut profile: Value = serde_json::from_slice(&f.read("profile.json")).unwrap();
    profile["peer"] = if cfg!(target_os = "macos") {
        "will@nimbini"
    } else {
        "mac"
    }
    .into();
    fs::write(
        f.0.join("profile.json"),
        serde_json::to_vec(&profile).unwrap(),
    )
    .unwrap();
    let bank = f.read("store/transactions.csv");
    fs::write(f.0.join("peer-store/rules.toml"), "independent edit").unwrap();
    assert!(!f.set(&["--apply"]).status.success());
    assert_eq!(bank, f.read("store/transactions.csv"));
    fs::copy(
        f.0.join("store/rules.toml"),
        f.0.join("peer-store/rules.toml"),
    )
    .unwrap();
    fs::write(f.0.join("fail-sync"), "").unwrap();
    let failed = f.set(&["--apply"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("local decision saved"));
    assert_ne!(bank, f.read("store/transactions.csv"));
    let saved = f.read("review.csv");
    assert!(!f.set(&["--correct", "--apply"]).status.success());
    assert_eq!(saved, f.read("review.csv"));
    fs::remove_file(f.0.join("fail-sync")).unwrap();
    let resumed = f.cmd(&["sync"]);
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    assert_eq!(
        f.read("store/transactions.csv"),
        f.read("peer-store/transactions.csv")
    );
    let profile: Value = serde_json::from_slice(&f.read("profile.json")).unwrap();
    assert!(profile["pending_sync"].is_null());
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn preview_apply_and_correction_preserve_bank_and_history() {
    let f = Fixture::new();
    f.init();
    let csv = f.read("review.csv");
    let report = f.read("report.md");
    let bank = f.read("store/transactions.csv");
    let preview = f.set(&[]);
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(csv, f.read("review.csv"));
    assert_eq!(report, f.read("report.md"));
    assert_eq!(bank, f.read("store/transactions.csv"));
    let applied = f.set(&["--apply"]);
    assert!(
        applied.status.success(),
        "{}",
        String::from_utf8_lossy(&applied.stderr)
    );
    let json: Value = serde_json::from_slice(&applied.stdout).unwrap();
    assert_eq!(json["summary"]["allocations"]["Business"], "11.50");
    assert_eq!(json["summary"]["allocations"]["Will personal"], "1.51");
    assert_eq!(json["summary"]["debits"], "18.01");
    let backup = PathBuf::from(json["backup"].as_str().unwrap());
    assert_eq!(
        fs::read(backup.join("store/transactions.csv")).unwrap(),
        bank
    );
    assert!(String::from_utf8(f.read("report.md"))
        .unwrap()
        .ends_with("# Original report\n\nPreserve this discussion verbatim.\n"));
    let after = f.read("store/transactions.csv");
    assert_eq!(
        String::from_utf8(after).unwrap(),
        String::from_utf8(bank)
            .unwrap()
            .replace("shopping,b1", "shopping,research,business,b1")
            .replace(
                "shopping,research,business,b1",
                "shopping|research|business,b1"
            )
    );
    assert!(String::from_utf8(f.read("agenda.md"))
        .unwrap()
        .contains("Confirmed professional reading"));
    let repeat = f.set(&["--apply"]);
    assert!(!repeat.status.success());
    let correct = f.cmd(&[
        "set",
        "r1",
        "--allocation",
        "personal",
        "--category",
        "Personal books",
        "--reason",
        "User corrected use",
        "--correct",
        "--apply",
    ]);
    assert!(
        correct.status.success(),
        "{}",
        String::from_utf8_lossy(&correct.stderr)
    );
    assert!(!String::from_utf8(f.read("store/transactions.csv"))
        .unwrap()
        .contains("|business"));
    assert!(String::from_utf8(f.read("agenda.md"))
        .unwrap()
        .contains("User corrected use"));
}

#[test]
fn invalid_split_duplicate_and_bank_mismatch_fail_closed() {
    for fault in ["split", "duplicate", "bank", "money"] {
        let f = Fixture::new();
        if fault == "split" {
            fs::write(
                f.0.join("review.csv"),
                String::from_utf8(f.read("review.csv"))
                    .unwrap()
                    .replace("1.51,1.50", "1.50,1.50"),
            )
            .unwrap();
        }
        if fault == "duplicate" {
            fs::write(
                f.0.join("review.csv"),
                String::from_utf8(f.read("review.csv"))
                    .unwrap()
                    .replace(",r2,", ",r1,"),
            )
            .unwrap();
        }
        if fault == "bank" {
            fs::write(
                f.0.join("store/transactions.csv"),
                String::from_utf8(f.read("store/transactions.csv"))
                    .unwrap()
                    .replace("-10.00", "-11.00"),
            )
            .unwrap();
        }
        if fault == "money" {
            fs::write(
                f.0.join("review.csv"),
                String::from_utf8(f.read("review.csv"))
                    .unwrap()
                    .replace("-10.00", "broken"),
            )
            .unwrap();
        }
        let before = f.read("report.md");
        let out = f.cmd(&[
            "init",
            "--csv",
            f.0.join("review.csv").to_str().unwrap(),
            "--report",
            f.0.join("report.md").to_str().unwrap(),
            "--agenda",
            f.0.join("agenda.md").to_str().unwrap(),
            "--store",
            f.0.join("store").to_str().unwrap(),
            "--apply",
        ]);
        assert!(!out.status.success(), "accepted fault {fault}");
        assert_eq!(before, f.read("report.md"));
        assert!(!f.0.join("profile.json").exists());
    }
}

#[test]
fn receipt_updates_are_allocation_neutral_and_protected_rows_refused() {
    let f = Fixture::new();
    f.init();
    let bank = f.read("store/transactions.csv");
    let out = f.cmd(&[
        "identify",
        "r1",
        "--product",
        "Example book",
        "--receipt-id",
        "receipt-1",
        "--receipt-date",
        "2026-01-01",
        "--evidence",
        "Matching amount and funding",
        "--apply",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(bank, f.read("store/transactions.csv"));
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["summary"]["allocations"]["Review"], "10.00");
    for id in ["r2", "r3", "r4"] {
        let out = f.cmd(&[
            "set",
            id,
            "--allocation",
            "personal",
            "--category",
            "Personal",
            "--reason",
            "No authority",
            "--correct",
            "--apply",
        ]);
        assert!(!out.status.success());
    }
    let out = f.cmd(&[
        "identify",
        "r2",
        "--product",
        "Same book",
        "--receipt-id",
        "<receipt-1>",
        "--receipt-date",
        "2026-01-01",
        "--evidence",
        "Already used",
        "--apply",
    ]);
    assert!(!out.status.success());
    assert_eq!(bank, f.read("store/transactions.csv"));
}

#[test]
fn invalid_batch_does_not_apply_earlier_valid_item() {
    let f = Fixture::new();
    f.init();
    let names = [
        "review.csv",
        "report.md",
        "agenda.md",
        "store/transactions.csv",
        "profile.json",
    ];
    let before: Vec<_> = names.iter().map(|n| f.read(n)).collect();
    let out = f.cmd(&[
        "set",
        "r1",
        "unknown",
        "--allocation",
        "personal",
        "--category",
        "Food",
        "--reason",
        "Confirmed",
        "--apply",
    ]);
    assert!(!out.status.success());
    for (name, bytes) in names.iter().zip(before) {
        assert_eq!(bytes, f.read(name));
    }
}

#[test]
fn missing_report_marker_or_changed_identity_prevents_all_writes() {
    for identity in [false, true] {
        let f = Fixture::new();
        f.init();
        if identity {
            fs::write(
                f.0.join("review.csv"),
                String::from_utf8(f.read("review.csv"))
                    .unwrap()
                    .replace("SHOP,Shop", "ALTERED,Shop"),
            )
            .unwrap();
        } else {
            fs::write(f.0.join("report.md"), "report replaced by user").unwrap();
        }
        let bank = f.read("store/transactions.csv");
        let csv = f.read("review.csv");
        let agenda = f.read("agenda.md");
        assert!(!f.set(&["--apply"]).status.success());
        assert_eq!(bank, f.read("store/transactions.csv"));
        assert_eq!(csv, f.read("review.csv"));
        assert_eq!(agenda, f.read("agenda.md"));
    }
}
