use anyhow::{Context, Result, bail};
use mail_parser::{Message, MessageParser, MimeHeaders, PartType};
use std::collections::BTreeMap;
use std::io::Read;

use crate::manifest::{self, HtmlManifest, Manifest, PdfManifest};

pub fn run(port: u16, no_open: bool) -> Result<()> {
    // Watchdog: meli's mailcap dispatch waits synchronously for this process
    // to exit. If anything in the pipeline hangs (transient races with mbsync
    // mid-writing the maildir file, browser-launch handle weirdness on macOS,
    // unforeseen edge cases), meli freezes indefinitely. Hard-bound the
    // entire operation so the worst case is "browser doesn't open and meli
    // unfreezes after 15s with an error notification" rather than a freeze
    // that requires ^kill -TERM to recover.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(15));
        log_step("15s timeout exceeded; aborting to unblock caller");
        std::process::exit(124);
    });

    log_step("reading stdin");
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).context("reading stdin")?;
    if bytes.is_empty() {
        bail!("no input on stdin (run via `meli :pipe-message mailforge pipe`)");
    }
    log_step(&format!("read {} bytes", bytes.len()));

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
    log_step("parsing");
    let m = match MessageParser::default().parse(&bytes[..]) {
        Some(msg) if has_renderable_part(&msg) => {
            log_step("building manifest from full RFC822");
            build_manifest(&msg, &dir)?
        }
        _ => {
            log_step("falling back to bare-HTML wrap");
            let wrapped = wrap_bare_html(&bytes);
            let msg = MessageParser::default()
                .parse(&wrapped[..])
                .context("parsing RFC822 message (after bare-HTML envelope wrap)")?;
            build_manifest(&msg, &dir)?
        }
    };
    log_step("writing manifest");
    manifest::write(&dir, &m)?;

    let url = format!("http://127.0.0.1:{port}/v/{id}");
    log_step(&format!("opening URL {url}"));
    // Print URL once to stdout so callers (e.g. ad-hoc CLI invocations) see
    // it. meli's mailcap dispatch ignores child stdout when copiousoutput is
    // unset, so this doesn't pollute meli's TUI.
    println!("mailforge: {url}");
    if !no_open {
        log_step("opening browser");
        let _ = open::that_detached(&url);
    }
    log_step("done");
    Ok(())
}

/// Append a timestamped diagnostic step to ~/.cache/mailforge/pipe.log.
/// Step-by-step trace makes future hangs diagnosable: when meli's mailcap
/// dispatch wedges, the last log line tells us exactly which phase blocked.
/// Fails silently to avoid escalating logging issues into pipeline failures.
fn log_step(msg: &str) {
    use std::io::Write;
    let Ok(cache) = manifest::cache_root() else { return };
    let _ = std::fs::create_dir_all(&cache);
    let log_path = cache.join("pipe.log");
    let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&log_path)
    else {
        return;
    };
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    let _ = writeln!(f, "{ts} pid={pid} {msg}");
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
        // Collapse anchors whose visible text is the URL itself (or otherwise
        // overwhelms the layout). Replaces visible text with `[<domain>]`
        // while keeping the href intact so click-through still works. No
        // third-party services involved — purely local string substitution.
        let rewritten = collapse_long_url_anchors(&rewritten);
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
///
/// Operates on raw bytes through a Vec<u8> rather than a String. Critical:
/// `out.push(c as char)` where `c: u8` would convert each non-ASCII byte to
/// its codepoint-as-Latin-1 equivalent and then UTF-8-re-encode it, double-
/// encoding every multi-byte UTF-8 sequence in the input (smart quotes,
/// accented characters, emoji, etc.) and producing mojibake. Byte-level
/// passthrough is correct because UTF-8 guarantees that `"`, `'`, and the
/// ASCII chars in `cid:` never appear inside multi-byte sequences — so
/// matching on those bytes won't false-positive mid-codepoint, and copying
/// through the surrounding bytes preserves whatever multi-byte content
/// happens to lie there.
fn rewrite_cid_refs(html: &str, assets: &BTreeMap<String, String>) -> String {
    if assets.is_empty() {
        return html.to_string();
    }
    let bytes = html.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(html.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if (c == b'"' || c == b'\'') && bytes[i + 1..].starts_with(b"cid:") {
            let quote = c;
            let start = i + 1 + 4; // past the cid:
            let Some(end_off) = bytes[start..].iter().position(|&b| b == quote) else {
                out.push(c);
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
            out.extend_from_slice(replaced.as_bytes());
            i = end + 1;
        } else {
            // Copy raw byte; multi-byte UTF-8 sequences pass through intact.
            out.push(c);
            i += 1;
        }
    }
    // Safety: input was &str (valid UTF-8). The scanner only matches on ASCII
    // bytes (which never appear in multi-byte UTF-8 sequences), copies all
    // non-match bytes verbatim, and inserts only valid UTF-8 (format!
    // strings). So `out` is valid UTF-8 by construction.
    String::from_utf8(out).expect(
        "rewrite_cid_refs preserves UTF-8: input was &str, byte ops only on ASCII",
    )
}

/// Collapse `<a href="LONG_URL">VISIBLE_TEXT</a>` anchors where VISIBLE_TEXT
/// is itself URL-shaped or excessively long, replacing the visible text with
/// `[domain]` while keeping the href intact. Click-through still works
/// against the original URL — this is purely visual decluttering.
///
/// Limitation: a single regex pass; doesn't handle anchors with nested HTML
/// inside (e.g. `<a><img></a>` or `<a>some <strong>bold</strong> text</a>`).
/// For those we leave the anchor untouched. That covers ~10% of mass-mailer
/// HTML; the long-URL-as-link-text case (the noisy 90%) is collapsed.
fn collapse_long_url_anchors(html: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // Anchor open tag, capture href, capture inner text up to first `<`.
        // (?is) = case-insensitive + dotall.
        Regex::new(r#"(?is)(<a\b[^>]*\bhref\s*=\s*["']([^"']+)["'][^>]*>)([^<]*)(</a>)"#)
            .unwrap()
    });

    re.replace_all(html, |caps: &regex::Captures<'_>| {
        let opening = &caps[1];
        let href = &caps[2];
        let visible_raw = &caps[3];
        let visible = visible_raw.trim();
        let closing = &caps[4];

        // Decide whether to collapse. Three triggers, in order of certainty:
        //  - visible text equals the href (the "raw URL pasted as link" case)
        //  - visible text starts with http(s):// (a URL with weird whitespace)
        //  - visible text is just plain long (>= 80 chars, no spaces)
        let is_url_text = visible == href
            || visible.starts_with("http://")
            || visible.starts_with("https://");
        let is_long_unbroken = visible.len() >= 80 && !visible.contains(' ');

        if !is_url_text && !is_long_unbroken {
            return caps[0].to_string();
        }

        let domain = extract_domain(href).unwrap_or_else(|| "link".to_string());
        format!("{opening}[{domain}]{closing}")
    })
    .into_owned()
}

