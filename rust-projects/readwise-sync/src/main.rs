//! Readwise Sync - Local backup of Readwise highlights and Reader articles
//!
//! Syncs to ~/Captures/readwise/ with incremental updates.
//! Run nightly via launchd/systemd.

use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use slug::slugify;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const READWISE_EXPORT_URL: &str = "https://readwise.io/api/v2/export/";
const READER_LIST_URL: &str = "https://readwise.io/api/v3/list/";

// ============================================================================
// Data structures for Readwise Highlights API (v2)
// ============================================================================

#[derive(Debug, Deserialize)]
struct HighlightsExportResponse {
    count: u32,
    #[serde(rename = "nextPageCursor", deserialize_with = "deserialize_optional_id")]
    next_page_cursor: Option<String>,
    results: Vec<Book>,
}

#[derive(Debug, Deserialize)]
struct Book {
    #[serde(rename = "user_book_id", deserialize_with = "deserialize_id")]
    id: String,
    title: String,
    author: Option<String>,
    #[serde(default)]
    category: String,
    source: Option<String>,
    #[serde(default)]
    num_highlights: u32,
    #[serde(default)]
    highlights: Vec<Highlight>,
    #[serde(default)]
    book_tags: Vec<Tag>,
    #[serde(rename = "unique_url")]
    source_url: Option<String>,
    cover_image_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Highlight {
    #[serde(deserialize_with = "deserialize_id")]
    id: String,
    text: String,
    note: Option<String>,
    location: Option<u32>,
    location_type: Option<String>,
    highlighted_at: Option<String>,
    url: Option<String>,
    color: Option<String>,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    is_deleted: bool,
}

#[derive(Debug, Deserialize)]
struct Tag {
    name: String,
}

// ============================================================================
// Data structures for Reader API (v3)
// ============================================================================

#[derive(Debug, Deserialize)]
struct ReaderListResponse {
    #[serde(default)]
    count: u32,
    #[serde(rename = "nextPageCursor", deserialize_with = "deserialize_optional_id", default)]
    next_page_cursor: Option<String>,
    #[serde(default)]
    results: Vec<Document>,
}

#[derive(Debug, Deserialize)]
struct Document {
    #[serde(deserialize_with = "deserialize_id")]
    id: String,
    url: String,
    title: Option<String>,
    author: Option<String>,
    #[serde(default)]
    category: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    tags: HashMap<String, serde_json::Value>,
    word_count: Option<u32>,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    reading_progress: f32,
    source_url: Option<String>,
    #[serde(default)]
    source: Option<String>,
    site_name: Option<String>,
    summary: Option<String>,
    notes: Option<String>,
    published_date: Option<String>,
    /// Full HTML content of the document (when withHtmlContent=true)
    html_content: Option<String>,
}

// ============================================================================
// Sync state persistence
// ============================================================================

#[derive(Debug, Serialize, Deserialize, Default)]
struct SyncState {
    last_highlights_sync: Option<String>,
    last_reader_sync: Option<String>,
}

impl SyncState {
    fn load(path: &PathBuf) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self, path: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }
}

// ============================================================================
// Main sync logic
// ============================================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Get API token
    let token = get_api_token()?;

    // Set up paths
    let base_dir = get_base_dir();
    let highlights_dir = base_dir.join("highlights");
    let reader_dir = base_dir.join("reader");
    let state_path = base_dir.join("sync-state.json");

    // Ensure directories exist
    fs::create_dir_all(&highlights_dir)?;
    fs::create_dir_all(&reader_dir)?;

    // Load sync state
    let mut state = SyncState::load(&state_path);
    let now = Utc::now().to_rfc3339();

    // Create HTTP client
    let client = create_client(&token)?;

    // Sync highlights
    println!("Syncing Readwise highlights...");
    let highlights_count = sync_highlights(&client, &highlights_dir, &state.last_highlights_sync)?;
    println!("  Synced {} books with highlights", highlights_count);
    state.last_highlights_sync = Some(now.clone());

    // Sync Reader documents
    println!("Syncing Reader documents...");
    let reader_count = sync_reader(&client, &reader_dir, &state.last_reader_sync)?;
    println!("  Synced {} documents", reader_count);
    state.last_reader_sync = Some(now);

    // Save state
    state.save(&state_path)?;
    println!("Sync complete!");

    Ok(())
}

fn get_api_token() -> Result<String, Box<dyn std::error::Error>> {
    // Try environment variable first
    if let Ok(token) = env::var("READWISE_TOKEN") {
        return Ok(token);
    }

    // Try config file
    let config_path = dirs::home_dir()
        .ok_or("Could not find home directory")?
        .join(".config")
        .join("readwise")
        .join("token");

    if config_path.exists() {
        let token = fs::read_to_string(&config_path)?.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }

    Err("No Readwise API token found. Set READWISE_TOKEN env var or create ~/.config/readwise/token".into())
}

