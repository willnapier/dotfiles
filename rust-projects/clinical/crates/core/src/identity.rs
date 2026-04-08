use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Top-level identity structure matching PRIVATE-FILE-TEMPLATE.yaml.
///
/// All fields are optional because new clients start with null values.
/// The YAML file uses `---` document separators; we parse the first document.
#[derive(Debug, Deserialize, Default)]
pub struct Identity {
    pub name: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    /// DOB as a string — YAML parses dates but serde_yaml gives us a string.
    pub dob: Option<String>,
    pub address: Option<String>,
    pub phone: Option<String>,
    pub email: Option<String>,

    pub tm3_id: Option<serde_yaml::Value>,
    pub diagnosis: Option<String>,
    pub diagnostic_code: Option<String>,

    #[serde(default)]
    pub funding: Funding,
    #[serde(default)]
    pub referrer: Referrer,

    #[serde(default)]
    pub professionals: Vec<Professional>,
    #[serde(default)]
    pub people: Vec<Person>,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub redactions: Vec<Redaction>,
    #[serde(default)]
    pub correspondence: Vec<Correspondence>,

    pub notes: Option<String>,
    #[serde(default)]
    pub related_clients: Vec<String>,

    #[serde(default = "default_status")]
    pub status: String,
    pub discharge_date: Option<String>,
}

fn default_status() -> String {
    "active".to_string()
}

#[derive(Debug, Deserialize)]
pub struct Funding {
    #[serde(rename = "type")]
    pub funding_type: Option<String>,
    pub rate: Option<serde_yaml::Value>,
    #[serde(default = "default_session_duration")]
    pub session_duration: u32,
    pub contact: Option<String>,
    pub policy: Option<String>,
    pub email: Option<String>,
}

fn default_session_duration() -> u32 {
    45
}

