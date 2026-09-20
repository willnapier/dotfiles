use std::{fs,process::Command};

#[test]
fn six_month_hook_keeps_booking_sources_and_tags() {
    let d=tempfile::tempdir().unwrap();let h=d.path();
    for p in ["Mail/cur","Mail/new","Mail/tmp"] {fs::create_dir_all(h.join(p)).unwrap();}
    fs::write(h.join("Mail/.notmuch-config"),format!("[database]\npath={}\n[user]\nname=Fixture\nprimary_email=fixture@example.org\n[new]\ntags=inbox;unread;\n[maildir]\nsynchronize_flags=false\n",h.join("Mail").display())).unwrap();
    for (id,from) in [("provider","booking.com"),("tagged","custom.test"),("noise","noise.test")] {
        fs::write(h.join(format!("Mail/cur/{id}")),format!("From: fixture@{from}\nTo: fixture@example.org\nSubject: Fixture\nDate: Fri, 01 Jan 2010 00:00:00 +0000\nMessage-ID: <{id}@example.org>\n\nSynthetic mail.\n")).unwrap();
    }
    let run=|bin:&str,args:&[&str]| {
        let o=Command::new(bin).args(args).env("HOME",h).env("NOTMUCH_CONFIG",h.join("Mail/.notmuch-config")).env("MAILFORGE_FAST_REINDEX","1").output().unwrap();
        assert!(o.status.success(),"{}",String::from_utf8_lossy(&o.stderr));String::from_utf8(o.stdout).unwrap()
    };
    run("notmuch",&["new"]);run("notmuch",&["tag","+booking","--","id:tagged@example.org"]);
    // The hook no longer files ANY inbound mail solely because it is old.
    run("notmuch",&["tag","+curator-retain","--","id:noise@example.org"]);
    assert_eq!(run("notmuch",&["count","tag:inbox"]).trim(),"3");
    let hook=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../notmuch/hooks/post-new");
    run("bash",&[hook.to_str().unwrap()]);
    assert_eq!(run("notmuch",&["count","tag:inbox"]).trim(),"3");
    assert_eq!(run("notmuch",&["count","id:noise@example.org and tag:inbox"]).trim(),"1");
}

#[test]
fn default_suspension_and_overlapping_archive_hold() {
    let d=tempfile::tempdir().unwrap();let h=d.path();
    for p in ["Mail/cur","Mail/new","Mail/tmp",".config/mailcurator"] {fs::create_dir_all(h.join(p)).unwrap();}
    fs::write(h.join("Mail/.notmuch-config"),format!("[database]\npath={}\n[user]\nname=Fixture\nprimary_email=fixture@example.org\n[new]\ntags=inbox;unread;\n[maildir]\nsynchronize_flags=false\n",h.join("Mail").display())).unwrap();
    for id in ["booking","noise"] {
        fs::write(h.join(format!("Mail/cur/{id}")),format!("From: {id}@example.org\nTo: fixture@example.org\nSubject: Fixture\nDate: Sat, 19 Sep 2026 00:00:00 +0000\nMessage-ID: <{id}@example.org>\n\nSynthetic mail only.\n")).unwrap();
    }
    let policies=r#"
[[policy]]
name="noise"
subject_contains="Fixture"
archive_after_days=1
delete_after_days=1
[[policy]]
name="booking"
from="booking@example.org"
archive_after_days=1
[[policy.extractor]]
category="bookings"
[[policy.extractor.field]]
name="property"
literal="Fixture"
"#;
    fs::write(h.join(".config/mailcurator/policies.toml"),policies).unwrap();
    let run=|bin: &str,args: &[&str]| {
        let o=Command::new(bin).args(args).env("HOME",h).env("NOTMUCH_CONFIG",h.join("Mail/.notmuch-config")).env("MAILCURATOR_FORCE","1").output().unwrap();
        assert!(o.status.success(),"{}",String::from_utf8_lossy(&o.stderr));String::from_utf8(o.stdout).unwrap()
    };
    run("notmuch", &["new"]);
    let binary=env!("CARGO_BIN_EXE_mailcurator");
    let preview=run(binary,&["run","--dry-run","--only","noise","--now"]);
    assert!(preview.contains("archived=1") && preview.contains("trashed=0"),"{preview}");
    run(binary,&["run","--only","noise","--now"]);
    assert_eq!(run("notmuch",&["count","tag:trash"]).trim(),"0");
    assert_eq!(run("notmuch",&["count","id:booking@example.org and tag:inbox and tag:booking"]).trim(),"1");
    assert_eq!(run("notmuch",&["count","id:noise@example.org and tag:inbox"]).trim(),"0");
    // Red control: explicit opt-in permits eligible noise trash, never bookings.
    fs::write(h.join(".config/mailcurator/policies.toml"),format!("allow_automatic_trash=true\n{policies}")).unwrap();
    run(binary,&["run","--only","noise","--now"]);
    assert_eq!(run("notmuch",&["count","tag:trash"]).trim(),"1");
    assert_eq!(run("notmuch",&["count","id:booking@example.org and tag:trash"]).trim(),"0");
}
