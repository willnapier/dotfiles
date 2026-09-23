use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use tempfile::TempDir;
use wiki_link_service::{
    audit, backlinks,
    daypage::{apply_entries, Store},
    logger::Logger,
    reconcile, resolve,
    wiki::{Ctx, Index},
};

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    store: Store,
    page: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("Forge");
        let pages = root.join("NapierianLogs/DayPages");
        fs::create_dir_all(&pages).unwrap();
        let page = pages.join("2026-09-23.md");
        fs::write(&page, "# DayPage\n\nOriginal entry\n").unwrap();
        let store = Store {
            pages,
            pending: temp.path().join("pending"),
            recovery: temp.path().join("recovery"),
        };
        Self {
            _temp: temp,
            root,
            store,
            page,
        }
    }
    fn queued(&self) -> String {
        fs::read_to_string(self.store.pending.join("2026-09-23.md")).unwrap_or_default()
    }
    fn import(&self, buffer: &str) -> String {
        self.store.import(&[self.root.clone()], &self.page, buffer).unwrap()
    }
}

#[test]
fn failed_save_keeps_queue_and_preserves_buffer_and_disk_independently() {
    let f = Fixture::new();
    f.store.queue("2026-09-23", "dev:: queued work").unwrap();
    let disk = fs::read_to_string(&f.page).unwrap();
    let buffer = "# DayPage\n\nUnsaved local draft\n";
    let merged = f.import(buffer);
    assert!(merged.contains("Unsaved local draft"));
    assert!(merged.contains("dev:: queued work"));
    assert_eq!(fs::read_to_string(&f.page).unwrap(), disk);
    assert_eq!(f.store.acknowledge().unwrap(), 1); // never treat stdout as a save
    assert!(f.queued().contains("dev:: queued work"));
    let recovery = fs::read_dir(&f.store.recovery).unwrap().next().unwrap().unwrap().path();
    assert_eq!(fs::read_to_string(recovery.join("buffer.md")).unwrap(), buffer);
    assert_eq!(fs::read_to_string(recovery.join("disk.md")).unwrap(), disk);
    assert_eq!(fs::metadata(&recovery).unwrap().permissions().mode() & 0o777, 0o700);
    for name in ["buffer.md", "disk.md", "queue.txt", "source.txt", "imported.md"] {
        assert_eq!(fs::metadata(recovery.join(name)).unwrap().permissions().mode() & 0o777, 0o600);
    }
    // An actual successful save, followed by acknowledgement, consumes the queue.
    fs::write(&f.page, &merged).unwrap();
    assert_eq!(f.store.acknowledge().unwrap(), 0);
    assert!(f.queued().is_empty());
}

#[test]
fn repeated_import_and_marker_updates_do_not_duplicate_entries() {
    let f = Fixture::new();
    f.store.queue("2026-09-23", "read:: [[Missing Target]]").unwrap();
    let first = f.import(&fs::read_to_string(&f.page).unwrap());
    assert!(first.contains("?[[Missing Target]]"));
    let second = f.import(&first);
    assert_eq!(first, second);
    fs::write(&f.page, first).unwrap();
    assert_eq!(f.store.acknowledge().unwrap(), 0);
}

#[test]
fn stale_buffer_import_does_not_omit_an_entry_already_saved_elsewhere() {
    let f = Fixture::new();
    f.store.queue("2026-09-23", "dev:: remote entry").unwrap();
    fs::write(&f.page, "# DayPage\n\ndev:: remote entry\n").unwrap();
    let merged = f.import("# DayPage\n\nLocal draft\n");
    assert!(merged.contains("Local draft"));
    assert!(merged.contains("dev:: remote entry"));
    assert!(f.queued().is_empty());
    assert!(!fs::read_to_string(&f.page).unwrap().contains("Local draft"));
}

