use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use std::path::Path;

use crate::client;
use crate::identity::{self, Identity};

/// A single find/replace substitution rule.
#[derive(Debug)]
struct Sub {
    find: String,
    replace: String,
    case_insensitive: bool,
}

/// Run the de-identify command.
pub fn run(id: &str, file: Option<&str>, dry_run: bool, list: bool) -> Result<()> {
    let client_dir = client::client_dir(id);
    let private_dir = client::private_dir(id);

    if !client_dir.exists() {
        bail!("Client directory not found: {}", client_dir.display());
    }

    let id_path = client::identity_path(id);
    if !id_path.exists() {
        bail!("identity.yaml not found: {}", id_path.display());
    }

    let ident = identity::load_identity(&id_path)?;

    // List mode
    if list {
        return list_private_files(&private_dir);
    }

    let source = match file {
        Some(f) => f,
        None => bail!("Specify a source file, or use --list to see available files."),
    };

    // Resolve source path
    let source_path = if Path::new(source).is_absolute() {
        source.into()
    } else {
        private_dir.join(source)
    };

    if !source_path.exists() {
        bail!("Source file not found: {}", source_path.display());
    }

    let content = std::fs::read_to_string(&source_path)
        .with_context(|| format!("Failed to read: {}", source_path.display()))?;

    // Build and sort substitution list
    let subs = build_subs(&ident);
    let sorted = sort_subs(subs);

    if dry_run {
        return print_dry_run(&sorted, &content);
    }

    // Apply substitutions
    let mut result = content;
    for sub in &sorted {
        result = apply_sub(&result, sub);
    }

    // Regex cleanup: phone/NHS numbers (3-3-4 pattern)
    let phone_re = Regex::new(r"\d{3}\s?\d{3}\s?\d{4}").unwrap();
    result = phone_re.replace_all(&result, "[number removed]").to_string();

    // Output filename: insert client ID after date prefix
    let source_name = source_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let output_name = make_output_name(&source_name, id);
    let output_path = client_dir.join(&output_name);

    std::fs::write(&output_path, &result)
        .with_context(|| format!("Failed to write: {}", output_path.display()))?;
    println!("De-identified: {}", output_path.display());
    println!();

    // Post-check: client name still present?
    let client_name = ident.name.as_deref().unwrap_or("");
    if !client_name.is_empty() {
        if result.to_lowercase().contains(&client_name.to_lowercase()) {
            println!("WARNING: Client name may still appear in output. Review manually.");
        } else {
            println!("Client name not found in output (good).");
        }
    }

    println!("Review the de-identified file before relying on it.");

    Ok(())
}

