#!/bin/bash

sketchybar --add item clock right \
           --set clock icon= update_freq=1 script="$PLUGIN_DIR/clock.sh"