/// Extract a friendly domain string from a URL, e.g.
/// `https://www.amazon.com/foo?bar=baz` → `amazon.com`.
fn extract_domain(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    let host = parsed.host_str()?;
    // Strip leading "www." for cleanliness.
    Some(host.strip_prefix("www.").unwrap_or(host).to_string())
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

    #[test]
    fn rewrite_cid_refs_preserves_utf8() {
        // Regression: smart apostrophe (U+2019, UTF-8 0xE2 0x80 0x99) and
        // other multi-byte UTF-8 must pass through rewrite_cid_refs unchanged.
        // Earlier code path did `out.push(c as char)` which double-encoded
        // each byte and produced mojibake (Confucius' → Confucius'\u{80}\u{99}).
        let mut assets = BTreeMap::new();
        assets.insert("img1".into(), "img1.png".into());
        let html = r#"Confucius’ philosophy "von Müller" 日本語 <img src="cid:img1">"#;
        let got = rewrite_cid_refs(html, &assets);
        assert!(got.contains("Confucius’"), "smart apostrophe must survive");
        assert!(got.contains("Müller"), "Latin-1 ü must survive");
        assert!(got.contains("日本語"), "CJK must survive");
        assert!(got.contains(r#"src="cid/img1.png""#), "cid rewrite must still fire");
    }

    #[test]
    fn collapse_anchor_with_url_as_text() {
        let html = r#"<a href="https://www.amazon.com/foo/bar?baz=1">https://www.amazon.com/foo/bar?baz=1</a>"#;
        let got = collapse_long_url_anchors(html);
        assert!(got.contains("[amazon.com]"));
        assert!(got.contains(r#"href="https://www.amazon.com/foo/bar?baz=1""#));
    }

    #[test]
    fn collapse_long_unbroken_text() {
        let raw = "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz1234";
        let html = format!(r#"<a href="https://example.com/x">{raw}</a>"#);
        let got = collapse_long_url_anchors(&html);
        assert!(got.contains("[example.com]"));
    }

    #[test]
    fn keep_short_descriptive_link_text() {
        let html = r#"<a href="https://www.amazon.com/foo/bar?baz=1">Buy on Amazon</a>"#;
        let got = collapse_long_url_anchors(html);
        assert_eq!(got, html, "short descriptive text should be preserved");
    }

    #[test]
    fn keep_anchor_with_nested_html() {
        // Regex captures only `[^<]*` so anchors with nested tags fall through.
        let html = r#"<a href="https://example.com/long/path/here">Click <strong>here</strong></a>"#;
        let got = collapse_long_url_anchors(html);
        assert_eq!(got, html, "anchors with nested html should not be touched");
    }

    #[test]
    fn extract_domain_strips_www() {
        assert_eq!(extract_domain("https://www.example.com/x"), Some("example.com".to_string()));
        assert_eq!(extract_domain("https://example.com/x"), Some("example.com".to_string()));
        assert_eq!(extract_domain("not a url"), None);
    }
}
