//! Real notmuch with synthetic mail only; no live accounts or external services.
use chrono::{Duration, Utc};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        for p in ["Mail/cur", "Mail/new", "Mail/tmp", ".config/mailcurator"] {
            fs::create_dir_all(home.join(p)).unwrap();
        }
        fs::write(home.join("Mail/.notmuch-config"), format!("[database]\npath={}\n[user]\nname=Fixture\nprimary_email=fixture@example.org\n[new]\ntags=inbox;unread;\n[maildir]\nsynchronize_flags=false\n", home.join("Mail").display())).unwrap();
        Self { _temp: temp, home }
    }
    fn config(&self, destination: bool, extra: &str) {
        let destination = if destination {
            "[verified_delivery]\nroot='Forge/Householding/Bookings'\nwriter='fixture-writer'\nnot_before='2020-01-01T00:00:00Z'\n"
        } else {
            ""
        };
        let text = format!(
            r#"{destination}
[[policy]]
name='noise'
subject_contains='Fixture'
archive_after_days=0
[[policy]]
name='booking'
from='hotel@example.org'
archive_after_days=0
[[policy.extractor]]
category='bookings'
[[policy.extractor.field]]
name='property'
body_regex='Property: ([^\r\n]+)'
[[policy.extractor.field]]
name='booking_ref'
body_regex='Reference: ([^\r\n]+)'
[[policy.extractor.field]]
name='checkin'
body_regex='Checkin: ([^\r\n]+)'
kind='date'
[[policy.extractor.field]]
name='checkout'
body_regex='Checkout: ([^\r\n]+)'
kind='date'
{extra}
"#
        );
        fs::write(self.home.join(".config/mailcurator/policies.toml"), text).unwrap();
    }
    fn mail(&self, name: &str, from: &str, subject: &str, body: &str) {
        let date = (Utc::now() - Duration::minutes(1)).to_rfc2822();
        fs::write(self.home.join(format!("Mail/cur/{name}")),format!("From: {from}\nTo: fixture@example.org\nSubject: {subject}\nDate: {date}\nMessage-ID: <{name}@example.org>\nContent-Type: text/plain\n\n{body}\n")).unwrap();
    }
    fn complete(&self) {
        let checkin = (Utc::now() + Duration::days(30)).date_naive();
        let checkout = checkin + Duration::days(2);
        self.mail("good", "hotel@example.org", "Fixture confirmation", &format!("Property: Example Inn\nReference: TEST123\nCheckin: {checkin}\nCheckout: {checkout}"));
    }
    fn invoke(&self, tool: &str, args: &[&str]) -> Output {
        Command::new(tool)
            .args(args)
            .env("HOME", &self.home)
            .env("HOSTNAME", "fixture-writer")
            .env("NOTMUCH_CONFIG", self.home.join("Mail/.notmuch-config"))
            .env("MAILCURATOR_FORCE", "1")
            .output()
            .unwrap()
    }
    fn ok(&self, tool: &str, args: &[&str]) -> String {
        let out = self.invoke(tool, args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }
    fn mc(&self, args: &[&str]) -> String {
        self.ok(
            &std::env::var("DELIVERY_TEST_BIN")
                .unwrap_or_else(|_| env!("CARGO_BIN_EXE_mailcurator").into()),
            args,
        )
    }
    fn inbox(&self, name: &str) -> bool {
        self.ok(
            "notmuch",
            &["count", &format!("id:{name}@example.org and tag:inbox")],
        )
        .trim()
            == "1"
    }
    fn documents(&self) -> Vec<PathBuf> {
        fs::read_dir(self.home.join("Forge/Householding/Bookings"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .collect()
    }
    fn index(&self) {
        self.ok("notmuch", &["new"]);
    }
}

#[test]
fn only_verified_delivery_releases_mail_and_reruns_are_idempotent() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    f.mail(
        "incomplete",
        "hotel@example.org",
        "Fixture incomplete",
        "Property: Elsewhere",
    );
    f.mail(
        "routine",
        "noise@example.org",
        "Fixture bulletin",
        "Routine news",
    );
    f.index();
    f.mc(&["run", "--dry-run", "--now"]);
    assert!(f.documents().is_empty());
    assert!(f.inbox("good"));
    assert!(
        !f.home
            .join(".local/share/mailcurator/delivery-receipts")
            .exists()
    );
    f.mc(&["run", "--now"]);
    assert!(!f.inbox("good"));
    assert!(f.inbox("incomplete"));
    assert!(!f.inbox("routine"));
    let docs = f.documents();
    assert_eq!(docs.len(), 1);
    let before = fs::read(&docs[0]).unwrap();
    let text = String::from_utf8(before.clone()).unwrap();
    assert!(text.contains("personal%3Agood%40example.org"));
    assert!(text.contains("Example Inn"));
    f.mc(&["run", "--now"]);
    assert_eq!(f.documents(), docs);
    assert_eq!(fs::read(&docs[0]).unwrap(), before);
    let report: serde_json::Value =
        serde_json::from_str(&f.mc(&["delivery-status", "--json"])).unwrap();
    assert_eq!(
        report["verified"], 1,
        "archived deliveries remain monitored"
    );
    let ledger = f
        .home
        .join(".local/share/mailcurator/bookings.fixture-writer.jsonl");
    assert_eq!(
        fs::read_to_string(ledger).unwrap().lines().count(),
        2,
        "retry must not append duplicates"
    );
}

#[test]
fn overlap_only_now_and_forged_tags_cannot_bypass_missing_delivery() {
    let f = Fixture::new();
    f.config(false, "");
    f.complete();
    f.index();
    f.ok(
        "notmuch",
        &[
            "tag",
            "+curator-booking-extracted",
            "+curator-delivery-verified",
            "--",
            "*",
        ],
    );
    f.mc(&["run", "--now", "--only", "noise"]);
    assert!(f.inbox("good"));
    let report: serde_json::Value =
        serde_json::from_str(&f.mc(&["delivery-status", "--json"])).unwrap();
    assert_eq!(report["total"], 1);
    assert_eq!(report["verified"], 0);
}

#[test]
fn destination_edits_removal_and_source_changes_invalidate_receipts() {
    for change in ["edit", "remove", "source"] {
        let f = Fixture::new();
        f.config(true, "");
        f.complete();
        f.index();
        f.mc(&["run", "--now"]);
        assert!(!f.inbox("good"));
        let path = f.documents().remove(0);
        f.ok("notmuch", &["tag", "+inbox", "--", "*"]);
        match change {
            "edit" => fs::write(&path, "Human edit").unwrap(),
            "remove" => fs::remove_file(&path).unwrap(),
            _ => {
                let path = f.home.join("Mail/cur/good");
                let mut s = fs::read_to_string(&path).unwrap();
                s.push_str("Changed source\n");
                fs::write(path, s).unwrap();
            }
        }
        f.mc(&["run", "--only", "noise", "--now"]);
        assert!(f.inbox("good"));
        f.ok("notmuch", &["tag", "-curator-booking-extracted", "--", "*"]);
        f.mc(&["run", "--now"]);
        assert!(f.inbox("good"));
        if change == "remove" {
            assert!(!path.exists(), "no resurrection");
        }
        if change == "edit" {
            assert_eq!(fs::read_to_string(path).unwrap(), "Human edit");
        }
    }
}

#[test]
fn financial_claim_and_unknown_booking_cues_hold_against_noise() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    f.mail(
        "unknown",
        "guesthouse@example.org",
        "Fixture booking confirmation",
        "Unrecognised provider",
    );
    f.index();
    f.ok("notmuch", &["tag", "+billing", "--", "id:good@example.org"]);
    f.mc(&["run", "--now"]);
    assert!(f.inbox("good"));
    assert!(f.inbox("unknown"));
    assert_eq!(
        f.documents().len(),
        1,
        "booking delivery does not satisfy finance claim"
    );
}

