//! Twilio REST API client for sending SMS messages.
//!
//! Uses the Messages API:
//! `POST https://api.twilio.com/2010-04-01/Accounts/{sid}/Messages.json`
//! with HTTP Basic auth (account_sid:auth_token).

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::config::SmsConfig;

/// Result of sending an SMS via Twilio.
#[derive(Debug, Clone)]
pub struct SmsResult {
    /// Twilio message SID (e.g. "SMxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx").
    pub message_sid: String,
    /// Twilio status (e.g. "queued", "sent", "delivered", "failed").
    pub status: String,
    /// Error message if the send failed.
    pub error_message: Option<String>,
}

/// Twilio API response for creating a message.
#[derive(Debug, Deserialize)]
struct TwilioMessageResponse {
    sid: Option<String>,
    status: Option<String>,
    error_code: Option<u32>,
    error_message: Option<String>,
}

/// Send an SMS via the Twilio REST API.
///
/// `to` should be in E.164 format (e.g. "+447700900000").
/// `body` is the message text (max 1600 chars for Twilio).
pub async fn send_sms(config: &SmsConfig, to: &str, body: &str) -> Result<SmsResult> {
    let auth_token = config.resolve_auth_token();
    if config.twilio_account_sid.is_empty() || auth_token.is_empty() {
        bail!("Twilio credentials not configured. Set twilio_account_sid and twilio_auth_token in [sms] config.");
    }

    if config.twilio_from_number.is_empty() {
        bail!("twilio_from_number not configured in [sms] config.");
    }

    let url = format!(
        "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
        config.twilio_account_sid
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .basic_auth(&config.twilio_account_sid, Some(auth_token))
        .form(&[
            ("From", config.twilio_from_number.as_str()),
            ("To", to),
            ("Body", body),
        ])
        .send()
        .await
        .context("Failed to send request to Twilio API")?;

    let status_code = response.status();
    let response_text = response
        .text()
        .await
        .context("Failed to read Twilio API response")?;

    let parsed: TwilioMessageResponse = serde_json::from_str(&response_text)
        .context("Failed to parse Twilio API response")?;

    if !status_code.is_success() {
        return Ok(SmsResult {
            message_sid: parsed.sid.unwrap_or_default(),
            status: "failed".to_string(),
            error_message: Some(
                parsed
                    .error_message
                    .unwrap_or_else(|| format!("HTTP {}", status_code)),
            ),
        });
    }

    if let Some(error_code) = parsed.error_code {
        return Ok(SmsResult {
            message_sid: parsed.sid.unwrap_or_default(),
            status: "failed".to_string(),
            error_message: Some(format!(
                "Twilio error {}: {}",
                error_code,
                parsed.error_message.unwrap_or_default()
            )),
        });
    }

    Ok(SmsResult {
        message_sid: parsed.sid.unwrap_or_default(),
        status: parsed.status.unwrap_or_else(|| "queued".to_string()),
        error_message: None,
    })
}
