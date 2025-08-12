#!/usr/bin/env bash

# Source the icon mapping function
source "$CONFIG_DIR/plugins/icon_map_fn.sh"

# Define workspace names
WORKSPACE_NAMES=(
  "1" "Focus"
  "2" "Comms"
  "3" "Scratch"
  "4" "Client"
)

# Create workspace items
for i in "${!WORKSPACE_NAMES[@]}"; do
  if (( i % 2 == 0 )); then
    SPACE_ID="${WORKSPACE_NAMES[$i]}"
    SPACE_NAME="${WORKSPACE_NAMES[$i+1]}"
    
    sketchybar --add item space.$SPACE_ID left \
               --set space.$SPACE_ID \
                     label="$SPACE_NAME" \
                     icon.drawing=off \
                     label.font="SF Pro:Semibold:15.0" \
                     label.padding_left=8 \
                     label.padding_right=8 \
                     label.y_offset=0 \
                     background.color=$RED_MUTED \
                     background.corner_radius=6 \
                     background.height=26 \
                     background.drawing=off \
                     script="$PLUGIN_DIR/aerospace.sh" \
                     click_script="aerospace workspace $SPACE_ID" \
               --subscribe space.$SPACE_ID aerospace_workspace_change
    
    # Create associated apps bracket for this space
    sketchybar --add bracket space_apps.$SPACE_ID space.$SPACE_ID \
               --set space_apps.$SPACE_ID \
                     background.drawing=off
  fi
done

# Create event item to handle window changes
sketchybar --add event aerospace_workspace_change \
           --add event aerospace_windows_change

# Initial workspace update
$PLUGIN_DIR/aerospace.sh