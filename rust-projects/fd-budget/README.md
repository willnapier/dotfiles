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
fd-budget stats                               # Overview (tag/account summary)
fd-budget untagged -l 10                      # Show untagged

# Email-evidence enrichment (joins bank rows to mailcurator bills.jsonl)
fd-budget enrich                              # Refresh ~/.config/fd-budget/matches.jsonl
fd-budget enrich --dry-run                    # Print summary only
fd-budget enrich --amount-tolerance 0.05      # Loosen amount match

# Stage 2 queries (read transactions.csv + bills.jsonl + matches.jsonl)
fd-budget stats --by-counterparty             # Spend per counterparty (top 30)
fd-budget stats --by-counterparty --year 2025
fd-budget stats --by-counterparty --month 2025-10 --limit 50

fd-budget tx vendor vodafone --with-evidence  # Drill into one vendor's rows
fd-budget tx vendor octopus --year 2025
fd-budget tx unmatched --over 50              # Big debits with no email evidence
fd-budget tx unmatched --over 100 --year 2025
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

**Reconcile against email evidence**:
1. Run mailcurator to refresh `~/.local/share/mailcurator/bills.jsonl`
2. `fd-budget enrich` — produces `~/.config/fd-budget/matches.jsonl` joining bank rows to email evidence by amount + date + vendor
3. Inspect: `fd-budget stats --by-counterparty` (top spend), `fd-budget tx unmatched --over 50` (large rows missing evidence), `fd-budget tx vendor <name> --with-evidence` (per-vendor drill)

Confidence tiers in `matches.jsonl`:
- `high` — substring vendor match, exact date
- `medium` — token overlap or date offset within window
- `ambiguous` — multiple equally-valid candidates
- `internal-transfer` — VISA payoff (excluded from spend aggregates)
- `none` — no email evidence found
