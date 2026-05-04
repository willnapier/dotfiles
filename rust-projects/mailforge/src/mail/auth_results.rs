//! Parse `Authentication-Results:` headers and decide whether DMARC/SPF/DKIM
//! attest the message's `From:` domain.
//!
//! Gmail (and most modern providers) leaves an `Authentication-Results`
//! header on every received message. The shape is:
//!
//! ```text
//! Authentication-Results: mx.google.com;
//!     dkim=pass header.i=@nytimes.com header.s=20240403 header.b=abcd;
//!     spf=pass (google.com: domain of bounces+...@nytimes.com designates
//!         167.89.83.182 as permitted sender) smtp.mailfrom=...;
//!     dmarc=pass (p=NONE sp=NONE dis=NONE) header.from=nytimes.com
//! ```
//!
//! This module extracts the three method/result pairs and returns a
//! verdict via [`AuthVerdict::passed`].
//!
//! ## Decision rules
//!
//! - **`dmarc=pass`** → trusted (`passed = true`).
//! - **`dmarc` absent** AND **`dkim=pass`** AND **`spf=pass`** → trusted.
//! - **Any explicit `dmarc=fail` / `dkim=fail` / `spf=fail`** → forced
//!   plaintext WITH a visible warning.
//! - **Header missing entirely** → not trusted, but no warning (legacy
//!   mail that genuinely has no auth-results).
//!
//! ## Why not pull a full RFC 8601 parser
//!
//! Gmail's emitted shape is stable. We only need three result values, not
//! the full method/property nesting. Hand-rolled parsing is a few dozen
//! lines, deterministic, and avoids a heavy dependency.

/// One method's result. Mirrors the RFC 8601 `result` token vocabulary,
/// reduced to the three verdicts we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodResult {
    Pass,
    Fail,
    /// Includes "none", "neutral", "policy", "permerror", "temperror",
    /// "softfail", and any unrecognised string. None of these constitute
    /// "the domain is verified"; some don't constitute "the domain is
    /// known to be spoofed" either, but we lump them together because the
    /// caller only needs pass/fail/other.
    Other,
}

impl MethodResult {
    fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "pass" => Self::Pass,
            "fail" => Self::Fail,
            _ => Self::Other,
        }
    }
}

/// Verdict from parsing one `Authentication-Results:` header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AuthVerdict {
    pub dmarc: Option<MethodResult>,
    pub dkim: Option<MethodResult>,
    pub spf: Option<MethodResult>,
    /// True iff the header existed (vs being absent from the message).
    /// Drives the "show a warning chip" decision: explicit fail = warn,
    /// no header at all = silent (legacy mail).
    pub header_present: bool,
}

impl AuthVerdict {
    /// "Auth passed" per the spec:
    /// - `dmarc=pass`, OR
    /// - `dkim=pass` AND `spf=pass` (both, when no DMARC was published).
    pub fn passed(&self) -> bool {
        if matches!(self.dmarc, Some(MethodResult::Pass)) {
            return true;
        }
        // Only fall back to dkim+spf when the message had no dmarc result
        // at all (i.e., the From-domain didn't publish a DMARC policy).
        // If dmarc is explicitly Fail/Other, don't promote.
        if self.dmarc.is_none()
            && matches!(self.dkim, Some(MethodResult::Pass))
            && matches!(self.spf, Some(MethodResult::Pass))
        {
            return true;
        }
        false
    }

    /// True iff some method explicitly failed (so the UI should show
    /// a "auth failed" warning chip).
    pub fn explicit_fail(&self) -> bool {
        matches!(self.dmarc, Some(MethodResult::Fail))
            || matches!(self.dkim, Some(MethodResult::Fail))
            || matches!(self.spf, Some(MethodResult::Fail))
    }
}

