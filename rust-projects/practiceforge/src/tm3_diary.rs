//! TM3 diary write-back — creates appointments directly via the TM3 API.
//!
//! Uses session cookies (shared with tm3-discover / tm3-clients) and
//! reqwest::blocking to call the ServiceStack endpoints captured from
//! the browser recon on 2026-04-19.
//!
//! Auth: session cookies only (no Bearer token). Required header: x-tm3-date.
//!
//! Booking flow:
//!   1. POST /api/json/reply/CreateTemporaryAppointmentRequest — reserve slot
//!   2. POST /api/json/reply/AppointmentBookRequest — commit booking
//!   3. POST /api/lock/release — release slot lock
//!
//! Run inside std::thread::spawn to avoid blocking the tokio executor.

use anyhow::{bail, Context, Result};
use chrono::Local;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

const TM3_BASE: &str = "https://changeofharleystreet.tm3app.com";
const TM3_SERVICE: &str = "tm3-session";
const TM3_ACCOUNT: &str = "changeofharleystreet";

/// IDs that vary per-practitioner/practice, loaded from [tm3_diary] config.toml.
#[derive(Debug, Clone)]
pub struct Tm3DiaryConfig {
    pub practitioner_id: u64,
    pub location_id: u64,
    /// stockId = the appointment type ID in TM3 (42 for standard session).
    pub stock_id: u64,
    pub service_type_id: u64,
}

impl Default for Tm3DiaryConfig {
    fn default() -> Self {
        // Defaults captured from the 2026-04-19 browser recon.
        Self {
            practitioner_id: 12,
            location_id: 1,
            stock_id: 42,
            service_type_id: 1,
        }
    }
}

impl Tm3DiaryConfig {
    pub fn load() -> Self {
        let Some(config) = crate::config::load_config() else {
            return Self::default();
        };
        let Some(section) = config.get("tm3_diary") else {
            return Self::default();
        };
        let mut cfg = Self::default();
        if let Some(v) = section.get("practitioner_id").and_then(|v| v.as_integer()) {
            cfg.practitioner_id = v as u64;
        }
        if let Some(v) = section.get("location_id").and_then(|v| v.as_integer()) {
            cfg.location_id = v as u64;
        }
        if let Some(v) = section.get("stock_id").and_then(|v| v.as_integer()) {
            cfg.stock_id = v as u64;
        }
        if let Some(v) = section.get("service_type_id").and_then(|v| v.as_integer()) {
            cfg.service_type_id = v as u64;
        }
        cfg
    }
}

/// Parameters for a single booking request.
#[derive(Debug, Clone)]
pub struct BookingRequest {
    pub tm3_customer_id: u64,
    /// ISO 8601 without timezone: "2026-04-21T14:30:00"
    pub start_dt: String,
    pub end_dt: String,
    pub duration_mins: u32,
}

/// Result of a successful booking.
#[derive(Debug, Serialize, Deserialize)]
pub struct BookingResult {
    pub appointment_id: Option<u64>,
    pub raw: Value,
}

/// HTTP client for the TM3 diary API.
pub struct Tm3DiaryClient {
    client: reqwest::blocking::Client,
    cookie_header: String,
}

