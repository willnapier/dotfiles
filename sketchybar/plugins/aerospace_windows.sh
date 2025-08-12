#!/usr/bin/env bash

# This script monitors window changes and triggers workspace updates
# It's called by AeroSpace or other events when windows are created, destroyed, or moved

# Simply trigger the main aerospace plugin to update everything
sketchybar --trigger aerospace_windows_change

# Also call the aerospace plugin directly for immediate update
$CONFIG_DIR/plugins/aerospace.sh