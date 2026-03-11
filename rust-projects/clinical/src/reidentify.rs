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
}

/// Run the re-identify command.
pub fn run(id: &str, file: &str, dry_run: bool, name_form: &str) -> Result<()> {
    let client_dir = client::client_dir(id);
    let private_dir = client::private_dir(id);

    if !client_dir.exists() {
        bail!("Client directory not found: {}", client_dir.display());
    }

    let id_path = client::identity_path(id);
    if !id_path.exists() {
        bail!("identity.yaml not found: {}", id_path.display());
    }

    // Resolve source path
    let source_path = if Path::new(file).is_absolute() {
        file.into()
    } else {
        client_dir.join(file)
    };

    if !source_path.exists() {
        bail!("Source file not found: {}", source_path.display());
    }

    let ident = identity::load_identity(&id_path)?;
    let content = std::fs::read_to_string(&source_path)
        .with_context(|| format!("Failed to read: {}", source_path.display()))?;

    let (subs, warnings) = build_subs(&ident, name_form, &content);

    if dry_run {
        return print_dry_run(&subs, &warnings, &content);
    }

    // Apply substitutions (sorted by length descending)
    let mut sorted = subs;
    sorted.sort_by(|a, b| b.find.len().cmp(&a.find.len()));

    let mut result = content;
    for sub in &sorted {
        result = result.replace(&sub.find, &sub.replace);
    }

    // Output goes to private/ with client ID stripped from filename
    let source_name = source_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let output_name = source_name.replace(&format!("{}-", id), "");
    let output_path = private_dir.join(&output_name);

    std::fs::write(&output_path, &result)
        .with_context(|| format!("Failed to write: {}", output_path.display()))?;
    println!("Re-identified: {}", output_path.display());
    println!();

    if !warnings.is_empty() {
        println!("Review needed:");
        for w in &warnings {
            println!("  - {}", w);
        }
        println!();
    }

    println!("File saved to private/ — review before sending.");

    Ok(())
}

/// Resolve a name form for the client.
fn resolve_client_name(ident: &Identity, form: &str) -> Option<String> {
    let name = ident.name.as_deref().unwrap_or("");
    let title = ident.title.as_deref().unwrap_or("");
    let first = name.split_whitespace().next().unwrap_or("");
    let surname = name.split_whitespace().last().unwrap_or("");

    match form {
        "first" if !first.is_empty() => Some(first.to_string()),
        "title" if !title.is_empty() && !surname.is_empty() => {
            Some(format!("{} {}", title, surname))
        }
        "full" if !name.is_empty() => Some(name.to_string()),
        _ if !name.is_empty() => Some(name.to_string()),
        _ => None,
    }
}

/// Extract the first name from a referrer name like "Dr Laura Pollock".
/// Skips common prefixes (Dr, Prof, Mr, Mrs, Ms, Miss).
fn referrer_first_name(referrer_name: &str) -> Option<String> {
    let prefixes = ["Dr", "Prof", "Professor", "Mr", "Mrs", "Ms", "Miss"];
    let parts: Vec<&str> = referrer_name.split_whitespace().collect();

    if parts.len() < 2 {
        return parts.first().map(|s| s.to_string());
    }

    // Skip leading prefix if present
    let start = if prefixes.iter().any(|p| parts[0].trim_end_matches('.') == *p) {
        1
    } else {
        0
    };

    parts.get(start).map(|s| s.to_string())
}

