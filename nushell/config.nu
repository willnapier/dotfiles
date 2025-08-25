# Nushell Config File
# version = "0.106.1"

# Load vendor autoload scripts conditionally
# Check for and source Starship prompt if available
if ($"($env.HOME)/.cache/nushell/starship-init.nu" | path exists) {
    source ~/.cache/nushell/starship-init.nu
}

# Source zoxide for smart directory jumping if available
if ($"($env.HOME)/.cache/nushell/zoxide-init.nu" | path exists) {
    source ~/.cache/nushell/zoxide-init.nu
}

# Helix-Yazi integration now built into the y function

# Solarized color schemes
let solarized_light = {
    separator: "#93a1a1"
    leading_trailing_space_bg: { attr: "n" }
    header: { fg: "#859900" attr: "b" }
    empty: "#268bd2"
    bool: "#2aa198"
    int: "#657b83"
    filesize: "#2aa198"
    duration: "#657b83"
    datetime: "#6c71c4"
    range: "#657b83"
    float: "#657b83"
    string: "#859900"
    nothing: "#657b83"
    binary: "#657b83"
    cell-path: "#657b83"
    row_index: { fg: "#859900" attr: "b" }
    record: "#657b83"
    list: "#657b83"
    closure: { fg: "#859900" attr: "b" }
    glob: { fg: "#2aa198" attr: "b" }
    block: "#657b83"
    hints: "#93a1a1"
    search_result: { bg: "#93a1a1" fg: "#fdf6e3" }
    shape_binary: { fg: "#6c71c4" attr: "b" }
    shape_block: { fg: "#268bd2" attr: "b" }
    shape_bool: "#2aa198"
    shape_closure: { fg: "#859900" attr: "b" }
    shape_custom: "#859900"
    shape_datetime: { fg: "#2aa198" attr: "b" }
    shape_directory: "#268bd2"
    shape_external: "#268bd2"
    shape_externalarg: { fg: "#859900" attr: "b" }
    shape_external_resolved: { fg: "#b58900" attr: "b" }
    shape_filepath: "#268bd2"
    shape_flag: { fg: "#268bd2" attr: "b" }
    shape_float: { fg: "#6c71c4" attr: "b" }
    shape_glob_interpolation: { fg: "#2aa198" attr: "b" }
    shape_globpattern: { fg: "#2aa198" attr: "b" }
    shape_int: { fg: "#6c71c4" attr: "b" }
    shape_internalcall: { fg: "#2aa198" attr: "b" }
    shape_keyword: { fg: "#268bd2" attr: "b" }
    shape_list: { fg: "#2aa198" attr: "b" }
    shape_literal: "#268bd2"
    shape_match_pattern: "#859900"
    shape_matching_brackets: { attr: u }
    shape_nothing: "#2aa198"
    shape_operator: "#b58900"
    shape_pipe: { fg: "#6c71c4" attr: "b" }
    shape_range: { fg: "#b58900" attr: "b" }
    shape_record: { fg: "#2aa198" attr: "b" }
    shape_redirection: { fg: "#6c71c4" attr: "b" }
    shape_signature: { fg: "#859900" attr: "b" }
    shape_string: "#859900"
    shape_string_interpolation: { fg: "#2aa198" attr: "b" }
    shape_table: { fg: "#268bd2" attr: "b" }
    shape_variable: "#6c71c4"
    shape_vardecl: "#6c71c4"
    shape_raw_string: { fg: "#cb4b16" }
    shape_garbage: {
        fg: "#fdf6e3"
        bg: "#dc322f"
        attr: "b"
    }
}