fn get_base_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join("Captures")
        .join("readwise")
}

fn create_client(token: &str) -> Result<Client, Box<dyn std::error::Error>> {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Token {}", token))?,
    );

    let client = Client::builder()
        .default_headers(headers)
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    Ok(client)
}

// ============================================================================
// Highlights sync (Readwise API v2)
// ============================================================================

fn sync_highlights(
    client: &Client,
    output_dir: &PathBuf,
    last_sync: &Option<String>,
) -> Result<u32, Box<dyn std::error::Error>> {
    let mut total_books = 0;
    let mut cursor: Option<String> = None;

    loop {
        let mut url = reqwest::Url::parse(READWISE_EXPORT_URL)?;

        if let Some(ref c) = cursor {
            url.query_pairs_mut().append_pair("pageCursor", c);
        }

        if let Some(ref since) = last_sync {
            url.query_pairs_mut().append_pair("updatedAfter", since);
        }

        let response: HighlightsExportResponse = client.get(url).send()?.json()?;

        for book in response.results {
            write_book_markdown(&book, output_dir)?;
            total_books += 1;
        }

        cursor = response.next_page_cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(total_books)
}

fn write_book_markdown(book: &Book, output_dir: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let filename = format!(
        "{}-{}.md",
        slugify(&book.category),
        slugify(&book.title.chars().take(50).collect::<String>())
    );
    let path = output_dir.join(&filename);

    let mut file = fs::File::create(&path)?;

    // Frontmatter
    writeln!(file, "---")?;
    writeln!(file, "title: \"{}\"", escape_yaml(&book.title))?;
    if let Some(ref author) = book.author {
        writeln!(file, "author: \"{}\"", escape_yaml(author))?;
    }
    writeln!(file, "category: {}", book.category)?;
    if let Some(ref source) = book.source {
        writeln!(file, "source: {}", source)?;
    }
    if let Some(ref url) = book.source_url {
        writeln!(file, "source_url: \"{}\"", url)?;
    }
    writeln!(file, "highlight_count: {}", book.num_highlights)?;
    writeln!(file, "readwise_id: {}", book.id)?;
    if !book.book_tags.is_empty() {
        let tags: Vec<&str> = book.book_tags.iter().map(|t| t.name.as_str()).collect();
        writeln!(file, "tags: [{}]", tags.join(", "))?;
    }
    writeln!(file, "---")?;
    writeln!(file)?;

    // Title
    writeln!(file, "# {}", book.title)?;
    if let Some(ref author) = book.author {
        writeln!(file, "*by {}*", author)?;
    }
    writeln!(file)?;

    if let Some(ref url) = book.source_url {
        writeln!(file, "Source: <{}>", url)?;
        writeln!(file)?;
    }

    // Highlights
    writeln!(file, "## Highlights")?;
    writeln!(file)?;

    for highlight in &book.highlights {
        if highlight.is_deleted {
            continue;
        }

        writeln!(file, "> {}", highlight.text.replace('\n', "\n> "))?;

        if let Some(ref note) = highlight.note {
            if !note.is_empty() {
                writeln!(file)?;
                writeln!(file, "**Note:** {}", note)?;
            }
        }

        if !highlight.tags.is_empty() {
            let tags: Vec<String> = highlight.tags.iter().map(|t| format!("#{}", t.name)).collect();
            writeln!(file, "\n{}", tags.join(" "))?;
        }

        if let Some(ref date) = highlight.highlighted_at {
            if let Some(short_date) = date.get(..10) {
                writeln!(file, "\n— {}", short_date)?;
            }
        }

        writeln!(file)?;
        writeln!(file, "---")?;
        writeln!(file)?;
    }

    Ok(())
}

// ============================================================================
// Reader sync (Readwise API v3)
// ============================================================================

fn sync_reader(
    client: &Client,
    output_dir: &PathBuf,
    last_sync: &Option<String>,
) -> Result<u32, Box<dyn std::error::Error>> {
    let mut total_docs = 0;
    let mut html_count = 0;
    let mut cursor: Option<String> = None;

    // Create html subdirectory for full snapshots
    let html_dir = output_dir.join("html");
    fs::create_dir_all(&html_dir)?;

    loop {
        let mut url = reqwest::Url::parse(READER_LIST_URL)?;

        // Request full HTML content for data sovereignty
        url.query_pairs_mut().append_pair("withHtmlContent", "true");

        if let Some(ref c) = cursor {
            url.query_pairs_mut().append_pair("pageCursor", c);
        }

        if let Some(ref since) = last_sync {
            url.query_pairs_mut().append_pair("updatedAfter", since);
        }

        let response: ReaderListResponse = client.get(url).send()?.json()?;

        for doc in response.results {
            let has_html = doc.html_content.is_some();
            write_document_markdown(&doc, output_dir, &html_dir)?;
            total_docs += 1;
            if has_html {
                html_count += 1;
            }
        }

        cursor = response.next_page_cursor;
        if cursor.is_none() {
            break;
        }
    }

    println!("    ({} with full HTML snapshots)", html_count);
    Ok(total_docs)
}

fn write_document_markdown(
    doc: &Document,
    output_dir: &PathBuf,
    html_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let title = doc.title.as_deref().unwrap_or("Untitled");
    let date_prefix = doc.created_at.get(..10).unwrap_or("unknown");
    let base_filename = format!(
        "{}-{}",
        date_prefix,
        slugify(&title.chars().take(50).collect::<String>())
    );
    let md_filename = format!("{}.md", base_filename);
    let html_filename = format!("{}.html", base_filename);
    let path = output_dir.join(&md_filename);

    // Save HTML snapshot if available
    let html_saved = if let Some(ref html_content) = doc.html_content {
        let html_path = html_dir.join(&html_filename);
        fs::write(&html_path, html_content)?;
        true
    } else {
        false
    };

    let mut file = fs::File::create(&path)?;

    // Frontmatter
    writeln!(file, "---")?;
    writeln!(file, "title: \"{}\"", escape_yaml(title))?;
    if let Some(ref author) = doc.author {
        writeln!(file, "author: \"{}\"", escape_yaml(author))?;
    }
    writeln!(file, "category: {}", doc.category)?;
    writeln!(file, "location: {}", doc.location)?;
    writeln!(file, "url: \"{}\"", doc.url)?;
    if let Some(ref source_url) = doc.source_url {
        writeln!(file, "source_url: \"{}\"", source_url)?;
    }
    if html_saved {
        writeln!(file, "html_snapshot: \"html/{}\"", html_filename)?;
    }
    if let Some(word_count) = doc.word_count {
        writeln!(file, "word_count: {}", word_count)?;
    }
    writeln!(file, "reading_progress: {:.0}%", doc.reading_progress * 100.0)?;
    writeln!(file, "created_at: {}", doc.created_at)?;
    writeln!(file, "updated_at: {}", doc.updated_at)?;
    writeln!(file, "readwise_id: \"{}\"", doc.id)?;
    if !doc.tags.is_empty() {
        let tags: Vec<&String> = doc.tags.keys().collect();
        writeln!(file, "tags: [{}]", tags.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))?;
    }
    writeln!(file, "---")?;
    writeln!(file)?;

    // Title and metadata
    writeln!(file, "# {}", title)?;
    if let Some(ref author) = doc.author {
        writeln!(file, "*by {}*", author)?;
    }
    writeln!(file)?;

    writeln!(file, "**URL:** <{}>", doc.url)?;
    writeln!(file, "**Status:** {} ({:.0}% read)", doc.location, doc.reading_progress * 100.0)?;
    if html_saved {
        writeln!(file, "**Local snapshot:** [[captures/readwise/reader/html/{}]]", html_filename)?;
    }
    writeln!(file)?;

    // Summary
    if let Some(ref summary) = doc.summary {
        if !summary.is_empty() {
            writeln!(file, "## Summary")?;
            writeln!(file)?;
            writeln!(file, "{}", summary)?;
            writeln!(file)?;
        }
    }

    // Notes
    if let Some(ref notes) = doc.notes {
        if !notes.is_empty() {
            writeln!(file, "## Notes")?;
            writeln!(file)?;
            writeln!(file, "{}", notes)?;
            writeln!(file)?;
        }
    }

    Ok(())
}

// ============================================================================
// Utilities
// ============================================================================

fn escape_yaml(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
}

/// Deserialize an ID that could be either a string or an integer
fn deserialize_id<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct IdVisitor;

    impl<'de> Visitor<'de> for IdVisitor {
        type Value = String;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or integer")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.to_string())
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.to_string())
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(v.to_string())
        }
    }

    deserializer.deserialize_any(IdVisitor)
}

/// Deserialize an optional ID that could be null, a string, or an integer
fn deserialize_optional_id<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};

    struct OptionalIdVisitor;

    impl<'de> Visitor<'de> for OptionalIdVisitor {
        type Value = Option<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("null, a string, or an integer")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(v.to_string()))
        }

        fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(v))
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(v.to_string()))
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Some(v.to_string()))
        }
    }

    deserializer.deserialize_any(OptionalIdVisitor)
}
