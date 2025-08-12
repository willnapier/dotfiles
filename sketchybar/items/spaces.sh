#!/bin/bash

# Add the aerospace workspace change event
sketchybar --add event aerospace_workspace_change

# Create workspace indicators for your 4 configured workspaces  
WORKSPACE_ICONS=("1" "2" "3" "4")
for i in "${!WORKSPACE_ICONS[@]}"
do
  sid="$(($i+1))"
  sketchybar --add item space."$sid" left \
             --set space."$sid" icon="${WORKSPACE_ICONS[i]}" \
                                icon.padding_left=7 \
                                icon.padding_right=7 \
                                label.font="sketchybar-app-font:Regular:16.0" \
                                label.padding_right=20 \
                                label.y_offset=-1 \
                                background.corner_radius=5 \
                                background.height=25 \
                                background.drawing=off \
                                script="$PLUGIN_DIR/aerospace.sh $sid" \
                                click_script="aerospace workspace $sid" \
             --subscribe space."$sid" aerospace_workspace_change
done

sketchybar --add item space_separator left \
           --set space_separator icon="􀆊" \
                                 icon.color=$ACCENT_COLOR \
                                 icon.padding_left=4 \
                                 label.drawing=off \
                                 background.drawing=off \
                                 script="$PLUGIN_DIR/aerospace_windows.sh" \
           --subscribe space_separator aerospace_workspace_change