let solarized_dark = {
    separator: "#586e75"
    leading_trailing_space_bg: { attr: "n" }
    header: { fg: "#859900" attr: "b" }
    empty: "#268bd2"
    bool: "#2aa198"
    int: "#839496"
    filesize: "#2aa198"
    duration: "#839496"
    datetime: "#6c71c4"
    range: "#839496"
    float: "#839496"
    string: "#859900"
    nothing: "#839496"
    binary: "#839496"
    cell-path: "#839496"
    row_index: { fg: "#859900" attr: "b" }
    record: "#839496"
    list: "#839496"
    closure: { fg: "#859900" attr: "b" }
    glob: { fg: "#2aa198" attr: "b" }
    block: "#839496"
    hints: "#586e75"
    search_result: { bg: "#586e75" fg: "#002b36" }
    shape_binary: { fg: "#6c71c4" attr: "b" }
    shape_block: { fg: "#268bd2" attr: "b" }
    shape_bool: "#2aa198"
    shape_closure: { fg: "#859900" attr: "b" }
    shape_custom: "#859900"
    shape_datetime: { fg: "#2aa198" attr: "b" }
    shape_directory: "#268bd2"
    shape_external: "#268bd2"
    shape_externalarg: { fg: "#859900" attr: "b" }
    shape_external_resolved: { fg: "#b58900" attr: "b" }
    shape_filepath: "#268bd2"
    shape_flag: { fg: "#268bd2" attr: "b" }
    shape_float: { fg: "#6c71c4" attr: "b" }
    shape_glob_interpolation: { fg: "#2aa198" attr: "b" }
    shape_globpattern: { fg: "#2aa198" attr: "b" }
    shape_int: { fg: "#6c71c4" attr: "b" }
    shape_internalcall: { fg: "#2aa198" attr: "b" }
    shape_keyword: { fg: "#268bd2" attr: "b" }
    shape_list: { fg: "#2aa198" attr: "b" }
    shape_literal: "#268bd2"
    shape_match_pattern: "#859900"
    shape_matching_brackets: { attr: u }
    shape_nothing: "#2aa198"
    shape_operator: "#b58900"
    shape_pipe: { fg: "#6c71c4" attr: "b" }
    shape_range: { fg: "#b58900" attr: "b" }
    shape_record: { fg: "#2aa198" attr: "b" }
    shape_redirection: { fg: "#6c71c4" attr: "b" }
    shape_signature: { fg: "#859900" attr: "b" }
    shape_string: "#859900"
    shape_string_interpolation: { fg: "#2aa198" attr: "b" }
    shape_table: { fg: "#268bd2" attr: "b" }
    shape_variable: "#6c71c4"
    shape_vardecl: "#6c71c4"
    shape_raw_string: { fg: "#cb4b16" }
    shape_garbage: {
        fg: "#002b36"
        bg: "#dc322f"
        attr: "b"
    }
}

# Basic Nushell configuration
$env.config = {
    show_banner: false # disable welcome banner
    
    ls: {
        use_ls_colors: true
        clickable_links: true
    }
    
    rm: {
        always_trash: false
    }
    
    table: {
        mode: rounded
        index_mode: always
        show_empty: true
    }
    
    error_style: "fancy"
    
    datetime_format: {
        normal: '%a, %d %b %Y %H:%M:%S %z'
        table: '%Y-%m-%d %I:%M:%S%p'
    }
    
    cursor_shape: {
        vi_insert: line
        vi_normal: block
    }
    
    edit_mode: vi
    
    completions: {
        case_sensitive: false
        quick: true
        partial: true
        algorithm: "fuzzy"
    }
    
    history: {
        max_size: 100_000
        sync_on_enter: true
        file_format: "sqlite"
    }
    
    color_config: {
        separator: "#93a1a1"
        leading_trailing_space_bg: { attr: "n" }
        header: { fg: "#859900" attr: "b" }
        empty: "#268bd2"
        bool: "#2aa198"
        int: "#657b83"
        filesize: "#2aa198"
        duration: "#657b83"
        datetime: "#6c71c4"
        range: "#657b83"
        float: "#657b83"
        string: "#859900"
        nothing: "#657b83"
        binary: "#657b83"
        cell-path: "#657b83"
        row_index: { fg: "#859900" attr: "b" }
        record: "#657b83"
        list: "#657b83"
        closure: { fg: "#859900" attr: "b" }
        glob: { fg: "#2aa198" attr: "b" }
        block: "#657b83"
        hints: "#93a1a1"
        search_result: { bg: "#93a1a1" fg: "#fdf6e3" }
        shape_binary: { fg: "#6c71c4" attr: "b" }
        shape_block: { fg: "#268bd2" attr: "b" }
        shape_bool: "#2aa198"
        shape_closure: { fg: "#859900" attr: "b" }
        shape_custom: "#859900"
        shape_datetime: { fg: "#2aa198" attr: "b" }
        shape_directory: "#268bd2"
        shape_external: "#268bd2"
        shape_externalarg: { fg: "#859900" attr: "b" }
        shape_external_resolved: { fg: "#b58900" attr: "b" }
        shape_filepath: "#268bd2"
        shape_flag: { fg: "#268bd2" attr: "b" }
        shape_float: { fg: "#6c71c4" attr: "b" }
        shape_glob_interpolation: { fg: "#2aa198" attr: "b" }
        shape_globpattern: { fg: "#2aa198" attr: "b" }
        shape_int: { fg: "#6c71c4" attr: "b" }
        shape_internalcall: { fg: "#2aa198" attr: "b" }
        shape_keyword: { fg: "#268bd2" attr: "b" }
        shape_list: { fg: "#2aa198" attr: "b" }
        shape_literal: "#268bd2"
        shape_match_pattern: "#859900"
        shape_matching_brackets: { attr: u }
        shape_nothing: "#2aa198"
        shape_operator: "#b58900"
        shape_pipe: { fg: "#6c71c4" attr: "b" }
        shape_range: { fg: "#b58900" attr: "b" }
        shape_record: { fg: "#2aa198" attr: "b" }
        shape_redirection: { fg: "#6c71c4" attr: "b" }
        shape_signature: { fg: "#859900" attr: "b" }
        shape_string: "#859900"
        shape_string_interpolation: { fg: "#2aa198" attr: "b" }
        shape_table: { fg: "#268bd2" attr: "b" }
        shape_variable: "#6c71c4"
        shape_vardecl: "#6c71c4"
        shape_raw_string: { fg: "#cb4b16" }
        shape_garbage: {
            fg: "#fdf6e3"
            bg: "#dc322f"
            attr: "b"
        }
    }
}

