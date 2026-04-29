use anyhow::{Context, Result, bail};
use mail_parser::{Message, MessageParser, MimeHeaders, PartType};
use std::collections::BTreeMap;
use std::io::Read;

use crate::manifest::{self, HtmlManifest, Manifest, PdfManifest};

pub fn run(port: u16, no_open: bool) -> Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).context("reading stdin")?;
    if bytes.is_empty() {
        bail!("no input on stdin (run via `meli :pipe-message meliview pipe`)");
    }

    let id = uuid::Uuid::new_v4().to_string();
    let dir = manifest::cache_root()?.join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;

    // Try parsing the input as-is first (the patched-meli + x-meli-pipe-message
    // path delivers a complete RFC822 message). If parsing yields nothing
    // useful — typically because we got bare HTML bytes from a vanilla MUA
    // that doesn't honour x-meli-pipe-message — fall back to wrapping the
    // input in a minimal RFC822 envelope and re-parsing. cid: assets won't
    // resolve in the fallback path (sibling parts aren't in scope) but at
    // least the HTML body renders without a hard error.
    let m = match MessageParser::default().parse(&bytes[..]) {
        Some(msg) if has_renderable_part(&msg) => build_manifest(&msg, &dir)?,
        _ => {
            let wrapped = wrap_bare_html(&bytes);
            let msg = MessageParser::default()
                .parse(&wrapped[..])
                .context("parsing RFC822 message (after bare-HTML envelope wrap)")?;
            build_manifest(&msg, &dir)?
        }
    };
    manifest::write(&dir, &m)?;

    let url = format!("http://127.0.0.1:{port}/v/{id}");
    eprintln!("meliview: {url}");
    if !no_open {
        let _ = open::that_detached(&url);
    }
    Ok(())
}

/// True when the parsed message has at least one HTML body or a PDF
/// attachment — i.e. something `build_manifest` can render.
fn has_renderable_part(msg: &Message<'_>) -> bool {
    if msg.html_body_count() > 0 {
        return true;
    }
    msg.attachments().any(|p| {
        p.content_type()
            .map(|ct| {
                ct.ctype().eq_ignore_ascii_case("application")
                    && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("pdf"))
            })
            .unwrap_or(false)
    })
}

/// Wrap bare HTML (or anything resembling it) in a minimal RFC822 envelope so
/// mail-parser produces a Message with one html_body. Used as the fallback
/// path when the upstream MUA gave us part-only bytes rather than the full
/// envelope. No cid: resolution possible from this path because sibling
/// parts are not in scope.
fn wrap_bare_html(body: &[u8]) -> Vec<u8> {
    const HEADERS: &[u8] = b"Content-Type: text/html; charset=utf-8\r\nSubject: (HTML part)\r\n\r\n";
    let mut out = Vec::with_capacity(HEADERS.len() + body.len());
    out.extend_from_slice(HEADERS);
    out.extend_from_slice(body);
    out
}

