#!/bin/bash

# Solarized Dark Color Palette
# Base colors
export BASE03=0xff002b36  # darkest background
export BASE02=0xff073642  # dark background
export BASE01=0xff586e75  # darkest gray
export BASE00=0xff657b83  # dark gray
export BASE0=0xff839496   # light gray
export BASE1=0xff93a1a1   # lightest gray
export BASE2=0xffeee8d5   # light background
export BASE3=0xfffdf6e3   # lightest background

# Accent colors
export YELLOW=0xffb58900
export ORANGE=0xffcb4b16
export RED=0xffdc322f
export RED_MUTED=0x99dc322f  # 60% opacity for deeper claret-like appearance
export MAGENTA=0xffd33682
export VIOLET=0xff6c71c4
export BLUE=0xff268bd2
export CYAN=0xff2aa198
export GREEN=0xff859900

# Semantic color mappings for SketchyBar
export BAR_COLOR=$BASE03        # Main bar background
export ITEM_BG_COLOR=$BASE02    # Item backgrounds
export ACCENT_COLOR=$CYAN       # Active/focused items
export LABEL_COLOR=$BASE2       # Regular text (using lighter shade)
export WHITE=$BASE3             # Highlighted text
export ICON_COLOR=$BASE1        # Icons (using lighter shade)
export HIGHLIGHT_COLOR=$BLUE    # Alternative highlight
export WARNING_COLOR=$YELLOW    # Warnings
export ERROR_COLOR=$RED         # Errors
export SUCCESS_COLOR=$GREEN     # Success states

# Extra bright colors for better visibility
export BRIGHT_WHITE=0xffffffff  # Pure white for maximum contrast
export BRIGHT_TEXT=$BASE2       # Bright text using Solarized light background