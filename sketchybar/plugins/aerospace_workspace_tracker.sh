#!/usr/bin/env bash

# Simple workspace tracker script called directly by AeroSpace
CONFIG_DIR="/Users/williamnapier/.config/sketchybar"
PLUGIN_DIR="$CONFIG_DIR/plugins"

# Source colors and icon mapping
source "$CONFIG_DIR/colors.sh"
source "$CONFIG_DIR/plugins/icon_map_fn.sh"

# Function to get app icon
get_app_icon() {
    local app_name="$1"
    __icon_map "$app_name"
    echo "$icon_result"
}

# Get current workspace
current_workspace=$(aerospace list-workspaces --focused 2>/dev/null || echo "")

# Debug log
echo "$(date): Workspace changed to $current_workspace" >> /tmp/aerospace_tracker.log

# Update all workspaces with icons and backgrounds
for workspace in 1 2 3 4; do
    # Get windows in this workspace
    windows_json=$(aerospace list-windows --workspace "$workspace" --json 2>/dev/null || echo "[]")
    window_count=$(echo "$windows_json" | jq '. | length' 2>/dev/null || echo "0")
    
    # Get app icons
    icons=""
    if [ "$window_count" -gt 0 ]; then
        app_names=$(echo "$windows_json" | jq -r '.[]."app-name"' | sort -u)
        while IFS= read -r app_name; do
            if [ -n "$app_name" ]; then
                icon=$(get_app_icon "$app_name")
                if [ -n "$icons" ]; then
                    icons="$icons $icon"
                else
                    icons="$icon"
                fi
            fi
        done <<< "$app_names"
    fi
    
    # Set background and icons
    if [ "$workspace" = "$current_workspace" ]; then
        # Focused workspace
        /opt/homebrew/bin/sketchybar --set space.$workspace \
            background.drawing=on \
            icon="$icons" \
            icon.drawing=on \
            icon.font="Hack Nerd Font:Regular:17.0" \
            icon.color=$BASE03 \
            drawing=on
    else
        # Unfocused workspace
        /opt/homebrew/bin/sketchybar --set space.$workspace \
            background.drawing=off \
            icon="$icons" \
            icon.drawing=on \
            icon.font="Hack Nerd Font:Regular:17.0" \
            icon.color=$BASE1 \
            drawing=on
    fi
done

echo "$(date): Updated all workspaces with icons" >> /tmp/aerospace_tracker.log