/// Parse one `Authentication-Results:` header value. The header may or
/// may not include the `Authentication-Results:` prefix — strip it if
/// present, then walk the comma/semicolon-separated method tokens.
///
/// Returns an `AuthVerdict` with `header_present = true`. The caller is
/// responsible for setting `header_present = false` when the header is
/// absent altogether.
pub fn parse(value: &str) -> AuthVerdict {
    let mut v = AuthVerdict { header_present: true, ..AuthVerdict::default() };
    // Drop a leading "Authentication-Results:" if the caller passed the
    // raw header line.
    let body = match value.find(':') {
        // The header name is always alpha+hyphen. If the prefix before
        // ':' is exactly "Authentication-Results" (case-insensitive),
        // strip it; otherwise leave the value alone (the colon may be
        // inside a comment).
        Some(idx) => {
            let prefix = &value[..idx];
            if prefix.eq_ignore_ascii_case("authentication-results") {
                &value[idx + 1..]
            } else {
                value
            }
        }
        None => value,
    };

    // Split on `;` first — top-level methods are semicolon-separated.
    // The first segment is usually the authserv-id (e.g. `mx.google.com`).
    // Subsequent segments are `method=result ...`.
    for segment in body.split(';') {
        let stripped = strip_comments(segment);
        let segment = stripped.trim();
        if segment.is_empty() {
            continue;
        }
        // Find the first `=` to identify a method=result clause. Real
        // headers also have spaces and key=value extensions after the
        // result; we only need the result token (i.e. the word right
        // after `=` before whitespace).
        let Some(eq) = segment.find('=') else { continue };
        let method = segment[..eq].trim().to_ascii_lowercase();
        let after = &segment[eq + 1..];
        let result_tok = after
            .split(|c: char| c.is_whitespace() || c == '(')
            .find(|s| !s.is_empty())
            .unwrap_or("");
        let result = MethodResult::from_str(result_tok);
        match method.as_str() {
            "dmarc" => v.dmarc = Some(result),
            "dkim" => v.dkim = Some(result),
            "spf" => v.spf = Some(result),
            _ => { /* ignore other methods (iprev, arc, etc.) */ }
        }
    }

    v
}