# Add explore configuration after main config - using upsert to merge properly
# Dynamically set colors based on theme
let explore_colors = if ($env.MACOS_THEME? | default "light") == "dark" {
    {
        selected_cell: { bg: "#586e75", fg: "#fdf6e3" }  # Dark theme: medium gray bg with light text
        selected_row: { fg: "#839496" }
        selected_column: "#268bd2"
        highlight: { bg: "#586e75", fg: "#fdf6e3" }
        status_bar_background: { fg: "#839496", bg: "#073642" }
        status_bar_foreground: { fg: "#fdf6e3", bg: "#073642" }
        command_bar: { fg: "#839496" }
        split_line: "#586e75"
        cursor: true
        table_mode: "rounded"
    }
} else {
    {
        selected_cell: { bg: "#657b83", fg: "#fdf6e3" }  # Light theme: darker gray bg with light text
        selected_row: { fg: "#657b83" }
        selected_column: "#268bd2"
        highlight: { bg: "#93a1a1", fg: "#002b36" }
        status_bar_background: { fg: "#657b83", bg: "#eee8d5" }
        status_bar_foreground: { fg: "#002b36", bg: "#eee8d5" }
        command_bar: { fg: "#657b83" }
        split_line: "#93a1a1"
        cursor: true
        table_mode: "rounded"
    }
}

$env.config = ($env.config | upsert explore $explore_colors)

# Zettelkasten workflow commands for Obsidian Forge vault
# Use alias instead of function for cd commands (functions can't change parent shell directory)
alias notes = cd $env.OBSIDIAN_VAULT

# Paste from Helix external registers
def hx-paste [register?: string] {
    if (which jq | is-empty) {
        print "Installing jq..."
        brew install jq
    }
    
    let result = if ($register == null) {
        # Interactive mode
        ^hx-register-paste
    } else {
        # Direct register access
        ^hx-register-paste $register
    }
    
    print $result
}

# Show all saved Helix registers
def hx-registers [] {
    let registers_file = $"($env.HOME)/.cache/helix-registers.json"
    
    if not ($registers_file | path exists) {
        print "No registers saved yet"
        return
    }
    
    if (which jq | is-not-empty) {
        ^jq -r 'to_entries | .[] | "\(.key): \(.value | split("\n")[0] | if length > 50 then .[:50] + "..." else . end)"' $registers_file
    } else {
        print "Install jq for better output: brew install jq"
        open $registers_file
    }
}

# Create a new note in Forge vault
def note-new [name?: string] {
    let note_name = if ($name == null) {
        print "Enter note name (without .md):"
        (input)
    } else {
        $name
    }
    
    # Add .md extension if not present
    let filename = if ($note_name | str ends-with ".md") {
        $note_name
    } else {
        $"($note_name).md"
    }
    
    let note_path = $"($env.OBSIDIAN_VAULT)/($filename)"
    
    # Check if file already exists
    if ($note_path | path exists) {
        print $"Note already exists: ($note_path)"
        hx $note_path
        return
    }
    
    # Create note with basic frontmatter
    let current_date = (date now | format date "%Y-%m-%d")
    let current_time = (date now | format date "%H:%M")
    
    let template = $"---
date created: ($current_date) ($current_time)
date modified: ($current_date) ($current_time)
---
# ($note_name | str replace ".md" "")

"
    
    $template | save $note_path
    print $"Created: ($note_path)"
    hx $note_path
}

def note-search [query: string] {
    if (which fd | is-empty) {
        print "fd not found, install with: brew install fd"
        return
    }
    
    fd --type f --extension md . $env.OBSIDIAN_VAULT 
    | par-each { |file| 
        let content = (open $file | str downcase)
        let query_lower = ($query | str downcase)
        if ($content | str contains $query_lower) { 
            {
                file: $file
                preview: (open $file | lines | first 3 | str join "\n")
            }
        } 
    } 
    | compact
}

