use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

use crate::scrolls::{advisor_scrolls, read_scroll};

/// Extract just the filename from a scroll name (handles ~/... paths)
fn scroll_display_name(name: &str) -> &str {
    Path::new(name)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(name)
}

/// Run the export command
pub fn run(advisor: &str, output: Option<&str>, zip: bool) -> Result<()> {
    let scrolls = advisor_scrolls(advisor);

    println!("Exporting scrolls for {} advisor:", advisor);
    for scroll in &scrolls {
        println!("  • {}", scroll);
    }
    println!();

    let output_dir = match output {
        Some(p) => PathBuf::from(p),
        None => dirs::home_dir()
            .expect("Could not find home directory")
            .join("Downloads"),
    };

    if zip {
        export_zip(advisor, &scrolls, &output_dir)
    } else {
        export_directory(advisor, &scrolls, &output_dir)
    }
}

/// Export scrolls to a directory
fn export_directory(advisor: &str, scrolls: &[&str], output_dir: &PathBuf) -> Result<()> {
    let timestamp = chrono::Local::now().format("%Y-%m-%d");
    let bundle_name = format!("{}-scrolls-{}", advisor, timestamp);
    let bundle_dir = output_dir.join(&bundle_name);

    fs::create_dir_all(&bundle_dir)
        .with_context(|| format!("Failed to create directory: {}", bundle_dir.display()))?;

    for scroll in scrolls {
        let content = read_scroll(scroll)?;
        let filename = scroll_display_name(scroll);
        let dest = bundle_dir.join(filename);
        fs::write(&dest, &content)
            .with_context(|| format!("Failed to write: {}", dest.display()))?;
    }

    // Create a README for the bundle
    let protocol_file = format!("{}-PROTOCOL.md", advisor.to_uppercase());
    let has_protocol = scrolls.iter().any(|s| s.ends_with("PROTOCOL.md"));
    let usage_steps = if has_protocol {
        format!(
            "1. Upload all files to your AI conversation\n\
            2. The AI will read WILLIAM-INDEX.md first to understand the system\n\
            3. Then load the protocol file ({})\n\
            4. Content scrolls provide context as needed",
            protocol_file
        )
    } else {
        "1. Upload all files to your AI conversation\n\
        2. The AI will read WILLIAM-INDEX.md first to understand the system\n\
        3. Remaining files provide context for the session".to_string()
    };
    let readme = format!(
        "# {} Scrolls Bundle\n\n\
        Exported: {}\n\n\
        ## Contents\n\n\
        {}\n\n\
        ## Usage\n\n\
        {}\n",
        advisor.to_uppercase(),
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        scrolls.iter().map(|s| format!("- {}", scroll_display_name(s))).collect::<Vec<_>>().join("\n"),
        usage_steps
    );
    fs::write(bundle_dir.join("README.md"), readme)?;

    println!("✓ Exported to: {}", bundle_dir.display());
    println!();
    println!("Upload these files to start your {} session.", advisor);

    Ok(())
}

/// Export scrolls to a zip file
fn export_zip(advisor: &str, scrolls: &[&str], output_dir: &PathBuf) -> Result<()> {
    let timestamp = chrono::Local::now().format("%Y-%m-%d");
    let zip_name = format!("{}-scrolls-{}.zip", advisor, timestamp);
    let zip_path = output_dir.join(&zip_name);

    let file = fs::File::create(&zip_path)
        .with_context(|| format!("Failed to create zip: {}", zip_path.display()))?;
    let mut zip = ZipWriter::new(file);

    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for scroll in scrolls {
        let content = read_scroll(scroll)?;
        let filename = scroll_display_name(scroll);
        zip.start_file(filename, options)?;
        zip.write_all(content.as_bytes())?;
    }

    zip.finish()?;

    println!("✓ Exported to: {}", zip_path.display());
    println!();
    println!("Upload this zip to start your {} session.", advisor);

    Ok(())
}
