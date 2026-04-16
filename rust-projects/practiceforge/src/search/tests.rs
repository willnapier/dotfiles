//! Tests for the search module.
//!
//! Uses tempfile::TempDir for isolation — no real client data touched.

use super::config::SearchConfig;
use super::index::{build_index, build_schema, is_index_stale, update_client_index};
use super::query::{search, search_field, search_within_client};
use tempfile::TempDir;

/// Create a temporary clinical directory with test client data.
fn setup_test_clinical_dir(tmp: &TempDir) -> std::path::PathBuf {
    let root = tmp.path().join("clinical");
    let clients = root.join("clients");

    // Client AB12
    let ab12 = clients.join("AB12");
    std::fs::create_dir_all(&ab12).unwrap();
    std::fs::write(
        ab12.join("identity.yaml"),
        r#"---
name: Alice Brown
dob: "1985-03-15"
status: active
funding:
  type: self-pay
  rate: 150
diagnosis: "Generalised anxiety disorder"
"#,
    )
    .unwrap();
    std::fs::write(
        ab12.join("notes.md"),
        r#"### 2026-01-10

Alice reported increased anxiety this week, particularly around work deadlines.
Explored values-based action in the context of occupational stress.

### 2026-01-17

Follow-up session. Alice practiced defusion exercises between sessions.
Noticed improvement in sleep quality. Discussed acceptance of uncertainty.
"#,
    )
    .unwrap();
    let corr_dir = ab12.join("correspondence");
    std::fs::create_dir_all(&corr_dir).unwrap();
    std::fs::write(
        corr_dir.join("referral-letter.md"),
        "Dear Dr Smith, I am writing to refer Alice Brown for psychological assessment.",
    )
    .unwrap();

    // Client CD34
    let cd34 = clients.join("CD34");
    std::fs::create_dir_all(&cd34).unwrap();
    std::fs::write(
        cd34.join("identity.yaml"),
        r#"---
name: Charlie Davies
dob: "1992-07-22"
status: active
funding:
  type: insurance
  rate: 180
  contact: BUPA
diagnosis: "Major depressive disorder, recurrent"
"#,
    )
    .unwrap();
    std::fs::write(
        cd34.join("notes.md"),
        r#"### 2026-02-01

Charlie presented with low mood and persistent fatigue.
Formulation draws on early attachment experiences.
Explored behavioural activation strategies.

### 2026-02-08

Charlie reported small improvement in activity levels.
Discussed the cognitive fusion maintaining depressive rumination.
"#,
    )
    .unwrap();

    // Client EF56 (discharged, no notes)
    let ef56 = clients.join("EF56");
    std::fs::create_dir_all(&ef56).unwrap();
    std::fs::write(
        ef56.join("identity.yaml"),
        r#"---
name: Eve Foster
status: discharged
funding:
  type: self-pay
"#,
    )
    .unwrap();

    root
}

fn test_config(tmp: &TempDir) -> SearchConfig {
    SearchConfig {
        enabled: true,
        index_path: tmp.path().join("search-index"),
        include_notes: true,
        include_correspondence: true,
    }
}

#[test]
fn test_schema_has_expected_fields() {
    let schema = build_schema();
    assert!(schema.get_field("client_id").is_ok());
    assert!(schema.get_field("name").is_ok());
    assert!(schema.get_field("notes_content").is_ok());
    assert!(schema.get_field("correspondence_content").is_ok());
    assert!(schema.get_field("funding_type").is_ok());
    assert!(schema.get_field("status").is_ok());
    assert!(schema.get_field("diagnosis").is_ok());
}

#[test]
fn test_build_index_and_search() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    // Build the index
    build_index(&config, &clinical_root).unwrap();

    // Search for a term that appears in Alice's notes
    let results = search(&config, "anxiety", 10).unwrap();
    assert!(!results.is_empty(), "Expected results for 'anxiety'");
    assert_eq!(results[0].client_id, "AB12");
    assert_eq!(results[0].name, "Alice Brown");
    assert!(results[0].score > 0.0);
}

#[test]
fn test_search_by_name() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    let results = search(&config, "Charlie", 10).unwrap();
    assert!(!results.is_empty(), "Expected results for 'Charlie'");
    assert_eq!(results[0].client_id, "CD34");
}