def note-links [file: path] {
    if not ($file | path exists) {
        print $"File not found: ($file)"
        return
    }
    
    open $file 
    | parse -r '\[\[([^\]]+)\]\]' 
    | get capture0 
    | uniq
}

def daily-note [] {
    let today = (date now | format date "%Y-%m-%d")
    let daily_dir = $"($env.OBSIDIAN_VAULT)/NapierianLogs/DayPages"
    
    # Create daily directory if it doesn't exist
    if not ($daily_dir | path exists) {
        mkdir $daily_dir
    }
    
    let note_path = $"($daily_dir)/($today).md"
    
    # Create note with template if it doesn't exist
    if not ($note_path | path exists) {
        let current_date = (date now | format date "%Y-%m-%d")
        let current_time = (date now | format date "%H:%M")
        let readable_date = (date now | format date "%A, %B %d, %Y")
        
        let template = $"---
tags:
- journal
date created: ($current_date) ($current_time)
date modified: ($current_date) ($current_time)

# Daily Metrics
sleep_hours: 
healthy_eating: false
exercise: false
piano_practice: false
meditation: false
reading: false
social_connection: false

# exercise_details: \"[[Exercise Log ($current_date)]]\"
# piano_details: \"[[Piano Practice Log ($current_date)]]\"
# health_details: \"[[Health Metrics ($current_date)]]\"
---
# ($readable_date)

## Links
- Previous: [[((date now) - 1day | format date '%Y-%m-%d')]]
- Next: [[((date now) + 1day | format date '%Y-%m-%d')]]
"
        $template | save $note_path
    }
    
    # Use hx alias which auto-detects theme based on system appearance
    hx $note_path
}

# Quick daily note opener by date
def daily-open [date?: string] {
    let target_date = if ($date == null) {
        (date now | format date "%Y-%m-%d")
    } else {
        $date
    }
    
    let daily_dir = $"($env.OBSIDIAN_VAULT)/NapierianLogs/DayPages"
    let note_path = $"($daily_dir)/($target_date).md"
    
    if ($note_path | path exists) {
        if (which $env.EDITOR | is-not-empty) {
            ^$env.EDITOR $note_path
        } else {
            print $"Daily note exists: ($note_path)"
            print "Set $env.EDITOR to open automatically"
        }
    } else {
        print $"Daily note doesn't exist: ($target_date)"
        print $"Create it by clicking [[($target_date)]] in Helix or run 'daily-note' for today"
    }
}

# Enhanced commands using skim (sk)
def note-find [] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "This command requires sk and fd. Install with: brew install sk fd"
        return
    }
    
    let preview_cmd = if (which bat | is-not-empty) { 
        "bat --color=always --style=numbers {}" 
    } else { 
        "cat {}" 
    }
    
    let selected = (
        fd --type f --extension md . $env.OBSIDIAN_VAULT 
        | sk --preview $preview_cmd --height 60% 
        | str trim
    )
    
    if not ($selected | is-empty) {
        ^$env.EDITOR $selected
    }
}

def note-grep [query?: string] {
    if (which sk | is-empty) or (which rg | is-empty) {
        print "This command requires sk and ripgrep. Install with: brew install sk ripgrep"
        return
    }
    
    let search_query = if ($query | is-empty) { "" } else { $query }
    let preview_cmd = if (which bat | is-not-empty) { 
        "bat --color=always --style=numbers --highlight-line {2} {1}" 
    } else { 
        "sed -n '{2}p' {1}" 
    }
    
    let selected = (
        rg --color=always --line-number --no-heading --smart-case $search_query $env.OBSIDIAN_VAULT 
        | sk --ansi --delimiter : --preview $preview_cmd --height 60% 
        | str trim
    )
    
    if not ($selected | is-empty) {
        let parts = ($selected | split row ":")
        if ($parts | length) >= 2 {
            let file = ($parts | get 0)
            let line = ($parts | get 1)
            ^$env.EDITOR $"+($line)" $file
        }
    }
}

def note-recent [] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "This command requires sk and fd. Install with: brew install sk fd"
        return
    }
    
    let preview_cmd = if (which bat | is-not-empty) { 
        "bat --color=always --style=numbers {}" 
    } else { 
        "cat {}" 
    }
    
    let selected = (
        fd --type f --extension md . $env.OBSIDIAN_VAULT --max-depth 5 
        | lines
        | each { |f| {path: $f, mtime: (ls $f | get modified.0)} }
        | sort-by mtime --reverse
        | first 20
        | get path
        | str join "\n"
        | sk --preview $preview_cmd --height 60% 
        | str trim
    )
    
    if not ($selected | is-empty) {
        ^$env.EDITOR $selected
    }
}

