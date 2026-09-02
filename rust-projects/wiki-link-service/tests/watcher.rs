//! End-to-end: a real notify watcher on a fixture tree in a tempdir, a file
//! created after startup, the backlink appearing, the heartbeat advancing.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use wiki_link_service::heartbeat::Heartbeat;
use wiki_link_service::logger::Logger;
use wiki_link_service::watch::{self, Which};
use wiki_link_service::wiki::Ctx;

fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < Duration::from_secs(30), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn watcher_handles_create_and_writes_heartbeat() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let forge = home.join("Forge");
    fs::create_dir_all(&forge).unwrap();
    fs::write(forge.join("A.md"), "# A\n\nbody text\n").unwrap();
    let state = home.join("state");

    let ctx = Ctx { roots: vec![forge.clone()], marker: home.join("marker"), logger: Logger { file: Some(home.join("log")), tag: None, quiet: true } };
    let state_dir = state.clone();
    std::thread::spawn(move || {
        let mut hb = Heartbeat::new(&state_dir, "backlinks");
        let _ = watch::run(Which::Backlinks, &ctx, 300, &mut hb);
    });
    let hb_path = state.join("wiki-link-service-backlinks.json");
    wait_for("startup heartbeat", || hb_path.exists());
    wait_for("monitoring banner", || fs::read_to_string(home.join("log")).map(|s| s.contains("🔍 Monitoring Forge for file events...")).unwrap_or(false));
    std::thread::sleep(Duration::from_millis(1500));

    fs::write(forge.join("B.md"), "# B\n\nlinks [[A]]\n").unwrap();
    wait_for("backlink in A.md", || fs::read_to_string(forge.join("A.md")).map(|s| s.contains("## Backlinks")).unwrap_or(false));
    assert_eq!(fs::read_to_string(forge.join("A.md")).unwrap(), "# A\n\nbody text\n\n\n## Backlinks\n\n- [[B]]\n");
    wait_for("heartbeat action", || fs::read_to_string(&hb_path).map(|s| s.contains("\"actions\":1")).unwrap_or(false));
    let hb = fs::read_to_string(&hb_path).unwrap();
    assert!(hb.contains("\"watcher\":\"wiki-link-service-backlinks\""), "{hb}");
    assert!(hb.contains("\"last_error\":null"), "{hb}");
    assert!(!hb.contains("\"last_action\":null"), "{hb}");
    assert!(Path::new(&home.join("marker")).exists());
}
