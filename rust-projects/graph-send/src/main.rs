//! graph-send — sendmail-style MIME forwarder via Microsoft Graph /me/sendMail.
//!
//! For M365 tenants where SmtpClientAuthenticationDisabled blocks SMTP
//! submission entirely (the COHS tenant is our case), Graph is the only
//! working send path. This binary is the standalone, vendor-neutral
//! sender that meli, mutt, or any compose pipeline can pipe MIME into.
//!
//! Usage:
//!     <mua produces MIME on stdout> | graph-send
//!     <mua produces MIME on stdout> | graph-send --account my-other-graph
//!
//! Token source: `pizauth show <account>`. Defaults to `cohs-graph`.
//! Override with --account or GRAPH_SEND_ACCOUNT env var.
//!
//! Scope of this binary
//! --------------------
//! Generic, non-clinical mail. Used by:
//!   - meli's COHS send_mail line (~/dotfiles/meli/config.toml)
//!   - any future Helix-driven compose pipeline
//!   - any script piping MIME for COHS or other Graph-only tenants
//!
//! NOT used by practiceforge for its own outbound mail (invoice emails,
//! GP letters, dashboard OTP). Practiceforge has its own in-tree Graph
//! transport (`crates/.../email/backends/graph.rs`) — sibling, not child.
//! Both consume `pizauth show <account>` for tokens; both POST to the
//! same `/me/sendMail` endpoint. The duplication is intentional: the
//! standalone binary's typed-stdin API is wrong for practiceforge's
//! programmatic structured-Envelope sends.
//!
//! Multi-practitioner deployment
//! -----------------------------
//! The default account name `cohs-graph` is a *convention*, not a
//! William-specific value — any COHS practitioner who follows the
//! convention can install this binary unchanged and have it work. For
//! personal/non-COHS Graph identities (e.g. a colleague's own M365
//! tenant), use `--account <their-account-name>`.
//!
//! Colleagues using practiceforge as a deployed clinical product DO NOT
//! need this binary on their machine. Their clinical mail flows through
//! practiceforge's in-tree transport. This binary is part of an
//! optional generic mail stack (meli + msmtp + graph-send) that lives
//! alongside practiceforge for users who also want a TUI mail reader.

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
