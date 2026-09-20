//! Verified delivery, not an extraction tag, authorises automated Inbox release.
//! Personal booking Markdown is the first adapter. No deletion or LLM path.
use crate::{config::Config, policy::Policy};
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use fs2::FileExt;
use mailparse::MailHeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
};

const LIMIT: u64 = 10 * 1024 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Destination {
    /// HOME-relative, portable on both hosts; outside the evidence/cache store.
    pub root: PathBuf,
    pub writer: String,
    /// Prospective boundary based on the source message date, not extraction time.
    pub not_before: String,
}

impl Destination {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.root.starts_with("Forge")
                && self.root.components().count() >= 3
                && self
                    .root
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "delivery root must be a HOME-relative Forge subdirectory without traversal"
        );
        ensure!(!self.writer.is_empty(), "delivery writer is required");
        DateTime::parse_from_rfc3339(&self.not_before)
            .context("delivery not_before must be an explicit RFC3339 date")?;
        Ok(())
    }
    fn path(&self, create: bool) -> Result<PathBuf> {
        self.validate()?;
        let path = home()?.join(&self.root);
        safe_dir(&path, create)?;
        Ok(path)
    }
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("home unavailable")
}
fn personal() -> bool {
    home().ok().is_some_and(|h| {
        std::env::var_os("NOTMUCH_CONFIG")
            .map(PathBuf::from)
            .as_ref()
            == Some(&h.join("Mail/.notmuch-config"))
    })
}
pub fn selected_account() -> Result<&'static str> {
    if personal() {
        return Ok("personal");
    }
    if std::env::var_os("NOTMUCH_CONFIG").map(PathBuf::from)
        == Some(home()?.join("Mail/.notmuch-cohs-config"))
    {
        return Ok("cohs");
    }
    bail!(
        "delivery-status requires explicit NOTMUCH_CONFIG for the canonical personal or cohs index"
    )
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn id(raw: &str) -> Result<String> {
    let id = raw.trim_start_matches('<').trim_end_matches('>');
    ensure!(
        !id.is_empty()
            && id.len() <= 998
            && id.contains('@')
            && !id.chars().any(|c| c.is_control()
                || c.is_whitespace()
                || matches!(c, '(' | ')' | ':' | '*' | '\\' | '"' | '<' | '>')),
        "ambiguous source identity"
    );
    Ok(id.to_string())
}
fn query(id: &str) -> Result<String> {
    Ok(format!("id:\"{}\"", self::id(id)?))
}
fn key(policy: &Policy, message: &str) -> Result<String> {
    Ok(digest(
        format!("personal\0{}\0{}", policy.name, id(message)?).as_bytes(),
    ))
}
fn policy_hash(policy: &Policy) -> Result<String> {
    // Scheduling and tags do not change the captured information contract.
    Ok(digest(&serde_json::to_vec(&(
        &policy.r#match,
        &policy.extractors,
        &policy.vendor_module,
    ))?))
}

