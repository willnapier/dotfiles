use crate::Transaction;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter};
use std::path::Path;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("CSV error: {0}")]
    Csv(#[from] csv::Error),
}

/// CSV header for our canonical format
const CSV_HEADERS: &[&str] = &[
    "date",
    "account",
    "tx_type",
    "amount",
    "description",
    "raw_description",
    "balance",
    "tags",
    "import_id",
];

pub struct CsvStore {
    path: std::path::PathBuf,
}

impl CsvStore {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Load all existing import IDs for deduplication
    pub fn load_import_ids(&self) -> Result<HashSet<String>, StoreError> {
        if !self.path.exists() {
            return Ok(HashSet::new());
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut csv_reader = csv::Reader::from_reader(reader);

        let mut ids = HashSet::new();
        for result in csv_reader.records() {
            let record = result?;
            // import_id is the last column (index 8)
            if let Some(id) = record.get(8) {
                ids.insert(id.to_string());
            }
        }

        Ok(ids)
    }

    /// Load all transactions from the store
    pub fn load_all(&self) -> Result<Vec<Transaction>, StoreError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut csv_reader = csv::Reader::from_reader(reader);

        let mut transactions = Vec::new();
        for result in csv_reader.deserialize() {
            let tx: StoredTransaction = result?;
            transactions.push(tx.into());
        }

        Ok(transactions)
    }

    /// Append new transactions to the store
    pub fn append(&self, transactions: &[Transaction]) -> Result<usize, StoreError> {
        if transactions.is_empty() {
            return Ok(0);
        }

        let file_exists = self.path.exists();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let writer = BufWriter::new(file);
        let mut csv_writer = csv::WriterBuilder::new()
            .has_headers(false) // We write headers manually
            .from_writer(writer);

        // Write header if new file
        if !file_exists {
            csv_writer.write_record(CSV_HEADERS)?;
        }

        for tx in transactions {
            let stored: StoredTransaction = tx.clone().into();
            csv_writer.serialize(&stored)?;
        }

        csv_writer.flush()?;
        Ok(transactions.len())
    }

    /// Rewrite the entire store (used when updating tags)
    pub fn rewrite(&self, transactions: &[Transaction]) -> Result<(), StoreError> {
        let file = File::create(&self.path)?;
        let writer = BufWriter::new(file);
        let mut csv_writer = csv::WriterBuilder::new()
            .has_headers(false) // We write headers manually
            .from_writer(writer);

        csv_writer.write_record(CSV_HEADERS)?;

        for tx in transactions {
            let stored: StoredTransaction = tx.clone().into();
            csv_writer.serialize(&stored)?;
        }

        csv_writer.flush()?;
        Ok(())
    }
}

/// Serializable transaction format for CSV
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct StoredTransaction {
    date: String,
    account: String,
    tx_type: String,
    amount: String,
    description: String,
    raw_description: String,
    balance: String,
    tags: String,
    import_id: String,
}

impl From<Transaction> for StoredTransaction {
    fn from(tx: Transaction) -> Self {
        Self {
            date: tx.date.to_string(),
            account: tx.account.to_string(),
            tx_type: tx.tx_type.as_str().to_string(),
            amount: tx.amount.to_string(),
            description: tx.description,
            raw_description: tx.raw_description,
            balance: tx.balance.map(|b| b.to_string()).unwrap_or_default(),
            tags: tx.tags.join("|"),
            import_id: tx.import_id,
        }
    }
}

impl From<StoredTransaction> for Transaction {
    fn from(stored: StoredTransaction) -> Self {
        use crate::{Account, TxType};
        use chrono::NaiveDate;
        use rust_decimal::Decimal;
        use std::str::FromStr;

        Self {
            date: NaiveDate::parse_from_str(&stored.date, "%Y-%m-%d")
                .unwrap_or_else(|_| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap()),
            account: stored.account.parse().unwrap_or(Account::Current),
            tx_type: TxType::from_code(&stored.tx_type),
            amount: Decimal::from_str(&stored.amount).unwrap_or_default(),
            description: stored.description,
            raw_description: stored.raw_description,
            balance: Decimal::from_str(&stored.balance).ok(),
            tags: if stored.tags.is_empty() {
                Vec::new()
            } else {
                stored.tags.split('|').map(String::from).collect()
            },
            import_id: stored.import_id,
        }
    }
}
