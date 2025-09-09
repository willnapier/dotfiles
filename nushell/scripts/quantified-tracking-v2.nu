#!/usr/bin/env nu
# ================================
# Quantified Tracking Parser V2
# Properly handles sibling activities and context inheritance
# ================================

# Parse duration strings to minutes
def parse-duration [val: string] {
    let val = $val | str downcase | str trim
    
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

# Main parsing function
export def parse-tracking [
    content: string
    date: string
] {
    mut results = []
    
    # Split by periods (entry terminators)
    let entries = ($content | str replace --all '\n' ' ' | split row '.')
    
    for entry in $entries {
        if ($entry | str trim | is-empty) { continue }
        if not ($entry | str contains '::') { continue }
        
        # Find all :: markers and their positions
        let trimmed = ($entry | str trim)
        
        # Parse format: "key:: value1, value2. key2:: value3."
        # Or with context: "r:: 60m philosophy:: 20m, history:: 30m."
        
        # Check if this is a simple entry (one ::) or complex (multiple ::)
        let marker_count = ($trimmed | split row '::' | length) - 1
        
        if $marker_count == 1 {
            # Simple entry like "w:: 40m reservoir, 30m SpraysHill"
            # Or hierarchical like "w.sh:: 30m 2700"
            let parts = ($trimmed | split row '::')
            let key = ($parts | get 0 | str trim)
            let value_str = ($parts | get 1 | str trim)
            
            # Handle comma-separated siblings
            let siblings = ($value_str | split row ',' | each {|s| $s | str trim})
            
            for sibling in $siblings {
                if ($sibling | is-empty) { continue }
                
                # Parse this sibling's values
                let tokens = ($sibling | split row ' ')
                mut duration = 0
                mut tags = []
                
                mut steps = null
                for token in $tokens {
                    if ($token =~ '\d+[mh]') {
                        $duration = (parse-duration $token)
                    } else if ($token =~ '^\d{3,5}$') {
                        # 3-5 digits = step count
                        $steps = ($token | into int)
                    } else if not ($token | is-empty) {
                        $tags = ($tags | append $token)
                    }
                }
                
                $results = ($results | append {
                    date: $date
                    key: $key
                    raw_value: $sibling
                    duration_m: $duration
                    steps: $steps
                    tags: ($tags | str join ' ')
                })
            }
        } else if $marker_count > 1 {
            # Complex entry with context inheritance
            # "r:: 60m philosophy:: 20m, history:: 30m"
            
            # First, get the root context
            let parts = ($trimmed | split row '::')
            let root_key = ($parts | get 0 | str trim | split row ' ' | last)
            let root_value_part = ($parts | get 1)
            
            # Check if root has its own duration (before any sub-keys)
            let root_tokens = ($root_value_part | split row ' ')
            mut root_duration = 0
            mut found_subkey = false
            
            for token in $root_tokens {
                if ($token | str ends-with '::') or ($token | str contains '::') {
                    $found_subkey = true
                    break
                } else if ($token =~ '\d+[mh]') {
                    $root_duration = (parse-duration $token)
                }
            }
            
            # If root has duration, add it
            if $root_duration > 0 {
                $results = ($results | append {
                    date: $date
                    key: $root_key
                    raw_value: $"($root_duration)m"
                    duration_m: $root_duration
                    tags: ''
                })
            }
            
            # Now parse the sub-entries with context inheritance
            # Find philosophy:: and history:: parts
            let remaining = ($parts | skip 1 | str join '::')
            
            # Split on words ending with :: to find sub-keys
            let sub_entries = ($remaining | str replace --all --regex '(\w+)::' '|$1::' | split row '|' | skip 1)
            
            for sub_entry in $sub_entries {
                if ($sub_entry | str trim | is-empty) { continue }
                
                let sub_parts = ($sub_entry | split row '::' | each {|p| $p | str trim})
                let sub_key = ($sub_parts | get 0)
                let sub_value = ($sub_parts | get 1 | default '')
                
                # Handle comma-separated values within this sub-key
                let sub_siblings = ($sub_value | split row ',' | each {|s| $s | str trim} | take while {|s| not ($s | str contains '::')})
                
                for sibling in $sub_siblings {
                    if ($sibling | is-empty) { continue }
                    
                    let tokens = ($sibling | split row ' ')
                    mut duration = 0
                    mut steps = null
                    mut tags = []
                    
                    for token in $tokens {
                        if ($token =~ '\d+[mh]') {
                            $duration = (parse-duration $token)
                        } else if ($token =~ '^\d{3,5}$') {
                            # 3-5 digits = step count
                            $steps = ($token | into int)
                        } else if not ($token | str contains '::') and not ($token | is-empty) {
                            $tags = ($tags | append $token)
                        }
                    }
                    
                    # Create the full key with context
                    let full_key = $"($root_key).($sub_key)"
                    
                    $results = ($results | append {
                        date: $date
                        key: $full_key
                        raw_value: $sibling
                        duration_m: $duration
                        steps: $steps
                        tags: ($tags | str join ' ')
                    })
                }
            }
        }
    }
    
    $results
}

# Test function
export def test-parsing [] {
    print "Testing V2 parser..."
    print "==================="
    
    # Test 1: Walking with step counts (separate entries)  
    print "\nTest 1: Walking with step counts"
    let test1 = (parse-tracking "w.sh:: 30m 2700. w.res:: 40m 3500." "2025-09-05")
    print ($test1 | select key raw_value duration_m steps tags | to md)
    let total_time = ($test1 | get duration_m | math sum)
    let total_steps = ($test1 | where steps != null | get steps | math sum)
    print $"Total walking: ($total_time)m, ($total_steps) steps"
    
    # Test 2: Context inheritance
    print "\nTest 2: Reading with context inheritance"
    let test2 = (parse-tracking "r:: 60m philosophy:: 20m, history:: 30m." "2025-09-05")
    print ($test2 | select key raw_value duration_m | to md)
    
    # Test 3: Mixed example
    print "\nTest 3: Travel with concurrent activities"
    let test3 = (parse-tracking "t:: uber 45m £30. cv:: 20m Anne philosophy, cv:: 5m Jenny." "2025-09-05")
    print ($test3 | select key raw_value duration_m tags | to md)
}

# Usage examples
# parse-tracking "w:: 40m 3500 reservoir, 30m 2700 SpraysHill." "2025-09-05"
# parse-tracking "r:: 60m philosophy:: 20m, history:: 30m." "2025-09-05"