use anyhow::{Context, Result};
use std::process::{Command, Stdio};

/// Send the initial "you have a letter" email with the secure link
pub fn send_link_email(
    recipient_email: &str,
    recipient_name: &str,
    link: &str,
    expiry_days: u32,
) -> Result<()> {
    let subject = "Clinical Letter Available";
    let body = format!(
        "Dear {recipient_name},\n\
         \n\
         A clinical letter is available for you. Please click the link below to access it securely.\n\
         \n\
         {link}\n\
         \n\
         You will be asked to verify your email address before the document is displayed.\n\
         \n\
         This link will expire in {expiry_days} days.\n\
         \n\
         Kind regards,\n\
         William Napier\n\
         Chartered Counselling Psychologist\n\
         CPsychol\n\
         \n\
         Change of Harley Street\n\
         37 Gloucester Place, London W1U 8JA\n\
         Tel: 020 3774 6533"
    );

    send_email(recipient_email, subject, &body)
}

/// Send OTP verification code
pub fn send_otp_email(recipient_email: &str, code: &str) -> Result<()> {
    let subject = "Verification Code";
    let body = format!(
        "Your verification code is: {code}\n\
         \n\
         This code is valid for 10 minutes.\n\
         \n\
         If you did not request this, please ignore this email."
    );

    send_email(recipient_email, subject, &body)
}

/// Notify Leigh that a letter has been sent and needs TM3 filing
pub fn send_notify_email(
    recipient_name: &str,
    recipient_email: &str,
    client_id: &str,
    filename: &str,
) -> Result<()> {
    let notify_email = std::env::var("CLINICAL_NOTIFY_EMAIL").ok();
    let notify_email = match notify_email {
        Some(e) if !e.is_empty() => e,
        _ => return Ok(()), // No notification configured — silently skip
    };

    let subject = format!("Action needed: {} letter sent", client_id);
    let body = format!(
        "Hi Leigh,\n\
         \n\
         A clinical letter has been sent securely:\n\
         \n\
         Client: {client_id}\n\
         Sent to: {recipient_name} ({recipient_email})\n\
         File: {filename}\n\
         \n\
         Please upload the PDF to TM3. You can find it in the client's \
         private/letters/ folder in Dropbox.\n\
         \n\
         This is an automated notification — no reply needed."
    );

    send_email(&notify_email, &subject, &body)
}

/// Route email through the appropriate backend
fn send_email(to: &str, subject: &str, body: &str) -> Result<()> {
    if let Ok(api_key) = std::env::var("RESEND_API_KEY") {
        send_via_resend(to, subject, body, &api_key)
    } else {
        send_via_himalaya(to, subject, body)
    }
}

/// Send email via Resend API (production — used when RESEND_API_KEY is set)
fn send_via_resend(to: &str, subject: &str, body: &str, api_key: &str) -> Result<()> {
    let from = std::env::var("CLINICAL_FROM_EMAIL")
        .unwrap_or_else(|_| "Clinical Portal <noreply@willnapier.com>".to_string());

    let payload = serde_json::json!({
        "from": from,
        "to": [to],
        "subject": subject,
        "text": body
    });

    let client = reqwest::blocking::Client::new();
    let resp = client
        .post("https://api.resend.com/emails")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .body(payload.to_string())
        .send()
        .context("Failed to call Resend API")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        anyhow::bail!("Resend API error ({status}): {body}");
    }

    Ok(())
}

/// Send email via himalaya (local dev — fallback when no RESEND_API_KEY)
fn send_via_himalaya(to: &str, subject: &str, body: &str) -> Result<()> {
    let from = std::env::var("CLINICAL_FROM_EMAIL")
        .unwrap_or_else(|_| "William Napier <will@willnapier.com>".to_string());

    let template = format!(
        "From: {from}\n\
         To: {to}\n\
         Subject: {subject}\n\
         \n\
         {body}"
    );

    let mut child = Command::new("himalaya")
        .args(["template", "send"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to launch himalaya — is it installed and in PATH?")?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(template.as_bytes())
            .context("Failed to write to himalaya stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("Failed to wait for himalaya")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("himalaya failed: {stderr}");
    }

    Ok(())
}
