#!/usr/bin/env nu
# ================================
# Quantified Tracking Parser
# Compatible with QUANTIFIED-TRACKING.md specification
# ================================

# Parse duration strings (30m, 2h, 90min) to minutes
def parse-duration [val: string] {
    let val = $val | str downcase
    
    # Try different patterns
    if ($val | str contains 'h') {
        let hours = ($val | str replace --all --regex '[^0-9.]' '' | into float)
        return ($hours * 60 | into int)
    } else if ($val | str contains 'min') {
        return ($val | str replace --all --regex '[^0-9]' '' | into int)
    } else if ($val | str contains 'm') {
        return ($val | str replace --all --regex '[^0-9]' '' | into int)
    } else {
        return 0
    }
}

# Parse named parameters (key=value pairs)
def parse-named-params [val: string] {
    let params = ($val | split row ' ' | where {|x| $x =~ '\w+=\S+'})
    
    mut result = {}
    for param in $params {
        let parts = ($param | split column '=' | get 0)
        if ($parts | length) >= 2 {
            let key = ($parts | get column1.0)
            let value = ($parts | get column2.0)
            $result = ($result | insert $key $value)
        }
    }
    
    $result
}

# Detect value types (duration, currency, distance, etc)
def detect-value-types [values: list] {
    mut result = {
        method: null
        duration_m: 0
        cost: null
        distance: null
        tags: []
    }
    
    let is_first = true
    for value in $values {
        # First non-duration/cost/distance value is the method
        if $is_first and not ($value =~ '^\d+[mh]|^[£$€¥]\d+|^\d+km') {
            $result.method = $value
            let is_first = false
        } else if ($value =~ '\d+[mh]|min') {
            $result.duration_m = (parse-duration $value)
        } else if ($value =~ '^[£$€¥]\d+') {
            $result.cost = $value
        } else if ($value =~ '^\d+km') {
            $result.distance = $value
        } else if not ($value =~ '=') {
            $result.tags = ($result.tags | append $value)
        }
    }
    
    $result
}

# Core parser for tracking entries with context inheritance and sibling support
export def parse-tracking [
    content: string
    date: string
] {
    # Split by periods (entry delimiter)
    let entries = ($content | str replace --all '\n' ' ' | split row '.')
    
    mut results = []
    mut current_context = null
    
    for entry in $entries {
        if ($entry | str trim) == '' { continue }
        
        # Check if entry contains :: marker
        if not ($entry | str contains '::') { continue }
        
        # Split on :: to find all key-value pairs in this entry
        let parts = ($entry | split row '::')
        
        mut i = 0
        while $i < (($parts | length) - 1) {
            let key_part = ($parts | get $i | str trim)
            let value_part = ($parts | get ($i + 1))
            
            # Extract the key from the end of key_part
            let key_tokens = ($key_part | split row ' ')
            let key = if ($key_tokens | length) > 0 {
                $key_tokens | last
            } else {
                $key_part
            }
            
            # Handle context inheritance
            let full_key = if ($key | str contains '.') {
                # Already has hierarchy
                $key
            } else if $current_context != null and $i > 0 {
                # Inherit context for non-root keys
                $'($current_context).($key)'
            } else {
                # Set as new context if root
                $current_context = $key
                $key
            }
            
            # Extract value up to next key or end
            let value_str = if ($i + 2) < ($parts | length) {
                # There's another key after this, extract up to it
                let next_part = ($value_part | split row ' ')
                let value_tokens = []
                for token in $next_part {
                    if ($token | str ends-with '::') or ($token | str contains '::') {
                        break
                    }
                    let value_tokens = ($value_tokens | append $token)
                }
                $value_tokens | str join ' '
            } else {
                # Last value in entry
                $value_part | str trim
            }
            
            # Parse comma-separated sibling activities
            # e.g., "40m 3500 reservoir, 30m 2700 SpraysHill"
            let siblings = ($value_str | split row ',' | each {|v| $v | str trim})
            
            for sibling in $siblings {
                if ($sibling | is-empty) { continue }
                
                # Parse each sibling value
                let sibling_tokens = ($sibling | split row ' ')
                
                # Detect value types for this sibling
                let typed_values = (detect-value-types $sibling_tokens)
                let named_params = (parse-named-params $sibling)
                
                # Combine everything for this sibling
                let record = {
                    date: $date
                    key: $full_key
                    raw_value: $sibling
                    duration_m: $typed_values.duration_m
                    method: $typed_values.method
                    cost: $typed_values.cost
                    distance: $typed_values.distance
                    tags: ($typed_values.tags | str join ' ')
                } | merge $named_params
                
                $results = ($results | append $record)
            }
            
            $i = $i + 1
        }
    }
    
    $results
}