/// Build re-identification substitution rules.
fn build_subs(ident: &Identity, name_form: &str, content: &str) -> (Vec<Sub>, Vec<String>) {
    let mut subs = Vec::new();
    let mut warnings = Vec::new();

    let client_name = ident.name.as_deref().unwrap_or("");
    let client_title = ident.title.as_deref().unwrap_or("");

    // --- Inline {ID:form} placeholders ---
    // Matches {ANYTHING:first}, {ANYTHING:full}, {ANYTHING:title}
    let placeholder_re = Regex::new(r"\{[A-Za-z0-9+]+:(first|full|title)\}").unwrap();
    let mut seen_placeholders = std::collections::HashSet::new();
    for cap in placeholder_re.captures_iter(content) {
        let whole = cap.get(0).unwrap().as_str().to_string();
        if seen_placeholders.contains(&whole) {
            continue;
        }
        seen_placeholders.insert(whole.clone());

        let form = &cap[1];
        if let Some(replacement) = resolve_client_name(ident, form) {
            subs.push(Sub {
                find: whole,
                replace: replacement,
            });
        }
    }

    // --- {referrer:first} placeholder ---
    if content.contains("{referrer:first}") {
        if let Some(ref_name) = &ident.referrer.name {
            if let Some(first) = referrer_first_name(ref_name) {
                subs.push(Sub {
                    find: "{referrer:first}".to_string(),
                    replace: first,
                });
            } else {
                warnings.push("Could not extract referrer first name".to_string());
            }
        } else {
            warnings.push("No referrer.name in identity.yaml — {referrer:first} not replaced".to_string());
        }
    }

    // --- {referrer:full} placeholder ---
    if content.contains("{referrer:full}") {
        if let Some(ref_name) = &ident.referrer.name {
            subs.push(Sub {
                find: "{referrer:full}".to_string(),
                replace: ref_name.clone(),
            });
        } else {
            warnings.push("No referrer.name in identity.yaml — {referrer:full} not replaced".to_string());
        }
    }

    // --- Legacy bare "Client" replacement (backwards compatibility) ---
    let name_replacement = resolve_client_name(ident, name_form);

    match &name_replacement {
        Some(replacement) => {
            // "Re: Client" → "Re: Title Name" (formal context, always use title form)
            let title_name = if !client_title.is_empty() {
                format!("{} {}", client_title, client_name)
            } else {
                client_name.to_string()
            };
            subs.push(Sub {
                find: "Re: Client".to_string(),
                replace: format!("Re: {}", title_name),
            });

            // General "Client" → chosen name form
            subs.push(Sub {
                find: "Client".to_string(),
                replace: replacement.clone(),
            });
        }
        None => {
            // Only warn about bare Client if there are no inline placeholders
            if seen_placeholders.is_empty() {
                warnings.push(
                    "No client name in identity.yaml — 'Client' not replaced".to_string(),
                );
            }
        }
    }

    // People: "initial (relationship)" → real name
    for person in &ident.people {
        if !person.name.is_empty() && !person.relationship.is_empty() {
            let initial = person.name.chars().next().unwrap();
            let rel_display = person.relationship.replace('_', " ");
            let de_id_form = format!("{} ({})", initial, rel_display);
            subs.push(Sub {
                find: de_id_form,
                replace: person.name.clone(),
            });
        }
    }

    // DOB
    if let Some(dob_str) = &ident.dob {
        if let Ok(dob) = parse_dob(dob_str) {
            // Prefer UK dot format, fallback to UK slash
            let dob_display = dob.format("%d.%m.%Y").to_string();
            subs.push(Sub {
                find: "[DOB removed]".to_string(),
                replace: dob_display,
            });
        }
    }

    // Policy number
    if let Some(policy) = &ident.funding.policy {
        if !policy.is_empty() {
            subs.push(Sub {
                find: "[policy number removed]".to_string(),
                replace: policy.clone(),
            });
        }
    }

    // Address
    if let Some(addr) = &ident.address {
        if !addr.is_empty() {
            subs.push(Sub {
                find: "[address removed]".to_string(),
                replace: addr.clone(),
            });
        }
    }

    // Phone
    if let Some(phone) = &ident.phone {
        if !phone.is_empty() {
            subs.push(Sub {
                find: "[phone removed]".to_string(),
                replace: phone.clone(),
            });
        }
    }

    // Email
    if let Some(email) = &ident.email {
        if !email.is_empty() {
            subs.push(Sub {
                find: "[email removed]".to_string(),
                replace: email.clone(),
            });
        }
    }

    // Ambiguous markers
    if content.contains("[number removed]") {
        warnings.push(
            "'[number removed]' found — review manually (could be NHS, phone, etc.)".to_string(),
        );
    }
    if content.contains("their organisation") {
        warnings.push(
            "'their organisation' found — review manually (entity name not reversible)"
                .to_string(),
        );
    }

    (subs, warnings)
}

