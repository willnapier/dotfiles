# fd-budget

CLI tool for analyzing First Direct bank transactions. Rust handles import/tagging, Nushell handles analysis.

## Quick Reference

### Rust CLI (`fd-budget`)

```bash
# Import transactions
fd-budget import ~/Downloads/MIDATA_*.csv --account current
fd-budget import ~/Downloads/MIDATA_*.csv --account visa

# Tag rules (pattern-based)
fd-budget tag add "TESCO" groceries food      # Add rule
fd-budget tag remove "TESCO" food             # Remove tag from rule
fd-budget tag test "TESCO STORES LONDON"      # Test what matches

# Tag rules (amount-based)
fd-budget tag add-amount 10000 income salary  # Exact amount match

# Manage tags
fd-budget tag list                            # Show all rules
fd-budget tag reapply                         # Re-tag all transactions

# Interactive tagging
fd-budget categorize -l 20                    # Process 20 untagged

# Stats
fd-budget stats                               # Overview
fd-budget untagged -l 10                      # Show untagged

# PayPal enrichment
fd-budget enrich-paypal ~/Downloads/paypal.csv --dry-run  # Preview matches
fd-budget enrich-paypal ~/Downloads/paypal.csv            # Enrich descriptions
```

### Nushell Commands (`budget-*`)

```nu
# Load data
tx                                            # All transactions as table

# Analysis
budget-by-tag                                 # Spending by tag
budget-by-tag --month 2025-10                 # Single month
budget-merchants 15                           # Top 15 merchants
budget-monthly                                # Monthly totals
budget-subscriptions                          # Recurring charges

# Search & filter
budget-search "AMAZON"                        # Find transactions
budget-untagged                               # Show untagged
budget-tag "food"                             # All with tag
budget-tag "food" --monthly                   # Monthly breakdown

# Sync (Mac <-> Linux)
budget-sync-to-nimbini                        # Push data to Linux
budget-sync-from-nimbini                      # Pull data from Linux
```

## Architecture

```
First Direct midata CSV
        │
        ▼
   fd-budget import (Rust)
   - parse, normalize, dedupe
   - apply tag rules
        │
        ▼
~/.config/fd-budget/transactions.csv
        │
        ▼
   Nushell queries (analysis)
   - sort, filter, group, aggregate
```

**Division of labor**:
- Rust: Import, normalize, dedupe, tag (one-way data pipeline)
- Nushell: All analysis and ad-hoc queries

## Data Files

```
~/.config/fd-budget/
├── transactions.csv    # All transactions (date,account,tx_type,amount,description,raw_description,balance,tags,import_id)
└── rules.toml          # Tag rules (pattern + amount based)
```

## Tag Rules

**Pattern rules** - case-insensitive substring match on description:
```toml
[[rules]]
pattern = "TESCO"
tags = ["groceries", "food"]
```

**Amount rules** - exact decimal match (for masked transfers):
```toml
[[amount_rules]]
amount = "10000"
tags = ["income", "salary"]
```

## Common Workflows

**Weekly review**:
```bash
fd-budget categorize -l 30      # Tag new transactions
budget-by-tag --month $(date +%Y-%m)
```

**Import new data**:
```bash
fd-budget import ~/Downloads/MIDATA_*.csv --account current
fd-budget stats
```

**Find spending patterns**:
```nu
budget-search "AMAZ" | group-by description | each { length } | sort-by -r
budget-tag "subscription" | group-by description | each { get amount | math sum }
```

**Enrich PayPal transactions**:
1. Export from PayPal: Activity → Download → CSV format
2. Preview: `fd-budget enrich-paypal ~/Downloads/Download.csv --dry-run`
3. Apply: `fd-budget enrich-paypal ~/Downloads/Download.csv`
4. Tag: `fd-budget categorize` (new merchant names now matchable)
