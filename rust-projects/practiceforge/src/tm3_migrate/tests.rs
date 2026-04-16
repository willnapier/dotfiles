//! Tests for TM3 migration module.
//!
//! Focus on data transformation and validation logic.
//! No tests for actual TM3 API calls or headless Chrome.

use super::calendar::{self, CalendarReport, TM3Appointment};
use super::clients::{self, MigrationReport};
use super::documents::DocReport;
use super::validate::{self, FieldMismatch, MissingClient, ValidationReport};
use crate::registry::types::{RegistryClient, RegistryFunding, RegistryReferrer};
use crate::tm3_clients::TM3Client;

fn make_tm3_client(id: u64, surname: &str, forename: &str) -> TM3Client {
    TM3Client {
        id,
        surname: surname.to_string(),
        forename: forename.to_string(),
        title: None,
        name: Some(format!("{} {}", forename, surname)),
        date_of_birth: Some("1990-05-15T00:00:00".to_string()),
        email: Some("test@example.com".to_string()),
        number: Some("+447700900123".to_string()),
        address: Some("123 Test Street".to_string()),
        post_code: Some("W1G 8QP".to_string()),
        gender: Some("Female".to_string()),
        practitioner_name: Some("Dr William Napier".to_string()),
        practitioner_id: Some(1),
        patient_group: Some("Private".to_string()),
        registration_date: Some("2024-01-15".to_string()),
    }
}

fn make_registry_client(client_id: &str, name: &str, tm3_id: Option<u64>) -> RegistryClient {
    RegistryClient {
        client_id: client_id.to_string(),
        name: name.to_string(),
        dob: Some("1990-05-15".to_string()),
        address: Some("123 Test Street, W1G 8QP".to_string()),
        phone: Some("+447700900123".to_string()),
        email: Some("test@example.com".to_string()),
        tm3_id,
        status: "active".to_string(),
        discharge_date: None,
        funding: RegistryFunding::default(),
        referrer: RegistryReferrer::default(),
        diagnosis: None,
        diagnostic_code: None,
    }
}

// --- MigrationReport tests ---

#[test]
fn migration_report_defaults_to_zero() {
    let report = MigrationReport::default();
    assert_eq!(report.imported, 0);
    assert_eq!(report.skipped, 0);
    assert!(report.errors.is_empty());
    assert!(report.warnings.is_empty());
    assert_eq!(report.total_processed(), 0);
}

#[test]
fn migration_report_total_processed() {
    let report = MigrationReport {
        imported: 10,
        skipped: 5,
        errors: vec!["err1".to_string(), "err2".to_string()],
        warnings: vec!["warn1".to_string()],
    };
    assert_eq!(report.total_processed(), 17);
}

#[test]
fn migration_report_display() {
    let report = MigrationReport {
        imported: 100,
        skipped: 20,
        errors: vec!["e".to_string()],
        warnings: vec!["w".to_string()],
    };
    let text = format!("{}", report);
    assert!(text.contains("100 imported"));
    assert!(text.contains("20 skipped"));
    assert!(text.contains("1 errors"));
    assert!(text.contains("1 warnings"));
}

// --- Field mapping tests ---

#[test]
fn map_tm3_to_registry_basic() {
    let tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    let reg = clients::map_tm3_to_registry(&tm3, "BR92");

    assert_eq!(reg.client_id, "BR92");
    assert_eq!(reg.name, "Briscoe, Elizabeth");
    assert_eq!(reg.dob.as_deref(), Some("1990-05-15"));
    assert_eq!(reg.email.as_deref(), Some("test@example.com"));
    assert_eq!(reg.phone.as_deref(), Some("+447700900123"));
    assert_eq!(reg.tm3_id, Some(4392));
    assert_eq!(reg.status, "active");
    assert_eq!(
        reg.funding.funding_type.as_deref(),
        Some("Private")
    );
}

#[test]
fn map_tm3_to_registry_address_with_postcode() {
    let tm3 = make_tm3_client(100, "Smith", "John");
    let reg = clients::map_tm3_to_registry(&tm3, "SM00");
    assert_eq!(
        reg.address.as_deref(),
        Some("123 Test Street, W1G 8QP")
    );
}

#[test]
fn map_tm3_to_registry_empty_fields() {
    let mut tm3 = make_tm3_client(200, "Doe", "Jane");
    tm3.email = None;
    tm3.number = Some(String::new());
    tm3.address = None;
    tm3.post_code = None;
    tm3.patient_group = None;
    tm3.date_of_birth = None;

    let reg = clients::map_tm3_to_registry(&tm3, "DO00");
    assert!(reg.email.is_none());
    assert!(reg.phone.is_none());
    assert!(reg.address.is_none());
    assert!(reg.dob.is_none());
    assert!(reg.funding.funding_type.is_none());
}