fn safe_dir(path: &Path, create: bool) -> Result<()> {
    let home = home()?;
    let relative = path
        .strip_prefix(&home)
        .context("directory must be below HOME")?;
    let mut current = home;
    for part in relative.components() {
        ensure!(
            matches!(part, Component::Normal(_)),
            "unsafe directory component"
        );
        current.push(part);
        if create && !current.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                match fs::DirBuilder::new().mode(0o700).create(&current) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
            }
            #[cfg(not(unix))]
            fs::create_dir(&current)?;
        }
        let meta = fs::symlink_metadata(&current)?;
        ensure!(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "unsafe destination directory"
        );
    }
    Ok(())
}
fn read(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= LIMIT,
        "unsafe or oversized file"
    );
    let mut bytes = Vec::new();
    File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= LIMIT, "file exceeded read limit");
    Ok(bytes)
}
/// Never overwrite a user's edit or follow an existing target symlink.
fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    if fs::symlink_metadata(path).is_ok() {
        ensure!(
            read(path)? == bytes,
            "existing destination differs; manual review required"
        );
        return Ok(());
    }
    let parent = path.parent().context("no parent")?;
    let temp = parent.join(format!(
        ".delivery-{}-{:016x}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&temp, path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(read(path)? == bytes, "destination conflict")
            }
            Err(e) => return Err(e.into()),
        }
        File::open(parent)?.sync_all()?;
        ensure!(read(path)? == bytes, "destination read-back failed");
        Ok(())
    })();
    // The temporary path is private to this invocation, never user data.
    let _ = fs::remove_file(temp);
    result
}
fn receipts(create: bool) -> Result<PathBuf> {
    let path = home()?.join(".local/share/mailcurator/delivery-receipts");
    safe_dir(&path, create)?;
    Ok(path)
}
pub fn run_lock() -> Result<File> {
    let root = receipts(true)?;
    let account = std::env::var_os("NOTMUCH_CONFIG").unwrap_or_default();
    let path = root.join(format!("run-{}.lock", digest(account.as_encoded_bytes())));
    if let Ok(meta) = fs::symlink_metadata(&path) {
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink(),
            "unsafe delivery lock"
        );
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive()
        .context("another mailcurator run is active")?;
    Ok(file)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    key: String,
    message_id: String,
    policy: String,
    policy_hash: String,
    source_hash: String,
    destination_hash: String,
    root: PathBuf,
    booking_key: String,
    checkin: NaiveDate,
    checkout: NaiveDate,
}
fn text<'a>(v: &'a Value, name: &str) -> &'a str {
    v.get(name).and_then(Value::as_str).unwrap_or("").trim()
}
pub fn date_needs_review(raw: &str) -> bool {
    if !regex::Regex::new(r"\b20\d{2}\b").unwrap().is_match(raw) {
        return true;
    }
    regex::Regex::new(r"\b(\d{1,2})/(\d{1,2})/20\d{2}\b")
        .unwrap()
        .captures(raw)
        .is_some_and(|c| {
            let a: u32 = c[1].parse().unwrap_or(0);
            let b: u32 = c[2].parse().unwrap_or(0);
            a != b && (1..=12).contains(&a) && (1..=12).contains(&b)
        })
}
fn explicit_date(raw: &str) -> Result<NaiveDate> {
    let raw = raw.replace(',', "");
    ensure!(
        !date_needs_review(&raw),
        "date year or day/month order unverified"
    );
    for format in [
        "%Y-%m-%d",
        "%A %d %B %Y",
        "%a %d %b %Y",
        "%d %B %Y",
        "%d %b %Y",
        "%d/%m/%Y",
    ] {
        if let Ok(date) = NaiveDate::parse_from_str(&raw, format) {
            return Ok(date);
        }
    }
    bail!("date unverified")
}
fn attachment_free(mail: &mailparse::ParsedMail<'_>) -> bool {
    let disposition = mail.get_content_disposition();
    disposition.disposition != mailparse::DispositionType::Attachment
        && !disposition.params.contains_key("filename")
        && !mail.ctype.params.contains_key("name")
        && (mail.ctype.mimetype.starts_with("multipart/")
            || matches!(mail.ctype.mimetype.as_str(), "text/plain" | "text/html"))
        && mail.subparts.iter().all(attachment_free)
}
fn source_link(message: &str) -> Result<String> {
    let encoded: String = format!("personal:{}", id(message)?)
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    Ok(format!("http://127.0.0.1:8765/mail/m/{encoded}"))
}