# ============================================
# TELEKASTEN-INSPIRED ZETTELKASTEN FUNCTIONS
# ============================================

# Simple calendar view for daily notes
def note-calendar [--month: int = 0] {
    let vault_path = $env.OBSIDIAN_VAULT
    let daily_dir = $"($vault_path)/NapierianLogs/DayPages"
    
    # Get target year and month
    let today = date now
    let target_year = ($today | format date "%Y" | into int)
    let target_month = (($today | format date "%m" | into int) + $month)
    
    # Adjust year if month overflows
    let actual_year = if $target_month > 12 {
        $target_year + 1
    } else if $target_month < 1 {
        $target_year - 1
    } else {
        $target_year
    }
    
    let actual_month = if $target_month > 12 {
        $target_month - 12
    } else if $target_month < 1 {
        $target_month + 12
    } else {
        $target_month
    }
    
    let month_str = ($actual_month | into string | fill --alignment right --character '0' --width 2)
    let year_str = ($actual_year | into string)
    
    # Get existing notes for this month
    let existing_notes = (
        try {
            glob $"($daily_dir)/($year_str)-($month_str)-*.md"
            | path basename 
            | str replace ".md" ""
            | parse "{year}-{month}-{day}"
            | get day
            | each { |d| $d | into int }
        } catch {
            []
        }
    )
    
    # Use system cal command for simplicity
    print $"Daily notes for ($year_str)-($month_str):"
    print "Existing notes:"
    if ($existing_notes | length) > 0 {
        print ($existing_notes | sort | each { |d| $"  ($year_str)-($month_str)-($d | into string | fill --alignment right --character '0' --width 2)" } | str join "\n")
    } else {
        print "  No daily notes found"
    }
    
    print ""
    print "Use: daily-note to create today's note"
    print "Use: note-calendar --month 1 for next month"
}

# Go to this week's note
def note-week [] {
    let week_num = (date now | format date "%Y-W%V")
    let week_file = $"($env.OBSIDIAN_VAULT)/NapierianLogs/WeekPages/($week_num).md"
    
    if not ($week_file | path exists) {
        # Create week note from template if it doesn't exist
        let template = $"($env.OBSIDIAN_VAULT)/Areas/Obsidian/Templates/WeekPage.md"
        if ($template | path exists) {
            open $template | save $week_file
            print $"Created week note: ($week_num)"
        } else {
            "# Week " + $week_num + "\n\n## Goals\n\n## Notes\n\n## Review\n" | save $week_file
        }
    }
    
    hx $week_file
}

# Find weekly notes
def note-week-find [] {
    let weekly_dir = $"($env.OBSIDIAN_VAULT)/NapierianLogs/WeekPages"
    
    if not ($weekly_dir | path exists) {
        mkdir $weekly_dir
    }
    
    let selected = (
        ls $"($weekly_dir)/*.md"
        | get name
        | path basename
        | sort --reverse
        | str join "\n"
        | sk --preview $"bat --color=always ($weekly_dir)/{}" --height 60%
        | str trim
    )
    
    if not ($selected | is-empty) {
        hx $"($weekly_dir)/($selected)"
    }
}

# Show backlinks - all notes that link to the current file
def note-backlinks [file?: path] {
    let target = if ($file | is-empty) {
        # Try to get current file from Helix (would need integration)
        print "Please provide a file path"
        return
    } else {
        $file | path basename | str replace ".md" ""
    }
    
    print $"Finding backlinks to: ($target)"
    
    # Search for [[target]] links
    rg $"\\[\\[($target)\\]\\]" $env.OBSIDIAN_VAULT --type md -l
    | lines
    | each { |f| 
        let content = (rg $"\\[\\[($target)\\]\\]" $f -C 1 | str join "\n")
        {file: ($f | path relative-to $env.OBSIDIAN_VAULT), context: $content}
    }
    | to md
}

# Yank (copy) wiki link for current note
def note-yank [file?: path] {
    let note_path = if ($file | is-empty) {
        print "Please provide a file path"
        return
    } else {
        $file
    }
    
    let link_text = $"[[($note_path | path basename | str replace '.md' '')]]"
    $link_text | pbcopy
    print $"Copied: ($link_text)"
}