impl Tm3DiaryClient {
    pub fn new() -> Result<Self> {
        let cookies = crate::session_cookies::load_cookies(TM3_SERVICE, TM3_ACCOUNT)
            .context("Failed to load TM3 session cookies")?;
        let cookie_header = cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; ");
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            cookie_header,
        })
    }

    fn now_header() -> String {
        Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let url = format!("{}{}", TM3_BASE, path);
        let resp = self
            .client
            .post(&url)
            .header("Cookie", &self.cookie_header)
            .header("Content-Type", "application/json")
            .header("x-tm3-date", Self::now_header())
            .json(body)
            .send()
            .with_context(|| format!("POST {} failed", path))?;

        let status = resp.status();
        let text = resp
            .text()
            .with_context(|| format!("Failed to read body from {}", path))?;

        if !status.is_success() {
            bail!(
                "TM3 {} returned HTTP {}: {}",
                path,
                status,
                &text[..text.len().min(400)]
            );
        }

        serde_json::from_str(&text)
            .with_context(|| format!("Failed to parse JSON from {}", path))
    }

    /// Execute the full three-step booking flow.
    pub fn book(&self, req: &BookingRequest, cfg: &Tm3DiaryConfig) -> Result<BookingResult> {
        // Step 1: Reserve slot and get temp_id
        let temp_body = serde_json::json!({
            "startDateTime": req.start_dt,
            "endDateTime": req.end_dt,
            "locationId": cfg.location_id,
            "practitionerId": cfg.practitioner_id,
            "roomId": -1,
        });
        let temp_resp =
            self.post("/api/json/reply/CreateTemporaryAppointmentRequest", &temp_body)?;
        let temp_id = temp_resp
            .get("id")
            .and_then(|v| v.as_u64())
            .context("CreateTemporaryAppointmentRequest: response missing 'id'")?;
        eprintln!("[tm3-diary] Slot reserved: tempId={}", temp_id);

        // Step 2: Build customer object from local TM3 cache
        let customer = self.customer_object(req.tm3_customer_id, cfg)?;

        // Step 3: Commit booking
        let background_job_id = Uuid::new_v4().to_string();
        let book_body = serde_json::json!({
            "startDateTime":       req.start_dt,
            "endDateTime":         req.end_dt,
            "practitionerId":      cfg.practitioner_id,
            "locationId":          cfg.location_id,
            "roomId":              -1,
            "apptType":            "A",
            "customer":            customer,
            "stockId":             cfg.stock_id,
            "serviceTypeId":       cfg.service_type_id,
            "updatePatientGroupToo": true,
            "customerGroupReference": "",
            "customerInsuranceNumber": "",
            "reminderType":        null,
            "additionalCharges":   [],
            "duration":            req.duration_mins,
            "prepaymentType":      "PayLater",
            "depositPhysicalTerminal": null,
            "firstAppointment":    "true",
            "authId":              null,
            "tempId":              temp_id,
            "backgroundJobId":     background_job_id,
            "depositPaid":         false,
            "resourceMode":        "practitioner",
        });
        let book_resp = self.post("/api/json/reply/AppointmentBookRequest", &book_body)?;
        let appointment_id = book_resp.get("id").and_then(|v| v.as_u64());
        eprintln!(
            "[tm3-diary] Booking committed. appointmentId={:?}",
            appointment_id
        );

        // Step 4: Release lock
        let lock_body = serde_json::json!({"objectType": "A"});
        let _ = self.post("/api/lock/release", &lock_body);

        Ok(BookingResult {
            appointment_id,
            raw: book_resp,
        })
    }

    /// Build the customer sub-object for AppointmentBookRequest from the TM3 client cache.
    fn customer_object(&self, tm3_id: u64, cfg: &Tm3DiaryConfig) -> Result<Value> {
        let clients = crate::tm3_clients::load_cache().context(
            "TM3 client cache empty — run 'practiceforge tm3-clients refresh' first",
        )?;
        let c = clients
            .iter()
            .find(|c| c.id == tm3_id)
            .with_context(|| {
                format!(
                    "tm3_id={} not found in cache — run 'practiceforge tm3-clients refresh'",
                    tm3_id
                )
            })?;

        let title = c.title.as_deref().unwrap_or("").to_string();
        let full_name = c
            .name
            .as_deref()
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                format!("{} {} {}", title, c.forename, c.surname)
                    .trim()
                    .to_string()
            });

        Ok(serde_json::json!({
            "id":                  c.id,
            "reference":           c.id.to_string(),
            "forename":            c.forename,
            "surname":             c.surname,
            "title":               title,
            "name":                full_name,
            "gender":              c.gender.as_deref().unwrap_or("Not specified"),
            "addressLine1": "", "addressLine2": "", "addressLine3": "",
            "addressLine4": "", "addressLine5": "",
            "invoiceAddressLine1": "", "invoiceAddressLine2": "", "invoiceAddressLine3": "",
            "invoiceAddressLine4": "", "invoiceAddressLine5": "",
            "useInvoiceAddress":   false,
            "address":             c.address.as_deref().unwrap_or(""),
            "invoiceAddress":      "",
            "postCode":            c.post_code.as_deref().unwrap_or(""),
            "registrationDate":    c.registration_date.as_deref().unwrap_or(""),
            "workTelephone":       "",
            "mobileTelephone":     c.number.as_deref().unwrap_or(""),
            "email":               c.email.as_deref().unwrap_or(""),
            "locationId":          cfg.location_id,
            "locationName":        "",
            "practitionerId":      cfg.practitioner_id,
            "practitionerTitle":   "Mr",
            "practitionerForename": "Will",
            "practitionerSurname": "Napier",
            "practitionerName":    "Mr Will Napier",
            "practitionerStatus":  1,
            "patientGroup":        c.patient_group.as_deref().unwrap_or("Private"),
            "status":              1,
            "provisional":         false,
            "height":              0,
            "weight":              0,
            "employer":            "None",
            "niNumber":            "",
            "hospitalNumber":      "",
            "insuranceNumber":     "",
            "alternativeRef1":     "",
            "groupReference":      "",
            "consent":             "",
            "category":            "Not Listed",
            "enquiry":             "None",
            "invoicePeriod":       "E",
            "balanceInvoiced":     0,
            "balanceOwed":         0,
            "uninvoicedCharges":   0,
            "uninvoicedTotal":     0,
            "billTo":              "P",
            "online":              false,
            "smsEnabled":          false,
            "sarStatus":           "",
            "hasAlerts":           false,
            "guardDelete":         [],
            "givenConsent":        true,
            "emailConsent":        false,
            "smsConsent":          false,
            "phoneConsent":        false,
            "accountId":           0,
            "accountType":         "C",
            "tags":                [],
            "overridePayAtClinic": false,
            "isBusiness":          false,
            "emergencyContact":    "",
            "isRequestingCard":    false,
        }))
    }
}