#[test]
fn test_search_by_diagnosis() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    let results = search(&config, "depressive", 10).unwrap();
    assert!(!results.is_empty(), "Expected results for 'depressive'");
    assert_eq!(results[0].client_id, "CD34");
}

#[test]
fn test_search_within_client() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    // Search for "defusion" within AB12 (should match)
    let results = search_within_client(&config, "AB12", "defusion").unwrap();
    assert!(!results.is_empty(), "Expected results for 'defusion' within AB12");
    assert_eq!(results[0].client_id, "AB12");

    // Search for "defusion" within CD34 (should not match)
    let results = search_within_client(&config, "CD34", "defusion").unwrap();
    assert!(results.is_empty(), "Expected no results for 'defusion' within CD34");
}

#[test]
fn test_search_correspondence() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    let results = search(&config, "referral assessment", 10).unwrap();
    assert!(!results.is_empty(), "Expected results for 'referral assessment'");
    assert_eq!(results[0].client_id, "AB12");
}

#[test]
fn test_search_no_results() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    let results = search(&config, "xyznonexistent", 10).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_search_field_notes() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    let results = search_field(&config, "behavioural activation", "notes", 10).unwrap();
    assert!(!results.is_empty(), "Expected results for 'behavioural activation' in notes");
    assert_eq!(results[0].client_id, "CD34");
}

#[test]
fn test_index_staleness() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);

    // No index exists — should be stale
    assert!(is_index_stale(&config, std::time::Duration::from_secs(3600)));

    // Build the index
    let clinical_root = setup_test_clinical_dir(&tmp);
    build_index(&config, &clinical_root).unwrap();

    // Just built — should not be stale with a 1-hour threshold
    assert!(!is_index_stale(&config, std::time::Duration::from_secs(3600)));

    // Should be stale with a 0-second threshold
    assert!(is_index_stale(&config, std::time::Duration::ZERO));
}

#[test]
fn test_update_client_index() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let config = test_config(&tmp);

    build_index(&config, &clinical_root).unwrap();

    // Modify Alice's notes
    let notes_path = clinical_root.join("clients").join("AB12").join("notes.md");
    std::fs::write(
        &notes_path,
        r#"### 2026-01-10

Alice reported a breakthrough with the mindfulness practice.
She described feeling grounded for the first time in months.
"#,
    )
    .unwrap();

    // Update just AB12
    update_client_index(&config, "AB12", &clinical_root).unwrap();

    // Old content should not match
    let results = search(&config, "defusion", 10).unwrap();
    assert!(
        results.is_empty() || results.iter().all(|r| r.client_id != "AB12"),
        "Old content should not match after update"
    );

    // New content should match
    let results = search(&config, "mindfulness breakthrough", 10).unwrap();
    assert!(!results.is_empty(), "New content should match after update");
    assert_eq!(results[0].client_id, "AB12");
}

#[test]
fn test_build_index_no_clients_dir() {
    let tmp = TempDir::new().unwrap();
    let config = test_config(&tmp);

    // Point at a root with no clients/ subdirectory
    let empty_root = tmp.path().join("empty");
    std::fs::create_dir_all(&empty_root).unwrap();

    let result = build_index(&config, &empty_root);
    assert!(result.is_err(), "Should fail with no clients directory");
}

#[test]
fn test_exclude_notes_config() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let mut config = test_config(&tmp);
    config.include_notes = false;

    build_index(&config, &clinical_root).unwrap();

    // Searching for content only in notes should return nothing
    let results = search(&config, "defusion", 10).unwrap();
    assert!(results.is_empty(), "Notes content should not be indexed when include_notes=false");
}

#[test]
fn test_exclude_correspondence_config() {
    let tmp = TempDir::new().unwrap();
    let clinical_root = setup_test_clinical_dir(&tmp);
    let mut config = test_config(&tmp);
    config.include_correspondence = false;

    build_index(&config, &clinical_root).unwrap();

    // Searching for content only in correspondence should not find it via that field
    let results = search_field(&config, "referral", "correspondence", 10).unwrap();
    assert!(
        results.is_empty(),
        "Correspondence content should not be indexed when include_correspondence=false"
    );
}