# Rename note and update all references
def note-rename [old_name: string, new_name: string] {
    let vault = $env.OBSIDIAN_VAULT
    let old_file = $"($vault)/($old_name).md"
    let new_file = $"($vault)/($new_name).md"
    
    if not ($old_file | path exists) {
        print $"File not found: ($old_file)"
        return
    }
    
    if ($new_file | path exists) {
        print $"File already exists: ($new_file)"
        return
    }
    
    # Find all files that reference the old note
    let refs = (rg $"\\[\\[($old_name)\\]\\]" $vault --type md -l | lines)
    
    print $"Found (($refs | length)) references to update"
    
    # Update all references
    for file in $refs {
        let content = (open $file)
        let updated = ($content | str replace --all $"[[($old_name)]]" $"[[($new_name)]]")
        $updated | save -f $file
        print $"Updated: ($file | path relative-to $vault)"
    }
    
    # Rename the file
    mv $old_file $new_file
    print $"Renamed: ($old_name) -> ($new_name)"
}

# Toggle todo checkbox in current line (for Helix integration)
def note-toggle-todo [] {
    print "Run in Helix: :pipe-to sed 's/\\[ \\]/[x]/g; s/\\[x\\]/[ ]/g'"
}

# Show all tags in vault
def note-tags [] {
    print "Analyzing tags in vault..."
    
    let tags = (
        rg "#[a-zA-Z][a-zA-Z0-9_-]*" $env.OBSIDIAN_VAULT --type md -o
        | lines
        | sort
        | uniq -c
        | sort -r
        | parse "{count} {tag}"
        | take 50
    )
    
    print "Top tags:"
    $tags | table
    
    # Interactive selection
    let selected = (
        $tags 
        | get tag 
        | str join "\n"
        | sk --prompt "Select tag to search: "
        | str trim
    )
    
    if not ($selected | is-empty) {
        note-grep $selected
    }
}

# Create note from template
def note-template [template_name?: string] {
    let template_dir = $"($env.OBSIDIAN_VAULT)/templates"
    
    if not ($template_dir | path exists) {
        print "No templates directory found"
        return
    }
    
    let template = if ($template_name | is-empty) {
        # Select template interactively
        ls $"($template_dir)/*.md"
        | get name
        | path basename
        | str join "\n"
        | sk --prompt "Select template: "
        | str trim
    } else {
        $"($template_name).md"
    }
    
    if ($template | is-empty) {
        return
    }
    
    let template_path = $"($template_dir)/($template)"
    if not ($template_path | path exists) {
        print $"Template not found: ($template)"
        return
    }
    
    # Get new note name
    let note_name = (input "Enter note name: ")
    if ($note_name | is-empty) {
        return
    }
    
    let note_path = $"($env.OBSIDIAN_VAULT)/($note_name).md"
    
    # Process template
    let content = (open $template_path)
    let processed = (
        $content
        | str replace --all "{{date}}" (date now | format date "%Y-%m-%d")
        | str replace --all "{{date:YYYY-MM-DD}}" (date now | format date "%Y-%m-%d")
        | str replace --all "{{time}}" (date now | format date "%H:%M")
        | str replace --all "{{title}}" $note_name
    )
    
    $processed | save $note_path
    print $"Created: ($note_path)"
    hx $note_path
}

# Navigate to previous/next daily note
def note-day-prev [] {
    let current = if (pwd | str ends-with "DayPages") {
        pwd | path basename
    } else {
        # Get current file name - would need Helix integration
        date now | format date "%Y-%m-%d"
    }
    
    let current_date = ($current | str substring 0..10 | into datetime)
    let prev_date = (($current_date - 1day) | format date "%Y-%m-%d")
    let prev_file = $"($env.OBSIDIAN_VAULT)/NapierianLogs/DayPages/($prev_date).md"
    
    if ($prev_file | path exists) {
        hx $prev_file
    } else {
        print $"No note for ($prev_date)"
    }
}

def note-day-next [] {
    let current = if (pwd | str ends-with "DayPages") {
        pwd | path basename
    } else {
        date now | format date "%Y-%m-%d"
    }
    
    let current_date = ($current | str substring 0..10 | into datetime)
    let next_date = (($current_date + 1day) | format date "%Y-%m-%d")
    let next_file = $"($env.OBSIDIAN_VAULT)/NapierianLogs/DayPages/($next_date).md"
    
    if ($next_file | path exists) {
        hx $next_file
    } else {
        print $"No note for ($next_date)"
    }
}

# Paste image from clipboard and create link
def note-paste-image [name?: string] {
    let img_name = if ($name | is-empty) {
        $"image-(date now | format date '%Y%m%d-%H%M%S').png"
    } else {
        $"($name).png"
    }
    
    let img_dir = $"($env.OBSIDIAN_VAULT)/attachments"
    mkdir $img_dir
    let img_path = $"($img_dir)/($img_name)"
    
    # Use pngpaste on macOS
    if (which pngpaste | is-not-empty) {
        pngpaste $img_path
        if ($img_path | path exists) {
            let link = $"![[attachments/($img_name)]]"
            $link | pbcopy
            print $"Image saved: ($img_path)"
            print $"Link copied: ($link)"
        } else {
            print "No image in clipboard"
        }
    } else {
        print "pngpaste not found. Install with: brew install pngpaste"
    }
}

