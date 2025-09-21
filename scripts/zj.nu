#!/usr/bin/env nu
# Smart Zellij launcher with screen-aware layout selection - Native Nushell implementation

def main [...args] {
    # Check if first argument is a layout override
    let layout_override = if ($args | length) > 0 {
        let first_arg = ($args | first)
        if $first_arg in ["laptop", "desktop", "desktop-27", "desktop-quarters", "desktop-niri"] {
            $first_arg
        } else {
            null
        }
    } else {
        null
    }

    # Remove layout from args if it was provided as override
    let remaining_args = if $layout_override != null {
        $args | skip 1
    } else {
        $args
    }

    # Known displays:
    # - 32" Dell 6K: 6016 x 3384 (standard desktop - home office)
    # - 27" Apple Thunderbolt: 2560 x 1440 (large laptop/secondary display)
    # - 1920x1080: Standard laptop/small desktop
    # - MacBook displays: Mobile/laptop

    # Get screen resolution using native Nushell (only if no override)
    let resolution = if ($nu.os-info.name == "macos") {
        # macOS - parse system_profiler output
        try {
            let display_info = (^system_profiler SPDisplaysDataType | lines | where $it =~ "Resolution")
            if ($display_info | length) > 0 {
                let res_line = ($display_info | first)
                # Extract numbers from "Resolution: 2560 x 1440" format
                let numbers = ($res_line | parse --regex 'Resolution:\s+(\d+)\s+x\s+(\d+)')
                if ($numbers | length) > 0 {
                    let parsed = ($numbers | first)
                    {
                        width: ($parsed.capture0 | into int),
                        height: ($parsed.capture1 | into int)
                    }
                } else {
                    {width: 2880, height: 1864}  # MacBook fallback
                }
            } else {
                {width: 2880, height: 1864}  # MacBook fallback
            }
        } catch {
            {width: 2880, height: 1864}  # Error fallback
        }
    } else {
        # Linux - Try Wayland (wlr-randr) first, fallback to X11 (xrandr)
        try {
            if (which wlr-randr | is-not-empty) {
                # Wayland compositor (Niri, Sway, etc.)
                let wlr_output = (^wlr-randr | lines | where $it =~ " current ")
                if ($wlr_output | length) > 0 {
                    let current_line = ($wlr_output | first)
                    # Extract resolution like "6016x3384" from wlr-randr output
                    let res_match = ($current_line | parse --regex '(\d+)x(\d+)')
                    if ($res_match | length) > 0 {
                        let parsed = ($res_match | first)
                        {
                            width: ($parsed.capture0 | into int),
                            height: ($parsed.capture1 | into int)
                        }
                    } else {
                        {width: 1920, height: 1080}  # Wayland fallback
                    }
                } else {
                    {width: 1920, height: 1080}  # No current display
                }
            } else if (which xrandr | is-not-empty) {
                # X11 fallback for systems not on Wayland
                let xrandr_output = (^xrandr | lines | where $it =~ "connected")
                if ($xrandr_output | length) > 0 {
                    let conn_line = ($xrandr_output | first)
                    # Extract resolution like "2560x1440" from xrandr output
                    let res_match = ($conn_line | parse --regex '(\d+)x(\d+)')
                    if ($res_match | length) > 0 {
                        let parsed = ($res_match | first)
                        {
                            width: ($parsed.capture0 | into int),
                            height: ($parsed.capture1 | into int)
                        }
                    } else {
                        {width: 1920, height: 1080}  # X11 fallback
                    }
                } else {
                    {width: 1920, height: 1080}  # No connected display
                }
            } else {
                {width: 1920, height: 1080}  # No display tools available
            }
        } catch {
            {width: 1920, height: 1080}  # Error fallback
        }
    }

    # Determine layout - use override if provided, otherwise auto-detect
    let layout = if $layout_override != null {
        print $"🎯 Using override layout: ($layout_override)"
        $layout_override
    } else if $resolution.width >= 6000 {
        print $"🖥️  Detected standard desktop display (6K): ($resolution.width)x($resolution.height)"
        "desktop"
    } else if $resolution.width >= 3440 {
        print $"🖥️  Detected ultrawide display: ($resolution.width)x($resolution.height)"
        "desktop-27"
    } else if $resolution.width >= 2560 {
        print $"💻 Detected large laptop display: ($resolution.width)x($resolution.height)"
        "desktop-27"
    } else if $resolution.width >= 1920 {
        print $"💻 Detected small desktop/laptop display: ($resolution.width)x($resolution.height)"
        "laptop"
    } else {
        print $"📱 Detected mobile display: ($resolution.width)x($resolution.height)"
        "laptop"
    }

    print $"🎯 Selected layout: ($layout)"

    # Clean up any zombie sessions before starting new one
    print "🧹 Cleaning up zombie sessions..."
    let exited_sessions = (
        try {
            ^zellij list-sessions --no-formatting
            | lines
            | where $it =~ "EXITED"
            | each { |line| $line | split row ' ' | get 0 }
        } catch {
            []
        }
    )

    if ($exited_sessions | length) > 0 {
        print $"Found ($exited_sessions | length) zombie sessions to clean up"
        for session in $exited_sessions {
            try {
                ^zellij delete-session $session
                print $"  ✅ Deleted: ($session)"
            } catch {
                print $"  ❌ Failed to delete: ($session)"
            }
        }
    }

    # Generate unique session name
    let base_name = $"zellij-($layout)"
    let session_name = try {
        let existing_sessions = (
            ^zellij list-sessions --no-formatting
            | lines
            | each { |line| $line | split row ' ' | get 0 }
        )

        if ($existing_sessions | where $it == $base_name | length) > 0 {
            # Generate unique name with timestamp
            let timestamp = (date now | format date "%H%M")
            $"($base_name)-($timestamp)"
        } else {
            $base_name
        }
    } catch {
        $base_name
    }

    # Launch Zellij with the appropriate layout
    print $"🚀 Starting Zellij session: ($session_name)"
    let zellij_args = if ($remaining_args | length) > 0 {
        [$"--session" $session_name "--layout" $layout] ++ $remaining_args
    } else {
        [$"--session" $session_name "--layout" $layout]
    }

    print $"Executing: zellij (($zellij_args | str join ' '))"
    let cmd = (["zellij"] | append $zellij_args)
    exec ...$cmd
}