impl Default for Funding {
    fn default() -> Self {
        Self {
            funding_type: None,
            rate: None,
            session_duration: 45,
            contact: None,
            policy: None,
            email: None,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct Referrer {
    pub name: Option<String>,
    pub role: Option<String>,
    pub practice: Option<String>,
    pub address: Option<String>,
    pub credentials: Option<String>,
    pub gmc: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Professional {
    pub name: String,
    pub role: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Person {
    pub name: String,
    pub relationship: String,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Redaction {
    pub find: String,
    pub replace: String,
}

#[derive(Debug, Deserialize)]
pub struct Correspondence {
    pub file: String,
    #[serde(rename = "type")]
    pub corr_type: Option<String>,
    pub from: Option<String>,
}

/// Load an Identity from a YAML file.
///
/// Handles multi-document YAML (files with `---` delimiters) by extracting
/// the content between the first pair of `---` markers.
pub fn load_identity(path: &Path) -> Result<Identity> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read identity file: {}", path.display()))?;

    parse_identity(&content)
}

/// Parse an Identity from YAML content string.
///
/// Handles multi-document YAML by extracting the first document body.
pub fn parse_identity(content: &str) -> Result<Identity> {
    // serde_yaml handles `---` document markers, but the template has
    // `---` at both start and end. Strip to get just the document body.
    let body = extract_first_document(content);

    let identity: Identity =
        serde_yaml::from_str(body).context("Failed to parse identity YAML")?;

    Ok(identity)
}

/// Extract the body of the first YAML document from content that may
/// have `---` delimiters at start and/or end.
fn extract_first_document(content: &str) -> &str {
    let trimmed = content.trim();

    // Find first `---`
    let after_first = if trimmed.starts_with("---") {
        let rest = &trimmed[3..];
        rest.trim_start_matches(|c: char| c == '\r' || c == '\n')
    } else {
        trimmed
    };

    // Find closing `---` if present
    if let Some(pos) = after_first.find("\n---") {
        &after_first[..pos]
    } else {
        after_first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"---
name: Jane Bloggs
title: Ms
aliases:
  - Jane
  - Ms Bloggs
  - Jane Bloggs
dob: 1992-03-15
address: 14 Elm Street, London W1 2AB
phone: "07700 900000"
email: jane@example.com

tm3_id: 1234
diagnosis: Generalised Anxiety Disorder
diagnostic_code: F41.1

funding:
  type: AXA
  rate: 198
  session_duration: 45
  contact: Sarah at AXA
  policy: AXA-PP-123456
  email: clinical@axahealth.co.uk

referrer:
  name: Dr Sarah Smith
  role: GP
  practice: Riverside Medical Centre
  address: 123 High St, London N1 1AA
  credentials: MBBS MRCGP
  gmc: null
  email: dr.smith@nhs.net

professionals:
  - name: Dr Patel
    role: Psychiatrist
    note: null

people:
  - name: Tom
    relationship: partner
    note: null
  - name: Sandra
    relationship: mother
    note: null

entities:
  - Linklaters
  - Cardinal Clinic

redactions:
  - find: AXA-PP-123456
    replace: "[policy number removed]"
  - find: 14 Elm Street
    replace: their home address

correspondence:
  - file: 2026-02-14-referral.md
    type: referral
    from: Dr Sarah Smith

notes: null
related_clients: []
status: active
discharge_date: null
---
"#;

    #[test]
    fn test_parse_full_identity() {
        let id = parse_identity(SAMPLE_YAML).unwrap();

        assert_eq!(id.name.as_deref(), Some("Jane Bloggs"));
        assert_eq!(id.title.as_deref(), Some("Ms"));
        assert_eq!(id.aliases.len(), 3);
        assert_eq!(id.aliases[0], "Jane");
        assert_eq!(id.dob.as_deref(), Some("1992-03-15"));
        assert_eq!(id.address.as_deref(), Some("14 Elm Street, London W1 2AB"));
        assert_eq!(id.phone.as_deref(), Some("07700 900000"));
        assert_eq!(id.email.as_deref(), Some("jane@example.com"));

        assert_eq!(
            id.diagnosis.as_deref(),
            Some("Generalised Anxiety Disorder")
        );
        assert_eq!(id.diagnostic_code.as_deref(), Some("F41.1"));

        // Funding
        assert_eq!(id.funding.funding_type.as_deref(), Some("AXA"));
        assert_eq!(id.funding.session_duration, 45);
        assert_eq!(id.funding.policy.as_deref(), Some("AXA-PP-123456"));
        assert_eq!(
            id.funding.email.as_deref(),
            Some("clinical@axahealth.co.uk")
        );

        // Referrer
        assert_eq!(id.referrer.name.as_deref(), Some("Dr Sarah Smith"));
        assert_eq!(id.referrer.role.as_deref(), Some("GP"));
        assert_eq!(
            id.referrer.practice.as_deref(),
            Some("Riverside Medical Centre")
        );

        // People
        assert_eq!(id.people.len(), 2);
        assert_eq!(id.people[0].name, "Tom");
        assert_eq!(id.people[0].relationship, "partner");
        assert_eq!(id.people[1].name, "Sandra");
        assert_eq!(id.people[1].relationship, "mother");

        // Entities
        assert_eq!(id.entities.len(), 2);
        assert_eq!(id.entities[0], "Linklaters");

        // Redactions
        assert_eq!(id.redactions.len(), 2);
        assert_eq!(id.redactions[0].find, "AXA-PP-123456");

        // Correspondence
        assert_eq!(id.correspondence.len(), 1);
        assert_eq!(id.correspondence[0].file, "2026-02-14-referral.md");

        assert_eq!(id.status, "active");
    }

    #[test]
    fn test_parse_minimal_identity() {
        let yaml = "---\nname: null\n---\n";
        let id = parse_identity(yaml).unwrap();
        assert!(id.name.is_none());
        assert_eq!(id.status, "active");
        assert_eq!(id.funding.session_duration, 45);
        assert!(id.people.is_empty());
    }

    #[test]
    fn test_parse_without_document_markers() {
        let yaml = "name: Test Person\nstatus: discharged\n";
        let id = parse_identity(yaml).unwrap();
        assert_eq!(id.name.as_deref(), Some("Test Person"));
        assert_eq!(id.status, "discharged");
    }

    #[test]
    fn test_extract_first_document() {
        let input = "---\nfoo: bar\n---\n";
        assert_eq!(extract_first_document(input), "foo: bar");

        let input2 = "foo: bar\n";
        assert_eq!(extract_first_document(input2), "foo: bar");

        let input3 = "---\nfoo: bar\nbaz: qux\n---\nextra stuff\n";
        assert_eq!(extract_first_document(input3), "foo: bar\nbaz: qux");
    }
}