# Find notes modified on same date as current
def note-same-date [date?: string] {
    let target_date = if ($date | is-empty) {
        date now | format date "%Y-%m-%d"
    } else {
        $date
    }
    
    print $"Finding notes modified on ($target_date)..."
    
    fd . $env.OBSIDIAN_VAULT --type f --extension md --changed-on $target_date
    | lines
    | each { |f| $f | path relative-to $env.OBSIDIAN_VAULT }
    | to md
}

def --env cd-notes [] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "This command requires sk and fd. Install with: brew install sk fd"
        return
    }
    
    let preview_cmd = if (which eza | is-not-empty) { 
        "eza --tree --color=always {} | head -20" 
    } else { 
        "ls -la {}" 
    }
    
    let dir = (
        fd --type d . $env.OBSIDIAN_VAULT 
        | ^env TERM=xterm-256color sk --preview $preview_cmd --height 60% 
        | str trim
    )
    
    if not ($dir | is-empty) {
        cd $dir  # Changed: cd directly instead of returning the string
    }
}

# Helix find - fuzzy find and open file in current directory
def hf [] {
    if (which sk | is-empty) {
        print "This command requires sk. Install with: brew install sk"
        return
    }
    
    let file = (ls | where type == "file" | get name | sk --height 40% | str trim)
    if not ($file | is-empty) {
        hx $file
    }
}

# Helix vault - fuzzy find any file in entire vault and open it
def hv [] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "This command requires sk and fd. Install with: brew install sk fd"
        return
    }
    
    let file = (
        fd --type f . $env.OBSIDIAN_VAULT 
        | sk --preview 'head -20 {}' --height 60% 
        | str trim
    )
    if not ($file | is-empty) {
        hx $file
    }
}

# Git aliases - conditional
if (which git | is-not-empty) {
    alias gs = git status
    alias ga = git add
    alias gc = git commit
    alias gp = git push
    alias gl = git log --oneline
    alias gd = git diff
    alias gco = git checkout
    alias gb = git branch
}

# Modern CLI tool aliases - all conditional
if (which bat | is-not-empty) {
    alias cat = bat --theme=$env.BAT_THEME?
}

if (which eza | is-not-empty) {
    alias ls = eza --color=auto --icons
    alias ll = eza --color=auto --icons --long --all --git
    alias la = eza --color=auto --icons --all
    alias tree = eza --color=auto --icons --tree
} else {
    alias ll = ls -la
    alias la = ls -a
}

if (which fd | is-not-empty) {
    alias find = fd
}

if (which rg | is-not-empty) {
    alias grep = rg --color=auto
}

if (which procs | is-not-empty) {
    alias ps = procs
}

if (which btop | is-not-empty) {
    alias top = btop
}

# Directory navigation
alias .. = cd ..
alias ... = cd ../..
alias .... = cd ../../..
alias cdn = cd-notes  # Direct alias to the function (not a subshell evaluation)
alias hx = hx-auto  # Auto-detect theme based on system appearance

# Zoxide shortcuts - conditional
if (which zoxide | is-not-empty) {
    alias j = z  # Jump to directory
    alias ji = zi  # Interactive jump
}

# Quick config access
alias nuconfig = ^$env.EDITOR $"($nu.default-config-dir)/config.nu"
alias nuenv = ^$env.EDITOR $"($nu.default-config-dir)/env.nu"

if ($"($env.HOME)/.config/wezterm/wezterm.lua" | path exists) {
    alias wezconfig = ^$env.EDITOR ~/.config/wezterm/wezterm.lua
}

if ($"($env.HOME)/.config/nvim/init.lua" | path exists) {
    alias nvimconfig = ^$env.EDITOR ~/.config/nvim/init.lua
}

if ($"($env.HOME)/.config/starship.toml" | path exists) {
    alias starshipconfig = ^$env.EDITOR ~/.config/starship.toml
}

# Zellij session management aliases
alias zj-save = ~/.dotfiles/scripts/zellij-sync.nu save
alias zj-restore = ~/.dotfiles/scripts/zellij-sync.nu restore  
alias zj-remote = ~/.dotfiles/scripts/zellij-sync.nu remote

# Quick aliases for common Zellij operations
alias zj = zellij
alias zj-ls = zellij list-sessions
alias zj-kill = zellij kill-session
alias zj-work = zellij --session work