/// Drop balanced `(...)` comments from a header segment. RFC 5322 allows
/// nested parentheses; we keep the parser simple and just track depth.
fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// Find the most-relevant `Authentication-Results:` header value in
/// a parsed `mail-parser` Message. Returns the verdict; if no such header
/// exists, returns an empty verdict with `header_present = false`.
///
/// When multiple `Authentication-Results` headers are present (e.g. one
/// from each MTA hop), prefer the one whose authserv-id matches a
/// known-good provider — Gmail's `mx.google.com`, Microsoft's
/// `mx.microsoft.com`, etc. Falls back to the first occurrence.
///
/// Uses `headers_raw()` to walk the raw header lines directly — that
/// returns `(&str name, &str raw_value)` pairs without forcing us to
/// pattern-match on `HeaderValue` (which `mail-parser` typically renders
/// as `HeaderValue::Empty` for non-RFC-defined headers like
/// Authentication-Results).
pub fn verdict_from_message(msg: &mail_parser::Message<'_>) -> AuthVerdict {
    let mut best: Option<&str> = None;
    let mut first: Option<&str> = None;

    for (name, raw_value) in msg.headers_raw() {
        if !name.eq_ignore_ascii_case("Authentication-Results") {
            continue;
        }
        if first.is_none() {
            first = Some(raw_value);
        }
        // Prefer the one whose authserv-id matches a known provider so
        // we read Gmail's verdict over a downstream forwarder's.
        let lc = raw_value.to_ascii_lowercase();
        if lc.contains("mx.google.com")
            || lc.contains("mx.microsoft.com")
            || lc.contains("hotmail.com")
        {
            best = Some(raw_value);
            break;
        }
    }

    match best.or(first) {
        Some(text) => parse(text),
        None => AuthVerdict::default(), // header_present = false
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_gmail_header() {
        let raw = "mx.google.com; \
            dkim=pass header.i=@nytimes.com header.s=20240403 header.b=ab; \
            spf=pass (google.com: domain of bounces+x@nytimes.com designates \
                  167.89.83.182 as permitted sender) smtp.mailfrom=x@nytimes.com; \
            dmarc=pass (p=NONE sp=NONE dis=NONE) header.from=nytimes.com";
        let v = parse(raw);
        assert_eq!(v.dmarc, Some(MethodResult::Pass));
        assert_eq!(v.dkim, Some(MethodResult::Pass));
        assert_eq!(v.spf, Some(MethodResult::Pass));
        assert!(v.passed(), "expected passed(); got {v:?}");
        assert!(!v.explicit_fail());
    }

    #[test]
    fn parses_dmarc_fail() {
        let raw = "mx.google.com; dkim=pass; spf=pass; dmarc=fail header.from=nytimes.com";
        let v = parse(raw);
        assert_eq!(v.dmarc, Some(MethodResult::Fail));
        assert!(!v.passed());
        assert!(v.explicit_fail());
    }

    #[test]
    fn parses_dkim_pass_spf_pass_no_dmarc() {
        // Some senders don't publish DMARC. Both dkim and spf passing
        // should be enough.
        let raw = "mx.google.com; dkim=pass header.i=@example.com; spf=pass";
        let v = parse(raw);
        assert_eq!(v.dmarc, None);
        assert_eq!(v.dkim, Some(MethodResult::Pass));
        assert_eq!(v.spf, Some(MethodResult::Pass));
        assert!(v.passed());
        assert!(!v.explicit_fail());
    }

    #[test]
    fn parses_dkim_pass_spf_fail_no_dmarc() {
        // Half-passing without DMARC: not trusted, but spf=fail is an
        // explicit fail so warn the user.
        let raw = "mx.google.com; dkim=pass; spf=fail";
        let v = parse(raw);
        assert!(!v.passed());
        assert!(v.explicit_fail());
    }

    #[test]
    fn parses_all_neutral() {
        // dkim=none + spf=neutral + dmarc=none (or absent) → not trusted
        // and no explicit fail.
        let raw = "mx.google.com; dkim=none; spf=neutral; dmarc=none";
        let v = parse(raw);
        assert_eq!(v.dmarc, Some(MethodResult::Other));
        assert_eq!(v.dkim, Some(MethodResult::Other));
        assert_eq!(v.spf, Some(MethodResult::Other));
        assert!(!v.passed());
        assert!(!v.explicit_fail());
    }

    #[test]
    fn dmarc_fail_overrides_dkim_pass() {
        // Even if dkim=pass and spf=pass, an explicit dmarc=fail means
        // the message MUST NOT be auto-trusted (dmarc is the strongest
        // From-alignment guarantee).
        let raw = "mx.google.com; dkim=pass; spf=pass; dmarc=fail";
        let v = parse(raw);
        assert!(!v.passed(), "dmarc=fail must veto the dkim+spf fallback: {v:?}");
        assert!(v.explicit_fail());
    }

    #[test]
    fn parses_with_authentication_results_prefix() {
        let raw = "Authentication-Results: mx.google.com; dmarc=pass header.from=nytimes.com";
        let v = parse(raw);
        assert_eq!(v.dmarc, Some(MethodResult::Pass));
        assert!(v.passed());
    }

    #[test]
    fn empty_value_returns_unparsed() {
        let v = parse("");
        assert!(v.header_present);
        assert!(!v.passed());
        assert!(!v.explicit_fail());
    }

    #[test]
    fn default_verdict_is_not_present() {
        let v = AuthVerdict::default();
        assert!(!v.header_present);
        assert!(!v.passed());
        assert!(!v.explicit_fail());
    }

    #[test]
    fn parses_nested_parens_in_spf() {
        let raw = "mx.google.com; \
            spf=pass (google.com: domain of x@y.com designates 1.2.3.4 (permitted) ); \
            dmarc=pass header.from=y.com";
        let v = parse(raw);
        assert_eq!(v.spf, Some(MethodResult::Pass));
        assert_eq!(v.dmarc, Some(MethodResult::Pass));
        assert!(v.passed());
    }

    #[test]
    fn parses_uppercase_method_names() {
        // RFC 8601 says method names are case-insensitive.
        let raw = "mx.google.com; DMARC=PASS header.from=x.com";
        let v = parse(raw);
        assert_eq!(v.dmarc, Some(MethodResult::Pass));
        assert!(v.passed());
    }
}
