use anyhow::{bail, Context, Result};
use std::process::Command;

use clinical_core::client;
use clinical_core::identity;

use crate::markdown;
use crate::session;

/// Run `clinical auth status`.
pub fn status(verbose: bool) -> Result<()> {
    let clients_dir = client::clients_dir();
    let client_files = session::find_client_md_files(&clients_dir)?;

    if client_files.is_empty() {
        println!("No client .md files found.");
        return Ok(());
    }

    let mut results = Vec::new();
    let mut skipped = 0u32;

    for (id, path) in &client_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;

        match session::compute_auth_status(id, &content) {
            Some(status) => results.push(status),
            None => {
                skipped += 1;
                if verbose {
                    eprintln!("  Skipped {} (no auth marker)", id);
                }
            }
        }
    }

    if results.is_empty() {
        println!("No clients with auth markers found.");
        println!("{} clients skipped (no auth marker).", skipped);
        return Ok(());
    }

    // Sort by remaining (most urgent first)
    results.sort_by_key(|r| r.remaining);

    println!("  --- Authorisation Status ---");
    println!();

    for row in &results {
        let auth_flag = if row.remaining <= 1 {
            "  URGENT"
        } else if row.remaining <= 2 {
            "  auth letter needed"
        } else {
            ""
        };

        let letter_flag = if !row.letter_status.is_empty() {
            format!("  {}", row.letter_status)
        } else {
            String::new()
        };

        println!(
            "  {:>6}  {:>25}  {:>3}/{:<3} sessions used  {} remaining{}{}",
            row.client_id,
            row.funder,
            row.sessions_used,
            row.sessions_authorised,
            row.remaining,
            auth_flag,
            letter_flag,
        );
    }

    println!();
    println!("  {} clients skipped (no auth marker).", skipped);

    Ok(())
}

/// Run `clinical auth check`.
pub fn check(append: bool) -> Result<()> {
    let clients_dir = client::clients_dir();
    let client_files = session::find_client_md_files(&clients_dir)?;

    let mut warnings = Vec::new();

    for (id, path) in &client_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read: {}", path.display()))?;

        if let Some(status) = session::compute_auth_status(id, &content) {
            if status.remaining <= 2 {
                let msg = if status.remaining <= 1 {
                    format!(
                        "{}: {} session remaining -- auth letter URGENT",
                        id,
                        status.remaining
                    )
                } else {
                    format!(
                        "{}: {} sessions remaining -- auth letter needed",
                        id,
                        status.remaining
                    )
                };
                warnings.push(msg);
            }
        }
    }

    if warnings.is_empty() {
        return Ok(());
    }

    let warning_text = warnings.join(", ");

    // Auth warnings are printed to stdout — PracticeForge dashboard
    // shows block expiry via the scheduling module's maintain command.
    // No DayPage entry needed.
    println!("Auth check: {}", warning_text);
    if !append {
        println!();
        println!("Run with --append to add to today's session log.");
    }

    Ok(())
}

/// Run `clinical auth letter`.
pub fn letter(id: &str, dry_run: bool) -> Result<()> {
    let client_dir = client::client_dir(id);
    if !client_dir.exists() {
        bail!("Client directory not found: {}", client_dir.display());
    }

    let notes_path = client::notes_path(id);
    if !notes_path.exists() {
        bail!("Client file not found: {}", notes_path.display());
    }

    let content = std::fs::read_to_string(&notes_path)
        .with_context(|| format!("Failed to read: {}", notes_path.display()))?;

    let therapy_commenced = markdown::extract_field(&content, "Therapy commenced")
        .unwrap_or_else(|| "[DATE]".to_string());
    let funding =
        markdown::extract_field(&content, "Funding").unwrap_or_else(|| "unknown".to_string());
    let sessions_info = session::sessions_info_string(&content);

    let draft = format!(
        "I am writing to request further authorisation for sessions for the above-named client, \
who has been attending approximately weekly sessions since {therapy_commenced}.\n\
\n\
Current authorisation status: {sessions_info}.\n\
\n\
PRESENTING CONCERNS\n\
\n\
[Brief statement of the initial referral reason and presenting difficulties]\n\
\n\
PROGRESS TO DATE\n\
\n\
[Summary of therapeutic work and key developments demonstrating therapeutic value]\n\
\n\
CLINICAL RATIONALE FOR CONTINUED TREATMENT\n\
\n\
[Why further sessions are needed — remaining therapeutic goals, risk of relapse without continued support]\n\
\n\
REQUEST\n\
\n\
[Number of further sessions requested and anticipated frequency]"
    );

    if dry_run {
        println!("--- Draft auth letter for {} ---", id);
        println!();
        println!("{}", draft);
        return Ok(());
    }

    // Write to drafts/
    let drafts_dir = client::drafts_dir(id);
    std::fs::create_dir_all(&drafts_dir)
        .with_context(|| format!("Failed to create: {}", drafts_dir.display()))?;

    let today = chrono::Local::now().format("%Y-%m-%d");
    let draft_path = drafts_dir.join(format!("{}-authorisation-draft.md", today));

    std::fs::write(&draft_path, &draft)
        .with_context(|| format!("Failed to write: {}", draft_path.display()))?;

    println!("Created: {}", draft_path.display());
    println!();
    println!("This is an AUTHORISATION letter to the insurer.");
    println!("Funding: {}", funding);
    println!();
    println!("Edit the draft body, then build with:");
    println!(
        "  clinical-letter-build {} --draft {} --to \"[Insurer]/[Clinical Team]\"",
        id,
        draft_path.display()
    );

    Ok(())
}

