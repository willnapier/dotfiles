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
    #[error(
        "corrupt store row {row} (import_id {import_id}): {detail}. \
         Refusing to load — fix or remove that row in the CSV. Running a tag \
         command now would rewrite the whole store from defaulted values and \
         lose the original permanently."
    )]
    CorruptRow {
        row: usize,
        import_id: String,
        detail: String,
    },
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
        for (i, result) in csv_reader.deserialize::<StoredTransaction>().enumerate() {
            let stored = result?;
            let import_id = stored.import_id.clone();
            let tx = Transaction::try_from(stored).map_err(|detail| StoreError::CorruptRow {
                row: i + 2, // 1-based data row, +1 for the header line
                import_id,
                detail,
            })?;
            transactions.push(tx);
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
            .has_headers(false)  // We write headers manually
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
            .has_headers(false)  // We write headers manually
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

impl TryFrom<StoredTransaction> for Transaction {
    type Error = String;

    /// Fallible on purpose. A cell that does not round-trip (bad date, amount,
    /// account, or a non-blank unparseable balance) is a CORRUPT row, not a
    /// `£0.00`/epoch/`Current` default. Silently defaulting used to (a) hide the
    /// money from every total while `--by-category` still claimed to "reconcile
    /// exactly", and (b) get written back over the original text on the next
    /// `tag` rewrite — irreversibly. Erroring here makes `load_all` fail closed,
    /// which also stops the destructive rewrite.
    fn try_from(stored: StoredTransaction) -> Result<Self, String> {
        use std::str::FromStr;
        use chrono::NaiveDate;
        use rust_decimal::Decimal;
        use crate::{Account, TxType};

        let date = NaiveDate::parse_from_str(&stored.date, "%Y-%m-%d")
            .map_err(|_| format!("unparseable date {:?}", stored.date))?;
        let account = Account::from_str(&stored.account)
            .map_err(|e| e.to_string())?;
        let amount = Decimal::from_str(&stored.amount)
            .map_err(|_| format!("unparseable amount {:?}", stored.amount))?;
        // Blank balance is legitimately absent (None); a non-blank cell that
        // fails to parse is corruption, not an absent balance.
        let balance = if stored.balance.trim().is_empty() {
            None
        } else {
            Some(
                Decimal::from_str(&stored.balance)
                    .map_err(|_| format!("unparseable balance {:?}", stored.balance))?,
            )
        };

        Ok(Self {
            date,
            account,
            tx_type: TxType::from_code(&stored.tx_type),
            amount,
            description: stored.description,
            raw_description: stored.raw_description,
            balance,
            tags: if stored.tags.is_empty() {
                Vec::new()
            } else {
                stored.tags.split('|').map(String::from).collect()
            },
            import_id: stored.import_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Account, TxType};
    use chrono::NaiveDate;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn stored(date: &str, amount: &str, balance: &str) -> StoredTransaction {
        StoredTransaction {
            date: date.into(),
            account: "current".into(),
            tx_type: "direct_debit".into(),
            amount: amount.into(),
            description: "x".into(),
            raw_description: "x".into(),
            balance: balance.into(),
            tags: String::new(),
            import_id: "abc123".into(),
        }
    }

    #[test]
    fn valid_row_round_trips() {
        let t = Transaction::try_from(stored("2025-06-01", "-12.50", "100.00")).unwrap();
        assert_eq!(t.amount, Decimal::from_str("-12.50").unwrap());
        assert_eq!(t.balance, Some(Decimal::from_str("100.00").unwrap()));
        assert_eq!(t.date, NaiveDate::from_ymd_opt(2025, 6, 1).unwrap());
    }

    #[test]
    fn empty_balance_is_none_not_error() {
        let t = Transaction::try_from(stored("2025-06-01", "-12.50", "")).unwrap();
        assert_eq!(t.balance, None);
    }

    #[test]
    fn corrupt_amount_is_rejected_not_zeroed() {
        // Previously -> Decimal::ZERO: money silently vanished from every total.
        let err = Transaction::try_from(stored("2025-06-01", "-£12.50", "")).unwrap_err();
        assert!(err.contains("amount"), "got: {err}");
    }

    #[test]
    fn corrupt_date_is_rejected_not_epoch() {
        // Previously -> 1970-01-01: row dropped from date filters, span blown open.
        let err = Transaction::try_from(stored("31/06/2025", "-12.50", "")).unwrap_err();
        assert!(err.contains("date"), "got: {err}");
    }

    #[test]
    fn corrupt_nonblank_balance_is_rejected() {
        let err = Transaction::try_from(stored("2025-06-01", "-12.50", "1,000")).unwrap_err();
        assert!(err.contains("balance"), "got: {err}");
    }

    #[test]
    fn unknown_account_is_rejected_not_defaulted() {
        // Previously -> Account::Current: mis-attributed the row's source.
        let mut s = stored("2025-06-01", "-12.50", "");
        s.account = "viza".into();
        assert!(Transaction::try_from(s).is_err());
    }

    #[test]
    fn tx_type_survives_store_round_trip() {
        // M14: written as as_str ("direct_debit"); previously read back as
        // Unknown via from_code, then erased on the next rewrite.
        let tx = Transaction {
            date: NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            account: Account::Current,
            tx_type: TxType::DirectDebit,
            amount: Decimal::from_str("-12.50").unwrap(),
            description: "x".into(),
            raw_description: "x".into(),
            balance: None,
            tags: vec![],
            import_id: "id".into(),
        };
        let stored: StoredTransaction = tx.clone().into();
        let back = Transaction::try_from(stored).unwrap();
        assert_eq!(back.tx_type, TxType::DirectDebit);
    }
}
