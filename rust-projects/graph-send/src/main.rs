//! graph-send — sendmail-style MIME forwarder via Microsoft Graph /me/sendMail.
//!
//! For M365 tenants where SmtpClientAuthenticationDisabled blocks SMTP
//! submission entirely (the COHS tenant is our case), Graph is the only
//! working send path. This binary is the standalone, vendor-neutral
//! sender that meli, mutt, or any compose pipeline can pipe MIME into.
//!
//! Usage:
//!     <mua produces MIME on stdout> | graph-send
//!
//! Token source: `pizauth show <account>`. Defaults to `cohs-graph`.
//! Override with --account or GRAPH_SEND_ACCOUNT env var.
//!
//! Why this is its own binary (not part of practiceforge): practiceforge
//! is a clinical practice product that happens to send mail. Generic
//! mail sending — for meli, scripts, the Helix-driven compose flow —
//! shouldn't depend on a clinical app. Practiceforge's own mail (invoice
//! emails, letters to GPs, dashboard OTP) still calls its in-tree Graph
//! transport directly; this binary serves the rest of the system.

use anyhow::{anyhow, bail, Context, Result};
use data_encoding::BASE64;
use std::io::Read;
use std::process::Command;
use std::time::Duration;

const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/v1.0";
const DEFAULT_ACCOUNT: &str = "cohs-graph";

fn main() -> Result<()> {
    let account = parse_account_arg()?;

    let mut mime = Vec::new();
    std::io::stdin()
        .read_to_end(&mut mime)
        .context("reading MIME message from stdin")?;
    if mime.is_empty() {
        bail!("empty MIME message on stdin — nothing to send");
    }

    let token = pizauth_token(&account)?;
    send_mime(&token, &mime)?;

    eprintln!("✓ Sent via Graph ({} bytes MIME, account: {account})", mime.len());
    Ok(())
}

/// Parse `--account NAME` from argv, falling back to GRAPH_SEND_ACCOUNT
/// env var, then to DEFAULT_ACCOUNT. Tolerates being called with no args
/// (the meli/sendmail use case).
fn parse_account_arg() -> Result<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--account" => {
                return args
                    .next()
                    .ok_or_else(|| anyhow!("--account requires a value"));
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other}"),
        }
    }
    Ok(std::env::var("GRAPH_SEND_ACCOUNT").unwrap_or_else(|_| DEFAULT_ACCOUNT.to_string()))
}

fn print_help() {
    eprintln!(
        "graph-send — pipe MIME via Microsoft Graph /me/sendMail\n\
         \n\
         USAGE:\n\
             <mua> | graph-send [--account <name>]\n\
         \n\
         OPTIONS:\n\
             --account <name>   pizauth account to fetch token from\n\
                                (default: cohs-graph, env: GRAPH_SEND_ACCOUNT)\n\
             -h, --help         show this help"
    );
}

fn pizauth_token(account: &str) -> Result<String> {
    let out = Command::new("pizauth")
        .args(["show", account])
        .output()
        .with_context(|| format!("invoking `pizauth show {account}`"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("pizauth show {account} failed: {}", stderr.trim());
    }
    let token = String::from_utf8(out.stdout)
        .context("pizauth output was not UTF-8")?
        .trim()
        .to_string();
    if token.is_empty() {
        bail!("pizauth returned empty token for account {account}");
    }
    Ok(token)
}

fn send_mime(token: &str, mime: &[u8]) -> Result<()> {
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building HTTP client")?;

    let url = format!("{GRAPH_BASE_URL}/me/sendMail");
    let encoded = BASE64.encode(mime);

    let resp = http
        .post(&url)
        .bearer_auth(token)
        .header("Content-Type", "text/plain")
        .body(encoded)
        .send()
        .with_context(|| format!("POST {url}"))?;

    let status = resp.status();
    if status.as_u16() == 202 {
        return Ok(());
    }

    let body = resp.text().unwrap_or_else(|_| "<no body>".to_string());
    let preview: String = body.chars().take(800).collect();
    let hint = match status.as_u16() {
        401 => " — token expired or revoked; run `pizauth refresh <account>`",
        403 => " — token missing Mail.Send scope; re-consent via pizauth",
        _ => "",
    };
    Err(anyhow!(
        "Graph /me/sendMail failed: HTTP {status}{hint}. Response body: {preview}"
    ))
}
