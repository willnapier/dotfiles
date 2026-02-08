use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

/// The base directory for all scrolls
pub fn scrolls_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join("Assistants/shared")
}

/// Content scrolls - change frequently, upload each session
pub const CONTENT_SCROLLS: &[&str] = &[
    "WILLIAM-PHILOSOPHICAL.md",
    "WILLIAM-BIOGRAPHICAL.md",
    "WILLIAM-LIFESTYLE.md",
    "WILLIAM-SOCIAL.md",
    "WILLIAM-DIETARY.md",
    "WILLIAM-FINANCIAL-PLANNING-CONTEXT.md",
    "WILLIAM-CHANGELOG.md",
];

/// Protocol files - rarely change, persist in AI project
pub const PROTOCOL_FILES: &[&str] = &[
    "WILLIAM-INDEX.md",
    "SENECA-PROTOCOL.md",
    "GEOFF-PROTOCOL.md",
    "DIANA-PROTOCOL.md",
];

/// Advisor configurations - which scrolls each advisor needs
pub fn advisor_scrolls(advisor: &str) -> Vec<&'static str> {
    match advisor.to_lowercase().as_str() {
        "seneca" => vec![
            "WILLIAM-INDEX.md",
            "SENECA-PROTOCOL.md",
            "WILLIAM-LIFESTYLE.md",
            "WILLIAM-SOCIAL.md",
            "WILLIAM-FINANCIAL-PLANNING-CONTEXT.md",
            "WILLIAM-CHANGELOG.md",
            // On-demand: PHILOSOPHICAL, BIOGRAPHICAL
        ],
        "geoff" => vec![
            "WILLIAM-INDEX.md",
            "GEOFF-PROTOCOL.md",
            "WILLIAM-PHILOSOPHICAL.md",
            "WILLIAM-CHANGELOG.md",
            // On-demand: BIOGRAPHICAL, LIFESTYLE
        ],
        "diana" => vec![
            "WILLIAM-INDEX.md",
            "DIANA-PROTOCOL.md",
            "WILLIAM-DIETARY.md",
            "WILLIAM-LIFESTYLE.md",
            "WILLIAM-CHANGELOG.md",
            // On-demand: BIOGRAPHICAL
        ],
        _ => {
            // Default: all content scrolls plus index
            let mut scrolls: Vec<&str> = vec!["WILLIAM-INDEX.md"];
            scrolls.extend(CONTENT_SCROLLS.iter());
            scrolls
        }
    }
}

/// Read a scroll's content
pub fn read_scroll(name: &str) -> Result<String> {
    let path = scrolls_dir().join(name);
    fs::read_to_string(&path)
        .with_context(|| format!("Failed to read scroll: {}", path.display()))
}

/// Write a scroll's content
pub fn write_scroll(name: &str, content: &str) -> Result<()> {
    let path = scrolls_dir().join(name);
    fs::write(&path, content)
        .with_context(|| format!("Failed to write scroll: {}", path.display()))
}

/// Get scroll sizes for display
pub fn scroll_sizes() -> Result<HashMap<String, usize>> {
    let mut sizes = HashMap::new();
    let dir = scrolls_dir();

    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "md") {
            if let Ok(metadata) = entry.metadata() {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                sizes.insert(name, metadata.len() as usize);
            }
        }
    }

    Ok(sizes)
}

/// Verify scroll consistency
pub fn verify() -> Result<()> {
    let dir = scrolls_dir();
    println!("Scrolls directory: {}", dir.display());
    println!();

    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Check all expected scrolls exist
    for scroll in CONTENT_SCROLLS.iter().chain(PROTOCOL_FILES.iter()) {
        let path = dir.join(scroll);
        if !path.exists() {
            if *scroll == "GEOFF-PROTOCOL.md" {
                warnings.push(format!("{} not found (optional)", scroll));
            } else {
                errors.push(format!("{} not found", scroll));
            }
        }
    }

    // Check INDEX references match actual files
    let index_path = dir.join("WILLIAM-INDEX.md");
    if index_path.exists() {
        let index_content = fs::read_to_string(&index_path)?;
        for scroll in CONTENT_SCROLLS {
            if !index_content.contains(scroll) {
                warnings.push(format!("{} not referenced in INDEX", scroll));
            }
        }
    }

    // Report results
    if errors.is_empty() && warnings.is_empty() {
        println!("✓ All scrolls present and consistent");
    } else {
        if !errors.is_empty() {
            println!("Errors:");
            for e in &errors {
                println!("  ✗ {}", e);
            }
        }
        if !warnings.is_empty() {
            println!("Warnings:");
            for w in &warnings {
                println!("  ⚠ {}", w);
            }
        }
    }

    Ok(())
}

/// List scroll state
pub fn list(full: bool) -> Result<()> {
    let dir = scrolls_dir();
    println!("Scrolls directory: {}", dir.display());
    println!();

    let sizes = scroll_sizes()?;

    println!("Content Scrolls (upload each session):");
    for scroll in CONTENT_SCROLLS {
        let size = sizes.get(*scroll).unwrap_or(&0);
        let size_kb = *size as f64 / 1024.0;
        println!("  {} ({:.1}KB)", scroll, size_kb);

        if full {
            if let Ok(content) = read_scroll(scroll) {
                let lines: Vec<&str> = content.lines().take(5).collect();
                for line in lines {
                    println!("    │ {}", line);
                }
                println!("    │ ...");
            }
        }
    }

    println!();
    println!("Protocol Files (persist in AI project):");
    for scroll in PROTOCOL_FILES {
        let path = dir.join(scroll);
        if path.exists() {
            let size = sizes.get(*scroll).unwrap_or(&0);
            let size_kb = *size as f64 / 1024.0;
            println!("  {} ({:.1}KB)", scroll, size_kb);
        } else {
            println!("  {} (not created)", scroll);
        }
    }

    Ok(())
}
