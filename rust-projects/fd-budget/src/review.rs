//! A saved review, not another ledger. Bank fields never change; decisions update
//! the worksheet, a generated report, and only allocation tags in the bank store.
use anyhow::{bail, ensure, Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Args, Subcommand, ValueEnum};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
    time::Instant,
};

type Hashes = BTreeMap<String, String>;
const FILES: [&str; 8] = [
    "transactions.csv",
    "rules.toml",
    "paypal.csv",
    "paypal_matches.jsonl",
    "matches.jsonl",
    "budgets.toml",
    "categories.toml",
    "smoothing.toml",
];
const PERSONAL: &str = "Will personal";
const JOINT: &str = "Joint household identified";
const REVIEW: &str = "Review";
const BUSINESS: &str = "Business";
const START: &str = "<!-- fd-budget-review:current:start -->";
const END: &str = "<!-- fd-budget-review:current:end -->";
const HISTORY: &str = "<!-- fd-budget-review:decisions -->";

#[derive(Args)]
pub struct ReviewArgs {
    /// Saved review settings (defaults to local application data).
    #[arg(long, global = true)]
    profile: Option<PathBuf>,
    #[command(subcommand)]
    command: ReviewCommand,
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// Adopt an existing worksheet. Preview unless --apply; no decisions change.
    Init {
        #[arg(long)]
        csv: PathBuf,
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        agenda: PathBuf,
        #[arg(long)]
        store: Option<PathBuf>,
        /// Sync each applied decision to this peer (will@nimbini on Mac; mac on Linux).
        #[arg(long)]
        peer: Option<String>,
        /// Already identified refunds of retained purchases, not fully refunded exclusions.
        #[arg(long, default_value = "0")]
        retained_refunds: Decimal,
        #[arg(long)]
        apply: bool,
    },
    /// Reconcile and show the largest remaining decisions, without writing.
    Status {
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Record a confirmed decision for one or more exact worksheet IDs.
    Set {
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long, value_enum)]
        allocation: Allocation,
        #[arg(long)]
        category: String,
        #[arg(long)]
        reason: String,
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Explicitly revise a previously allocated row. Splits/refunds remain protected.
        #[arg(long)]
        correct: bool,
        #[arg(long)]
        apply: bool,
    },
    /// Attach receipt evidence to an existing row; does not allocate or add expenditure.
    Identify {
        id: String,
        #[arg(long)]
        product: String,
        #[arg(long)]
        merchant: Option<String>,
        #[arg(long)]
        receipt_id: String,
        #[arg(long)]
        receipt_date: NaiveDate,
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        apply: bool,
    },
    /// Verify peer parity, or finish a previously interrupted sync safely.
    Sync,
    /// Read-only store hashes, also used over SSH. Does not need a profile.
    Fingerprint {
        #[arg(long)]
        store: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Allocation {
    Personal,
    Joint,
    Business,
}
impl Allocation {
    fn tag(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Joint => "joint",
            Self::Business => "business",
        }
    }
    fn group(self) -> &'static str {
        match self {
            Self::Personal => PERSONAL,
            Self::Joint => JOINT,
            Self::Business => "Outside living costs",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    csv: PathBuf,
    report: PathBuf,
    agenda: PathBuf,
    store: PathBuf,
    peer: Option<String>,
    baseline: String,
    row_count: usize,
    retained_refunds: Decimal,
    pending_sync: Option<Pending>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Pending {
    before: Hashes,
    after: Hashes,
}

#[derive(Clone)]
struct Table {
    headers: Vec<String>,
    rows: Vec<BTreeMap<String, String>>,
}
impl Table {
    fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read(path)?).with_context(|| format!("reading {}", path.display()))
    }
    fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = csv::Reader::from_reader(bytes);
        let headers: Vec<String> = reader.headers()?.iter().map(str::to_owned).collect();
        ensure!(
            headers.iter().collect::<BTreeSet<_>>().len() == headers.len(),
            "duplicate CSV headers"
        );
        let mut rows = Vec::new();
        for record in reader.records() {
            rows.push(
                headers
                    .iter()
                    .cloned()
                    .zip(record?.iter().map(str::to_owned))
                    .collect(),
            );
        }
        ensure!(!rows.is_empty(), "empty CSV");
        Ok(Self { headers, rows })
    }
    fn bytes(&self) -> Result<Vec<u8>> {
        let mut writer = csv::Writer::from_writer(Vec::new());
        writer.write_record(&self.headers)?;
        for row in &self.rows {
            writer.write_record(self.headers.iter().map(|h| row[h].as_str()))?;
        }
        Ok(writer.into_inner()?)
    }
    fn index(&self) -> Result<BTreeMap<String, usize>> {
        let mut ids = BTreeMap::new();
        for (i, row) in self.rows.iter().enumerate() {
            let id = field(row, "import_id")?;
            ensure!(
                !id.is_empty() && ids.insert(id.to_owned(), i).is_none(),
                "empty/duplicate transaction ID: {id}"
            );
        }
        Ok(ids)
    }
    fn immutable_hash(&self, fields: &[&str]) -> Result<String> {
        let mut records = Vec::new();
        for row in &self.rows {
            records.push(
                fields
                    .iter()
                    .map(|f| field(row, f).map(str::to_owned))
                    .collect::<Result<Vec<_>>>()?,
            );
        }
        records.sort();
        Ok(hash(&serde_json::to_vec(&records)?))
    }
}
const BANK_FIELDS: [&str; 7] = [
    "date",
    "account",
    "amount",
    "description",
    "original_tags",
    "import_id",
    "fd_budget_import_id",
];
type Row = BTreeMap<String, String>;
fn field<'a>(row: &'a Row, key: &str) -> Result<&'a str> {
    row.get(key)
        .map(String::as_str)
        .with_context(|| format!("missing column {key}"))
}
fn money(text: &str) -> Result<Decimal> {
    let n = Decimal::from_str(text).with_context(|| format!("invalid money {text:?}"))?;
    ensure!(
        (n * Decimal::from(100)).fract().is_zero(),
        "fractional penny: {text}"
    );
    Ok(n)
}
fn optional_money(row: &Row, key: &str) -> Result<Decimal> {
    let text = field(row, key)?;
    if text.is_empty() {
        Ok(Decimal::ZERO)
    } else {
        money(text)
    }
}
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn store_dir() -> PathBuf {
    dirs::home_dir()
        .expect("home directory")
        .join(".config/fd-budget")
}
fn default_profile() -> PathBuf {
    dirs::home_dir()
        .expect("home directory")
        .join(".local/share/fd-budget-review/default.json")
}
fn default_peer() -> &'static str {
    if cfg!(target_os = "macos") {
        "will@nimbini"
    } else {
        "mac"
    }
}
fn hashes(store: &Path) -> Result<Hashes> {
    FILES
        .iter()
        .map(|name| {
            Ok((
                name.to_string(),
                hash(
                    &fs::read(store.join(name))
                        .with_context(|| format!("missing store file {name}"))?,
                ),
            ))
        })
        .collect()
}
fn remote_hashes(peer: &str) -> Result<Hashes> {
    ensure!(
        peer == default_peer(),
        "unsupported peer; use {}",
        default_peer()
    );
    let out = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            peer,
            "fd-budget review fingerprint",
        ])
        .output()?;
    ensure!(
        out.status.success(),
        "peer fingerprint failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let result: Hashes = serde_json::from_slice(&out.stdout).context("invalid peer fingerprint")?;
    ensure!(
        result.keys().map(String::as_str).collect::<BTreeSet<_>>() == FILES.into_iter().collect(),
        "peer file inventory mismatch"
    );
    ensure!(
        result
            .values()
            .all(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit())),
        "invalid peer hash"
    );
    Ok(result)
}

