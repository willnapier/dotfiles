//! End-to-end PayPal merchant-recovery test over SYNTHETIC fixtures.
//!
//! Proves the full pipeline using only the public crate API, from
//! FD-style bank CSVs + a PayPal CSV export through to the recovered merchant
//! surfacing in the query layer and the rule engine:
//!
//!   FD CSV  ──parse──▶ transactions
//!   PayPal CSV ──parse/store──▶ paypal.csv sidecar
//!   recover() ──▶ paypal_matches.jsonl sidecar
//!   join_with_recovery() ──▶ recovered merchant as counterparty
//!   apply_rules_with_recovery() ──▶ a Streamflix rule tags the bare PAYPAL row
//!
//! Covers all three real cases (direct-GBP, two-leg, FX-chain), checks no
//! double-count is introduced, and asserts >= 95% of £-value is recovered.
//!
//! Synthetic data ONLY — fictional merchants, round amounts.

use fd_budget::import::parse_midata_current_4col;
use fd_budget::paypal::{
    parse_paypal_csv, recover, write_recoveries, PayPalStore, RecoverOptions, RecoveryIndex,
};
use fd_budget::query::{aggregate_for_test, join_with_recovery, load_matches, Source};
use fd_budget::tags::{apply_rules_with_recovery, TagRules};
use fd_budget::Account;
use rust_decimal::Decimal;
use std::str::FromStr;

const BOM: &str = "\u{feff}";

/// FD new 4-column current-account export: Date, Description, Amount, Balance.
/// Four bare `PAYPAL PAYMENT` debits (one per case) plus a non-PayPal control.
fn fd_csv() -> &'static str {
    "Date,Description,Amount,Balance\n\
     05/03/2026,PAYPAL PAYMENT,-12.99,1000.00\n\
     10/04/2026,PAYPAL PAYMENT,-45.00,955.00\n\
     10/05/2026,PAYPAL PAYMENT,-272.01,683.00\n\
     12/05/2026,PAYPAL PAYMENT,-117.23,565.77\n\
     13/05/2026,TESCO STORES,-30.00,535.77\n"
}