# Parse a daily note file (extracts date from filename)
export def parse-daily-note [file: path] {
    let filename = ($file | path basename)
    let date = if ($filename =~ '^\d{4}-\d{2}-\d{2}') {
        $filename | str replace '.md' ''
    } else {
        # Try to extract from path segments
        let parts = ($file | path split)
        let date_part = ($parts | where {|p| $p =~ '^\d{4}-\d{2}-\d{2}'} | first)
        $date_part | default 'unknown'
    }
    
    let content = (open --raw $file)
    parse-tracking $content $date
}

# Batch process a folder of journal files
export def parse-tracking-folder [folder: path] {
    ls $"($folder)/**/*.md"
    | where name =~ '\d{4}-\d{2}-\d{2}'
    | each {|f| parse-daily-note $f.name }
    | flatten
}

# Summary by day and key
export def summary-by-day [entries] {
    $entries
    | group-by date
    | transpose date activities
    | each {|day|
        let summary = ($day.activities
            | group-by key
            | transpose key entries
            | each {|g|
                {
                    key: $g.key
                    total_m: ($g.entries.duration_m | math sum)
                    count: ($g.entries | length)
                }
            })
        {
            date: $day.date
            activities: $summary
            total_m: ($summary.total_m | math sum)
        }
    }
}

# Summary by key across all time
export def summary-by-key [entries] {
    $entries
    | group-by key
    | transpose key entries
    | each {|g|
        {
            key: $g.key
            total_m: ($g.entries.duration_m | math sum)
            total_h: (($g.entries.duration_m | math sum) / 60 | math round --precision 1)
            days: ($g.entries.date | uniq | length)
            avg_m_per_day: (($g.entries.duration_m | math sum) / ($g.entries.date | uniq | length) | math round)
        }
    }
    | sort-by total_m --reverse
}

# Find all piano practice (hierarchical)
export def find-piano [entries] {
    $entries
    | where {|e| $e.key | str starts-with 'p' or $e.key | str starts-with 'p.'}
}

# Calculate productive travel time
export def productive-travel [entries] {
    # Group by date and look for travel followed by other activities
    $entries
    | group-by date
    | transpose date entries
    | each {|day|
        let travel = ($day.entries | where {|e| $e.key | str starts-with 't'})
        let travel_time = ($travel.duration_m | math sum)
        
        # This is simplified - real implementation would need sequential parsing
        {
            date: $day.date
            travel_m: $travel_time
            travel_h: ($travel_time / 60 | math round --precision 1)
        }
    }
}

# Test function for sibling parsing
export def test-sibling-parsing [] {
    print "Testing sibling activity parsing..."
    
    # Test case 1: Walking with multiple segments
    let test1 = (parse-tracking "w:: 40m 3500 reservoir, 30m 2700 SpraysHill." "2025-09-05")
    let walk_total = ($test1 | where key == "w" | get duration_m | math sum)
    print $"Walking test: expecting 70m, got ($walk_total)m"
    
    # Test case 2: Reading with subcategories
    let test2 = (parse-tracking "r:: 60m philosophy:: 20m, history:: 30m." "2025-09-05")
    let phil = ($test2 | where key == "r.philosophy" | get duration_m | math sum)
    let hist = ($test2 | where key == "r.history" | get duration_m | math sum)
    print $"Reading test: philosophy=($phil)m, history=($hist)m"
    
    # Show parsed records
    print "\nParsed walking records:"
    print ($test1 | select key raw_value duration_m tags | to md)
    
    print "\nParsed reading records:"
    print ($test2 | select key raw_value duration_m | to md)
}

# ================================
# Example Usage
# ================================

# Parse today's entry
# let today = (date now | format date '%Y-%m-%d')
# let entries = (parse-daily-note $'($env.HOME)/Obsidian.nosync/Forge/NapierianLogs/DayPages/($today).md')

# Parse and summarize a month
# let entries = (parse-tracking-folder ~/Obsidian.nosync/Forge/NapierianLogs/DayPages/)
# summary-by-key $entries

# Manual parse with explicit date
# parse-tracking "t:: uber 45m £30. r:: 60m philosophy:: 20m, history:: 30m." "2025-09-05"

# Test sibling parsing
# test-sibling-parsing