#[test]
fn map_tm3_to_registry_address_without_postcode() {
    let mut tm3 = make_tm3_client(300, "Test", "User");
    tm3.address = Some("456 Another Road".to_string());
    tm3.post_code = None;

    let reg = clients::map_tm3_to_registry(&tm3, "TE00");
    assert_eq!(
        reg.address.as_deref(),
        Some("456 Another Road")
    );
}

// --- Client ID derivation tests ---

#[test]
fn derive_client_id_basic() {
    let tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    assert_eq!(clients::derive_client_id(&tm3), "BR92");
}

#[test]
fn derive_client_id_short_surname() {
    let tm3 = make_tm3_client(1234, "Li", "Wei");
    assert_eq!(clients::derive_client_id(&tm3), "LI34");
}

#[test]
fn derive_client_id_low_number() {
    let tm3 = make_tm3_client(7, "Napier", "William");
    assert_eq!(clients::derive_client_id(&tm3), "NA07");
}

#[test]
fn derive_client_id_exact_hundred() {
    let tm3 = make_tm3_client(100, "Smith", "John");
    assert_eq!(clients::derive_client_id(&tm3), "SM00");
}

// --- CalendarReport tests ---

#[test]
fn calendar_report_defaults() {
    let report = CalendarReport::default();
    assert_eq!(report.appointments_created, 0);
    assert_eq!(report.series_detected, 0);
    assert_eq!(report.skipped, 0);
    assert!(report.errors.is_empty());
}

#[test]
fn calendar_report_display() {
    let report = CalendarReport {
        appointments_created: 50,
        series_detected: 8,
        skipped: 3,
        errors: vec!["e1".to_string()],
    };
    let text = format!("{}", report);
    assert!(text.contains("50 appointments created"));
    assert!(text.contains("8 series detected"));
}

// --- Datetime parsing tests ---

#[test]
fn parse_datetime_space_format() {
    let result = calendar::parse_datetime("2025-03-15 14:30");
    assert!(result.is_some());
    let (date, time) = result.unwrap();
    assert_eq!(date.to_string(), "2025-03-15");
    assert_eq!(time.format("%H:%M").to_string(), "14:30");
}

#[test]
fn parse_datetime_iso_format() {
    let result = calendar::parse_datetime("2025-03-15T14:30:00");
    assert!(result.is_some());
    let (date, time) = result.unwrap();
    assert_eq!(date.to_string(), "2025-03-15");
    assert_eq!(time.format("%H:%M").to_string(), "14:30");
}

#[test]
fn parse_datetime_iso_no_seconds() {
    let result = calendar::parse_datetime("2025-03-15T14:30");
    assert!(result.is_some());
}

#[test]
fn parse_datetime_invalid() {
    assert!(calendar::parse_datetime("not a date").is_none());
    assert!(calendar::parse_datetime("").is_none());
}

// --- Series detection tests ---

#[test]
fn detect_series_finds_recurring() {
    let appointments: Vec<TM3Appointment> = (0..5)
        .map(|i| TM3Appointment {
            client_name: "Smith, John".to_string(),
            start_time: format!("2025-03-{:02} 10:00", 3 + i * 7), // weekly Mondays
            end_time: format!("2025-03-{:02} 10:50", 3 + i * 7),
            practitioner: "Dr Napier".to_string(),
            status: "completed".to_string(),
        })
        .collect();

    let series = calendar::detect_series(&appointments);
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].0, "Smith, John");
    assert_eq!(series[0].3.len(), 5);
}

#[test]
fn detect_series_ignores_too_few() {
    let appointments = vec![
        TM3Appointment {
            client_name: "Doe, Jane".to_string(),
            start_time: "2025-03-03 10:00".to_string(),
            end_time: "2025-03-03 10:50".to_string(),
            practitioner: "Dr Napier".to_string(),
            status: "completed".to_string(),
        },
        TM3Appointment {
            client_name: "Doe, Jane".to_string(),
            start_time: "2025-03-10 10:00".to_string(),
            end_time: "2025-03-10 10:50".to_string(),
            practitioner: "Dr Napier".to_string(),
            status: "completed".to_string(),
        },
    ];

    let series = calendar::detect_series(&appointments);
    assert!(series.is_empty(), "Two appointments should not form a series");
}

#[test]
fn detect_series_separates_different_clients() {
    let mut appointments = Vec::new();
    for i in 0..4 {
        appointments.push(TM3Appointment {
            client_name: "Smith, John".to_string(),
            start_time: format!("2025-03-{:02} 10:00", 3 + i * 7),
            end_time: format!("2025-03-{:02} 10:50", 3 + i * 7),
            practitioner: "Dr Napier".to_string(),
            status: "completed".to_string(),
        });
        appointments.push(TM3Appointment {
            client_name: "Doe, Jane".to_string(),
            start_time: format!("2025-03-{:02} 14:00", 4 + i * 7),
            end_time: format!("2025-03-{:02} 14:50", 4 + i * 7),
            practitioner: "Dr Napier".to_string(),
            status: "completed".to_string(),
        });
    }

    let series = calendar::detect_series(&appointments);
    assert_eq!(series.len(), 2);
}

