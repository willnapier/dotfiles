//! Search query execution — run queries against the Tantivy index.

use anyhow::{Context, Result};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{Index, SnippetGenerator};

use super::config::SearchConfig;
use super::index::build_schema;

/// A single search result with client metadata and a highlighted snippet.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub client_id: String,
    pub name: String,
    pub score: f32,
    pub snippet: String,
}

/// Full-text search across all indexed clients.
///
/// Searches across name, notes, correspondence, and diagnosis fields.
/// Returns results ordered by relevance score.
pub fn search(config: &SearchConfig, query_str: &str, limit: usize) -> Result<Vec<SearchResult>> {
    if !config.index_path.exists() {
        anyhow::bail!(
            "Search index not found at {}. Run with --reindex to build it.",
            config.index_path.display()
        );
    }

    let schema = build_schema();
    let index = Index::open_in_dir(&config.index_path)
        .context("Failed to open search index")?;

    let reader = index
        .reader()
        .context("Failed to create index reader")?;

    let searcher = reader.searcher();

    // Fields to search across
    let name_field = schema.get_field("name").unwrap();
    let notes_field = schema.get_field("notes_content").unwrap();
    let corr_field = schema.get_field("correspondence_content").unwrap();
    let diagnosis_field = schema.get_field("diagnosis").unwrap();

    let query_parser = QueryParser::for_index(
        &index,
        vec![name_field, notes_field, corr_field, diagnosis_field],
    );

    let query = query_parser
        .parse_query(query_str)
        .with_context(|| format!("Failed to parse query: {}", query_str))?;

    let top_docs = searcher
        .search(&query, &TopDocs::with_limit(limit))
        .context("Search failed")?;

    // Build snippet generator for the notes field (most likely to have matches)
    let snippet_generator = SnippetGenerator::create(&searcher, &query, notes_field)
        .context("Failed to create snippet generator")?;

    let client_id_field = schema.get_field("client_id").unwrap();

    let mut results = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in top_docs {
        let doc = searcher
            .doc::<tantivy::TantivyDocument>(doc_address)
            .context("Failed to retrieve document")?;

        let client_id = doc
            .get_first(client_id_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let name = doc
            .get_first(name_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Try to get a snippet from notes first
        let snippet = snippet_generator.snippet_from_doc(&doc);
        let snippet_text = snippet.to_html();
        let snippet_text = if snippet_text.trim().is_empty() {
            // Fall back to trying other fields
            try_snippet_from_field(&searcher, &query, corr_field, &doc)
                .or_else(|| try_snippet_from_field(&searcher, &query, diagnosis_field, &doc))
                .or_else(|| try_snippet_from_field(&searcher, &query, name_field, &doc))
                .unwrap_or_default()
        } else {
            snippet_text
        };

        results.push(SearchResult {
            client_id,
            name,
            score,
            snippet: snippet_text,
        });
    }

    Ok(results)
}

/// Search within a specific client's indexed data.
pub fn search_within_client(
    config: &SearchConfig,
    client_id: &str,
    query_str: &str,
) -> Result<Vec<SearchResult>> {
    if !config.index_path.exists() {
        anyhow::bail!(
            "Search index not found at {}. Run with --reindex to build it.",
            config.index_path.display()
        );
    }

    let schema = build_schema();
    let index = Index::open_in_dir(&config.index_path)
        .context("Failed to open search index")?;

    let reader = index
        .reader()
        .context("Failed to create index reader")?;

    let searcher = reader.searcher();

    let client_id_field = schema.get_field("client_id").unwrap();
    let name_field = schema.get_field("name").unwrap();
    let notes_field = schema.get_field("notes_content").unwrap();
    let corr_field = schema.get_field("correspondence_content").unwrap();
    let diagnosis_field = schema.get_field("diagnosis").unwrap();

    // Build a combined query: client_id filter AND the text query
    let query_parser = QueryParser::for_index(
        &index,
        vec![name_field, notes_field, corr_field, diagnosis_field],
    );

    // Combine: filter by client_id AND match the text query
    let combined_query = format!("client_id:\"{}\" AND ({})", client_id, query_str);
    let query = query_parser
        .parse_query(&combined_query)
        .with_context(|| format!("Failed to parse query: {}", combined_query))?;

    let top_docs = searcher
        .search(&query, &TopDocs::with_limit(100))
        .context("Search failed")?;

    let snippet_generator = SnippetGenerator::create(&searcher, &query, notes_field)
        .context("Failed to create snippet generator")?;

    let mut results = Vec::new();
    for (score, doc_address) in top_docs {
        let doc = searcher
            .doc::<tantivy::TantivyDocument>(doc_address)
            .context("Failed to retrieve document")?;

        let doc_client_id = doc
            .get_first(client_id_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Double-check client_id matches (belt and braces)
        if doc_client_id != client_id {
            continue;
        }

        let name = doc
            .get_first(name_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let snippet = snippet_generator.snippet_from_doc(&doc);
        let snippet_text = snippet.to_html();
        let snippet_text = if snippet_text.trim().is_empty() {
            try_snippet_from_field(&searcher, &query, corr_field, &doc)
                .or_else(|| try_snippet_from_field(&searcher, &query, diagnosis_field, &doc))
                .unwrap_or_default()
        } else {
            snippet_text
        };

        results.push(SearchResult {
            client_id: doc_client_id,
            name,
            score,
            snippet: snippet_text,
        });
    }

    Ok(results)
}

/// Try to generate a snippet from a specific field. Returns None if empty.
fn try_snippet_from_field(
    searcher: &tantivy::Searcher,
    query: &dyn tantivy::query::Query,
    field: tantivy::schema::Field,
    doc: &tantivy::TantivyDocument,
) -> Option<String> {
    let generator = SnippetGenerator::create(searcher, query, field).ok()?;
    let snippet = generator.snippet_from_doc(doc);
    let html = snippet.to_html();
    if html.trim().is_empty() {
        None
    } else {
        Some(html)
    }
}

/// Search restricted to a specific field.
pub fn search_field(
    config: &SearchConfig,
    query_str: &str,
    field_name: &str,
    limit: usize,
) -> Result<Vec<SearchResult>> {
    if !config.index_path.exists() {
        anyhow::bail!(
            "Search index not found at {}. Run with --reindex to build it.",
            config.index_path.display()
        );
    }

    let schema = build_schema();

    // Map user-facing field names to schema fields
    let field_key = match field_name {
        "notes" => "notes_content",
        "correspondence" => "correspondence_content",
        "identity" | "name" => "name",
        "diagnosis" => "diagnosis",
        other => other,
    };

    let target_field = schema
        .get_field(field_key)
        .map_err(|_| anyhow::anyhow!("Unknown field: {}", field_name))?;

    let index = Index::open_in_dir(&config.index_path)
        .context("Failed to open search index")?;

    let reader = index.reader().context("Failed to create index reader")?;
    let searcher = reader.searcher();

    let query_parser = QueryParser::for_index(&index, vec![target_field]);
    let query = query_parser
        .parse_query(query_str)
        .with_context(|| format!("Failed to parse query: {}", query_str))?;

    let top_docs = searcher
        .search(&query, &TopDocs::with_limit(limit))
        .context("Search failed")?;

    let snippet_generator = SnippetGenerator::create(&searcher, &query, target_field)
        .context("Failed to create snippet generator")?;

    let client_id_field = schema.get_field("client_id").unwrap();
    let name_field = schema.get_field("name").unwrap();

    let mut results = Vec::with_capacity(top_docs.len());
    for (score, doc_address) in top_docs {
        let doc = searcher
            .doc::<tantivy::TantivyDocument>(doc_address)
            .context("Failed to retrieve document")?;

        let client_id = doc
            .get_first(client_id_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let name = doc
            .get_first(name_field)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let snippet = snippet_generator.snippet_from_doc(&doc);

        results.push(SearchResult {
            client_id,
            name,
            score,
            snippet: snippet.to_html(),
        });
    }

    Ok(results)
}