#[test]
fn completion_matching_is_exact_and_missing_targets_remain_queued() {
    let f = Fixture::new();
    f.store.queue("2026-09-23", "DONE:TASK1").unwrap();
    f.store.queue("2026-09-23", "DONE:MISSING").unwrap();
    let merged = f.import("# DayPage\n\n- [ ] TASK10 other\n- [ ] TASK1 intended\n");
    assert!(merged.contains("- [ ] TASK10 other"));
    assert!(merged.contains("- [x] TASK1 intended"));
    fs::write(&f.page, merged).unwrap();
    assert_eq!(f.store.acknowledge().unwrap(), 1);
    assert_eq!(f.queued(), "DONE:MISSING\n");
}

#[test]
fn concurrent_producers_and_acknowledgement_lose_no_entries() {
    let f = Fixture::new();
    std::thread::scope(|scope| {
        for i in 0..12 {
            let store = &f.store;
            scope.spawn(move || {
                store.queue("2026-09-23", &format!("dev:: worker {i}")).unwrap();
                store.acknowledge().unwrap();
            });
        }
    });
    assert_eq!(f.queued().lines().count(), 12);
    for i in 0..12 {
        assert!(f.queued().lines().any(|l| l == format!("dev:: worker {i}")));
    }
}

#[test]
fn invalid_dates_paths_and_unreadable_queue_fail_without_changing_page() {
    let f = Fixture::new();
    assert!(f.store.queue("../../escape", "entry").is_err());
    assert!(f.store.queue("2026-02-30", "entry").is_err());
    assert!(f.store.queue("2026-09-23", "entry\nsecond").is_err());
    let outside = f.root.join("Other.md");
    fs::write(&outside, "other").unwrap();
    assert!(f.store.import(&[f.root.clone()], &outside, "draft").is_err());
    fs::create_dir_all(&f.store.pending).unwrap();
    fs::create_dir(f.store.pending.join("2026-09-23.md")).unwrap();
    let before = fs::read(&f.page).unwrap();
    assert!(f.store.import(&[f.root.clone()], &f.page, "draft").is_err());
    assert_eq!(fs::read(&f.page).unwrap(), before);
}

#[test]
fn watchers_leave_daypage_bytes_alone_but_still_update_ordinary_notes() {
    let f = Fixture::new();
    fs::write(&f.page, "# DayPage\n\n[[Missing Target]]\n").unwrap();
    let other = f.root.join("Other.md");
    fs::write(&other, "# Other\n\n[[2026-09-23]] [[Missing Target]]\n").unwrap();
    let ctx = Ctx::new(vec![f.root.clone()], Logger::silent());
    let before = fs::read(&f.page).unwrap();
    resolve::handle_change(&ctx, "Write", &f.page, None);
    backlinks::handle_change(&ctx, "Write", &other, None);
    assert_eq!(fs::read(&f.page).unwrap(), before);
    assert_eq!(resolve::handle_change(&ctx, "Write", &other, None).actions, 1);
    assert!(fs::read_to_string(other).unwrap().contains("?[[Missing Target]]"));
    let imported = f.import(std::str::from_utf8(&before).unwrap());
    assert!(imported.contains("?[[Missing Target]]"));
    assert!(imported.contains("## Backlinks"));
    assert!(imported.contains("[[Other]]"));
    assert_eq!(fs::read(&f.page).unwrap(), before);
    let report = audit::audit(&[f.root.clone()]);
    assert_eq!(report.daypages_deferred, 1);
    assert!(!report.sections.iter().any(|s| s.path == f.page));
    reconcile::reconcile(&[f.root.clone()], true).unwrap();
    assert_eq!(fs::read(&f.page).unwrap(), before);
    // DayPages still contribute outgoing links to the rest of the graph.
    assert!(Index::build(&[f.root]).position(&f.page).is_some());
}

#[test]
fn queue_insertion_does_not_confuse_inline_heading_text() {
    let original = "# DayPage\n\nMention ## Backlinks in prose\n\n## Backlinks\n\n- [[Other]]\n";
    let result = apply_entries(original, "dev:: first\ndev:: first\ndev:: second\n");
    assert!(result.contains("Mention ## Backlinks in prose"));
    assert_eq!(result.matches("dev:: first").count(), 1);
    assert!(result.find("dev:: second").unwrap() < result.find("\n## Backlinks\n").unwrap());
}