/// Parse a practiceforge datetime string ("YYYY-MM-DD HH:MM") into
/// the TM3 format ("YYYY-MM-DDTHH:MM:SS") and derive the end datetime.
pub fn parse_datetime(datetime: &str, duration_mins: u32) -> Result<(String, String)> {
    use chrono::NaiveDateTime;
    let dt = NaiveDateTime::parse_from_str(datetime, "%Y-%m-%d %H:%M")
        .with_context(|| format!("Invalid datetime '{}' — expected YYYY-MM-DD HH:MM", datetime))?;
    let end = dt + chrono::Duration::minutes(duration_mins as i64);
    let start_s = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
    let end_s = end.format("%Y-%m-%dT%H:%M:%S").to_string();
    Ok((start_s, end_s))
}

/// Read tm3_id from a client's identity.yaml using a cheap line-scan.
/// Returns None if not set or not a valid numeric ID.
pub fn read_tm3_id(client_id: &str) -> Option<u64> {
    use std::fs;
    let clients_root = crate::config::clients_dir();
    let candidates = [
        clients_root.join(client_id).join("identity.yaml"),
        clients_root
            .join(client_id)
            .join("private")
            .join("identity.yaml"),
    ];
    for path in &candidates {
        if let Ok(text) = fs::read_to_string(path) {
            for line in text.lines() {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("tm3_id:") {
                    let without_comment = rest.split('#').next().unwrap_or("");
                    let val = without_comment
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'');
                    if !val.is_empty() && val != "null" && val != "~" {
                        return val.parse::<u64>().ok();
                    }
                }
            }
        }
    }
    None
}