#[derive(Default, Serialize)]
struct Summary {
    rows: usize,
    debits: Decimal,
    credits: Decimal,
    allocations: BTreeMap<String, Decimal>,
    #[serde(skip)]
    categories: BTreeMap<String, BTreeMap<String, Decimal>>,
}
fn add(map: &mut BTreeMap<String, Decimal>, key: &str, n: Decimal) {
    *map.entry(key.to_owned()).or_default() += n;
}
fn parts(row: &Row) -> Result<Vec<(String, String, Decimal)>> {
    let n = -money(field(row, "amount")?)?;
    let group = field(row, "group")?;
    let category = field(row, "category")?;
    let shares = [
        optional_money(row, "allocated_personal_gbp")?,
        optional_money(row, "allocated_business_gbp")?,
        optional_money(row, "allocated_joint_gbp")?,
    ];
    if n <= Decimal::ZERO {
        ensure!(
            group == "Credits" && shares.iter().all(Decimal::is_zero),
            "credit misclassified: {}",
            field(row, "import_id")?
        );
        return Ok(Vec::new());
    }
    if group.starts_with("Split ") {
        ensure!(
            matches!(
                group,
                "Split personal / business" | "Split personal / joint"
            ),
            "unknown split group"
        );
        ensure!(
            shares.iter().all(|x| *x >= Decimal::ZERO) && shares.iter().sum::<Decimal>() == n,
            "split does not reconcile: {}",
            field(row, "import_id")?
        );
        ensure!(
            (group == "Split personal / business" && shares[2].is_zero())
                || (group == "Split personal / joint" && shares[1].is_zero()),
            "wrong split columns"
        );
        return Ok([PERSONAL, BUSINESS, JOINT]
            .iter()
            .zip(shares)
            .filter(|(_, n)| !n.is_zero())
            .map(|(g, n)| (g.to_string(), format!("{category} — allocated share"), n))
            .collect());
    }
    ensure!(
        shares.iter().all(Decimal::is_zero),
        "allocation columns on non-split row"
    );
    ensure!(
        matches!(
            group,
            PERSONAL
                | JOINT
                | REVIEW
                | "Flat costs paid by Will"
                | "Joint mortgage"
                | "Outside living costs"
        ),
        "unknown group {group}"
    );
    let key = if group == "Outside living costs" {
        if category.starts_with("Business") {
            BUSINESS
        } else {
            category
        }
    } else {
        group
    };
    Ok(vec![(key.to_owned(), category.to_owned(), n)])
}
fn summary(table: &Table) -> Result<Summary> {
    table.index()?;
    let mut s = Summary {
        rows: table.rows.len(),
        ..Summary::default()
    };
    for row in &table.rows {
        NaiveDate::parse_from_str(field(row, "date")?, "%Y-%m-%d")?;
        let n = money(field(row, "amount")?)?;
        if n < Decimal::ZERO {
            s.debits -= n;
        } else {
            s.credits += n;
        }
        for (group, category, n) in parts(row)? {
            add(&mut s.allocations, &group, n);
            add(s.categories.entry(group).or_default(), &category, n);
        }
    }
    ensure!(
        s.debits > Decimal::ZERO && s.allocations.values().sum::<Decimal>() == s.debits,
        "debit partition does not reconcile"
    );
    Ok(s)
}
fn retained(s: &Summary) -> Decimal {
    [
        PERSONAL,
        JOINT,
        "Flat costs paid by Will",
        "Joint mortgage",
        REVIEW,
    ]
    .iter()
    .map(|g| s.allocations.get(*g).copied().unwrap_or_default())
    .sum()
}
fn validate(profile: &Profile, table: &Table, bank: &Table) -> Result<Summary> {
    ensure!(profile.version == 1, "unsupported profile version");
    for column in [
        "merchant",
        "analysis_tags",
        "reason",
        "evidence",
        "identified_product",
        "receipt_message_id",
        "receipt_date",
        "receipt_bank_date_delta_days",
        "related_refund_import_id",
    ] {
        ensure!(
            table.headers.iter().any(|h| h == column),
            "missing worksheet column {column}"
        );
    }
    ensure!(
        bank.headers.iter().any(|h| h == "tags"),
        "missing bank tags column"
    );
    ensure!(
        table.rows.len() == profile.row_count
            && table.immutable_hash(&BANK_FIELDS)? == profile.baseline,
        "worksheet bank identity changed since adoption"
    );
    let ids = bank.index()?;
    let mut mapped = BTreeSet::new();
    for row in &table.rows {
        let id = field(row, "fd_budget_import_id")?;
        if id.is_empty() {
            continue;
        }
        ensure!(mapped.insert(id), "duplicate live bank mapping: {id}");
        let live = &bank.rows[*ids
            .get(id)
            .with_context(|| format!("missing live row {id}"))?];
        for key in ["date", "account"] {
            ensure!(
                field(row, key)? == field(live, key)?,
                "live {key} mismatch for {id}"
            );
        }
        ensure!(
            money(field(row, "amount")?)? == money(field(live, "amount")?)?,
            "live amount mismatch for {id}"
        );
    }
    summary(table)
}
fn gbp(n: Decimal) -> String {
    let n = n.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    let raw = format!("{:.2}", n.abs());
    let (whole, fraction) = raw.split_once('.').unwrap();
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!(
        "{}£{out}.{fraction}",
        if n.is_sign_negative() { "−" } else { "" }
    )
}
fn md(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}
fn render(profile: &Profile, table: &Table, s: &Summary) -> String {
    let first = table.rows.iter().map(|r| r["date"].as_str()).min().unwrap();
    let last = table.rows.iter().map(|r| r["date"].as_str()).max().unwrap();
    let mut out = format!("{START}\n## Current allocation — generated from the worksheet\n\nWindow: **{first}–{last}**. {} original bank rows; debits **{}**, credits **{}**. These are historical payments, not a recurring budget. Joint costs are separate from flat costs and mortgages; this records Will’s payments, not Jenny’s.\n\n| Allocation | Year | Average per month |\n|---|---:|---:|\n", s.rows, gbp(s.debits), gbp(s.credits));
    for g in [
        PERSONAL,
        JOINT,
        "Flat costs paid by Will",
        "Joint mortgage",
        REVIEW,
    ] {
        let n = s.allocations.get(g).copied().unwrap_or_default();
        out += &format!("| {g} | {} | {} |\n", gbp(n), gbp(n / Decimal::from(12)));
    }
    out += &format!("| **Retained living / property / gifts** | **{}** | **{}** |\n\nIdentified refunds of retained purchases: **{}**; retained net of those refunds: **{}**. Fully refunded purchases are excluded separately, so their credits are not subtracted again.\n", gbp(retained(s)), gbp(retained(s)/Decimal::from(12)), gbp(profile.retained_refunds), gbp(retained(s)-profile.retained_refunds));
    for g in [JOINT, "Flat costs paid by Will", PERSONAL, REVIEW] {
        out += &format!("\n### {g}\n\n| Category | Amount |\n|---|---:|\n");
        if let Some(categories) = s.categories.get(g) {
            for (c, n) in categories {
                out += &format!("| {} | {} |\n", md(c), gbp(*n));
            }
        }
    }
    out += "\n### Debit reconciliation\n\n| Partition | Amount |\n|---|---:|\n";
    for (g, n) in &s.allocations {
        out += &format!("| {} | {} |\n", md(g), gbp(*n));
    }
    out += &format!("| **All debits** | **{}** |\n\nSplit rows are counted once and distributed using their explicit monetary shares. Business use is an allocation decision, not certification of tax deductibility.\n{END}", gbp(s.debits));
    out
}
fn replace_generated(report: &str, generated: &str) -> Result<String> {
    ensure!(
        report.matches(START).count() == 1
            && report.matches(END).count() == 1
            && report.matches(HISTORY).count() == 1,
        "report markers missing/duplicated; refusing prose replacements"
    );
    let start = report.find(START).unwrap();
    let end = report.find(END).unwrap() + END.len();
    ensure!(start < end, "report markers out of order");
    Ok(format!(
        "{}{}{}",
        &report[..start],
        generated,
        &report[end..]
    ))
}

