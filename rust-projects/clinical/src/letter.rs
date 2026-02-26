use anyhow::{bail, Context, Result};

use crate::client;
use crate::markdown;
use crate::session;

/// Run `clinical update-letter`.
pub fn run(id: &str, dry_run: bool) -> Result<()> {
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
    let referring_doctor = markdown::extract_field(&content, "Referring doctor");

    // Count total sessions
    let lines: Vec<&str> = content.lines().collect();
    let session_section_idx = session::find_session_section(&lines).unwrap_or(0);
    let total_sessions = session::count_sessions(&lines[(session_section_idx + 1)..]);

    let session_count_field = markdown::extract_field(&content, "Session count");
    let session_info = if let Some(count) = session_count_field {
        format!("{} sessions", count)
    } else {
        format!("{} documented sessions", total_sessions)
    };

    let draft = format!(
        "Thank you for referring the above-named client, who has been attending approximately \
weekly sessions since {therapy_commenced}. This letter provides a clinical update at {session_info}.\n\
\n\
PRESENTING CONCERNS\n\
\n\
[Brief recap of the referral reason and initial presentation]\n\
\n\
THERAPEUTIC APPROACH\n\
\n\
[Brief description of the therapeutic model and approach used]\n\
\n\
PROGRESS\n\
\n\
[Key developments, shifts in presentation, gains made]\n\
\n\
CURRENT FOCUS AND PLAN\n\
\n\
[What therapy is currently addressing and anticipated direction]"
    );

    if dry_run {
        println!("--- Draft update letter for {} ---", id);
        println!();
        println!("{}", draft);
        return Ok(());
    }

    let drafts_dir = client::drafts_dir(id);
    std::fs::create_dir_all(&drafts_dir)
        .with_context(|| format!("Failed to create: {}", drafts_dir.display()))?;

    let today = chrono::Local::now().format("%Y-%m-%d");
    let draft_path = drafts_dir.join(format!("{}-update-draft.md", today));

    std::fs::write(&draft_path, &draft)
        .with_context(|| format!("Failed to write: {}", draft_path.display()))?;

    println!("Created: {}", draft_path.display());
    if let Some(ref doc) = referring_doctor {
        println!("To: {}", doc);
    }
    println!();
    println!("Edit the draft body, then build with:");
    println!(
        "  clinical-letter-build {} --draft {}",
        id,
        draft_path.display()
    );
    println!();
    println!("The template uses referrer details from identity.yaml for the addressee.");

    Ok(())
}