fn build_manifest(msg: &Message<'_>, dir: &std::path::Path) -> Result<Manifest> {
    let subject = msg.subject().map(|s| s.to_string());
    let from = msg
        .from()
        .and_then(|a| a.first())
        .and_then(|a| a.address.as_deref().map(|s| s.to_string()));
    let date = msg.date().map(|d| d.to_rfc3339());

    if msg.html_body_count() > 0 {
        let html = msg
            .body_html(0)
            .context("body_html(0) returned None despite html_body_count > 0")?
            .into_owned();

        // Collect inline assets keyed by raw cid (no <>).
        // Iterate attachments() so we don't treat the message's own html/text body
        // as inline assets (mail-parser assigns them internal placeholder CIDs).
        let mut assets: BTreeMap<String, String> = BTreeMap::new();
        std::fs::create_dir_all(dir.join("cid")).ok();

        for part in msg.attachments() {
            let Some(cid) = part.content_id() else { continue };
            let cid = strip_brackets(cid).to_string();
            let bytes: &[u8] = match &part.body {
                PartType::Binary(b) | PartType::InlineBinary(b) => b.as_ref(),
                PartType::Text(s) | PartType::Html(s) => s.as_bytes(),
                _ => continue,
            };
            let ext = part
                .content_type()
                .and_then(|ct| {
                    let mime = format!(
                        "{}/{}",
                        ct.ctype(),
                        ct.subtype().unwrap_or("octet-stream")
                    );
                    mime_guess::get_mime_extensions_str(&mime)
                        .and_then(|exts| exts.first().map(|s| s.to_string()))
                })
                .unwrap_or_else(|| "bin".to_string());
            let safe = sanitize_filename(&cid);
            let filename = format!("{safe}.{ext}");
            let dest = dir.join("cid").join(&filename);
            std::fs::write(&dest, bytes)
                .with_context(|| format!("writing inline asset {}", dest.display()))?;
            assets.insert(cid, filename);
        }

        // Rewrite cid: references in the HTML to relative URLs.
        let rewritten = rewrite_cid_refs(&html, &assets);
        let html_file = "body.html";
        std::fs::write(dir.join(html_file), rewritten)
            .with_context(|| format!("writing {}/{}", dir.display(), html_file))?;

        Ok(Manifest::Html(HtmlManifest {
            subject,
            from,
            date,
            html_file: html_file.to_string(),
            assets,
        }))
    } else if let Some(pdf_part) = msg.attachments().find(|p| {
        p.content_type()
            .map(|ct| ct.ctype().eq_ignore_ascii_case("application") && ct.subtype().is_some_and(|s| s.eq_ignore_ascii_case("pdf")))
            .unwrap_or(false)
    }) {
        let bytes: &[u8] = match &pdf_part.body {
            PartType::Binary(b) | PartType::InlineBinary(b) => b.as_ref(),
            _ => bail!("application/pdf part had unexpected body type"),
        };
        let pdf_file = "doc.pdf";
        std::fs::write(dir.join(pdf_file), bytes)
            .with_context(|| format!("writing {}/{}", dir.display(), pdf_file))?;
        Ok(Manifest::Pdf(PdfManifest {
            subject,
            from,
            date,
            pdf_file: pdf_file.to_string(),
            pdf_filename: pdf_part.attachment_name().map(|s| s.to_string()),
        }))
    } else {
        bail!("no text/html body and no application/pdf attachment found")
    }
}

fn strip_brackets(s: &str) -> &str {
    s.trim().trim_start_matches('<').trim_end_matches('>')
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Rewrite `cid:NAME` references in HTML to `cid/<sanitized>.<ext>` relative URLs.
/// Handles both src="cid:..." and src='cid:...' forms (and background, href, etc.).
fn rewrite_cid_refs(html: &str, assets: &BTreeMap<String, String>) -> String {
    if assets.is_empty() {
        return html.to_string();
    }
    // Cheap approach: scan for "cid:" preceded by a quote, replace through the closing quote.
    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for a quote followed by cid:
        let c = bytes[i];
        if (c == b'"' || c == b'\'') && bytes[i + 1..].starts_with(b"cid:") {
            let quote = c;
            let start = i + 1 + 4; // past the cid:
            // Find closing quote
            let Some(end_off) = bytes[start..].iter().position(|&b| b == quote) else {
                out.push(c as char);
                i += 1;
                continue;
            };
            let end = start + end_off;
            let raw = &html[start..end];
            let cid = strip_brackets(raw);
            let replaced = match assets.get(cid) {
                Some(filename) => format!("{}cid/{}{}", quote as char, filename, quote as char),
                None => format!("{}cid:{}{}", quote as char, raw, quote as char),
            };
            out.push_str(&replaced);
            i = end + 1;
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_double_quoted_cid() {
        let mut assets = BTreeMap::new();
        assets.insert("abc@x".into(), "abc_x.png".into());
        let html = r#"<img src="cid:abc@x">"#;
        assert_eq!(rewrite_cid_refs(html, &assets), r#"<img src="cid/abc_x.png">"#);
    }

    #[test]
    fn rewrites_single_quoted_cid() {
        let mut assets = BTreeMap::new();
        assets.insert("a".into(), "a.png".into());
        let html = "<img src='cid:a'>";
        assert_eq!(rewrite_cid_refs(html, &assets), "<img src='cid/a.png'>");
    }

    #[test]
    fn unknown_cid_left_alone() {
        let assets = BTreeMap::new();
        let html = r#"<img src="cid:unknown">"#;
        assert_eq!(rewrite_cid_refs(html, &assets), html);
    }

    #[test]
    fn strips_brackets_in_lookup() {
        let mut assets = BTreeMap::new();
        assets.insert("abc@x".into(), "abc_x.png".into());
        let html = r#"<img src="cid:<abc@x>">"#;
        assert_eq!(rewrite_cid_refs(html, &assets), r#"<img src="cid/abc_x.png">"#);
    }

    #[test]
    fn sanitize_keeps_safe_chars() {
        assert_eq!(sanitize_filename("abc-1_2.def"), "abc-1_2_def");
    }
}