struct Lock(PathBuf);
impl Lock {
    fn acquire(path: &Path) -> Result<Self> {
        let path = path.with_extension("lock");
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "review locked: {}; inspect the owning process before recovering a stale lock",
                    path.display()
                )
            })?;
        writeln!(f, "{}", std::process::id())?;
        Ok(Self(path))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("review-{}.tmp", std::process::id()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| -> Result<()> {
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&temp, meta.permissions())?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
fn read_profile(path: &Path) -> Result<Profile> {
    Ok(serde_json::from_slice(
        &fs::read(path).context("no review profile; run review init first")?,
    )?)
}
fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(value)?)
}

// Snapshot first; compare-and-swap each file; roll back only our own writes on a
// recoverable local error. A process crash leaves the durable manifest/backups.
fn commit(
    profile: &Profile,
    changes: Vec<(PathBuf, Vec<u8>)>,
    originals: &BTreeMap<PathBuf, Option<Vec<u8>>>,
) -> Result<PathBuf> {
    let root = dirs::home_dir()
        .context("home directory")?
        .join(".local/share/fd-budget-review/backups");
    let backup = root.join(format!(
        "{}-{}",
        Local::now().format("%Y%m%d-%H%M%S%.6f"),
        std::process::id()
    ));
    fs::create_dir_all(&backup)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&backup, fs::Permissions::from_mode(0o700))?;
    }
    fs::create_dir(backup.join("store"))?;
    for name in FILES {
        fs::copy(profile.store.join(name), backup.join("store").join(name))?;
    }
    let mut manifest = Vec::new();
    for (i, (path, after)) in changes.iter().enumerate() {
        let before = originals.get(path).context("missing before image")?;
        if let Some(bytes) = before {
            fs::write(backup.join(format!("{i}.before")), bytes)?;
        }
        manifest.push(serde_json::json!({"path":path,"before":before.as_ref().map(|v|hash(v)),"after":hash(after),"snapshot":format!("{i}.before")}));
    }
    fs::write(backup.join("manifest.json"), json_bytes(&manifest)?)?;
    for (path, expected) in originals {
        ensure!(
            &read_optional(path)? == expected,
            "concurrent edit to {}; no writes",
            path.display()
        );
    }
    let mut written: Vec<(&PathBuf, &Vec<u8>)> = Vec::new();
    for (path, bytes) in &changes {
        let result = (|| -> Result<()> {
            ensure!(
                &read_optional(path)? == &originals[path],
                "concurrent edit: {}",
                path.display()
            );
            write_atomic(path, bytes)
        })();
        if let Err(e) = result {
            for (p, after) in written.iter().rev() {
                if read_optional(p)?.as_deref() == Some(after.as_slice()) {
                    if let Some(before) = &originals[*p] {
                        write_atomic(p, before)?;
                    } else {
                        fs::remove_file(p)?;
                    }
                }
            }
            bail!(
                "local update failed: {e:#}; recovery snapshots: {}",
                backup.display()
            );
        }
        written.push((path, bytes));
    }
    for (path, bytes) in &changes {
        ensure!(
            fs::read(path)? == *bytes,
            "write verification failed: {}",
            path.display()
        );
    }
    Ok(backup)
}
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
fn original(paths: &[PathBuf]) -> Result<BTreeMap<PathBuf, Option<Vec<u8>>>> {
    let mut result = BTreeMap::new();
    for p in paths {
        ensure!(
            result.insert(p.clone(), read_optional(p)?).is_none(),
            "overlapping output paths"
        );
    }
    Ok(result)
}
fn pending_compatible(p: &Pending, remote: &Hashes) -> bool {
    remote.len() == p.before.len()
        && remote
            .iter()
            .all(|(k, v)| p.before.get(k) == Some(v) || p.after.get(k) == Some(v))
}
fn sync(profile: &mut Profile, path: &Path) -> Result<bool> {
    let Some(peer) = profile.peer.as_deref() else {
        ensure!(profile.pending_sync.is_none(), "pending sync without peer");
        return Ok(false);
    };
    let local = hashes(&profile.store)?;
    let remote = remote_hashes(peer)?;
    if let Some(pending) = &profile.pending_sync {
        ensure!(
            local == pending.after,
            "local store changed during pending sync; reconcile manually"
        );
        ensure!(
            pending_compatible(pending, &remote),
            "peer changed independently; refusing sync"
        );
        if remote != local {
            let output = Command::new("fd-budget-sync")
                .args(["push", "--apply"])
                .output()?;
            ensure!(
                output.status.success(),
                "local decision saved, sync failed; run review sync: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        ensure!(
            hashes(&profile.store)? == local && remote_hashes(peer)? == local,
            "local decision saved; sync verification failed; run review sync"
        );
        profile.pending_sync = None;
        write_atomic(path, &json_bytes(profile)?)?;
    } else {
        ensure!(
            local == remote,
            "peer diverged; reconcile before further decisions"
        );
    }
    Ok(true)
}
fn next(table: &Table, limit: usize) -> Vec<&Row> {
    let mut rows: Vec<_> = table.rows.iter().filter(|r| r["group"] == REVIEW).collect();
    rows.sort_by(|a, b| {
        money(&a["amount"])
            .unwrap()
            .cmp(&money(&b["amount"]).unwrap())
            .then(a["date"].cmp(&b["date"]))
    });
    rows.truncate(limit);
    rows
}
fn output(
    table: &Table,
    s: &Summary,
    limit: usize,
    applied: bool,
    synced: bool,
    backup: Option<PathBuf>,
    timer: Instant,
) -> Result<()> {
    let pending: Vec<_> = next(table, limit)
        .into_iter()
        .map(|r| {
            [
                "import_id",
                "date",
                "amount",
                "merchant",
                "category",
                "identified_product",
                "receipt_message_id",
            ]
            .into_iter()
            .map(|k| (k, r[k].as_str()))
            .collect::<BTreeMap<_, _>>()
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"applied":applied,"synced":synced,"backup":backup,"elapsed_ms":timer.elapsed().as_millis(),"summary":s,"next":pending})
        )?
    );
    Ok(())
}
fn text_ok(s: &str) -> Result<()> {
    ensure!(
        !s.trim().is_empty() && !s.contains(['\n', '\r']) && !s.contains("<!--"),
        "empty/multiline/marker-containing text"
    );
    Ok(())
}
fn receipt_key(s: &str) -> &str {
    s.trim().trim_start_matches('<').trim_end_matches('>')
}
fn tags_update(text: &str, allocation: Allocation, tags: &[String]) -> Result<String> {
    let mut values: Vec<String> = text
        .split('|')
        .filter(|s| !s.is_empty() && !["personal", "joint", "business"].contains(s))
        .map(str::to_owned)
        .collect();
    for tag in tags
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(allocation.tag()))
    {
        ensure!(
            !tag.is_empty() && !tag.contains(['|', '\n', '\r']) && tag.trim() == tag,
            "invalid tag"
        );
        ensure!(
            !["personal", "joint", "business"].contains(&tag) || tag == allocation.tag(),
            "conflicting allocation tag"
        );
        if !values.iter().any(|v| v == tag) {
            values.push(tag.to_owned());
        }
    }
    Ok(values.join("|"))
}

