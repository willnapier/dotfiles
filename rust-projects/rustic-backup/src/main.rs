mod exec;
use anyhow::{bail, Context, Result};
use chrono::Utc;
use exec::{Exec, Real};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::SystemTime,
};

struct Config {
    root: PathBuf,
    profile: String,
    max_age: u64,
    ssh_root: PathBuf,
}
impl Config {
    fn env_file(&self) -> PathBuf {
        self.root
            .join("secrets")
            .join(format!("{}.env", self.profile))
    }
    fn password(&self) -> PathBuf {
        self.root
            .join("secrets")
            .join(format!("{}.password", self.profile))
    }
    fn marker(&self) -> PathBuf {
        self.root.join("last-success").join(&self.profile)
    }
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("no parent")?)?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut f = File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}
fn record(c: &Config, action: &str, status: &str, detail: &str) -> Result<()> {
    atomic(
        &c.root
            .join("outcomes")
            .join(format!("{}-{action}.json", c.profile)),
        &serde_json::to_vec(
            &serde_json::json!({"profile":c.profile,"action":action,"status":status,"detail":detail,"time":Utc::now().to_rfc3339(),"version":env!("CARGO_PKG_VERSION")}),
        )?,
    )
}
fn private_file(path: &Path) -> Result<()> {
    let m = fs::metadata(path).context("required private file missing")?;
    if !m.is_file() || m.permissions().mode() & 0o777 != 0o600 {
        bail!("required private file must be regular and mode 0600")
    }
    Ok(())
}
fn credentials(c: &Config, ex: &dyn Exec) -> Result<Vec<(String, String)>> {
    private_file(&c.env_file())?;
    private_file(&c.password())?;
    // Preserve the existing trusted shell .env format. Never log its stdout,
    // stderr or values; credentials remain in memory and the child's environment.
    let out=ex.run("bash", &["-c", "set -ae; source \"$1\"; printf '%s\\0' \"${OPENDAL_ENDPOINT:?}\" \"${OPENDAL_USER:?}\" \"${OPENDAL_KEY:?}\" \"${OPENDAL_ROOT:?}\"", "rustic-credentials", c.env_file().to_str().context("env path")?]);
    if !out.ok() {
        bail!("cannot load required OPENDAL credentials")
    }
    let values: Vec<_> = out.stdout.split_terminator('\0').collect();
    if values.len() != 4 || values.iter().any(|s| s.is_empty()) {
        bail!("incomplete OPENDAL credentials")
    }
    private_file(Path::new(values[2]))?;
    let mut env: Vec<_> = [
        "OPENDAL_ENDPOINT",
        "OPENDAL_USER",
        "OPENDAL_KEY",
        "OPENDAL_ROOT",
    ]
    .iter()
    .zip(values)
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    env.push((
        "RUSTIC_PASSWORD_FILE".into(),
        c.password().to_string_lossy().into(),
    ));
    Ok(env)
}
fn version(ex: &dyn Exec) -> Result<()> {
    let r = ex.run("rustic", &["--version"]);
    if !r.ok()
        || r.stdout.split_whitespace().next() != Some("rustic")
        || r.stdout
            .split_whitespace()
            .nth(1)
            .map(|v| v.trim_start_matches('v'))
            != Some("0.11.3")
    {
        bail!("expected rustic 0.11.3; version check failed")
    }
    Ok(())
}
fn master_pids(out: &str, root: &Path, key: &str) -> Vec<String> {
    let prefix = format!("ssh -E {}/.ssh-connection", root.display());
    out.lines()
        .filter_map(|l| {
            let l = l.trim();
            let i = l.find(char::is_whitespace)?;
            let (pid, args) = l.split_at(i);
            let args = args.trim();
            (pid.parse::<u32>().is_ok()
                && args.starts_with(&prefix)
                && args.contains(&format!(" -i {key} ")))
            .then(|| pid.to_string())
        })
        .collect()
}
fn cleanup(c: &Config, env: &[(String, String)], ex: &dyn Exec) -> Result<()> {
    let uid = ex.run("id", &["-u"]);
    if !uid.ok() {
        bail!("cannot determine SSH master owner")
    }
    let r = ex.run("ps", &["-u", &uid.out(), "-o", "pid=,args="]);
    if !r.ok() {
        bail!("cannot inspect Rustic SSH masters")
    }
    let key = &env
        .iter()
        .find(|(k, _)| k == "OPENDAL_KEY")
        .context("missing key")?
        .1;
    for pid in master_pids(&r.stdout, &c.ssh_root, key) {
        let r = ex.run("kill", &["-TERM", &pid]);
        if !r.ok() && ex.run("kill", &["-0", &pid]).ok() {
            bail!("could not terminate Rustic SSH master")
        }
    }
    Ok(())
}
fn health(c: &Config, now: SystemTime) -> Result<String> {
    private_file(&c.env_file())?;
    private_file(&c.password())?;
    let marker = c.marker();
    let age = now
        .duration_since(
            fs::metadata(&marker)
                .context("configured but no successful backup marker")?
                .modified()?,
        )
        .context("backup marker is in the future")?
        .as_secs();
    let stamp = fs::read_to_string(marker)?;
    chrono::DateTime::parse_from_rfc3339(stamp.trim()).context("invalid success marker")?;
    if age > c.max_age {
        bail!(
            "{} last succeeded {} hours ago (threshold {}h)",
            c.profile,
            age / 3600,
            c.max_age / 3600
        )
    }
    Ok(format!(
        "{}: healthy; last success {}",
        c.profile,
        stamp.trim()
    ))
}
fn perform(
    c: &Config,
    action: &str,
    rest: &[String],
    env: &[(String, String)],
    ex: &dyn Exec,
) -> Result<String> {
    let mut args = vec!["-P", c.profile.as_str()];
    match action {
        "check-data-subset" => args.extend(["check", "--read-data", "--read-data-subset=10%"]),
        _ => args.push(action),
    }
    args.extend(rest.iter().map(String::as_str));
    let r = ex.operation(&args, env);
    if !r.ok() {
        bail!("{} {action} failed (exit {})", c.profile, r.exit_code)
    }
    if action == "backup" {
        atomic(
            &c.marker(),
            format!("{}\n", Utc::now().format("%Y-%m-%dT%H:%M:%SZ")).as_bytes(),
        )?;
    }
    Ok(format!("{} {action} succeeded", c.profile))
}
fn run(c: &Config, action: &str, rest: &[String], ex: &dyn Exec) -> Result<String> {
    if action == "status" {
        return health(c, SystemTime::now());
    }
    version(ex)?;
    let env = credentials(c, ex)?;
    fs::create_dir_all(&c.root)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(c.root.join("operation.lock"))?;
    lock.try_lock_exclusive()
        .context("another Rustic repository operation is already running")?;
    // Hold the lock until both Rustic and its dedicated SSH masters are finished.
    let result = perform(c, action, rest, &env, ex);
    let cleaned = cleanup(c, &env, ex);
    result.and_then(|s| {
        cleaned?;
        Ok(s)
    })
}
fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let usage="usage: rustic-backup backup|check|check-data-subset|snapshots|status|init|restore SNAPSHOT[:PATH] DESTINATION";
    if args.first().is_some_and(|s| s == "--help" || s == "-h") {
        println!("{usage}");
        return std::process::ExitCode::SUCCESS;
    }
    if args.first().is_some_and(|s| s == "--version") {
        println!("rustic-backup {}", env!("CARGO_PKG_VERSION"));
        return std::process::ExitCode::SUCCESS;
    }
    let Some(action) = args.first() else {
        eprintln!("{usage}");
        return 2.into();
    };
    if ![
        "backup",
        "check",
        "check-data-subset",
        "snapshots",
        "status",
        "init",
        "restore",
    ]
    .contains(&action.as_str())
        || (action == "restore" && args.len() != 3)
        || (action != "restore" && args.len() != 1)
    {
        eprintln!("{usage}");
        return 2.into();
    }
    let result = (|| -> Result<()> {
        let home = PathBuf::from(std::env::var_os("HOME").context("HOME not set")?);
        let profile =
            std::env::var("RUSTIC_BACKUP_PROFILE").unwrap_or_else(|_| "nimbini-general".into());
        if profile.is_empty()
            || !profile
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("invalid profile name")
        }
        let c = Config {
            root: std::env::var_os("RUSTIC_BACKUP_STATE_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state/rustic-backup")),
            profile,
            max_age: std::env::var("RUSTIC_BACKUP_MAX_AGE")
                .unwrap_or_else(|_| "129600".into())
                .parse()
                .context("invalid max age")?,
            ssh_root: std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state")),
        };
        record(&c, action, "running", "operation started")?;
        let r = run(&c, action, &args[1..], &Real);
        match r {
            Ok(s) => {
                record(&c, action, "success", &s)?;
                println!("rustic-backup: outcome=success {s}");
                Ok(())
            }
            Err(e) => {
                record(&c, action, "error", &format!("{e:#}"))?;
                Err(e)
            }
        }
    })();
    if let Err(e) = result {
        eprintln!("rustic-backup: {e:#}");
        let _ = Real.run(
            "notify-user",
            &[
                "--tool",
                "rustic-backup",
                "--urgency",
                "critical",
                "Rustic backup",
                &format!("{action} failed; see Rustic outcome/log"),
            ],
        );
        return 1.into();
    }
    std::process::ExitCode::SUCCESS
}
#[cfg(test)]
mod tests {
    use super::*;
    use exec::{CmdResult, Fake};
    fn config(t: &tempfile::TempDir) -> Config {
        Config {
            root: t.path().into(),
            profile: "synthetic".into(),
            max_age: 3600,
            ssh_root: "/state".into(),
        }
    }
    fn private(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    }
    #[test]
    fn version_red_green() {
        for (s, ok) in [
            ("rustic v0.11.3", true),
            ("rustic 0.11.3", true),
            ("rustic 0.11.4", false),
            ("", false),
        ] {
            let mut f = Fake::default();
            f.respond("rustic", &["--version"], CmdResult::success(s));
            assert_eq!(version(&f).is_ok(), ok);
        }
        assert!(version(&Fake::default()).is_err());
    }
    #[test]
    fn failed_backup_cannot_advance_marker_green_can() {
        let t = tempfile::tempdir().unwrap();
        let c = config(&t);
        atomic(&c.marker(), b"old\n").unwrap();
        let mut f = Fake::default();
        f.respond(
            "rustic",
            &["-P", "synthetic", "backup"],
            CmdResult::failure(3, "partial backup"),
        );
        assert!(perform(&c, "backup", &[], &[], &f).is_err());
        assert_eq!(fs::read_to_string(c.marker()).unwrap(), "old\n");
        f.respond(
            "rustic",
            &["-P", "synthetic", "backup"],
            CmdResult::success("snapshot saved"),
        );
        assert!(perform(&c, "backup", &[], &[], &f).is_ok());
        private(&c.env_file());
        private(&c.password());
        assert!(health(&c, SystemTime::now()).is_ok());
        assert!(health(&c, SystemTime::now() + std::time::Duration::from_secs(3601)).is_err());
    }
    #[test]
    fn missing_credentials_marker_and_bad_permissions_are_red() {
        let t = tempfile::tempdir().unwrap();
        let c = config(&t);
        assert!(health(&c, SystemTime::now()).is_err());
        private(&c.env_file());
        private(&c.password());
        assert!(health(&c, SystemTime::now()).is_err());
        atomic(&c.marker(), b"invalid").unwrap();
        assert!(health(&c, SystemTime::now()).is_err());
        fs::set_permissions(c.password(), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(private_file(&c.password()).is_err());
    }
    #[test]
    fn checks_propagate_failure_and_keep_backup_marker() {
        let t = tempfile::tempdir().unwrap();
        let c = config(&t);
        let mut f = Fake::default();
        f.respond(
            "rustic",
            &[
                "-P",
                "synthetic",
                "check",
                "--read-data",
                "--read-data-subset=10%",
            ],
            CmdResult::success("ok"),
        );
        assert!(perform(&c, "check-data-subset", &[], &[], &f).is_ok());
        assert!(!c.marker().exists());
        assert!(perform(&c, "check", &[], &[], &f).is_err());
    }
    #[test]
    fn cleanup_only_dedicated_masters_and_outcome_record() {
        assert_eq!(master_pids("12 ssh -E /state/.ssh-connection123 -i /secret/key -M host\n13 ssh -E /state/.ssh-connection9 -i /other/key host\n14 ssh host",Path::new("/state"),"/secret/key"),vec!["12"]);
        let t = tempfile::tempdir().unwrap();
        let c = config(&t);
        record(&c, "backup", "error", "synthetic failure").unwrap();
        let v: serde_json::Value = serde_json::from_slice(
            &fs::read(c.root.join("outcomes/synthetic-backup.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(v["status"], "error");
    }
    #[test]
    fn operation_lock_excludes_another_process() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("lock");
        let a = File::create(&p).unwrap();
        let b = File::create(&p).unwrap();
        a.try_lock_exclusive().unwrap();
        assert!(b.try_lock_exclusive().is_err());
    }
}