/// Parse a DOB string (ISO or UK format).
fn parse_dob(dob_str: &str) -> Result<NaiveDate> {
    if let Ok(d) = NaiveDate::parse_from_str(dob_str, "%Y-%m-%d") {
        return Ok(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(dob_str, "%d/%m/%Y") {
        return Ok(d);
    }
    bail!("Could not parse DOB: {}", dob_str)
}

/// Print dry-run output.
fn print_dry_run(subs: &[Sub], warnings: &[String], content: &str) -> Result<()> {
    println!("Re-identification rules:");
    println!();

    for sub in subs {
        let found = content.contains(&sub.find);
        let marker = if found { "MATCH" } else { "     " };
        println!("  {}: \"{}\" -> \"{}\"", marker, sub.find, sub.replace);
    }
    println!();

    if !warnings.is_empty() {
        println!("Warnings:");
        for w in warnings {
            println!("  - {}", w);
        }
        println!();
    }

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
            aliases: vec!["Jane".to_string()],
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
            funding: Funding {
                policy: Some("AXA-PP-123456".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_re_identify_full_pipeline() {
        let ident = test_identity();
        let de_identified = "Dear William,\n\n\
            Re: Client\n\n\
            Client reports difficulties since her relationship with T (partner) ended.\n\
            S (mother) has been supportive. She works at their organisation.\n\
            Policy: [policy number removed].\n";

        let (subs, warnings) = build_subs(&ident, "full", de_identified);
        let mut sorted = subs;
        sorted.sort_by(|a, b| b.find.len().cmp(&a.find.len()));

        let mut result = de_identified.to_string();
        for sub in &sorted {
            result = result.replace(&sub.find, &sub.replace);
        }

        assert!(result.contains("Re: Ms Jane Bloggs"));
        assert!(result.contains("Jane Bloggs reports"));
        assert!(result.contains("Tom ended"));
        assert!(result.contains("Sandra has been"));
        assert!(result.contains("AXA-PP-123456"));

        // "their organisation" is ambiguous — not replaced, but warning emitted
        assert!(result.contains("their organisation"));
        assert!(warnings
            .iter()
            .any(|w| w.contains("their organisation")));
    }

    #[test]
    fn test_inline_placeholders_mixed_forms() {
        let ident = test_identity();
        let content = "Re: {JB92:full}\n\nDear {referrer:first},\n\n\
            {JB92:first} has been engaging well. \
            I would recommend continued sessions for {JB92:title}.\n";

        let (subs, _) = build_subs(&ident, "full", content);
        let mut sorted = subs;
        sorted.sort_by(|a, b| b.find.len().cmp(&a.find.len()));

        let mut result = content.to_string();
        for sub in &sorted {
            result = result.replace(&sub.find, &sub.replace);
        }

        assert!(result.contains("Re: Jane Bloggs"));
        assert!(result.contains("Dear Sarah,"));
        assert!(result.contains("Jane has been"));
        assert!(result.contains("sessions for Ms Bloggs"));
    }

    #[test]
    fn test_referrer_first_name_with_prefix() {
        assert_eq!(referrer_first_name("Dr Sarah Smith"), Some("Sarah".to_string()));
        assert_eq!(referrer_first_name("Prof James Wilson"), Some("James".to_string()));
        assert_eq!(referrer_first_name("Laura Pollock"), Some("Laura".to_string()));
        assert_eq!(referrer_first_name("Dr. Anna Lee"), Some("Anna".to_string()));
    }

    #[test]
    fn test_referrer_full_placeholder() {
        let ident = test_identity();
        let content = "Referrer: {referrer:full}";
        let (subs, _) = build_subs(&ident, "full", content);

        let ref_sub = subs.iter().find(|s| s.find == "{referrer:full}").unwrap();
        assert_eq!(ref_sub.replace, "Dr Sarah Smith");
    }

    #[test]
    fn test_inline_placeholders_coexist_with_legacy() {
        let ident = test_identity();
        // Content with both inline placeholders and bare "Client"
        let content = "{JB92:first} is doing well. Client attended regularly.";
        let (subs, _) = build_subs(&ident, "full", content);

        let inline = subs.iter().find(|s| s.find == "{JB92:first}").unwrap();
        assert_eq!(inline.replace, "Jane");

        let legacy = subs.iter().find(|s| s.find == "Client").unwrap();
        assert_eq!(legacy.replace, "Jane Bloggs");
    }

    #[test]
    fn test_name_form_first() {
        let ident = test_identity();
        let content = "Client is doing well.";
        let (subs, _) = build_subs(&ident, "first", content);

        let client_sub = subs.iter().find(|s| s.find == "Client").unwrap();
        assert_eq!(client_sub.replace, "Jane");
    }

    #[test]
    fn test_name_form_title() {
        let ident = test_identity();
        let content = "Client is doing well.";
        let (subs, _) = build_subs(&ident, "title", content);

        let client_sub = subs.iter().find(|s| s.find == "Client").unwrap();
        assert_eq!(client_sub.replace, "Ms Bloggs");
    }

    #[test]
    fn test_name_form_full() {
        let ident = test_identity();
        let content = "Client is doing well.";
        let (subs, _) = build_subs(&ident, "full", content);

        let client_sub = subs.iter().find(|s| s.find == "Client").unwrap();
        assert_eq!(client_sub.replace, "Jane Bloggs");
    }

    #[test]
    fn test_re_client_always_formal() {
        let ident = test_identity();
        let content = "Re: Client";
        let (subs, _) = build_subs(&ident, "first", content);

        let re_sub = subs.iter().find(|s| s.find == "Re: Client").unwrap();
        assert_eq!(re_sub.replace, "Re: Ms Jane Bloggs");
    }

    #[test]
    fn test_output_filename_strips_id() {
        let name = "2026-02-14-EB88-referral.md";
        let result = name.replace(&format!("{}-", "EB88"), "");
        assert_eq!(result, "2026-02-14-referral.md");
    }

    #[test]
    fn test_people_re_identification() {
        let ident = test_identity();
        let content = "T (partner) and S (mother) were discussed.";
        let (subs, _) = build_subs(&ident, "full", content);

        let mut sorted = subs;
        sorted.sort_by(|a, b| b.find.len().cmp(&a.find.len()));

        let mut result = content.to_string();
        for sub in &sorted {
            result = result.replace(&sub.find, &sub.replace);
        }

        assert!(result.contains("Tom"));
        assert!(result.contains("Sandra"));
        assert!(!result.contains("T (partner)"));
        assert!(!result.contains("S (mother)"));
    }

    #[test]
    fn test_ambiguous_warnings() {
        let ident = test_identity();
        let content = "She works at their organisation. Call [number removed].";
        let (_, warnings) = build_subs(&ident, "full", content);

        assert!(warnings.iter().any(|w| w.contains("their organisation")));
        assert!(warnings.iter().any(|w| w.contains("[number removed]")));
    }
}
