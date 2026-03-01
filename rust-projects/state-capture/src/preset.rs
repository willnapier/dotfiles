use crate::config::{Capture, Config, Settings};
use std::process::Command;

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Preset {
    Arch,
    Debian,
    Fedora,
    Macos,
}

impl std::fmt::Display for Preset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Preset::Arch => write!(f, "arch"),
            Preset::Debian => write!(f, "debian"),
            Preset::Fedora => write!(f, "fedora"),
            Preset::Macos => write!(f, "macos"),
        }
    }
}

/// Auto-detect platform/distro by checking uname and package managers.
pub fn detect_distro() -> Option<Preset> {
    if is_macos() {
        Some(Preset::Macos)
    } else if has_command("pacman") {
        Some(Preset::Arch)
    } else if has_command("apt") {
        Some(Preset::Debian)
    } else if has_command("dnf") {
        Some(Preset::Fedora)
    } else {
        None
    }
}

fn is_macos() -> bool {
    std::env::consts::OS == "macos"
}

fn has_command(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Captures common to all Linux presets.
fn linux_common_captures() -> Vec<Capture> {
    vec![
        Capture {
            name: "system-services-enabled".into(),
            command: r#"systemctl list-unit-files --type=service --state=enabled --no-legend --no-pager | awk '{print $1}'"#.into(),
            output: "system-services-enabled.txt".into(),
            sort: true,
        },
        Capture {
            name: "system-timers-enabled".into(),
            command: r#"systemctl list-unit-files --type=timer --state=enabled --no-legend --no-pager | awk '{print $1}'"#.into(),
            output: "system-timers-enabled.txt".into(),
            sort: true,
        },
        Capture {
            name: "user-units-enabled".into(),
            command: r#"systemctl --user list-unit-files --state=enabled --no-legend --no-pager | awk '{print $1}'"#.into(),
            output: "user-units-enabled.txt".into(),
            sort: true,
        },
    ]
}

/// Captures common to all presets (Linux and macOS).
fn universal_captures() -> Vec<Capture> {
    vec![
        Capture {
            name: "npm-globals".into(),
            command: "npm list -g --depth=0 --parseable 2>/dev/null | tail -n +2 | xargs -I{} basename {}".into(),
            output: "npm-globals.txt".into(),
            sort: true,
        },
        Capture {
            name: "user-groups".into(),
            command: r#"id -nG | tr " " "\n""#.into(),
            output: "user-groups.txt".into(),
            sort: true,
        },
    ]
}

pub fn preset_config(preset: Preset) -> Config {
    let mut captures = match preset {
        Preset::Arch => {
            let mut c = vec![
                Capture {
                    name: "arch-packages".into(),
                    command: "pacman -Qqe".into(),
                    output: "arch-packages.txt".into(),
                    sort: true,
                },
                Capture {
                    name: "arch-packages-aur".into(),
                    command: "pacman -Qqem".into(),
                    output: "arch-packages-aur.txt".into(),
                    sort: true,
                },
            ];
            c.extend(linux_common_captures());
            c
        }
        Preset::Debian => {
            let mut c = vec![Capture {
                name: "debian-packages".into(),
                command: "dpkg-query -W -f='${Package}\\n'".into(),
                output: "debian-packages.txt".into(),
                sort: true,
            }];
            c.extend(linux_common_captures());
            c
        }
        Preset::Fedora => {
            let mut c = vec![Capture {
                name: "fedora-packages".into(),
                command: "dnf list installed --quiet | tail -n +2 | awk '{print $1}'".into(),
                output: "fedora-packages.txt".into(),
                sort: true,
            }];
            c.extend(linux_common_captures());
            c
        }
        Preset::Macos => vec![
            Capture {
                name: "brew-formulae".into(),
                command: "brew list --formula -1".into(),
                output: "brew-formulae.txt".into(),
                sort: true,
            },
            Capture {
                name: "brew-casks".into(),
                command: "brew list --cask -1".into(),
                output: "brew-casks.txt".into(),
                sort: true,
            },
            Capture {
                name: "launchagents".into(),
                command: r#"ls ~/Library/LaunchAgents/ 2>/dev/null | sed 's/\.plist$//' "#.into(),
                output: "launchagents.txt".into(),
                sort: true,
            },
        ],
    };

    captures.extend(universal_captures());

    // Arch gets extra capture for cargo crates
    if matches!(preset, Preset::Arch) {
        captures.push(Capture {
            name: "cargo-crates".into(),
            command: "capture-cargo-crates".into(),
            output: "cargo-crates.txt".into(),
            sort: false,
        });
    }

    Config {
        settings: Settings {
            state_dir: "~/dotfiles".into(),
        },
        captures,
    }
}
