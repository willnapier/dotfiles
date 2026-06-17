//! PayPal merchant recovery.
//!
//! First Direct's 4-column current-account export strips the PayPal merchant —
//! every PayPal purchase posts as a bare `PAYPAL PAYMENT  -£X`. This module
//! reads PayPal's own CSV export (which HAS the merchant) into a typed sidecar
//! ([`store`]) and joins it back to the bank rows ([`recover`]) to recover the
//! merchant REPRODUCIBLY — no AI in the loop. Output is a sidecar
//! (`paypal.csv` + `paypal_matches.jsonl`); `transactions.csv` is never
//! rewritten by import/recovery.

pub mod recover;
pub mod store;

pub use recover::{
    is_bare_paypal_payment, load_recoveries, recover, write_recoveries, Leg, RecoverOptions,
    RecoverSummary, Recovery, RecoveryConfidence, RecoveryIndex, RecoveryRow,
};
pub use store::{deduplicate, parse_paypal_csv, PayPalError, PayPalStore, PayPalTxn};