/// List .md files in private/ that are available for de-identification.
fn list_private_files(private_dir: &Path) -> Result<()> {
    let skip = ["identity.yaml", "reference.md", "raw-notes.md"];

    let mut files: Vec<String> = std::fs::read_dir(private_dir)
        .with_context(|| format!("Failed to read: {}", private_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".md") && !skip.contains(&name.as_str()) {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    files.sort();

    if files.is_empty() {
        println!("No correspondence files in private/.");
    } else {
        println!("Files available to de-identify:");
        for f in &files {
            println!("  {}", f);
        }
    }

    Ok(())
}

/// Build the list of substitution rules from an Identity.
fn build_subs(ident: &Identity) -> Vec<Sub> {
    let mut subs = Vec::new();

    // 1. Client name + aliases → "Client"
    let client_name = ident.name.as_deref().unwrap_or("");
    if !client_name.is_empty() {
        subs.push(Sub {
            find: client_name.to_string(),
            replace: "Client".to_string(),
            case_insensitive: true,
        });
    }

    for alias in &ident.aliases {
        if !alias.is_empty() && alias != client_name {
            subs.push(Sub {
                find: alias.clone(),
                replace: "Client".to_string(),
                case_insensitive: true,
            });
        }
    }

    // Title + surname combo (e.g. "Ms Bloggs")
    let client_title = ident.title.as_deref().unwrap_or("");
    if !client_title.is_empty() && !client_name.is_empty() {
        if let Some(surname) = client_name.split_whitespace().last() {
            let titled = format!("{} {}", client_title, surname);
            subs.push(Sub {
                find: titled,
                replace: "Client".to_string(),
                case_insensitive: true,
            });
        }
    }

    // 2. Client DOB — multiple formats
    if let Some(dob_str) = &ident.dob {
        if let Ok(dob) = parse_dob(dob_str) {
            let formats = [
                dob.format("%Y-%m-%d").to_string(),
                dob.format("%d/%m/%Y").to_string(),
                dob.format("%d.%m.%Y").to_string(),
                dob.format("%d %B %Y").to_string(),
                dob.format("%d %b %Y").to_string(),
            ];
            for fmt in formats {
                subs.push(Sub {
                    find: fmt,
                    replace: "[DOB removed]".to_string(),
                    case_insensitive: false,
                });
            }
        }
    }

    // 3. People → "initial (relationship)"
    for person in &ident.people {
        if !person.name.is_empty() && !person.relationship.is_empty() {
            let initial = person.name.chars().next().unwrap();
            let rel_display = person.relationship.replace('_', " ");
            let replacement = format!("{} ({})", initial, rel_display);
            subs.push(Sub {
                find: person.name.clone(),
                replace: replacement,
                case_insensitive: true,
            });
        }
    }

    // 4. Referrer — kept as-is (professional contact, not client PHI)

    // 5. Entities → generic
    for entity in &ident.entities {
        if !entity.is_empty() {
            subs.push(Sub {
                find: entity.clone(),
                replace: "their organisation".to_string(),
                case_insensitive: true,
            });
        }
    }

    // 6. Redactions → specified replacement
    for redaction in &ident.redactions {
        if !redaction.find.is_empty() {
            subs.push(Sub {
                find: redaction.find.clone(),
                replace: redaction.replace.clone(),
                case_insensitive: false,
            });
        }
    }

    // 7. Funding policy number
    if let Some(policy) = &ident.funding.policy {
        if !policy.is_empty() {
            subs.push(Sub {
                find: policy.clone(),
                replace: "[policy number removed]".to_string(),
                case_insensitive: false,
            });
        }
    }

    // 8. Client address, phone, email
    if let Some(addr) = &ident.address {
        if !addr.is_empty() {
            subs.push(Sub {
                find: addr.clone(),
                replace: "[address removed]".to_string(),
                case_insensitive: false,
            });
        }
    }
    if let Some(phone) = &ident.phone {
        if !phone.is_empty() {
            subs.push(Sub {
                find: phone.clone(),
                replace: "[phone removed]".to_string(),
                case_insensitive: false,
            });
        }
    }
    if let Some(email) = &ident.email {
        if !email.is_empty() {
            subs.push(Sub {
                find: email.clone(),
                replace: "[email removed]".to_string(),
                case_insensitive: false,
            });
        }
    }

    subs
}

/// Sort substitutions by find-string length descending (longest match first).
fn sort_subs(mut subs: Vec<Sub>) -> Vec<Sub> {
    subs.sort_by(|a, b| b.find.len().cmp(&a.find.len()));
    subs
}

/// Apply a single substitution to the content.
fn apply_sub(content: &str, sub: &Sub) -> String {
    if sub.case_insensitive {
        // Build a case-insensitive regex from the literal find string
        let escaped = regex::escape(&sub.find);
        let pattern = format!("(?i){}", escaped);
        match Regex::new(&pattern) {
            Ok(re) => re.replace_all(content, sub.replace.as_str()).to_string(),
            Err(_) => content.replace(&sub.find, &sub.replace),
        }
    } else {
        content.replace(&sub.find, &sub.replace)
    }
}

/// Parse a DOB string that may be ISO format or other common formats.
fn parse_dob(dob_str: &str) -> Result<NaiveDate> {
    // Try ISO first (YYYY-MM-DD)
    if let Ok(d) = NaiveDate::parse_from_str(dob_str, "%Y-%m-%d") {
        return Ok(d);
    }
    // UK format (DD/MM/YYYY)
    if let Ok(d) = NaiveDate::parse_from_str(dob_str, "%d/%m/%Y") {
        return Ok(d);
    }
    bail!("Could not parse DOB: {}", dob_str)
}

/// Generate output filename by inserting client ID after date prefix.
///
/// Input: `2026-02-14-referral.md`, id: `EB88` → `2026-02-14-EB88-referral.md`
/// Input: `referral.md`, id: `EB88` → `2026-02-26-EB88-referral.md` (today's date)
fn make_output_name(source_name: &str, id: &str) -> String {
    let date_re = Regex::new(r"^\d{4}-\d{2}-\d{2}-").unwrap();

    if date_re.is_match(source_name) {
        // Has date prefix — insert client ID after date
        let date_part = &source_name[..11]; // "2026-02-14-"
        let rest = &source_name[11..];
        format!("{}{}-{}", date_part, id, rest)
    } else {
        // No date — prepend today's date + client ID
        let today = chrono::Local::now().format("%Y-%m-%d");
        format!("{}-{}-{}", today, id, source_name)
    }
}

/// Print dry-run output showing all rules and which match.
fn print_dry_run(subs: &[Sub], content: &str) -> Result<()> {
    println!("Substitution rules (longest first):");
    println!();

    for sub in subs {
        let ci_label = if sub.case_insensitive {
            " (case-insensitive)"
        } else {
            ""
        };
        println!("  \"{}\" -> \"{}\"{}", sub.find, sub.replace, ci_label);
    }
    println!();

    let mut match_count = 0;
    for sub in subs {
        let found = if sub.case_insensitive {
            content.to_lowercase().contains(&sub.find.to_lowercase())
        } else {
            content.contains(&sub.find)
        };
        if found {
            match_count += 1;
            println!("  MATCH: \"{}\"", sub.find);
        }
    }
    println!();
    println!("{} rules matched.", match_count);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::*;

    fn test_identity() -> Identity {
        Identity {
            name: Some("Jane Bloggs".to_string()),
            title: Some("Ms".to_string()),
            aliases: vec![
                "Jane".to_string(),
                "Ms Bloggs".to_string(),
                "Jane Bloggs".to_string(),
            ],
            dob: Some("1992-03-15".to_string()),
            address: Some("14 Elm Street, London W1 2AB".to_string()),
            phone: Some("07700 900000".to_string()),
            email: Some("jane@example.com".to_string()),
            people: vec![
                Person {
                    name: "Tom".to_string(),
                    relationship: "partner".to_string(),
                    note: None,
                },
                Person {
                    name: "Sandra".to_string(),
                    relationship: "mother".to_string(),
                    note: None,
                },
            ],
            entities: vec!["Linklaters".to_string()],
            redactions: vec![Redaction {
                find: "Biscuit".to_string(),
                replace: "the family pet".to_string(),
            }],
            funding: Funding {
                policy: Some("AXA-PP-123456".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_build_subs_count() {
        let ident = test_identity();
        let subs = build_subs(&ident);

        // Client name (1) + aliases that differ from name (2: "Jane", "Ms Bloggs";
        // "Jane Bloggs" == name so skipped) + title+surname "Ms Bloggs" (already in aliases
        // but added separately) + DOB (5 formats) + people (2) + entities (1) +
        // redactions (1) + policy (1) + address (1) + phone (1) + email (1) = many
        assert!(!subs.is_empty());

        // Verify key substitutions exist
        let finds: Vec<&str> = subs.iter().map(|s| s.find.as_str()).collect();
        assert!(finds.contains(&"Jane Bloggs"));
        assert!(finds.contains(&"Jane"));
        assert!(finds.contains(&"Tom"));
        assert!(finds.contains(&"Sandra"));
        assert!(finds.contains(&"Linklaters"));
        assert!(finds.contains(&"Biscuit"));
        assert!(finds.contains(&"AXA-PP-123456"));
        assert!(finds.contains(&"14 Elm Street, London W1 2AB"));
        assert!(finds.contains(&"07700 900000"));
        assert!(finds.contains(&"jane@example.com"));
    }

    #[test]
    fn test_sort_longest_first() {
        let subs = vec![
            Sub {
                find: "Jo".to_string(),
                replace: "X".to_string(),
                case_insensitive: false,
            },
            Sub {
                find: "Jonathan".to_string(),
                replace: "Y".to_string(),
                case_insensitive: false,
            },
        ];
        let sorted = sort_subs(subs);
        assert_eq!(sorted[0].find, "Jonathan");
        assert_eq!(sorted[1].find, "Jo");
    }

    #[test]
    fn test_apply_sub_case_insensitive() {
        let sub = Sub {
            find: "Jane".to_string(),
            replace: "Client".to_string(),
            case_insensitive: true,
        };
        assert_eq!(apply_sub("I saw Jane today", &sub), "I saw Client today");
        assert_eq!(apply_sub("I saw jane today", &sub), "I saw Client today");
        assert_eq!(apply_sub("I saw JANE today", &sub), "I saw Client today");
    }

    #[test]
    fn test_apply_sub_case_sensitive() {
        let sub = Sub {
            find: "AXA-PP-123".to_string(),
            replace: "[removed]".to_string(),
            case_insensitive: false,
        };
        assert_eq!(apply_sub("Policy: AXA-PP-123", &sub), "Policy: [removed]");
        assert_eq!(apply_sub("Policy: axa-pp-123", &sub), "Policy: axa-pp-123");
    }

    #[test]
    fn test_full_de_identify_pipeline() {
        let ident = test_identity();
        let subs = sort_subs(build_subs(&ident));

        let input = "Dear William,\n\n\
            Re: Jane Bloggs (DOB: 15/03/1992)\n\n\
            Jane reports difficulties since her relationship with Tom ended.\n\
            Her mother Sandra has been supportive. She works at Linklaters.\n\
            Policy: AXA-PP-123456. The family pet Biscuit provides comfort.\n";

        let mut result = input.to_string();
        for sub in &subs {
            result = apply_sub(&result, sub);
        }

        // Regex cleanup
        let phone_re = Regex::new(r"\d{3}\s?\d{3}\s?\d{4}").unwrap();
        result = phone_re.replace_all(&result, "[number removed]").to_string();

        assert!(!result.contains("Jane"));
        assert!(!result.contains("Bloggs"));
        assert!(result.contains("Client"));
        assert!(result.contains("T (partner)"));
        assert!(result.contains("S (mother)"));
        assert!(result.contains("their organisation"));
        assert!(result.contains("[DOB removed]"));
        assert!(result.contains("[policy number removed]"));
        assert!(result.contains("the family pet"));
    }

    #[test]
    fn test_people_replacement_format() {
        let ident = test_identity();
        let subs = build_subs(&ident);

        let tom_sub = subs.iter().find(|s| s.find == "Tom").unwrap();
        assert_eq!(tom_sub.replace, "T (partner)");

        let sandra_sub = subs.iter().find(|s| s.find == "Sandra").unwrap();
        assert_eq!(sandra_sub.replace, "S (mother)");
    }

    #[test]
    fn test_make_output_name_with_date() {
        let result = make_output_name("2026-02-14-referral.md", "EB88");
        assert_eq!(result, "2026-02-14-EB88-referral.md");
    }

    #[test]
    fn test_make_output_name_without_date() {
        let result = make_output_name("referral.md", "EB88");
        // Should start with today's date
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert!(result.starts_with(&today));
        assert!(result.ends_with("-EB88-referral.md"));
    }

    #[test]
    fn test_parse_dob_iso() {
        let d = parse_dob("1992-03-15").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(1992, 3, 15).unwrap());
    }

    #[test]
    fn test_parse_dob_uk() {
        let d = parse_dob("15/03/1992").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(1992, 3, 15).unwrap());
    }

    #[test]
    fn test_dob_formats() {
        let ident = Identity {
            dob: Some("1992-03-15".to_string()),
            ..Default::default()
        };
        let subs = build_subs(&ident);
        let dob_subs: Vec<&str> = subs
            .iter()
            .filter(|s| s.replace == "[DOB removed]")
            .map(|s| s.find.as_str())
            .collect();

        assert!(dob_subs.contains(&"1992-03-15"));
        assert!(dob_subs.contains(&"15/03/1992"));
        assert!(dob_subs.contains(&"15.03.1992"));
        assert!(dob_subs.contains(&"15 March 1992"));
        assert!(dob_subs.contains(&"15 Mar 1992"));
    }
}