#[test]
fn overlapping_information_policy_requires_every_destination() {
    let f = Fixture::new();
    f.config(true, "[[policy]]\nname='finance'\nfrom='hotel@example.org'\n[[policy.extractor]]\ncategory='bills'\n");
    f.complete();
    f.index();
    f.mc(&["run", "--now"]);
    assert!(f.inbox("good"));
}

#[test]
fn cancellation_attachment_and_inferred_dates_never_release() {
    for variant in ["cancel", "attachment", "yearless"] {
        let f = Fixture::new();
        f.config(true, "");
        f.complete();
        let path = f.home.join("Mail/cur/good");
        let text = fs::read_to_string(&path).unwrap();
        let text = match variant {
            "cancel" => text.replace("Fixture confirmation", "Fixture cancellation"),
            "attachment" => text.replace("Content-Type: text/plain", "Content-Type: application/pdf\nContent-Disposition: attachment; filename=booking.pdf"),
            _ => { let date = (Utc::now() + Duration::days(30)).date_naive(); text.replace(&date.to_string(), &date.format("%d %B").to_string()) }
        };
        fs::write(path, text).unwrap();
        f.index();
        f.mc(&["run", "--now"]);
        assert!(f.inbox("good"), "{variant}");
        assert!(f.documents().is_empty());
    }
}

