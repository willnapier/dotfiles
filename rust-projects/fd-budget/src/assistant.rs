//! Content-free discovery only. Existing record outputs are NOT authorised by this interface.
use clap::Subcommand;

/// Keep malformed assistant requests out of durable diagnostics too.
/// Legacy CLI parsing/help remain unchanged.
pub fn parse_cli<T: clap::Parser>() -> T {
    match T::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            if std::env::args_os().skip(1).any(|arg| arg == "assistant") && error.use_stderr() {
                println!(
                    "{}",
                    serde_json::json!({
                        "protocol": "forge-assistant", "schema_version": 1,
                        "program": "fd-budget", "status": "refused",
                        "error": "invalid_request", "enforces_access_control": false
                    })
                );
                std::process::exit(2);
            }
            error.exit()
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum Action {
    /// Print static operation metadata without reading config, ledgers or mail.
    Capabilities,
}

pub fn run(action: &Action) {
    match action {
        Action::Capabilities => println!("{}", include_str!("assistant-capabilities.json").trim()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn manifest_does_not_authorise_existing_sensitive_commands() {
        let m: serde_json::Value =
            serde_json::from_str(include_str!("assistant-capabilities.json")).unwrap();
        assert_eq!(m["schema_version"], 1);
        assert_eq!(m["program"], "fd-budget");
        assert_eq!(m["coverage"], "explicit_subset");
        assert_eq!(m["enforces_access_control"], false);
        let operations = m["operations"].as_array().unwrap();
        assert_eq!(operations.len(), 1);
        assert_eq!(operations[0]["id"], "capabilities");
        assert_eq!(operations[0]["effect"], "none");
        assert_eq!(operations[0]["output"], "static_metadata");
    }
}
