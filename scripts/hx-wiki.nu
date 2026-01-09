#!/usr/bin/env nu
# hx-wiki - Wiki link handler for Helix (Nushell Re-nu version)
# Extracts wiki links from current line and opens them intelligently
# Supports cross-directory links with ~/path syntax
#
# Usage: Called by Space+w in Helix (see helix/config.toml)
# Features:
# - Prioritizes image embeds (![[...]]) over text links ([[...]])
# - Auto-search across ~/Forge, ~/Admin, and ~/Assistants directories
# - Explicit path support: [[~/Admin/Budget]] or [[~/Archives/File]]
# - Smart file type handling: text in Helix, media in system viewer
# - Reception inbox for daily note workflows
# - Daily note template with metrics and navigation

def main [] {
    # Read input from Helix pipe
    let line = (cat | str trim)

    # Output the original line back to Helix (pipe-to expects output)
    print $line

    # Debug logging
    $"CALLED: ($line)\n" | save --append /tmp/hx-wiki-debug.log

    # Clear old temp file
    rm -f /tmp/helix-current-link.md
    let target_file = "/tmp/helix-current-link.md"

    # Extract wiki links from line (supports [[link]], ![[image]], ?[[unresolved]])
    # Prioritize image embeds (![[...]]) over regular links ([[...]])
    let all_links = try {
        $line | rg -o '[!?]?\[\[[^\]]+\]\]' | lines
    } catch {
        []
    }

    if ($all_links | is-empty) {
        return
    }

    # Check for image embeds first (start with !)
    let image_links = ($all_links | where {|link| $link | str starts-with "!"})
    let first_link = if ($image_links | is-empty) {
        $all_links | first
    } else {
        $image_links | first
    }

    if ($first_link | is-empty) {
        return
    }

    # Extract link content from [[link]] (strip !, ?, or both)
    let link = ($first_link | str replace -r '[!?]*\[\[(.*)\]\]' '$1')

    if ($link | is-empty) {
        return
    }

    # Clean link (remove anchors and fragments)
    let clean_link = ($link | str replace -r '[#|].*' '')

    # Define paths
    let vault = $"($env.HOME)/Forge"
    let admin_dir = $"($env.HOME)/Admin"
    let assistants_dir = $"($env.HOME)/Assistants"
    let daily_dir = $"($vault)/NapierianLogs/DayPages"

    # NEW: Check if link starts with ~/ or / (explicit path)
    if ($clean_link | str starts-with "~/") {
        let expanded_path = ($clean_link | str replace "~/" $"($env.HOME)/")

        # Try exact path
        if ($expanded_path | path exists) {
            handle_existing_file $expanded_path $target_file
            return
        }

        # Try with .md extension
        let md_path = $"($expanded_path).md"
        if ($md_path | path exists) {
            handle_existing_file $md_path $target_file
            return
        }

        # Path doesn't exist - create it
        create_new_file $md_path $clean_link $target_file
        return
    }

    if ($clean_link | str starts-with "/") {
        # Absolute path
        if ($clean_link | path exists) {
            handle_existing_file $clean_link $target_file
            return
        }

        let md_path = $"($clean_link).md"
        if ($md_path | path exists) {
            handle_existing_file $md_path $target_file
            return
        }

        create_new_file $md_path $clean_link $target_file
        return
    }

    # Search for existing file across Forge and Admin
    # Strip .md if present to avoid double-extension search (^file.md.md$)
    let search_name = if ($clean_link | str ends-with ".md") {
        $clean_link
    } else {
        $"($clean_link).md"
    }
    let existing_file = try {
        fd -t f $"^($search_name)$" $vault $admin_dir $assistants_dir | lines | first
    } catch {
        ""
    }

    if not ($existing_file | is-empty) {
        # Found exact match
        ln -sf $existing_file $target_file
        return
    }

    # Check if this is a media file and search linked_media directory
    let media_extensions = ["pdf", "PDF", "jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "mp4", "mov", "avi", "mp3", "wav", "m4a", "JPG", "JPEG", "PNG", "GIF"]
    let extension = ($clean_link | path parse | get extension)

    if ($extension in $media_extensions) {
        # Handle links that already include linked_media/ prefix
        let media_file = if ($clean_link | str starts-with "linked_media/") {
            $"($vault)/($clean_link)"
        } else {
            $"($vault)/linked_media/($clean_link)"
        }

        if ($media_file | path exists) {
            # Media file found - open with system viewer
            open_file $media_file
            return
        }
    }

    # Check for journal date pattern
    if ($clean_link | str contains "Journal") and ($clean_link =~ '\d{4}-\d{2}-\d{2}') {
        let date_part = ($clean_link | parse -r '\d{4}-\d{2}-\d{2}' | get capture0.0)
        let journal_file = try {
            fd -t f $"($date_part).*\\.md$" $vault $admin_dir $assistants_dir | lines | first
        } catch {
            ""
        }

        if not ($journal_file | is-empty) {
            ln -sf $journal_file $target_file
            return
        }
    }

    # Determine where to create new file based on context
    let current_file = try {
        if ("/tmp/helix-current-link.md" | path type) == "symlink" {
            ls -l /tmp/helix-current-link.md | get target.0
        } else {
            ""
        }
    } catch {
        ""
    }

    # Build target path
    let file = if ($current_file | str contains "/DayPages/") {
        # We're in a DayPages file
        if ($clean_link =~ '^\d{4}-\d{2}-\d{2}$') {
            # Date format - create in DayPages
            $"($daily_dir)/($clean_link).md"
        } else {
            # Regular note - create in Reception inbox
            let reception_dir = $"($vault)/Reception"
            mkdir $reception_dir
            if not ($clean_link | str ends-with ".md") {
                $"($reception_dir)/($clean_link).md"
            } else {
                $"($reception_dir)/($clean_link)"
            }
        }
    } else if ($clean_link =~ '^\d{4}-\d{2}-\d{2}$') {
        # Daily note format
        $"($daily_dir)/($clean_link).md"
    } else {
        # Regular note - create in Reception inbox (not vault root)
        let reception_dir = $"($vault)/Reception"
        mkdir $reception_dir
        if ($clean_link | str contains ".") and not ($clean_link | str ends-with ".md") {
            # Has non-md extension - use as-is
            $"($reception_dir)/($clean_link)"
        } else if not ($clean_link | str ends-with ".md") {
            $"($reception_dir)/($clean_link).md"
        } else {
            $"($reception_dir)/($clean_link)"
        }
    }

    # Handle existing or create new file
    if ($file | path exists) {
        handle_existing_file $file $target_file
    } else if ($clean_link =~ '^\d{4}-\d{2}-\d{2}$') {
        # Create daily note with template
        create_daily_note $file $clean_link $target_file $vault
    } else {
        # Create regular note or handle media
        create_new_file $file $clean_link $target_file
    }
}

# Handle existing file based on type
def handle_existing_file [file: string, target_file: string] {
    let extension = ($file | path parse | get extension)

    match $extension {
        "md" | "txt" | "text" => {
            # Text files - create symlink for Helix
            ln -sf $file $target_file
        }
        "pdf" | "PDF" => {
            # Open in system viewer
            open_file $file
        }
        "png" | "jpg" | "jpeg" | "gif" | "PNG" | "JPG" | "JPEG" | "GIF" => {
            # Open images in system viewer
            open_file $file
        }
        _ => {
            # Try system default
            open_file $file
        }
    }
}

# Cross-platform file opener (backgrounded to avoid blocking Helix)
def open_file [file: string] {
    let os = (sys host | get name)
    if $os == "Darwin" {
        ^open $file
    } else {
        # Background xdg-open to prevent blocking when called from Helix pipe
        ^bash -c $"xdg-open '($file)' </dev/null >/dev/null 2>&1 &"
    }
}

# Create new file with appropriate handling
def create_new_file [file: string, clean_link: string, target_file: string] {
    let extension = ($file | path parse | get extension)

    # Check if it's a media file that doesn't exist
    let media_extensions = ["jpg", "jpeg", "png", "gif", "bmp", "webp", "svg", "pdf", "mp4", "mov", "avi", "mp3", "wav", "m4a"]

    if ($extension | str downcase) in $media_extensions {
        # Media file doesn't exist - create error message
        let error_content = $"# File Not Found

The linked media file does not exist:

**Path**: ($file)
**Link**: [[($clean_link)]]

This could mean:
- The file was moved or deleted
- The filename is misspelled in the wiki link
- The media hasn't been imported yet

## Search for similar files

```bash
# Search for files with similar names
fd -i \"($file | path basename)\" ~/Forge/linked_media/
```
"
        $error_content | save -f $target_file
    } else {
        # Create regular note
        let note_dir = ($file | path dirname)
        mkdir $note_dir

        let current_date = (date now | format date "%Y-%m-%d")
        let current_time = (date now | format date "%H:%M")

        let content = $"---
tags:
-
date created: ($current_date) ($current_time)
date modified: ($current_date) ($current_time)
---
# ($clean_link)



## Backlinks

"
        $content | save -f $file
        ln -sf $file $target_file
    }
}

# Create daily note with full template
def create_daily_note [file: string, clean_link: string, target_file: string, vault: string] {
    let daily_dir = $"($vault)/NapierianLogs/DayPages"
    mkdir $daily_dir

    let current_time = (date now | format date "%H:%M")

    # Format readable date
    let readable_date = try {
        date now | format date "%A, %B %d, %Y"
    } catch {
        $clean_link
    }

    # Calculate previous and next dates
    let date_obj = ($clean_link | into datetime)
    let prev_date = ($date_obj | sub 1day | format date "%Y-%m-%d")
    let next_date = ($date_obj | add 1day | format date "%Y-%m-%d")

    let content = $"---
tags:
- journal
date created: ($clean_link) ($current_time)
date modified: ($clean_link) ($current_time)

# Daily Metrics
sleep_hours:
healthy_eating: false
exercise: false
piano_practice: false
meditation: false
reading: false
social_connection: false

# exercise_details: \"[[Exercise Log ($clean_link)]]\"
# piano_details: \"[[Piano Practice Log ($clean_link)]]\"
# health_details: \"[[Health Metrics ($clean_link)]]\"
---
# ($readable_date)

## Tasks
- [ ]

## Notes

## Journal

## Links
- Previous: [[($prev_date)]]
- Next: [[($next_date)]]

## Backlinks

"

    $content | save -f $file
    ln -sf $file $target_file
}
