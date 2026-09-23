# logkeep

Cap a tool's own log at N bytes, keeping exactly one predecessor (`<log>.1`). Library only; audit row D2-24.

```toml
[dependencies]
logkeep = { path = "../logkeep" }
```

```rust
// The tool opens its log by path each run (timer-run oneshot): rename strategy.
match logkeep::cap(&log, 5 * logkeep::MB) {
    Ok(o) if o.rolled() => eprintln!("rolled {} to {}", log.display(), logkeep::predecessor(&log).display()),
    Ok(_) => {}
    Err(e) => eprintln!("could not cap {}: {e}", log.display()), // advisory — carry on
}

// A supervisor holds the log open for the tool's whole life
// (launchd StandardOutPath, systemd StandardOutput=append:): copy-truncate.
let _ = logkeep::cap_in_place(&log, 20 * logkeep::MB);
```

`cap` on a supervisor-held file frees nothing until the unit restarts (the supervisor keeps the renamed inode); `cap_in_place` on a file the tool reopens by path also works, at the cost of a copy. Both return `Outcome::{Missing, Kept(bytes), Rolled(bytes)}`; a missing log is not an error, a directory at the path is.
