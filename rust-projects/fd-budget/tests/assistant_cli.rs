//! Black-box contract tests: synthetic input only; never query real records.
use serde_json::Value;
use std::process::{Command, Output};
const BIN: &str = env!("CARGO_BIN_EXE_fd-budget");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("CLINICAL_ROOT", "/nonexistent-forge-assistant-fixture")
        .env("NOTMUCH_CONFIG", "/nonexistent-forge-assistant-fixture")
        .env("XDG_CONFIG_HOME", "/nonexistent-forge-assistant-fixture")
        .env("RUST_LOG", "synthetic-invalid-log-filter[")
        .output()
        .unwrap()
}

#[test]
fn capabilities_are_exact_static_metadata_despite_unavailable_data_sources() {
    let out = run(&["assistant", "capabilities"]);
    assert!(out.status.success());
    assert!(out.stderr.is_empty(), "unexpected diagnostic output");
    let actual: Value = serde_json::from_slice(&out.stdout).unwrap();
    let expected: Value =
        serde_json::from_str(include_str!("../src/assistant-capabilities.json")).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual["enforces_access_control"], false);
}

#[test]
fn malformed_assistant_request_does_not_echo_arguments() {
    let out = run(&["assistant", "synthetic-private-canary-DO-NOT-ECHO"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stderr.is_empty());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["error"], "invalid_request");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("DO-NOT-ECHO"));
}

#[test]
fn legacy_parser_help_version_and_unknown_command_keep_their_channels() {
    let help = run(&["--help"]);
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage:"));
    assert!(String::from_utf8_lossy(&help.stdout).contains("import"));
    let unknown = run(&["synthetic-unknown-legacy-command"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(unknown.stdout.is_empty());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("synthetic-unknown-legacy-command"));
    let version = run(&["--version"]);
    // This product did not have a root --version option before the addition.
    assert_eq!(version.status.code(), Some(2));
    assert!(version.stdout.is_empty());
    assert!(String::from_utf8_lossy(&version.stderr).contains("--version"));
}
