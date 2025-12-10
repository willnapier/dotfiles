# fd-budget Nushell commands
# Load transactions from fd-budget store

def tx [] {
    open ($env.HOME | path join ".config/fd-budget/transactions.csv")
}

# Spending by tag
def budget-by-tag [--month: string] {
    let data = (tx)
    let filtered = if ($month | is-not-empty) {
        $data | where ($it.date | str starts-with $month)
    } else {
        $data
    }
    $filtered
    | where amount < 0
    | group-by tags
    | transpose tag rows
    | each { {tag: $in.tag, total: ($in.rows.amount | math sum | math round -p 2), count: ($in.rows | length)} }
    | sort-by total
}

# Spending by merchant
def budget-merchants [n: int = 20] {
    tx
    | where amount < 0
    | group-by description
    | transpose merchant rows
    | each { {merchant: $in.merchant, total: ($in.rows.amount | math sum | math round -p 2), count: ($in.rows | length)} }
    | sort-by total
    | first $n
}

# Monthly spending trend
def budget-monthly [] {
    tx
    | where amount < 0
    | each { $in | insert month ($in.date | into datetime | format date "%Y-%m") }
    | group-by month
    | transpose month rows
    | each { {month: $in.month, spent: ($in.rows.amount | math sum | math round -p 2)} }
    | sort-by month
}

# Subscriptions summary
def budget-subscriptions [] {
    tx
    | where ($it.tags =~ "subscription")
    | group-by description
    | transpose sub rows
    | each { {
        subscription: $in.sub,
        monthly: (($in.rows.amount | math sum) / 12 | math round -p 2),
        annual: ($in.rows.amount | math sum | math round -p 2)
    } }
    | sort-by annual
}

# Untagged merchants (to help with categorization)
def budget-untagged [n: int = 20] {
    tx
    | where ($it.tags | str length) == 0 and amount < 0
    | group-by description
    | transpose merchant rows
    | each { {merchant: $in.merchant, total: ($in.rows.amount | math sum | math round -p 2), count: ($in.rows | length)} }
    | sort-by total
    | first $n
}

# Search transactions by description
def budget-search [pattern: string] {
    tx | where ($it.raw_description =~ $pattern) | select date account amount raw_description tags
}

# Spending for a specific tag
def budget-tag [tag: string, --monthly] {
    let data = tx | where ($it.tags =~ $tag) and amount < 0
    if $monthly {
        $data
        | each { $in | insert month ($in.date | into datetime | format date "%Y-%m") }
        | group-by month
        | transpose month rows
        | each { {month: $in.month, spent: ($in.rows.amount | math sum | math round -p 2)} }
        | sort-by month
    } else {
        $data | select date amount description tags
    }
}
