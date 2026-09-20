//! Synthetic mailbox only. Proves live tag semantics and opt-in boundaries.
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
fn run(home: &Path, tool: &str, args: &[&str]) -> Output {
    Command::new(tool)
        .args(args)
        .env("HOME", home)
        .env("NOTMUCH_CONFIG", home.join("Mail/.notmuch-config"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                home.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .env("MAILCURATOR_FORCE", "1")
        .output()
        .unwrap()
}
fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
fn overlapping_trash_cannot_destroy_evidence_even_only_now() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    for sub in [
        "Mail/cur",
        "Mail/new",
        "Mail/tmp",
        ".config/mailcurator",
        "bin",
    ] {
        fs::create_dir_all(home.join(sub)).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(
            home.join("bin/claude"),
            "#!/bin/sh\ntouch \"$HOME/llm-invoked\"\nprintf '{}\\n'\n",
        )
        .unwrap();
        fs::set_permissions(home.join("bin/claude"), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(home.join("Mail/.notmuch-config"), format!("[database]\npath={}\n[user]\nname=Fixture\nprimary_email=fixture@example.org\n[new]\ntags=inbox;unread;\n[maildir]\nsynchronize_flags=false\n", home.join("Mail").display())).unwrap();
    for (id, sender) in [("bill", "vendor"), ("noise", "noise")] {
        fs::write(home.join(format!("Mail/cur/{id}")), format!("From: {sender}@example.org\nTo: fixture@example.org\nSubject: Fixture\nMessage-ID: <{id}@example.org>\nDate: Sat, 19 Sep 2026 00:00:00 +0000\nContent-Type: text/html\n\n<html>No amount or date here.</html>\n")).unwrap();
    }
    fs::write(
        home.join(".config/mailcurator/policies.toml"),
        r#"
allow_automatic_trash = true
[[policy]]
name = "noise"
subject_contains = "Fixture"
delete_after_days = 1
[[policy]]
name = "bill"
from = "vendor@example.org"
archive_after_days = 1
delete_after_days = 1
vendor_module = "generic_hotel"
[[policy.extractor]]
category = "bills"
[[policy.extractor.field]]
name = "vendor"
literal = "Fixture"
"#,
    )
    .unwrap();
    ok(run(home, "notmuch", &["new"]));
    let binary = env!("CARGO_BIN_EXE_mailcurator");
    let dry = ok(run(
        home,
        binary,
        &["run", "--dry-run", "--now", "--only", "noise"],
    ));
    assert!(dry.contains("trashed=1"), "{dry}");
    assert_eq!(
        ok(run(home, "notmuch", &["count", "tag:curator-retain"])).trim(),
        "0"
    );
    ok(run(home, binary, &["run", "--now", "--only", "noise"]));
    assert_eq!(
        ok(run(
            home,
            "notmuch",
            &["count", "id:bill@example.org and tag:trash"]
        ))
        .trim(),
        "0"
    );
    assert_eq!(
        ok(run(
            home,
            "notmuch",
            &["count", "id:noise@example.org and tag:trash"]
        ))
        .trim(),
        "1"
    );
    // Test a vendor whose incomplete fields normally invite the LLM fallback.
    let out = ok(run(home, binary, &["run", "--now"]));
    assert!(!out.contains("llm-calls="));
    assert!(
        !home.join("llm-invoked").exists(),
        "default run must not invoke Claude"
    );
    assert_eq!(
        ok(run(
            home,
            "notmuch",
            &[
                "count",
                "id:bill@example.org and tag:curator-retain and tag:inbox and not tag:trash"
            ]
        ))
        .trim(),
        "1"
    );
    let evidence = ok(run(home, binary, &["evidence", "--json"]));
    let v: serde_json::Value = serde_json::from_str(&evidence).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["exceptions"], 1);
    assert_eq!(v["rows"][0]["account"], "personal");
    // Positive control: the fixture genuinely exercises the fallback when authorised.
    ok(run(
        home,
        "notmuch",
        &[
            "tag",
            "-curator-bill-extracted",
            "--",
            "id:bill@example.org",
        ],
    ));
    ok(run(home, binary, &["--allow-llm", "run", "--now"]));
    assert!(home.join("llm-invoked").exists());
}
#[test]
fn llm_opt_in_rejects_cohs_before_any_work() {
    let home = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mailcurator"))
        .args(["--allow-llm", "run"])
        .env("HOME", home.path())
        .env(
            "NOTMUCH_CONFIG",
            home.path().join("Mail/.notmuch-cohs-config"),
        )
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("CoHS and custom indexes are not eligible")
    );
}

#[test]
fn on_arrival_cannot_bypass_retention() {
    let home = tempfile::tempdir().unwrap();
    let cfg = home.path().join("policies.toml");
    for action in ["tags_add=['trash']", "tags_remove=['curator-retain']", "tags_remove=['inbox']", "tags_remove=['receipts']", "tags_remove=['billing']", "tags_remove=['Expenses']"] {
        fs::write(&cfg, format!("[[policy]]\nname='unsafe'\nfrom='fixture@example.org'\n[policy.on_arrival]\n{action}\n")).unwrap();
        let out = run(home.path(), env!("CARGO_BIN_EXE_mailcurator"), &["--config", cfg.to_str().unwrap(), "validate"]);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("cannot bypass evidence retention"));
    }
}
