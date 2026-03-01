use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeSet;

use crate::capture::{read_baseline, run_command_live};
use crate::config::Config;

#[derive(Debug, Serialize)]
pub struct DriftReport {
    pub has_drift: bool,
    pub captures: Vec<CaptureDrift>,
}

#[derive(Debug, Serialize)]
pub struct CaptureDrift {
    pub name: String,
    pub status: DriftStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DriftStatus {
    Clean,
    Drift,
    NoBaseline,
    Error,
}

/// Compare live state against baselines. Returns the full drift report.
pub fn check_drift(config: &Config, quiet: bool) -> Result<DriftReport> {
    let state_dir = config.state_dir();
    let mut captures = Vec::new();
    let mut any_drift = false;

    for cap in &config.captures {
        let baseline = match read_baseline(&state_dir, &cap.output)? {
            Some(b) => b,
            None => {
                if !quiet {
                    eprintln!("  {} — no baseline (run 'state-capture capture' first)", cap.name);
                }
                captures.push(CaptureDrift {
                    name: cap.name.clone(),
                    status: DriftStatus::NoBaseline,
                    added: vec![],
                    removed: vec![],
                });
                continue;
            }
        };

        match run_command_live(&cap.command) {
            Ok(mut live) => {
                if cap.sort {
                    live = sort_lines(&live);
                }

                let baseline_set: BTreeSet<&str> = baseline
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect();
                let live_set: BTreeSet<&str> =
                    live.lines().filter(|l| !l.trim().is_empty()).collect();

                let added: Vec<String> = live_set
                    .difference(&baseline_set)
                    .map(|s| s.to_string())
                    .collect();
                let removed: Vec<String> = baseline_set
                    .difference(&live_set)
                    .map(|s| s.to_string())
                    .collect();

                let status = if added.is_empty() && removed.is_empty() {
                    DriftStatus::Clean
                } else {
                    any_drift = true;
                    DriftStatus::Drift
                };

                if !quiet {
                    match &status {
                        DriftStatus::Clean => println!("  {} — clean", cap.name),
                        DriftStatus::Drift => {
                            println!("  {} — DRIFT", cap.name);
                            for a in &added {
                                println!("    + {}", a);
                            }
                            for r in &removed {
                                println!("    - {}", r);
                            }
                        }
                        _ => {}
                    }
                }

                captures.push(CaptureDrift {
                    name: cap.name.clone(),
                    status,
                    added,
                    removed,
                });
            }
            Err(e) => {
                if !quiet {
                    eprintln!("  {} — ERROR: {}", cap.name, e);
                }
                any_drift = true;
                captures.push(CaptureDrift {
                    name: cap.name.clone(),
                    status: DriftStatus::Error,
                    added: vec![],
                    removed: vec![],
                });
            }
        }
    }

    Ok(DriftReport {
        has_drift: any_drift,
        captures,
    })
}

fn sort_lines(text: &str) -> String {
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort();
    let mut result = lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}