# Yazi function with directory change support
def --env y [...args] {
    let tmp = (mktemp -t "yazi-cwd.XXXXXX")
    
    # Run yazi with any provided arguments
    if ($args | length) > 0 {
        yazi ...$args --cwd-file $tmp
    } else {
        yazi --cwd-file $tmp
    }
    
    # Change to the directory yazi was in when it exited
    let cwd = (open $tmp | str trim)
    if $cwd != "" and $cwd != $env.PWD {
        cd $cwd
    }
    
    # Clean up temp file
    rm -f $tmp
}

# Helper function to reload config
def reload-config [] {
    exec nu
}

# Yazi integration - open at last Neovim location
def --env yz [] {
    let tmp = (mktemp -t "yazi-cwd.XXXXXX")
    
    # If we have a specific file, open yazi with it selected
    if ('~/.nvim_last_file' | path expand | path exists) {
        let file = open ~/.nvim_last_file | str trim
        if ($file | path exists) {
            # Pass the file as the entry argument - this will select it
            yazi $file --cwd-file $tmp
        } else {
            # Fall back to directory if file doesn't exist
            let dir = if ('~/.nvim_last_dir' | path expand | path exists) { 
                open ~/.nvim_last_dir | str trim 
            } else { 
                $env.PWD 
            }
            cd $dir
            yazi --cwd-file $tmp
        }
    } else {
        # No specific file, just use directory
        let dir = if ('~/.nvim_last_dir' | path expand | path exists) { 
            open ~/.nvim_last_dir | str trim 
        } else { 
            $env.PWD 
        }
        cd $dir
        yazi --cwd-file $tmp
    }
    
    # Handle directory change after yazi exits
    if ($tmp | path exists) {
        let cwd = (open $tmp | str trim)
        if ($cwd | is-not-empty) and ($cwd != $env.PWD) {
            cd $cwd
        }
    }
    
    # Clean up temp file
    rm -f $tmp
}

# File Management Power Functions
# Quick file browser - replaces Yazi for quick looks
def l [...args] {
    if ($args | is-empty) {
        ls | sort-by type name
    } else {
        ls ...$args | sort-by type name
    }
}

# Find files by pattern (replaces fd/find)
def ff [pattern: string] {
    ls **/* | where name =~ $pattern | select name size modified
}

# Quick preview files
def preview [file: path] {
    let ext = ($file | path parse | get extension)
    if $ext in [txt md yaml yml toml json nu] {
        open $file | lines | first 20
    } else if $ext == pdf {
        echo "PDF file - use 'open' to view"
    } else {
        ls $file
    }
}

# Bulk rename with pattern
def bulk-rename [pattern: string, replacement: string] {
    ls | where name =~ $pattern | each {|f| 
        let new_name = ($f.name | str replace $pattern $replacement)
        echo $"($f.name) -> ($new_name)"
    }
    print "Run with --execute to perform rename"
}

# Copy file to macOS clipboard
def clip [file: path] {
    if ($file | path exists) {
        if ($file | path parse | get extension) == pdf {
            # For PDFs, copy the file reference (for drag/drop)
            $"file://($file | path expand)" | pbcopy
            echo $"✓ ($file) copied to clipboard"
        } else {
            # For text files, copy contents
            open $file | pbcopy
            echo $"✓ Contents of ($file) copied to clipboard"
        }
    }
}

# Smart directory navigation (up)
def --env up [levels: int = 1] {
    let path = (1..$levels | each { ".." } | str join "/")
    cd $path
}

# Recent files
def recent [--days: int = 7] {
    ls **/* | where modified > ((date now) - ($days * 24hr)) | sort-by modified -r
}

# Interactive file explorer (Yazi replacement!)
def e [...path] {
    let target = if ($path | is-empty) { "." } else { $path | str join "/" }
    ls $target | explore
}

# File find and edit - open file from search results by index
def fe [pattern: string, index: int = 0] {
    let files = (ff $pattern | get name)
    let count = ($files | length)
    if $count == 0 {
        echo $"No files found matching '($pattern)'"
    } else if $index >= $count {
        echo $"Index ($index) out of range. Found ($count) files."
        echo "Available files:"
        $files | enumerate | each {|f| echo $"  ($f.index): ($f.item)"}
    } else {
        let file = ($files | get $index)
        echo $"Opening: ($file)"
        hx $file
    }
}

# Initialize zoxide
source ~/.zoxide.nu
source ~/.config/nushell/zotero-commands.nu

# Link Manager aliases
alias links-status = link-service status
alias links-logs = link-service logs  
alias links-restart = link-service restart
alias links-test = link-service test