// --- ValidationReport tests ---

#[test]
fn validation_report_defaults() {
    let report = ValidationReport::default();
    assert_eq!(report.total_tm3, 0);
    assert_eq!(report.total_registry, 0);
    assert!(report.missing.is_empty());
    assert!(report.extra.is_empty());
    assert!(report.mismatches.is_empty());
    assert!(report.missing_documents.is_empty());
}

#[test]
fn validation_report_display() {
    let report = ValidationReport {
        total_tm3: 100,
        total_registry: 95,
        missing: vec![MissingClient {
            tm3_id: 1,
            name: "Test".to_string(),
        }],
        extra: vec!["EX01".to_string()],
        mismatches: vec![],
        missing_documents: vec!["AB12".to_string()],
    };
    let text = format!("{}", report);
    assert!(text.contains("100 TM3 clients"));
    assert!(text.contains("95 registry clients"));
    assert!(text.contains("Missing from registry: 1"));
    assert!(text.contains("Extra in registry"));
}

// --- Field comparison tests ---

#[test]
fn compare_fields_matching() {
    let tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    let reg = make_registry_client("BR92", "Briscoe, Elizabeth", Some(4392));
    let mismatches = validate::compare_fields(&tm3, &reg);
    assert!(mismatches.is_empty(), "Matching records should produce no mismatches");
}

#[test]
fn compare_fields_name_mismatch() {
    let tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    let reg = make_registry_client("BR92", "Briscoe, Liz", Some(4392));
    let mismatches = validate::compare_fields(&tm3, &reg);
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].field, "name");
}

#[test]
fn compare_fields_email_mismatch() {
    let tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    let mut reg = make_registry_client("BR92", "Briscoe, Elizabeth", Some(4392));
    reg.email = Some("different@example.com".to_string());
    let mismatches = validate::compare_fields(&tm3, &reg);
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].field, "email");
}

#[test]
fn compare_fields_phone_normalisation() {
    // Same phone number, different formatting
    let mut tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    tm3.number = Some("07700 900 123".to_string());
    let mut reg = make_registry_client("BR92", "Briscoe, Elizabeth", Some(4392));
    reg.phone = Some("07700900123".to_string());
    let mismatches = validate::compare_fields(&tm3, &reg);
    // Phone should match after normalisation (stripping spaces)
    let phone_mismatches: Vec<_> = mismatches.iter().filter(|m| m.field == "phone").collect();
    assert!(
        phone_mismatches.is_empty(),
        "Same phone with different formatting should not be a mismatch"
    );
}

#[test]
fn compare_fields_skips_empty() {
    // If TM3 has no email, don't flag it as a mismatch
    let mut tm3 = make_tm3_client(4392, "Briscoe", "Elizabeth");
    tm3.email = None;
    let reg = make_registry_client("BR92", "Briscoe, Elizabeth", Some(4392));
    let mismatches = validate::compare_fields(&tm3, &reg);
    let email_mismatches: Vec<_> = mismatches.iter().filter(|m| m.field == "email").collect();
    assert!(
        email_mismatches.is_empty(),
        "Missing TM3 email should not produce a mismatch"
    );
}

// --- DocReport tests ---

#[test]
fn doc_report_defaults() {
    let report = DocReport::default();
    assert_eq!(report.downloaded, 0);
    assert_eq!(report.already_present, 0);
    assert!(report.failed.is_empty());
    assert_eq!(report.total_processed(), 0);
}

#[test]
fn doc_report_total_processed() {
    let report = DocReport {
        downloaded: 5,
        already_present: 10,
        failed: vec!["err".to_string()],
    };
    assert_eq!(report.total_processed(), 16);
}

#[test]
fn doc_report_display() {
    let report = DocReport {
        downloaded: 42,
        already_present: 100,
        failed: vec![],
    };
    let text = format!("{}", report);
    assert!(text.contains("42 downloaded"));
    assert!(text.contains("100 already present"));
    assert!(text.contains("0 failed"));
}

// --- Name matching tests ---

#[test]
fn compare_fields_case_insensitive_name() {
    let tm3 = make_tm3_client(100, "SMITH", "JOHN");
    let reg = make_registry_client("SM00", "smith, john", Some(100));
    let mismatches = validate::compare_fields(&tm3, &reg);
    let name_mismatches: Vec<_> = mismatches.iter().filter(|m| m.field == "name").collect();
    assert!(
        name_mismatches.is_empty(),
        "Case-insensitive names should match"
    );
}

#[test]
fn field_mismatch_display() {
    let m = FieldMismatch {
        client_id: "BR92".to_string(),
        tm3_id: 4392,
        field: "email".to_string(),
        tm3_value: "a@b.com".to_string(),
        registry_value: "c@d.com".to_string(),
    };
    let text = format!("{}", m);
    assert!(text.contains("BR92"));
    assert!(text.contains("4392"));
    assert!(text.contains("email"));
    assert!(text.contains("a@b.com"));
    assert!(text.contains("c@d.com"));
}
