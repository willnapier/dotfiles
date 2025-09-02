#!/bin/bash
# Enhanced directory preview for skim

DIR="${1:-$FILE_PATH}"

if [ -d "$DIR" ]; then
    echo "📁 Directory: $(basename "$DIR")"
    echo "═══════════════════════════════════════"
    echo ""
    
    # Count files and dirs with better formatting
    FILE_COUNT=$(find "$DIR" -maxdepth 1 -type f 2>/dev/null | wc -l | tr -d ' ')
    DIR_COUNT=$(find "$DIR" -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
    DIR_COUNT=$((DIR_COUNT - 1))  # Exclude the directory itself
    
    # Calculate total size
    TOTAL_SIZE=$(du -sh "$DIR" 2>/dev/null | cut -f1)
    
    # Directory stats table
    printf "┌──────────────┬─────────────┬──────────────┐\n"
    printf "│ %-12s │ %-11s │ %-12s │\n" "Files" "Folders" "Total Size"
    printf "├──────────────┼─────────────┼──────────────┤\n"
    printf "│ %-12s │ %-11s │ %-12s │\n" "$FILE_COUNT" "$DIR_COUNT" "$TOTAL_SIZE"
    printf "└──────────────┴─────────────┴──────────────┘\n"
    echo ""
    echo "📂 Contents (first 15 items):"
    echo "─────────────────────────────"
    ls -la "$DIR" 2>/dev/null | head -16 | tail -15
else
    echo "❌ Not a directory: $DIR"
fi