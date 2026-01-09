use anyhow::Result;
use chrono::Local;

use crate::scrolls::{read_scroll, write_scroll};

/// Append an entry to the changelog
pub fn append_entry(entry: &str) -> Result<()> {
    let mut changelog = read_scroll("WILLIAM-CHANGELOG.md")?;

    // Ensure proper spacing before new entry
    if !changelog.ends_with("\n\n") {
        if !changelog.ends_with('\n') {
            changelog.push('\n');
        }
        changelog.push('\n');
    }

    changelog.push_str(entry);

    if !changelog.ends_with('\n') {
        changelog.push('\n');
    }

    write_scroll("WILLIAM-CHANGELOG.md", &changelog)
}

/// Generate a changelog entry from module updates
pub fn generate_entry(updates: &[(String, String)]) -> Result<String> {
    let date = Local::now().format("%Y-%m-%d");
    let modules: Vec<String> = updates.iter().map(|(name, _)| name.clone()).collect();

    let entry = format!(
        r#"### {} — Module import from conversation

**Advisor**: (imported via module tool)
**Context**: Automated import from conversation file
**Modules changed**: {}

**Summary of changes**:
- Imported module updates from external conversation

---
"#,
        date,
        if modules.is_empty() {
            "None".to_string()
        } else {
            modules.join(", ")
        }
    );

    Ok(entry)
}