#[test]
fn crash_before_receipt_adopts_identical_file_without_duplicate() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    f.index();
    f.mc(&["run", "--now"]);
    let docs = f.documents();
    let bytes = fs::read(&docs[0]).unwrap();
    for e in fs::read_dir(f.home.join(".local/share/mailcurator/delivery-receipts"))
        .unwrap()
        .flatten()
    {
        if e.path().extension().is_some_and(|ext| ext == "json") {
            fs::remove_file(e.path()).unwrap();
        }
    }
    f.ok(
        "notmuch",
        &["tag", "+inbox", "-curator-booking-extracted", "--", "*"],
    );
    f.mc(&["run", "--now"]);
    assert!(!f.inbox("good"));
    assert_eq!(f.documents(), docs);
    assert_eq!(fs::read(&docs[0]).unwrap(), bytes);
}

#[test]
fn hook_without_capability_does_not_age_file_unknown_mail() {
    let f = Fixture::new();
    f.config(false, "");
    f.mail(
        "old",
        "unknown@example.org",
        "Old personal correspondence",
        "Keep",
    );
    let path = f.home.join("Mail/cur/old");
    let raw = fs::read_to_string(&path).unwrap();
    let raw = raw
        .lines()
        .map(|l| {
            if l.starts_with("Date:") {
                "Date: Fri, 01 Jan 2010 00:00:00 +0000"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(path, raw).unwrap();
    f.index();
    let hook = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../notmuch/hooks/post-new");
    let out = Command::new("bash")
        .arg(hook)
        .env("HOME", &f.home)
        .env("NOTMUCH_CONFIG", f.home.join("Mail/.notmuch-config"))
        .env("MAILFORGE_FAST_REINDEX", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(f.inbox("old"));
}

#[test]
fn missing_archived_record_is_reported_without_unarchiving_or_recreating() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    f.index();
    f.mc(&["run", "--now"]);
    fs::remove_file(f.documents().remove(0)).unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&f.mc(&["delivery-status", "--json"])).unwrap();
    assert_eq!(report["total"], 1);
    assert_eq!(report["verified"], 0);
    assert_eq!(report["held"], 1);
    assert!(!f.inbox("good"));
    assert!(f.documents().is_empty());
}

#[test]
fn symlink_destination_and_wrong_writer_fail_closed_and_retry_recovers() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    f.index();
    fs::create_dir_all(f.home.join("Forge/Householding")).unwrap();
    fs::create_dir_all(f.home.join("outside")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        f.home.join("outside"),
        f.home.join("Forge/Householding/Bookings"),
    )
    .unwrap();
    f.mc(&["run", "--now"]);
    assert!(f.inbox("good"));
    assert_eq!(fs::read_dir(f.home.join("outside")).unwrap().count(), 0);
    fs::remove_file(f.home.join("Forge/Householding/Bookings")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mailcurator"))
        .args(["run", "--now"])
        .env("HOME", &f.home)
        .env("HOSTNAME", "other-writer")
        .env("MAILCURATOR_FORCE", "1")
        .env("NOTMUCH_CONFIG", f.home.join("Mail/.notmuch-config"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(f.inbox("good"));
    f.mc(&["run", "--now"]);
    assert!(!f.inbox("good"));
    assert_eq!(f.documents().len(), 1);
}

#[test]
fn a_second_overlapping_reservation_is_held_not_silently_delivered() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    f.index();
    f.mc(&["run", "--now"]);
    let path = f.home.join("Mail/cur/good");
    let second = fs::read_to_string(path)
        .unwrap()
        .replace("good@example.org", "second@example.org")
        .replace("TEST123", "OTHER456")
        .replace("Example Inn", "Other Inn");
    fs::write(f.home.join("Mail/cur/second"), second).unwrap();
    f.index();
    f.mc(&["run", "--now"]);
    assert!(f.inbox("second"));
    assert_eq!(f.documents().len(), 1);
}

#[test]
fn on_arrival_financial_claim_added_after_snapshot_still_blocks_archive() {
    let f = Fixture::new();
    f.config(true, "");
    f.complete();
    let path = f.home.join(".config/mailcurator/policies.toml");
    let config = fs::read_to_string(&path).unwrap().replace(
        "name='booking'",
        "name='booking'\non_arrival.tags_add=['receipts']",
    );
    fs::write(path, config).unwrap();
    f.index();
    f.mc(&["run", "--now"]);
    assert!(
        f.inbox("good"),
        "fresh financial claims must not use the stale pre-arrival snapshot"
    );
}
