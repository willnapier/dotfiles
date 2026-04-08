use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub type Pool = Arc<Mutex<Connection>>;

pub fn init(path: &str) -> Result<Pool> {
    let conn = Connection::open(path)?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS documents (
            id TEXT PRIMARY KEY,
            token TEXT UNIQUE NOT NULL,
            filename TEXT NOT NULL,
            recipient_email TEXT NOT NULL,
            recipient_name TEXT NOT NULL,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            revoked INTEGER NOT NULL DEFAULT 0,
            access_count INTEGER NOT NULL DEFAULT 0,
            max_accesses INTEGER
        );

        CREATE TABLE IF NOT EXISTS access_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            document_id TEXT NOT NULL REFERENCES documents(id),
            accessed_at TEXT NOT NULL,
            ip_address TEXT,
            user_agent TEXT,
            action TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS otp_codes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            document_id TEXT NOT NULL REFERENCES documents(id),
            email TEXT NOT NULL,
            code TEXT NOT NULL,
            created_at TEXT NOT NULL,
            used INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            document_id TEXT NOT NULL REFERENCES documents(id),
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL
        );
        ",
    )?;

    Ok(Arc::new(Mutex::new(conn)))
}

pub struct Document {
    pub id: String,
    pub token: String,
    pub filename: String,
    pub recipient_email: String,
    pub recipient_name: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub revoked: bool,
    pub access_count: i64,
}

pub fn insert_document(
    pool: &Pool,
    filename: &str,
    recipient_email: &str,
    recipient_name: &str,
    expiry_days: u32,
) -> Result<(String, String)> {
    let conn = pool.lock().unwrap();
    let id = Uuid::new_v4().to_string();
    let token = Uuid::new_v4().to_string();
    let now = Utc::now();
    let expires = now + chrono::Duration::days(expiry_days as i64);

    conn.execute(
        "INSERT INTO documents (id, token, filename, recipient_email, recipient_name, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            id,
            token,
            filename,
            recipient_email,
            recipient_name,
            now.to_rfc3339(),
            expires.to_rfc3339(),
        ],
    )?;

    Ok((id, token))
}

pub fn get_document_by_token(pool: &Pool, token: &str) -> Result<Option<Document>> {
    let conn = pool.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, token, filename, recipient_email, recipient_name, created_at, expires_at, revoked, access_count
         FROM documents WHERE token = ?1",
    )?;

    let doc = stmt
        .query_row(rusqlite::params![token], |row| {
            Ok(Document {
                id: row.get(0)?,
                token: row.get(1)?,
                filename: row.get(2)?,
                recipient_email: row.get(3)?,
                recipient_name: row.get(4)?,
                created_at: row
                    .get::<_, String>(5)?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_default(),
                expires_at: row
                    .get::<_, String>(6)?
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_default(),
                revoked: row.get::<_, i64>(7)? != 0,
                access_count: row.get(8)?,
            })
        })
        .ok();

    Ok(doc)
}

pub fn log_access(pool: &Pool, document_id: &str, ip: &str, user_agent: &str, action: &str) -> Result<()> {
    let conn = pool.lock().unwrap();
    conn.execute(
        "INSERT INTO access_log (document_id, accessed_at, ip_address, user_agent, action)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![document_id, Utc::now().to_rfc3339(), ip, user_agent, action],
    )?;
    Ok(())
}

pub fn store_otp(pool: &Pool, document_id: &str, email: &str, code: &str) -> Result<()> {
    let conn = pool.lock().unwrap();
    conn.execute(
        "INSERT INTO otp_codes (document_id, email, code, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![document_id, email, code, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn verify_otp(pool: &Pool, document_id: &str, email: &str, code: &str) -> Result<bool> {
    let conn = pool.lock().unwrap();

    // OTP valid for 10 minutes
    let cutoff = (Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM otp_codes
         WHERE document_id = ?1 AND email = ?2 AND code = ?3 AND used = 0 AND created_at > ?4",
        rusqlite::params![document_id, email, code, cutoff],
        |row| row.get(0),
    )?;

    if count > 0 {
        conn.execute(
            "UPDATE otp_codes SET used = 1
             WHERE document_id = ?1 AND email = ?2 AND code = ?3",
            rusqlite::params![document_id, email, code],
        )?;
        conn.execute(
            "UPDATE documents SET access_count = access_count + 1 WHERE id = ?1",
            rusqlite::params![document_id],
        )?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// List all documents with their current status
pub fn list_documents(pool: &Pool) -> Result<Vec<DocumentSummary>> {
    let conn = pool.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT token, filename, recipient_email, recipient_name, created_at, expires_at, revoked, access_count
         FROM documents ORDER BY created_at DESC",
    )?;

    let docs = stmt
        .query_map([], |row| {
            Ok(DocumentSummary {
                token: row.get(0)?,
                filename: row.get(1)?,
                recipient_email: row.get(2)?,
                recipient_name: row.get(3)?,
                created_at: row.get(4)?,
                expires_at: row.get(5)?,
                revoked: row.get::<_, i64>(6)? != 0,
                access_count: row.get(7)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(docs)
}

#[derive(serde::Serialize)]
pub struct DocumentSummary {
    pub token: String,
    pub filename: String,
    pub recipient_email: String,
    pub recipient_name: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked: bool,
    pub access_count: i64,
}

/// Revoke a document — prevents further access
pub fn revoke_document(pool: &Pool, document_id: &str) -> Result<()> {
    let conn = pool.lock().unwrap();
    conn.execute(
        "UPDATE documents SET revoked = 1 WHERE id = ?1",
        rusqlite::params![document_id],
    )?;
    Ok(())
}

/// Create a session token after successful OTP verification. Valid for 1 hour.
pub fn create_session(pool: &Pool, document_id: &str) -> Result<String> {
    let conn = pool.lock().unwrap();
    let token = Uuid::new_v4().to_string();
    let now = Utc::now();
    let expires = now + chrono::Duration::hours(1);

    conn.execute(
        "INSERT INTO sessions (token, document_id, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![token, document_id, now.to_rfc3339(), expires.to_rfc3339()],
    )?;

    Ok(token)
}

/// Validate a session token for a specific document. Returns true if valid and not expired.
pub fn validate_session(pool: &Pool, session_token: &str, document_id: &str) -> Result<bool> {
    let conn = pool.lock().unwrap();
    let now = Utc::now().to_rfc3339();

    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sessions
         WHERE token = ?1 AND document_id = ?2 AND expires_at > ?3",
        rusqlite::params![session_token, document_id, now],
        |row| row.get(0),
    )?;

    Ok(count > 0)
}