pub fn run(args: ReviewArgs) -> Result<()> {
    let timer = Instant::now();
    let path = args.profile.unwrap_or_else(default_profile);
    if let ReviewCommand::Fingerprint { store } = args.command {
        println!(
            "{}",
            serde_json::to_string(&hashes(&store.unwrap_or_else(store_dir))?)?
        );
        return Ok(());
    }
    let _lock = Lock::acquire(&path)?;
    if let ReviewCommand::Init {
        csv,
        report,
        agenda,
        store,
        peer,
        retained_refunds,
        apply,
    } = args.command
    {
        ensure!(!path.exists(), "profile already exists");
        ensure!(retained_refunds >= Decimal::ZERO, "negative refunds");
        money(&retained_refunds.to_string())?;
        let table = Table::load(&csv)?;
        let profile = Profile {
            version: 1,
            csv: fs::canonicalize(csv)?,
            report: fs::canonicalize(report)?,
            agenda: fs::canonicalize(agenda)?,
            store: fs::canonicalize(store.unwrap_or_else(store_dir))?,
            peer,
            baseline: table.immutable_hash(&BANK_FIELDS)?,
            row_count: table.rows.len(),
            retained_refunds,
            pending_sync: None,
        };
        if profile.peer.is_some() {
            ensure!(
                profile.store == fs::canonicalize(store_dir())?,
                "peer sync requires the default store"
            );
        }
        let s = validate(
            &profile,
            &table,
            &Table::load(&profile.store.join("transactions.csv"))?,
        )?;
        let before = original(&[
            path.clone(),
            profile.csv.clone(),
            profile.report.clone(),
            profile.agenda.clone(),
            profile.store.join("transactions.csv"),
        ])?;
        ensure!(
            Table::parse(
                before[&profile.csv]
                    .as_deref()
                    .context("worksheet missing")?
            )?
            .bytes()?
                == table.bytes()?,
            "worksheet changed during adoption"
        );
        let old = String::from_utf8(before[&profile.report].clone().context("report missing")?)?;
        ensure!(!old.contains(START), "report already adopted");
        let report=format!("# Personal and joint expenditure review\n\n{}\n\n## Decisions recorded by fd-budget review\n\n{HISTORY}\n\n## Historical report — snapshot at adoption on {}\n\nThe narrative below is preserved as evidence. Its figures describe earlier stages; the generated tables above are authoritative for current allocations.\n\n{}",render(&profile,&table,&s),Local::now().format("%Y-%m-%d"),old);
        let mut backup = None;
        if apply {
            if let Some(peer) = &profile.peer {
                ensure!(
                    hashes(&profile.store)? == remote_hashes(peer)?,
                    "peer diverged"
                );
            }
            backup = Some(commit(
                &profile,
                vec![
                    (profile.report.clone(), report.into_bytes()),
                    (path.clone(), json_bytes(&profile)?),
                ],
                &before,
            )?);
        }
        return output(&table, &s, 5, apply, false, backup, timer);
    }
    let mut profile = read_profile(&path)?;
    let paths = [
        profile.csv.clone(),
        profile.report.clone(),
        profile.agenda.clone(),
        profile.store.join("transactions.csv"),
        path.clone(),
    ];
    let before = original(&paths)?;
    let mut table = Table::parse(
        before[&profile.csv]
            .as_deref()
            .context("worksheet missing")?,
    )?;
    let mut bank = Table::parse(
        before[&profile.store.join("transactions.csv")]
            .as_deref()
            .context("bank store missing")?,
    )?;
    let before_summary = validate(&profile, &table, &bank)?;
    match args.command {
        ReviewCommand::Status { limit } => {
            return output(&table, &before_summary, limit, false, false, None, timer)
        }
        ReviewCommand::Sync => {
            let synced = sync(&mut profile, &path)?;
            return output(&table, &before_summary, 5, false, synced, None, timer);
        }
        _ => {}
    }
    ensure!(
        profile.pending_sync.is_none(),
        "finish pending sync with review sync before another change"
    );
    let before_hashes = hashes(&profile.store)?;
    let ids = table.index()?;
    let bank_ids = bank.index()?;
    let mut notes = Vec::new();
    let mut business_notes = Vec::new();
    let mut changed_bank = BTreeSet::new();
    let apply = match args.command {
        ReviewCommand::Set {
            ids: selected,
            allocation,
            category,
            reason,
            tags,
            correct,
            apply,
        } => {
            text_ok(&category)?;
            text_ok(&reason)?;
            let mut seen = BTreeSet::new();
            for id in selected {
                ensure!(seen.insert(id.clone()), "duplicate requested ID {id}");
                let row = &mut table.rows[*ids
                    .get(&id)
                    .with_context(|| format!("unknown exact review ID {id}"))?];
                ensure!(
                    money(&row["amount"])? < Decimal::ZERO,
                    "cannot allocate a credit"
                );
                let old_group = row["group"].clone();
                let old_category = row["category"].clone();
                ensure!(old_group==REVIEW || (correct && matches!(old_group.as_str(),PERSONAL|JOINT|"Outside living costs") && (old_group!="Outside living costs" || old_category=="Business (confirmed by Will)")),"row already allocated or protected; --correct required for personal/joint/confirmed-business corrections");
                let live_id = field(row, "fd_budget_import_id")?.to_owned();
                let live = &mut bank.rows[*bank_ids
                    .get(&live_id)
                    .context("missing linked live bank row")?];
                let tag_text = tags_update(&live["tags"], allocation, &tags)?;
                live.insert("tags".into(), tag_text);
                changed_bank.insert(live_id);
                let tag_text = tags_update(&row["analysis_tags"], allocation, &tags)?;
                row.insert("analysis_tags".into(), tag_text);
                row.insert("group".into(), allocation.group().into());
                let target_category = if matches!(allocation, Allocation::Business) {
                    "Business (confirmed by Will)"
                } else {
                    &category
                };
                row.insert("category".into(), target_category.into());
                row.insert("reason".into(), reason.clone());
                row.entry("evidence".into()).and_modify(|e| {
                    e.push_str(&format!(
                        "; confirmed {} allocation on {}; linked live tags updated",
                        allocation.tag(),
                        Local::now().format("%Y-%m-%d")
                    ))
                });
                let product = if row["identified_product"].is_empty() {
                    &row["merchant"]
                } else {
                    &row["identified_product"]
                };
                let note = format!(
                    "{} — {} — {} — **{} / {}**. {} Bank ID `{}`.{}",
                    row["date"],
                    md(product),
                    gbp(-money(&row["amount"])?),
                    allocation.tag(),
                    md(&category),
                    md(&reason),
                    id,
                    if old_group != REVIEW {
                        format!(" Corrects {} / {}.", md(&old_group), md(&old_category))
                    } else {
                        String::new()
                    }
                );
                if matches!(allocation, Allocation::Business)
                    || (old_group == "Outside living costs" && old_category.starts_with("Business"))
                {
                    business_notes.push(format!(
                        "- {note} Receipt: {} ({}).",
                        md(&row["receipt_message_id"]),
                        md(&row["receipt_date"])
                    ));
                }
                notes.push(format!("- {note}"));
            }
            apply
        }
        ReviewCommand::Identify {
            id,
            product,
            merchant,
            receipt_id,
            receipt_date,
            evidence,
            apply,
        } => {
            for text in [&product, &receipt_id, &evidence] {
                text_ok(text)?;
            }
            if let Some(m) = &merchant {
                text_ok(m)?;
            }
            let idx = *ids.get(&id).context("unknown exact review ID")?;
            ensure!(
                table.rows.iter().enumerate().all(|(i, r)| i == idx
                    || receipt_key(&r["receipt_message_id"]) != receipt_key(&receipt_id)),
                "receipt already attached to another row; inspect split shipment manually"
            );
            let row = &mut table.rows[idx];
            ensure!(
                row["receipt_message_id"].is_empty()
                    || receipt_key(&row["receipt_message_id"]) == receipt_key(&receipt_id),
                "existing receipt differs; inspect manually"
            );
            let bank_date = NaiveDate::parse_from_str(&row["date"], "%Y-%m-%d")?;
            row.insert("identified_product".into(), product.clone());
            if let Some(m) = merchant {
                row.insert("merchant".into(), m);
            }
            row.insert("receipt_message_id".into(), receipt_id);
            row.insert("receipt_date".into(), receipt_date.to_string());
            row.insert(
                "receipt_bank_date_delta_days".into(),
                (bank_date - receipt_date).num_days().to_string(),
            );
            row.entry("evidence".into())
                .and_modify(|e| e.push_str(&format!("; {evidence}")));
            notes.push(format!(
                "- Receipt identified for `{id}`: {}. Allocation unchanged. {}",
                md(&product),
                md(&evidence)
            ));
            apply
        }
        _ => unreachable!(),
    };
    let s = validate(&profile, &table, &bank)?;
    let before_bank = Table::parse(
        before[&profile.store.join("transactions.csv")]
            .as_deref()
            .context("bank store missing")?,
    )?;
    ensure!(
        before_bank.rows.len() == bank.rows.len(),
        "live row count changed"
    );
    for (old, new) in before_bank.rows.iter().zip(&bank.rows) {
        for (key, value) in old {
            if key != "tags" {
                ensure!(new.get(key) == Some(value), "bank field changed");
            }
        }
        if old["tags"] != new["tags"] {
            ensure!(
                changed_bank.contains(&old["import_id"]),
                "unexpected tag change"
            );
        }
    }
    let old_report = String::from_utf8(before[&profile.report].clone().context("report missing")?)?;
    let report = replace_generated(&old_report, &render(&profile, &table, &s))?.replace(
        HISTORY,
        &format!(
            "{HISTORY}\n\n### {}\n\n{}",
            Local::now().format("%Y-%m-%d %H:%M:%S"),
            notes.join("\n")
        ),
    );
    let mut changes = vec![
        (profile.csv.clone(), table.bytes()?),
        (profile.report.clone(), report.into_bytes()),
    ];
    if !changed_bank.is_empty() {
        changes.push((profile.store.join("transactions.csv"), bank.bytes()?));
    }
    if !business_notes.is_empty() {
        let old = String::from_utf8(before[&profile.agenda].clone().context("agenda missing")?)?;
        let marker = "## Accrued since 14 Sep — for the next send";
        ensure!(
            old.matches(marker).count() == 1,
            "agenda insertion heading missing/duplicated"
        );
        let note=format!("{marker}\n\n### Review decisions — {}\n\n{}\n\nCheck existing company entries before adding/reimbursing. No company-ledger posting or message has been made.",Local::now().format("%Y-%m-%d"),business_notes.join("\n"));
        changes.push((
            profile.agenda.clone(),
            old.replacen(marker, &note, 1).into_bytes(),
        ));
    }
    if !apply {
        return output(&table, &s, 5, false, false, None, timer);
    }
    if let Some(peer) = &profile.peer {
        ensure!(
            profile.store == fs::canonicalize(store_dir())?,
            "peer sync requires default store"
        );
        ensure!(
            remote_hashes(peer)? == before_hashes,
            "peer diverged; no local changes applied"
        );
        let mut after = before_hashes.clone();
        if !changed_bank.is_empty() {
            after.insert("transactions.csv".into(), hash(&bank.bytes()?));
        }
        profile.pending_sync = Some(Pending {
            before: before_hashes.clone(),
            after,
        });
    }
    changes.push((path.clone(), json_bytes(&profile)?));
    ensure!(
        hashes(&profile.store)? == before_hashes,
        "store changed concurrently"
    );
    let backup = commit(&profile, changes, &before)?;
    let synced = sync(&mut profile, &path).with_context(|| {
        format!(
            "local decision saved; backup {}; use review sync to resume",
            backup.display()
        )
    })?;
    output(&table, &s, 5, true, synced, Some(backup), timer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_resume_accepts_only_known_before_or_after_files() {
        let before = Hashes::from([("a".into(), "old-a".into()), ("b".into(), "old-b".into())]);
        let after = Hashes::from([("a".into(), "new-a".into()), ("b".into(), "new-b".into())]);
        let p = Pending {
            before: before.clone(),
            after: after.clone(),
        };
        assert!(pending_compatible(&p, &before));
        assert!(pending_compatible(&p, &after));
        let mut partial = before.clone();
        partial.insert("a".into(), "new-a".into());
        assert!(pending_compatible(&p, &partial));
        partial.insert("a".into(), "independent-edit".into());
        assert!(!pending_compatible(&p, &partial));
        partial.remove("a");
        assert!(!pending_compatible(&p, &partial));
    }
    #[test]
    fn cents_rounding_and_conflicting_tags_are_explicit() {
        assert!(money("1.001").is_err());
        assert!(money("not-money").is_err());
        assert_eq!(gbp(Decimal::from_str("1087.845").unwrap()), "£1,087.85");
        assert_eq!(
            tags_update("shopping|business|research", Allocation::Personal, &[]).unwrap(),
            "shopping|research|personal"
        );
        assert!(tags_update("shopping", Allocation::Personal, &["joint".into()]).is_err());
        assert!(tags_update("shopping", Allocation::Personal, &["a|b".into()]).is_err());
    }
}