pub fn capture(
    destination: &Destination,
    policy: &Policy,
    record: &Value,
    raw: &[u8],
) -> Result<()> {
    ensure!(
        personal() && text(record, "account") == "personal",
        "unverified personal account"
    );
    ensure!(
        destination.writer == crate::store::machine_id(),
        "not the configured delivery writer"
    );
    ensure!(
        !policy.quarantine
            && policy.extractors.len() == 1
            && policy.extractors[0].category == "bookings",
        "unsupported or quarantined delivery policy"
    );
    ensure!(raw.len() as u64 <= LIMIT, "oversized source");
    let parsed = mailparse::parse_mail(raw)?;
    ensure!(attachment_free(&parsed), "uncaptured attachments");
    let message = id(text(record, "message_id"))?;
    ensure!(
        id(&parsed
            .headers
            .get_first_value("Message-ID")
            .context("missing Message-ID")?)?
            == message,
        "source identity mismatch"
    );
    verify_source(&message, &digest(raw))?;
    let received = DateTime::parse_from_rfc2822(
        &parsed
            .headers
            .get_first_value("Date")
            .context("source date missing")?,
    )?;
    ensure!(
        received >= DateTime::parse_from_rfc3339(&destination.not_before)?
            && received <= Utc::now(),
        "source outside prospective delivery window"
    );
    ensure!(
        !record
            .get("_provenance")
            .is_some_and(|p| p.to_string().to_lowercase().contains("llm")),
        "LLM-derived extraction cannot auto-deliver"
    );
    ensure!(
        record.get("_date_inferred") != Some(&Value::Bool(true)),
        "inferred date requires review"
    );
    let subject = parsed
        .headers
        .get_first_value("Subject")
        .unwrap_or_default()
        .to_lowercase();
    ensure!(
        !["cancel", "amend", "change", "refund", "reminder"]
            .iter()
            .any(|s| subject.contains(s)),
        "booking update requires review"
    );
    ensure!(
        !text(record, "property").is_empty() && !text(record, "booking_ref").is_empty(),
        "incomplete booking identity"
    );
    let checkin = explicit_date(text(record, "checkin"))?;
    let checkout = explicit_date(text(record, "checkout"))?;
    ensure!(
        checkin >= received.date_naive() && checkout > checkin,
        "invalid or historical stay"
    );
    let key = key(policy, &message)?;
    let receipt_dir = receipts(true)?;
    let receipt_path = receipt_dir.join(format!("{key}.json"));
    // Previously delivered then removed/edited content must never be resurrected.
    if fs::symlink_metadata(&receipt_path).is_ok() {
        return verify(destination, policy, &message).map(|_| ());
    }
    // Existing extraction evidence can reveal a duplicate even when it predates
    // delivery receipts. Never silently turn a second confirmation into a stay.
    let mut bytes_read = 0usize;
    for path in crate::store::category_paths_all("bookings")? {
        let bytes = read(&path)?;
        bytes_read += bytes.len();
        ensure!(bytes_read <= 64 * 1024 * 1024, "booking evidence too large");
        for line in std::str::from_utf8(&bytes)?
            .lines()
            .filter(|l| !l.trim().is_empty())
        {
            let prior: Value = serde_json::from_str(line)?;
            if text(&prior, "account") == "cohs"
                || id(text(&prior, "message_id")).ok().as_deref() == Some(&message)
            {
                continue;
            }
            ensure!(
                text(&prior, "booking_ref") != text(record, "booking_ref"),
                "related booking evidence requires review"
            );
            if let (Ok(a), Ok(b)) = (
                explicit_date(text(&prior, "checkin")),
                explicit_date(text(&prior, "checkout")),
            ) {
                ensure!(
                    !(checkin < b && a < checkout),
                    "overlapping booking evidence requires review"
                );
            }
        }
    }
    let booking_key = digest(
        format!(
            "{}\0{}",
            text(record, "property"),
            text(record, "booking_ref")
        )
        .as_bytes(),
    );
    let entries = fs::read_dir(&receipt_dir)?.collect::<std::io::Result<Vec<_>>>()?;
    ensure!(entries.len() <= 10000, "receipt inventory too large");
    for entry in entries {
        if entry.path().extension().is_some_and(|e| e == "json") {
            let prior: Receipt = serde_json::from_slice(&read(&entry.path())?)?;
            ensure!(
                prior.booking_key != booking_key
                    && !(checkin < prior.checkout && prior.checkin < checkout),
                "related or overlapping booking requires review"
            );
        }
    }
    let root = destination.path(true)?;
    // A complete structured capture, not merely a link or selected display fields.
    // Indented JSON is inert Markdown; source values cannot inject HTML or links.
    let mut payload = record.clone();
    payload
        .as_object_mut()
        .context("invalid extraction")?
        .remove("extracted_at");
    let json = serde_json::to_string_pretty(&payload)?
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    let body = format!(
        "---\nmailcurator_schema: 1\ndestination_kind: local_file\nitem_id: {key}\n---\n\n# Booking correspondence\n\nEvidence as of: {}\n\nProperty:\n\n    {}\n\nCheck-in: {checkin}\n\nCheck-out: {checkout}\n\n[Original email]({})\n\nThis is a captured booking record, not proof of payment or current cancellation status. Later changes require review of the Inbox. This folder is NOT a complete booking inventory.\nKeep personal notes in a separate document; edits or removal of this managed record stop automatic filing.\n\n## Captured information\n\n{}\n",
        received.to_rfc3339(),
        text(record, "property").replace(['\r', '\n'], " "),
        source_link(&message)?,
        json.lines()
            .map(|line| format!("    {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let target = root.join(format!("{key}.md"));
    publish(&target, body.as_bytes())?;
    let receipt = Receipt {
        version: 1,
        key,
        message_id: message.clone(),
        policy: policy.name.clone(),
        policy_hash: policy_hash(policy)?,
        source_hash: digest(raw),
        destination_hash: digest(body.as_bytes()),
        root: destination.root.clone(),
        booking_key,
        checkin,
        checkout,
    };
    publish(&receipt_path, &serde_json::to_vec(&receipt)?)?;
    verify(destination, policy, &message)?;
    Ok(())
}

fn search(output: &str, q: &str) -> Result<Vec<String>> {
    let result = Command::new("notmuch")
        .args([
            "search",
            "--format=json",
            "--exclude=false",
            &format!("--output={output}"),
            q,
        ])
        .output()?;
    ensure!(result.status.success(), "mail index query failed");
    ensure!(
        result.stdout.len() <= 16 * 1024 * 1024,
        "index result too large"
    );
    let rows: Vec<String> = serde_json::from_slice(&result.stdout)?;
    ensure!(rows.len() <= 10000, "index result too large");
    Ok(rows)
}
fn verify(destination: &Destination, policy: &Policy, message: &str) -> Result<PathBuf> {
    ensure!(personal(), "account_unverified");
    ensure!(
        !policy.quarantine
            && policy.extractors.len() == 1
            && policy.extractors[0].category == "bookings",
        "unsupported_or_quarantined"
    );
    let key = key(policy, message)?;
    let receipt: Receipt = serde_json::from_slice(
        &read(
            &receipts(false)
                .context("receipt_unavailable")?
                .join(format!("{key}.json")),
        )
        .context("receipt_unavailable")?,
    )
    .context("receipt_invalid")?;
    ensure!(
        receipt.version == 1
            && receipt.key == key
            && receipt.message_id == id(message)?
            && receipt.policy == policy.name
            && receipt.policy_hash == policy_hash(policy)?
            && receipt.root == destination.root,
        "receipt_stale"
    );
    let target = destination
        .path(false)
        .context("destination_unavailable")?
        .join(format!("{key}.md"));
    ensure!(
        digest(&read(&target).context("destination_unavailable")?) == receipt.destination_hash,
        "destination_changed"
    );
    verify_source(message, &receipt.source_hash)?;
    Ok(target)
}
fn verify_source(message: &str, source_hash: &str) -> Result<()> {
    let files = search("files", &query(message)?)?;
    ensure!(!files.is_empty(), "source_missing");
    for file in files {
        ensure!(
            digest(&read(Path::new(&file)).context("source_unavailable")?) == source_hash,
            "source_changed_or_ambiguous"
        );
    }
    Ok(())
}

pub struct ArchiveGate<'a> {
    config: &'a Config,
    required: BTreeSet<String>,
    owners: BTreeMap<String, Vec<&'a Policy>>,
    financial: BTreeSet<String>,
}
#[derive(Serialize)]
pub struct Report {
    pub account: &'static str,
    pub total: usize,
    pub verified: usize,
    pub held: usize,
    pub rows: Vec<Status>,
}
#[derive(Serialize)]
pub struct Status {
    pub message_id: String,
    pub verified: bool,
    pub destinations: Vec<PathBuf>,
    pub reason: &'static str,
}

impl<'a> ArchiveGate<'a> {
    pub fn destination(&self) -> Option<&Destination> {
        self.config.delivery.as_ref()
    }
    pub fn new(config: &'a Config) -> Result<Self> {
        let mut owners: BTreeMap<String, Vec<&Policy>> = BTreeMap::new();
        let mut required: BTreeSet<String> = search(
            "messages",
            &format!("tag:inbox and ({})", information_query(config)),
        )?
        .into_iter()
        .collect();
        // Continue checking delivered records after the email has left Inbox.
        // A missing or edited destination remains a visible exception, never a
        // signal to resurrect/delete source or destination content.
        let receipt_root = home()?.join(".local/share/mailcurator/delivery-receipts");
        let mut receipt_queries = Vec::new();
        if personal() && receipt_root.exists() {
            safe_dir(&receipt_root, false)?;
            let entries = fs::read_dir(&receipt_root)?.collect::<std::io::Result<Vec<_>>>()?;
            ensure!(entries.len() <= 10000, "receipt inventory too large");
            for entry in entries {
                if entry.path().extension().is_some_and(|e| e == "json") {
                    let receipt: Receipt = serde_json::from_slice(&read(&entry.path())?)?;
                    required.insert(id(&receipt.message_id)?);
                    receipt_queries.push(query(&receipt.message_id)?);
                }
            }
        }
        let scope = if receipt_queries.is_empty() {
            "tag:inbox".to_string()
        } else {
            format!("tag:inbox or ({})", receipt_queries.join(" or "))
        };
        let financial = search(
            "messages",
            &format!("({scope}) and ({})", financial_query(config)),
        )?
        .into_iter()
        .collect();
        for policy in &config.policies {
            if policy.extractors.is_empty() {
                continue;
            }
            for message in search(
                "messages",
                &format!("({scope}) and ({})", policy.base_query()),
            )? {
                owners.entry(message).or_default().push(policy);
            }
        }
        Ok(Self {
            config,
            required,
            owners,
            financial,
        })
    }
    fn status(&self, message: &str) -> Status {
        let mut status = Status {
            message_id: message.to_string(),
            verified: false,
            destinations: vec![],
            reason: "delivery_not_verified",
        };
        if self.financial.contains(message) {
            status.reason = "financial_destination_required";
            return status;
        }
        let Some(destination) = self.destination() else {
            status.reason = "destination_not_configured";
            return status;
        };
        let Some(owners) = self.owners.get(message) else {
            status.reason = "information_without_delivery_policy";
            return status;
        };
        for policy in owners {
            match verify(destination, policy, message) {
                Ok(path) => status.destinations.push(path),
                Err(error) => {
                    status.reason = match error.to_string().as_str() {
                        "account_unverified" => "account_unverified",
                        "unsupported_or_quarantined" => "unsupported_or_quarantined",
                        "receipt_unavailable" => "receipt_unavailable",
                        "receipt_invalid" => "receipt_invalid",
                        "receipt_stale" => "receipt_stale",
                        "destination_changed" => "destination_changed",
                        "destination_unavailable" => "destination_unavailable",
                        "source_missing" => "source_missing",
                        "source_unavailable" => "source_unavailable",
                        "source_changed_or_ambiguous" => "source_changed_or_ambiguous",
                        _ => "verification_error",
                    };
                    return status;
                }
            }
        }
        status.verified = true;
        status.reason = "verified";
        status
    }
    pub fn report(&self) -> Result<Report> {
        let rows: Vec<_> = self
            .required
            .iter()
            .map(|message| self.status(message))
            .collect();
        let verified = rows.iter().filter(|r| r.verified).count();
        Ok(Report {
            account: selected_account()?,
            total: rows.len(),
            held: rows.len() - verified,
            verified,
            rows,
        })
    }
    /// Delivery never grants destruction authority. Even explicit legacy trash
    /// opt-in cannot delete information matches or acknowledged source ids.
    pub fn trash_query(&self, q: &str) -> Result<String> {
        let mut protected = vec![information_query(self.config)];
        for message in &self.required {
            protected.push(query(message)?);
        }
        Ok(format!("({q}) and not ({})", protected.join(" or ")))
    }
    pub fn archive(&self, q: &str, dry_run: bool) -> Result<(u64, u64)> {
        let mut archived = 0;
        let mut held = 0;
        let information = information_query(self.config);
        let fresh: BTreeSet<_> = search("messages", &format!("({q}) and ({information})"))?
            .into_iter()
            .collect();
        let financial_query = financial_query(self.config);
        let fresh_financial: BTreeSet<_> =
            search("messages", &format!("({q}) and ({financial_query})"))?
                .into_iter()
                .collect();
        for message in search("messages", q)? {
            let exact = query(&message)?;
            let needs_delivery = self.required.contains(&message) || fresh.contains(&message);
            if fresh_financial.contains(&message)
                || (needs_delivery && !self.status(&message).verified)
            {
                held += 1;
                if !dry_run {
                    crate::notmuch::apply_tag_changes(&exact, &["curator-delivery-pending"], &[])?;
                }
            } else {
                archived += 1;
                if !dry_run {
                    // Recheck classification at mutation time for routine mail too.
                    let eligible = if needs_delivery {
                        format!("({exact}) and not ({financial_query})")
                    } else {
                        format!("({exact}) and not ({information})")
                    };
                    crate::notmuch::apply_tag_changes(
                        &eligible,
                        &[],
                        &["inbox", "curator-delivery-pending"],
                    )?;
                }
            }
        }
        Ok((archived, held))
    }
}

fn information_query(config: &Config) -> String {
    // A deliberately broad deterministic hold net, not a booking classifier.
    // Unknown senders with these cues must not fall through to a noise policy.
    let mut terms = vec!["tag:booking or tag:billing or tag:receipts or tag:Expenses or tag:curator-retain or tag:curator-delivery-pending or subject:booking or subject:reservation or subject:accommodation or subject:itinerary or subject:\"check-in\" or from:airbnb.com or from:booking.com or from:marriott.com or from:premierinn.com or from:travelodge.co.uk or from:agoda.com or from:expedia.com or from:hotels.com".to_string()];
    terms.extend(
        config
            .policies
            .iter()
            .filter(|p| {
                !p.extractors.is_empty()
                    || p.on_arrival.tags_add.iter().any(|tag| {
                        matches!(
                            tag.as_str(),
                            "booking"
                                | "billing"
                                | "receipts"
                                | "Expenses"
                                | "curator-retain"
                                | "curator-delivery-pending"
                        )
                    })
            })
            .map(|p| format!("({})", p.base_query())),
    );
    format!("({})", terms.join(" or "))
}

fn financial_query(config: &Config) -> String {
    let mut terms = vec!["tag:billing or tag:receipts or tag:Expenses".to_string()];
    terms.extend(
        config
            .policies
            .iter()
            .filter(|p| {
                p.on_arrival
                    .tags_add
                    .iter()
                    .any(|tag| matches!(tag.as_str(), "billing" | "receipts" | "Expenses"))
            })
            .map(|p| format!("({})", p.base_query())),
    );
    format!("({})", terms.join(" or "))
}
