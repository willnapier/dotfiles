#!/usr/bin/env bash

# Source the icon mapping function
source "$CONFIG_DIR/plugins/icon_map_fn.sh"
source "$CONFIG_DIR/colors.sh"

# Function to get app icon for an app name
get_app_icon() {
    local app_name="$1"
    __icon_map "$app_name"
    echo "$icon_result"
}

# Function to get windows for a specific workspace
get_workspace_windows() {
    local workspace_id=$1
    aerospace list-windows --workspace "$workspace_id" --json 2>/dev/null || echo "[]"
}

# Function to update workspace indicators
update_workspaces() {
    # Get current workspace
    current_workspace=$(aerospace list-workspaces --focused 2>/dev/null || echo "")
    
    # Clear all backgrounds first
    sketchybar --set space.1 background.drawing=off \
               --set space.2 background.drawing=off \
               --set space.3 background.drawing=off \
               --set space.4 background.drawing=off
    
    # Set background on current workspace
    if [ -n "$current_workspace" ]; then
        sketchybar --set space.$current_workspace background.drawing=on
    fi
}

# Handle aerospace events
case "$SENDER" in
    "aerospace_workspace_change")
        update_workspaces
        ;;
    "aerospace_windows_change")
        update_workspaces
        ;;
    "forced")
        update_workspaces
        ;;
    *)
        # Initial load or manual trigger
        update_workspaces
        ;;
esac