/// Extract all auto-fillable fields for an insurer authorisation form.
/// Outputs JSON ready for Healthcode form filling.
pub fn form(id: &str) -> Result<()> {
    let client_dir = client::client_dir(id);
    if !client_dir.exists() {
        bail!("Client directory not found: {}", client_dir.display());
    }

    // Load identity.yaml
    let id_path = client::identity_path(id);
    let ident = identity::load_identity(&id_path)
        .with_context(|| format!("Failed to load identity: {}", id_path.display()))?;

    // Load client notes for session data
    let notes_path = client::notes_path(id);
    let content = std::fs::read_to_string(&notes_path)
        .with_context(|| format!("Failed to read: {}", notes_path.display()))?;

    // Compute auth status
    let auth = session::compute_auth_status(id, &content);

    // Extract first session date
    let first_session = markdown::extract_field(&content, "Therapy commenced")
        .unwrap_or_default();

    // Extract diagnosis
    let diagnosis = ident.diagnosis.as_deref().unwrap_or("");
    let diagnostic_code = ident.diagnostic_code.as_deref().unwrap_or("");

    // Extract funding details
    let membership = ident.funding.policy.as_deref().unwrap_or("");
    let funder = ident.funding.funding_type.as_deref().unwrap_or("");

    // Extract referrer
    let referrer_name = ident.referrer.name.as_deref().unwrap_or("");

    // Client details
    let client_name = ident.name.as_deref().unwrap_or("");
    let client_title = ident.title.as_deref().unwrap_or("");
    let client_dob = ident.dob.as_deref().unwrap_or("");
    let client_address = ident.address.as_deref().unwrap_or("");
    let client_phone = ident.phone.as_deref().unwrap_or("");

    // Build the form payload
    let payload = serde_json::json!({
        "patient": {
            "name": if !client_title.is_empty() && !client_name.is_empty() {
                format!("{} {}", client_title, client_name)
            } else {
                client_name.to_string()
            },
            "date_of_birth": client_dob,
            "address": client_address,
            "phone": client_phone,
            "membership_number": membership,
            "claim_number": ""
        },
        "specialist": {
            "name": "William Napier",
            "area_of_expertise": "Counselling Psychology",
            "telephone": "020 3774 6533",
            "provider_number": "",
            "company": "Change of Harley Street"
        },
        "referral": {
            "referred_by": referrer_name,
            "funder": funder
        },
        "clinical": {
            "diagnosis": diagnosis,
            "diagnostic_code": diagnostic_code,
            "model_of_therapy": "ACT/CBS (Acceptance and Commitment Therapy / Contextual Behavioural Science)",
            "risk_severity": "",
            "history": "[TO COMPLETE: Full history of condition under this claim]",
            "treatment_to_date": "[TO COMPLETE: What treatment has been provided]",
            "progress_to_date": "[TO COMPLETE: Progress made to date]"
        },
        "sessions": {
            "first_session_date": first_session,
            "total_sessions_to_date": auth.as_ref().map_or(0, |a| a.total_sessions),
            "sessions_authorised": auth.as_ref().map_or(0, |a| a.sessions_authorised),
            "sessions_used_current_auth": auth.as_ref().map_or(0, |a| a.sessions_used),
            "sessions_remaining": auth.as_ref().map_or(0, |a| a.remaining.max(0) as u32),
            "additional_sessions_requested": "[TO COMPLETE: Number requested]",
            "rationale_for_additional": "[TO COMPLETE: Clinical rationale]",
            "estimated_discharge_date": "[TO COMPLETE: Estimated date]"
        },
        "meta": {
            "client_id": id,
            "form_type": "axa_extension",
            "generated": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
            "fields_requiring_input": [
                "patient.claim_number",
                "specialist.provider_number",
                "clinical.risk_severity",
                "clinical.history",
                "clinical.treatment_to_date",
                "clinical.progress_to_date",
                "sessions.additional_sessions_requested",
                "sessions.rationale_for_additional",
                "sessions.estimated_discharge_date"
            ]
        }
    });

    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
