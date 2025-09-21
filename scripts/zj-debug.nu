#!/usr/bin/env nu
# Diagnostic version of zj to find path errors

print "=== ZJ DEBUG START ==="
print $"HOME: ($env.HOME)"
print $"PATH: ($env.PATH)"

# Test 1: Check if wlr-randr works
print "\n=== Testing wlr-randr ==="
if (which wlr-randr | is-not-empty) {
    print "wlr-randr found"
    try {
        let output = (^wlr-randr)
        print "wlr-randr output OK"
    } catch {|e|
        print $"wlr-randr error: ($e)"
    }
} else {
    print "wlr-randr NOT found"
}

# Test 2: Check Nushell config loading
print "\n=== Testing Nushell config ==="
print $"Config dir: ($nu.default-config-dir)"
print $"Config file: ($nu.config-path)"
print $"Env file: ($nu.env-path)"

# Test 3: Try to load a minimal Nushell session
print "\n=== Testing minimal nu session ==="
try {
    ^nu -c "print 'Minimal nu works'"
} catch {|e|
    print $"Nu session error: ($e)"
}

# Test 4: Check for any OBSIDIAN environment variables
print "\n=== Checking environment ==="
$env | transpose key value | where key =~ "OBSIDIAN" | each {|row|
    print $"Found env var: ($row.key) = ($row.value)"
}

# Test 5: Check what zellij layout would be selected
print "\n=== Layout selection test ==="
try {
    let wlr_output = (^wlr-randr | lines | where $it =~ " current ")
    if ($wlr_output | length) > 0 {
        let current_line = ($wlr_output | first)
        print $"Current display: ($current_line)"
        let res_match = ($current_line | parse --regex '(\d+)x(\d+)')
        if ($res_match | length) > 0 {
            let parsed = ($res_match | first)
            let width = ($parsed.capture0 | into int)
            print $"Width detected: ($width)"
            if $width >= 6000 {
                print "Would select: desktop-niri layout"
            }
        }
    }
} catch {|e|
    print $"Resolution detection error: ($e)"
}

print "\n=== ZJ DEBUG END ==="
print "\nNow run 'zj' and tell me if the error appears BEFORE or AFTER the Zellij UI starts"