/// PayPal activity export (UTF-8-with-BOM, 15 columns) covering:
///  - direct GBP:  -12.99 Streamflix
///  - two-leg:     +45.00 deposit  &  -45.00 "Acme Shop"
///  - FX chain 1:  +272.01 deposit, -272.01 conversion, -299.40 EUR "Acme Foreign GmbH"
///  - FX chain 2:  +117.23 deposit, -117.23 conversion, -149.99 USD "Foreign Media Co"
fn paypal_csv() -> String {
    format!(
        "{BOM}Date,Time,Time zone,Name,Type,Status,Currency,Amount,Fees,Total,Exchange Rate,Receipt ID,Balance,Transaction ID,Item Title\n\
         05/03/2026,10:00:00,GMT,Streamflix,Express Checkout Payment,Completed,GBP,-12.99,0.00,-12.99,,,0.00,PP-DIRECT,Monthly plan\n\
         10/04/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,45.00,0.00,45.00,,,45.00,PP-DEP-2,\n\
         10/04/2026,09:01:00,GMT,Acme Shop,General Payment,Completed,GBP,-45.00,0.00,-45.00,,,0.00,PP-PAY-2,Widget\n\
         10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,272.01,0.00,272.01,,,272.01,PP-DEP-3,\n\
         10/05/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-272.01,0.00,-272.01,,,0.00,PP-CONV-3,\n\
         10/05/2026,09:02:00,GMT,Acme Foreign GmbH,Express Checkout Payment,Completed,EUR,-299.40,0.00,-299.40,1.1009,,0.00,PP-FX-3,Gadget\n\
         12/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,117.23,0.00,117.23,,,117.23,PP-DEP-4,\n\
         12/05/2026,09:01:00,GMT,,General Currency Conversion,Completed,GBP,-117.23,0.00,-117.23,,,0.00,PP-CONV-4,\n\
         12/05/2026,09:02:00,GMT,Foreign Media Co,Express Checkout Payment,Completed,USD,-149.99,0.00,-149.99,1.2794,,0.00,PP-FX-4,Stream\n"
    )
}

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "fd-budget-e2e-{tag}-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn e2e_recovers_all_three_cases_and_surfaces_them() {
    let dir = tmpdir("full");

    // 1. Parse the FD bank export.
    let txs = parse_midata_current_4col(fd_csv().as_bytes(), Account::Current).unwrap();
    assert_eq!(txs.len(), 5);

    // 2. Import the PayPal export into the sidecar (idempotent).
    let pp_path = dir.join("paypal.csv");
    let pp_store = PayPalStore::new(&pp_path);
    let parsed = parse_paypal_csv(paypal_csv().as_bytes()).unwrap();
    assert_eq!(parsed.len(), 9);
    let existing = pp_store.load_transaction_ids().unwrap();
    let fresh = fd_budget::paypal::deduplicate(parsed, &existing);
    assert_eq!(pp_store.append(&fresh).unwrap(), 9);

    // Re-import the same file: nothing added (idempotent by Transaction ID).
    let parsed2 = parse_paypal_csv(paypal_csv().as_bytes()).unwrap();
    let existing2 = pp_store.load_transaction_ids().unwrap();
    let fresh2 = fd_budget::paypal::deduplicate(parsed2, &existing2);
    assert_eq!(pp_store.append(&fresh2).unwrap(), 0);

    // 3. Recover merchants.
    let paypal_rows = pp_store.load_all().unwrap();
    let (recoveries, summary) = recover(&txs, &paypal_rows, RecoverOptions::default());

    // All four bare PAYPAL rows recovered; TESCO untouched.
    assert_eq!(summary.bare_paypal_rows, 4);
    assert_eq!(summary.recovered, 4);
    assert_eq!(summary.direct_gbp, 1);
    assert_eq!(summary.two_leg, 1);
    assert_eq!(summary.fx_chain, 2);

    // >= 95% of £-value recovered (here: 100%).
    assert!(
        summary.pct_value_recovered() >= 95.0,
        "only {:.1}% of £-value recovered",
        summary.pct_value_recovered()
    );

    // 4. Write + reload the recovery sidecar.
    let matches_path = dir.join("paypal_matches.jsonl");
    write_recoveries(&matches_path, &recoveries).unwrap();
    let idx = RecoveryIndex::load(&matches_path).unwrap();
    assert_eq!(idx.len(), 4);

    // Map bank import_ids to recovered merchant by amount (each PAYPAL debit is
    // a distinct amount in this fixture).
    let by_amount = |amt: &str| -> Option<String> {
        let target = Decimal::from_str(amt).unwrap();
        txs.iter()
            .find(|t| t.amount == target && t.raw_description.contains("PAYPAL"))
            .and_then(|t| idx.recovered_merchant_for(&t.import_id))
            .map(String::from)
    };
    assert_eq!(by_amount("-12.99").as_deref(), Some("Streamflix")); // direct
    assert_eq!(by_amount("-45.00").as_deref(), Some("Acme Shop")); // two-leg
    assert_eq!(by_amount("-272.01").as_deref(), Some("Acme Foreign GmbH")); // FX EUR
    assert_eq!(by_amount("-117.23").as_deref(), Some("Foreign Media Co")); // FX USD

    // 5. Surface in the query layer: recovered merchant becomes counterparty.
    let emails: Vec<fd_budget::enrich::EmailRow> = vec![];
    let no_matches = load_matches(dir.join("nonexistent.jsonl")).unwrap_or_default();
    let joined = join_with_recovery(&txs, &emails, &no_matches, &idx);

    let streamflix = joined
        .iter()
        .find(|r| r.tx.amount == Decimal::from_str("-12.99").unwrap())
        .unwrap();
    assert_eq!(streamflix.counterparty_name(), "Streamflix");
    assert_eq!(streamflix.source(), Source::PayPalRecovered);

    let fx = joined
        .iter()
        .find(|r| r.tx.amount == Decimal::from_str("-272.01").unwrap())
        .unwrap();
    assert_eq!(fx.counterparty_name(), "Acme Foreign GmbH");

    // TESCO control still falls back to bank-only.
    let tesco = joined
        .iter()
        .find(|r| r.tx.raw_description.contains("TESCO"))
        .unwrap();
    assert_eq!(tesco.source(), Source::BankOnly);

    // 6. NO double-count: aggregate over all PAYPAL+TESCO debits equals the raw
    // sum of the five debits exactly once (recovery relabels, never duplicates).
    let (agg, _internal, reconciled, bank_only) = aggregate_for_test(&joined);
    let agg_total: Decimal = agg.iter().map(|(_, total, _)| *total).sum();
    let raw_total: Decimal = txs.iter().map(|t| t.amount.abs()).sum();
    assert_eq!(agg_total, raw_total, "aggregate introduced a double-count");
    // Four recovered merchants -> reconciled; TESCO -> bank-only.
    assert_eq!(
        reconciled,
        Decimal::from_str("12.99").unwrap()
            + Decimal::from_str("45.00").unwrap()
            + Decimal::from_str("272.01").unwrap()
            + Decimal::from_str("117.23").unwrap()
    );
    assert_eq!(bank_only, Decimal::from_str("30.00").unwrap());

    // 7. Rule engine: a "Streamflix" rule tags the bare PAYPAL row via recovery
    // (it cannot match the bank text "PAYPAL PAYMENT").
    let mut rules = TagRules::default();
    rules.add_rule(
        "Streamflix",
        vec!["subscription".to_string()],
        None,
        None,
        None,
        None,
        None,
    );
    let mut tagged = txs.clone();
    apply_rules_with_recovery(&mut tagged, &rules, &idx);
    let streamflix_tx = tagged
        .iter()
        .find(|t| t.amount == Decimal::from_str("-12.99").unwrap())
        .unwrap();
    assert!(
        streamflix_tx.tags.contains(&"subscription".to_string()),
        "Streamflix rule did not tag the recovered PAYPAL row"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn e2e_disambiguates_recurring_amount_by_date() {
    // Two -12.99 PAYPAL rows in a month bind to distinct PayPal payments by
    // nearest date — no leg double-claimed.
    let dir = tmpdir("recurring");
    let fd = "Date,Description,Amount,Balance\n\
              03/06/2026,PAYPAL PAYMENT,-12.99,1000.00\n\
              17/06/2026,PAYPAL PAYMENT,-12.99,987.00\n";
    let txs = parse_midata_current_4col(fd.as_bytes(), Account::Current).unwrap();

    let pp = format!(
        "{BOM}Date,Time,Time zone,Name,Type,Status,Currency,Amount,Fees,Total,Exchange Rate,Receipt ID,Balance,Transaction ID,Item Title\n\
         03/06/2026,10:00:00,GMT,Streamflix,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-A,\n\
         17/06/2026,10:00:00,GMT,Newsly,General Payment,Completed,GBP,-12.99,0,-12.99,,,0,PP-B,\n"
    );
    let paypal_rows = parse_paypal_csv(pp.as_bytes()).unwrap();
    let (recs, summary) = recover(&txs, &paypal_rows, RecoverOptions::default());
    assert_eq!(summary.recovered, 2);

    let matches_path = dir.join("paypal_matches.jsonl");
    write_recoveries(&matches_path, &recs).unwrap();
    let idx = RecoveryIndex::load(&matches_path).unwrap();

    let early = txs
        .iter()
        .find(|t| t.date.to_string() == "2026-06-03")
        .unwrap();
    let late = txs
        .iter()
        .find(|t| t.date.to_string() == "2026-06-17")
        .unwrap();
    assert_eq!(
        idx.recovered_merchant_for(&early.import_id),
        Some("Streamflix")
    );
    assert_eq!(idx.recovered_merchant_for(&late.import_id), Some("Newsly"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// REGRESSION (HIGH — FX foreign-leg cross-link), end to end through the
/// sidecar store. Two same-day FX chains, distinct amounts/merchants. Proves
/// the `Exchange Rate` column survives parse → paypal.csv store → reload, and
/// the amount link binds each foreign leg to its OWN chain (no swap). With the
/// old day-granular nearest-by-id pick, bank-1 would have grabbed "RIGHT-B".
#[test]
fn e2e_two_same_day_fx_chains_do_not_swap_through_store() {
    let dir = tmpdir("fx-swap");
    let fd = "Date,Description,Amount,Balance\n\
              10/05/2026,PAYPAL PAYMENT,-100.00,900.00\n\
              10/05/2026,PAYPAL PAYMENT,-200.00,700.00\n";
    let txs = parse_midata_current_4col(fd.as_bytes(), Account::Current).unwrap();

    // EUR leg: 110 * 0.909091 ≈ 100.00; USD leg: 260 * 0.769231 ≈ 200.00.
    // Foreign-leg ids ordered so the OLD code swapped (PP-FX-1 = RIGHT-B sorts
    // before PP-FX-2 = RIGHT-A).
    let pp = format!(
        "{BOM}Date,Time,Time zone,Name,Type,Status,Currency,Amount,Fees,Total,Exchange Rate,Receipt ID,Balance,Transaction ID,Item Title\n\
         10/05/2026,09:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,100.00,0.00,100.00,,,100.00,PP-DEP-A,\n\
         10/05/2026,09:00:01,GMT,,General Currency Conversion,Completed,GBP,-100.00,0.00,-100.00,,,0.00,PP-CONV-A,\n\
         10/05/2026,09:00:02,GMT,RIGHT-A,Express Checkout Payment,Completed,EUR,-110.00,0.00,-110.00,0.909091,,0.00,PP-FX-2,Widget\n\
         10/05/2026,11:00:00,GMT,,Bank Deposit to PP Account ,Completed,GBP,200.00,0.00,200.00,,,200.00,PP-DEP-B,\n\
         10/05/2026,11:00:01,GMT,,General Currency Conversion,Completed,GBP,-200.00,0.00,-200.00,,,0.00,PP-CONV-B,\n\
         10/05/2026,11:00:02,GMT,RIGHT-B,Express Checkout Payment,Completed,USD,-260.00,0.00,-260.00,0.769231,,0.00,PP-FX-1,Gadget\n"
    );

    // Import into the sidecar store (round-trips the new columns).
    let pp_path = dir.join("paypal.csv");
    let pp_store = PayPalStore::new(&pp_path);
    let parsed = parse_paypal_csv(pp.as_bytes()).unwrap();
    let existing = pp_store.load_transaction_ids().unwrap();
    let fresh = fd_budget::paypal::deduplicate(parsed, &existing);
    assert_eq!(pp_store.append(&fresh).unwrap(), 6);

    // Recover from the RELOADED store (proves exchange_rate survived the store).
    let paypal_rows = pp_store.load_all().unwrap();
    let (recs, summary) = recover(&txs, &paypal_rows, RecoverOptions::default());
    assert_eq!(summary.fx_chain, 2);

    let matches_path = dir.join("paypal_matches.jsonl");
    write_recoveries(&matches_path, &recs).unwrap();
    let idx = RecoveryIndex::load(&matches_path).unwrap();

    let by_amount = |amt: &str| -> Option<String> {
        let target = Decimal::from_str(amt).unwrap();
        txs.iter()
            .find(|t| t.amount == target)
            .and_then(|t| idx.recovered_merchant_for(&t.import_id))
            .map(String::from)
    };
    assert_eq!(by_amount("-100.00").as_deref(), Some("RIGHT-A"));
    assert_eq!(by_amount("-200.00").as_deref(), Some("RIGHT-B"));

    let _ = std::fs::remove_dir_all(&dir);
}
