#!/bin/bash
# Cross-platform system theme detection
# Returns: "dark" or "light"

if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS: Check AppleInterfaceStyle
    if defaults read -g AppleInterfaceStyle 2>/dev/null | grep -q "Dark"; then
        echo "dark"
    else
        echo "light"
    fi
elif [[ "$OSTYPE" == "linux-gnu"* ]]; then
    # Linux: Check various sources

    # Try GNOME settings
    if command -v gsettings &> /dev/null; then
        color_scheme=$(gsettings get org.gnome.desktop.interface color-scheme 2>/dev/null)
        if echo "$color_scheme" | grep -q "dark"; then
            echo "dark"
            exit 0
        elif echo "$color_scheme" | grep -q "default"; then
            echo "light"  # default usually means light theme
            exit 0
        fi
    fi

    # Try KDE settings
    if [[ -f "$HOME/.config/kdeglobals" ]]; then
        if grep -q "ColorScheme=.*Dark" "$HOME/.config/kdeglobals" 2>/dev/null; then
            echo "dark"
            exit 0
        fi
    fi

    # Check environment variable (can be set in shell config)
    if [[ "$THEME_PREFERENCE" == "dark" ]]; then
        echo "dark"
        exit 0
    fi

    # Check time of day as fallback (dark after 6pm, before 7am)
    hour=$(date +%H)
    if [[ $hour -ge 18 ]] || [[ $hour -lt 7 ]]; then
        echo "dark"
    else
        echo "light"
    fi
else
    # Default to dark for unknown systems
    echo "dark"
fi