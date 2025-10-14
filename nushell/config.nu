# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/config.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Config File
# version = "0.106.1"

# Load vendor autoload scripts conditionally
# Check for and source Starship prompt if available
if ($"($env.HOME)/.cache/nushell/starship-init.nu" | path exists) {
    source ~/.cache/nushell/starship-init.nu
}

# Initialize zoxide with reliable function-based approach
# Custom zoxide functions (more reliable than aliases)
def --env z [query?: string] {
    if ($query | is-empty) {
        cd ~
    } else {
        cd (zoxide query $query)
    }
}

# Interactive zoxide - rich skim experience matching fsh
def --env zi [] {
    if (which sk | is-empty) {
        print "sk (skim) not found. Install with: brew install sk"
        return
    }
    
    let selected = (
        zoxide query -l 
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk 
            --preview 'eza --tree --color=always --level=2 {} | head -20' 
            --preview-window 'right:60%' 
            --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' 
            --prompt "📁 Zoxide: "
        | str trim
    )
    
    if not ($selected | is-empty) {
        cd $selected
    }
}

# ---- Dotfiles Sync Functions ----
# Manual sync functions for cross-platform dotfiles management
def sync-push [] { 
    cd ~/dotfiles
    git add .
    let timestamp = (date now | format date '%H:%M')
    git commit -m $"Manual sync ($timestamp)"
    git push
    print "✅ Dotfiles pushed to GitHub"
}

def sync-pull [] {
    cd ~/dotfiles
    git pull
    dotter deploy
    print "✅ Dotfiles pulled and deployed"
}

# ---- Editor Workflow Functions ----
# Open daily note in Zed (cursor at line 3)
def zed-daily [] {
    let daily_file = (^~/.local/bin/daily-note --print-path)
    zed $"($daily_file):3"
}

# ---- Zettelkasten Workflow Functions ----
# Promote fleeting note to permanent Zettelkasten
# Usage: promote note-name (without .md extension)
def promote [note_name: string] {
    let fleeting_path = $"($env.HOME)/Forge/fleeting/($note_name).md"
    let permanent_path = $"($env.HOME)/Forge/($note_name).md"

    if not ($fleeting_path | path exists) {
        print $"❌ Note not found: ($note_name)"
        print $"   (Looking in ~/Forge/fleeting/)"
        return
    }

    if ($permanent_path | path exists) {
        print $"⚠️  Permanent note already exists: ($note_name)"
        print $"   Use a different name or manually resolve conflict"
        return
    }

    mv $fleeting_path $permanent_path
    print $"✓ Promoted: ($note_name)"
    print $"  From: fleeting/($note_name).md"
    print $"  To:   ($note_name).md"
}

# Review fleeting notes inbox
def fleeting [] {
    let fleeting_dir = $"($env.HOME)/Forge/fleeting"

    if not ($fleeting_dir | path exists) {
        print "No fleeting notes directory found"
        print $"Create it with: mkdir ($fleeting_dir)"
        return
    }

    let count = (ls $fleeting_dir | where type == file | length)

    if $count == 0 {
        print "📭 Inbox empty - no fleeting notes to process"
        return
    }

    print $"📬 ($count) fleeting notes in inbox:\n"
    let notes = (ls $fleeting_dir | where type == file | get name | each {|n| $"  - ($n | path basename)"})
    print ($notes | str join "\n")
    print $"\nUse 'cd ~/Forge/fleeting && fsh' to browse, or 'promote <name>' to promote"
}

# ---- Quick Logging Functions ----
# Log entry directly to today's DayPage and trigger collection
# Usage: log "P.website:: 2hr implemented-nav"
#        log "p.c:: 45min Bach-Prelude"
def log [entry: string] {
    let today = (date now | format date "%Y-%m-%d")
    let daypage = $"($env.HOME)/Forge/NapierianLogs/DayPages/($today).md"

    # Ensure DayPages directory exists
    let daypage_dir = ($daypage | path dirname)
    if not ($daypage_dir | path exists) {
        mkdir $daypage_dir
    }

    # Append entry with blank line for spacing
    $"\n($entry)" | save --append $daypage

    # Trigger collection silently
    try {
        ^collect-entries out+err> /dev/null
    } catch {
        # Collection errors logged separately, don't interrupt workflow
    }

    print $"✓ Logged to ($today): ($entry)"
}

# Pomodoro timer with automatic logging (background job)
# Usage: pomo "P.website" "implement-navigation"
#        pomo "p.c" "Bach-Prelude-practice"
#        pomo-status  # Check current pomodoro
def pomo [
    project: string   # Project/activity key (P.name or p.c, etc.)
    task: string      # Task description (use-hyphens for multi-word)
] {
    let state_file = $"($env.HOME)/.local/share/pomo-state.json"

    # Check if pomodoro already running
    if ($state_file | path exists) {
        let current = (open $state_file | from json)
        print $"⚠️  Pomodoro already running: ($current.project) - ($current.task)"
        print $"   Started at ($current.start_time)"
        return
    }

    # Save pomodoro state
    {
        project: $project,
        task: $task,
        start_time: (date now | format date "%H:%M"),
        start_timestamp: (date now | into int)
    } | to json | save $state_file

    print $"🍅 Starting 25min Pomodoro: ($project) - ($task)"
    print $"   Focus time begins now. Terminal remains available for work.\n"

    # Capture variables for closure
    let proj = $project
    let tsk = $task
    let state = $state_file

    # Launch background timer job
    let job_id = (job spawn --tag $"pomo-($project)" {
        sleep 25min

        # Auto-log the completed pomodoro
        let today = (date now | format date "%Y-%m-%d")
        let daypage = $"($env.HOME)/Forge/NapierianLogs/DayPages/($today).md"
        $"\n($proj):: 25min ($tsk)" | save --append $daypage

        # Trigger collection
        try {
            ^collect-entries out+err> /dev/null
        }

        # Desktop notification (cross-platform)
        try {
            if (which osascript | is-not-empty) {
                ^osascript -e $'display notification "($tsk)" with title "🍅 Pomodoro Complete!" sound name "Glass"'
            } else if (which notify-send | is-not-empty) {
                ^notify-send -u critical "🍅 Pomodoro Complete!" $"($tsk)"
            }
        }

        # Clean up state file
        if ($state | path exists) {
            rm $state
        }
    })

    print $"   Timer running in background (Job ID: ($job_id))"
    print $"   Use 'pomo-status' to check current session"
    print $"   Use 'job list' to see all background jobs"
}

# Check current pomodoro status
def pomo-status [] {
    let state_file = $"($env.HOME)/.local/share/pomo-state.json"

    if not ($state_file | path exists) {
        print "No active pomodoro session"
        return
    }

    let current = (open $state_file | from json)
    let now_ts = (date now | into int)
    let elapsed_ns = ($now_ts - $current.start_timestamp)
    let elapsed_min = ($elapsed_ns / 1_000_000_000 / 60 | math round)
    let remaining_min = (25 - $elapsed_min)

    print $"🍅 Active Pomodoro:"
    print $"   Project: ($current.project)"
    print $"   Task: ($current.task)"
    print $"   Started: ($current.start_time)"
    print $"   Elapsed: ($elapsed_min) minutes"
    print $"   Remaining: ($remaining_min) minutes"
}

# End current pomodoro early and log actual time worked
def pomo-end [] {
    let state_file = $"($env.HOME)/.local/share/pomo-state.json"

    if not ($state_file | path exists) {
        print "No active pomodoro to end"
        return
    }

    let current = (open $state_file | from json)

    # Calculate actual elapsed time
    let now_ts = (date now | into int)
    let elapsed_ns = ($now_ts - $current.start_timestamp)
    let elapsed_min = ($elapsed_ns / 1_000_000_000 / 60 | math round)

    # Log with actual duration
    log $"($current.project):: ($elapsed_min)min ($current.task)"

    # Clean up state
    rm $state_file

    print $"✓ Pomodoro ended early: ($current.project) - ($current.task)"
    print $"   Logged ($elapsed_min)min (instead of 25min)"
}

# Cancel current pomodoro (without logging)
def pomo-cancel [] {
    let state_file = $"($env.HOME)/.local/share/pomo-state.json"

    if not ($state_file | path exists) {
        print "No active pomodoro to cancel"
        return
    }

    let current = (open $state_file | from json)
    rm $state_file

    print $"❌ Cancelled pomodoro: ($current.project) - ($current.task)"
    print $"   (Not logged)"
}

# ---- Unified Project Root Detection & Tools ----
use ~/.config/nushell/scripts/project-root-detection.nu *
use ~/.config/nushell/scripts/serpl.nu *
use ~/.config/nushell/completions/serpl-completions.nu *
alias serpl-any = serpl-anywhere
# Example to customize project markers globally:
# $env.PROJECT_ROOT_MARKERS = [".git" ".vault-root" ".obsidian" "Cargo.toml" ".my-custom-marker"]

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
        table: '%Y-%m-%d %H:%M:%S'
    }
    
    cursor_shape: {
        vi_insert: line
        vi_normal: block
    }
    
    edit_mode: emacs  # Use emacs mode for cross-platform text navigation compatibility

    keybindings: [
        {
            name: disable_ctrl_z
            modifier: control
            keycode: char_z
            mode: [emacs, vi_normal, vi_insert]
            event: null  # Disable Ctrl+Z to prevent accidental suspension
        }
    ]

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
let explore_colors = if ($env.SYSTEM_THEME? | default "light") == "dark" {
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

# Disable Ctrl+Z at terminal level (for subprocesses like Claude Code)
# The keybinding above only disables it in Nushell itself
if (which stty | is-not-empty) {
    try { ^stty susp undef } catch { }
}

# Zettelkasten workflow commands for Forge
# Use alias instead of function for cd commands (functions can't change parent shell directory)
alias notes = cd $env.FORGE

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

# Create a new note in Forge
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
    
    let note_path = $"($env.FORGE)/($filename)"
    
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
    
    fd --type f --extension md . $env.FORGE 
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
    let daily_dir = $"($env.HOME)/Forge/NapierianLogs/DayPages"
    
    # Create daily directory if it doesn't exist
    if not ($daily_dir | path exists) {
        mkdir $daily_dir
    }
    
    let note_path = $"($daily_dir)/($today).md"
    
    # Create note with template if it doesn't exist
    if not ($note_path | path exists) {
        let template_path = $"($env.HOME)/Forge/Areas/PKMStrategies/Templates/DayPage.md"
        
        if ($template_path | path exists) {
            # Read template from file and process variables
            let current_date = (date now | format date "%Y-%m-%d")
            let current_time = (date now | format date "%H:%M")
            let readable_date = (date now | format date "%A, %B %d, %Y")
            
            # Read and process the template
            let template_content = (open $template_path)
            let processed = (
                $template_content
                | str replace --all "\{\{date\}\}" $current_date
                | str replace --all "\{\{time24\}\}" $current_time
                | str replace --all "\{\{hdate\}\}" $readable_date
                | str replace --all "<cursor>" ""
            )
            
            $processed | save $note_path
        } else {
            # Fallback to hardcoded template if file doesn't exist
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
    }
    
    # Use hx alias which auto-detects theme based on system appearance
    hx $"($note_path):3"
}


# Quick daily note opener by date
def daily-open [date?: string] {
    let target_date = if ($date == null) {
        (date now | format date "%Y-%m-%d")
    } else {
        $date
    }
    
    let daily_dir = $"($env.FORGE)/NapierianLogs/DayPages"
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

# Navigate to previous day's note (creates if needed)
# Usage: prev-day [--days 1]
def prev-day [--days: int = 1, --helix] {
    let target_date = ((date now) - ($days * 1day) | format date "%Y-%m-%d")
    let daily_dir = $"($env.HOME)/Forge/NapierianLogs/DayPages"
    let note_path = $"($daily_dir)/($target_date).md"

    # Create directory if needed
    if not ($daily_dir | path exists) {
        mkdir $daily_dir
    }

    # Create note with template if it doesn't exist
    if not ($note_path | path exists) {
        let template_path = $"($env.HOME)/Forge/Areas/PKMStrategies/Templates/DayPage.md"
        let readable_date = ((date now) - ($days * 1day) | format date "%A, %B %d, %Y")
        let current_time = (date now | format date "%H:%M")

        if ($template_path | path exists) {
            let template_content = (open $template_path)
            let processed = (
                $template_content
                | str replace --all "\{\{date\}\}" $target_date
                | str replace --all "\{\{time24\}\}" $current_time
                | str replace --all "\{\{hdate\}\}" $readable_date
                | str replace --all "<cursor>" ""
            )
            $processed | save $note_path
            print $"📝 Created: ($target_date)"
        } else {
            # Fallback template
            let template = $"---
tags:
- journal
date created: ($target_date) ($current_time)
date modified: ($target_date) ($current_time)
---
# ($readable_date)

"
            $template | save $note_path
            print $"📝 Created: ($target_date)"
        }
    }

    # Helix mode: create symlink for Helix to open
    if $helix {
        rm -f /tmp/helix-day-target.md
        ^ln -sf $note_path /tmp/helix-day-target.md
    } else {
        print $"📅 Opening: ($target_date)"
        hx $note_path
    }
}

# Navigate to next day's note (creates if needed)
# Usage: next-day [--days 1]
def next-day [--days: int = 1, --helix] {
    let target_date = ((date now) + ($days * 1day) | format date "%Y-%m-%d")
    let daily_dir = $"($env.HOME)/Forge/NapierianLogs/DayPages"
    let note_path = $"($daily_dir)/($target_date).md"

    # Create directory if needed
    if not ($daily_dir | path exists) {
        mkdir $daily_dir
    }

    # Create note with template if it doesn't exist
    if not ($note_path | path exists) {
        let template_path = $"($env.HOME)/Forge/Areas/PKMStrategies/Templates/DayPage.md"
        let readable_date = ((date now) + ($days * 1day) | format date "%A, %B %d, %Y")
        let current_time = (date now | format date "%H:%M")

        if ($template_path | path exists) {
            let template_content = (open $template_path)
            let processed = (
                $template_content
                | str replace --all "\{\{date\}\}" $target_date
                | str replace --all "\{\{time24\}\}" $current_time
                | str replace --all "\{\{hdate\}\}" $readable_date
                | str replace --all "<cursor>" ""
            )
            $processed | save $note_path
            print $"📝 Created: ($target_date)"
        } else {
            # Fallback template
            let template = $"---
tags:
- journal
date created: ($target_date) ($current_time)
date modified: ($target_date) ($current_time)
---
# ($readable_date)

"
            $template | save $note_path
            print $"📝 Created: ($target_date)"
        }
    }

    # Helix mode: create symlink for Helix to open
    if $helix {
        rm -f /tmp/helix-day-target.md
        ^ln -sf $note_path /tmp/helix-day-target.md
    } else {
        print $"📅 Opening: ($target_date)"
        hx $note_path
    }
}

# Redundant note search functions removed - use Yazi equivalents instead:
# - note-find → 's' key in Yazi (project-aware file search)
# - note-grep → 'S' key in Yazi (project-aware content search) 
# - note-recent → 'R' key in Yazi (recent files, 7 days)
# Keeping only Helix-initiated functions (hf, hv) which serve distinct workflow

# ============================================
# TELEKASTEN-INSPIRED ZETTELKASTEN FUNCTIONS
# ============================================

# Simple calendar view for daily notes
def note-calendar [--month: int = 0] {
    let vault_path = $env.FORGE
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
    let week_file = $"($env.FORGE)/NapierianLogs/WeekPages/($week_num).md"
    
    if not ($week_file | path exists) {
        # Create week note from template if it doesn't exist
        let template = $"($env.FORGE)/Areas/Obsidian/Templates/WeekPage.md"
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
    let weekly_dir = $"($env.FORGE)/NapierianLogs/WeekPages"
    
    if not ($weekly_dir | path exists) {
        mkdir $weekly_dir
    }
    
    let selected = (
        ls $"($weekly_dir)/*.md"
        | get name
        | path basename
        | sort --reverse
        | str join "\n"
        | ^env TERM=xterm TERMINFO="" TERMINFO_DIRS="" sk --preview $"mdcat --columns 80 ($weekly_dir)/{}" --height 60%
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
    rg $"\\[\\[($target)\\]\\]" $env.FORGE --type md -l
    | lines
    | each { |f| 
        let content = (rg $"\\[\\[($target)\\]\\]" $f -C 1 | str join "\n")
        {file: ($f | path relative-to $env.FORGE), context: $content}
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
    let vault = $env.FORGE
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
    print "Run in Helix: :pipe-to sd '\\[ \\]' '[TEMP]' | sd '\\[x\\]' '[ ]' | sd '\\[TEMP\\]' '[x]'"
}

# Show all tags in vault
def note-tags [] {
    print "Analyzing tags in vault..."
    
    let tags = (
        rg "#[a-zA-Z][a-zA-Z0-9_-]*" $env.FORGE --type md -o
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
        # Use fsearch (universal content search) since note-grep was removed
        print $"🔍 Searching for tag: ($selected)"
        let results = (rg -i $selected $env.FORGE --type md -l)
        if not ($results | is-empty) {
            $results | lines | each { |file| print $"📄 ($file)" }
        } else {
            print "❌ No files found with tag: ($selected)"
        }
    }
}

# Create note from template
def note-template [template_name?: string] {
    let template_dir = $"($env.FORGE)/templates"
    
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
    
    let note_path = $"($env.FORGE)/($note_name).md"
    
    # Process template
    let content = (open $template_path)
    let processed = (
        $content
        | str replace --all "\\{\\{date\\}\\}" (date now | format date "%Y-%m-%d")
        | str replace --all "\\{\\{date:YYYY-MM-DD\\}\\}" (date now | format date "%Y-%m-%d")
        | str replace --all "\\{\\{time\\}\\}" (date now | format date "%H:%M")
        | str replace --all "\\{\\{title\\}\\}" $note_name
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
    let prev_file = $"($env.FORGE)/NapierianLogs/DayPages/($prev_date).md"
    
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
    let next_file = $"($env.FORGE)/NapierianLogs/DayPages/($next_date).md"
    
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
    
    let img_dir = $"($env.FORGE)/attachments"
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
    
    fd . $env.FORGE --type f --extension md --changed-on $target_date
    | lines
    | each { |f| $f | path relative-to $env.FORGE }
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
        fd --type d . $env.FORGE 
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
    
    let file = (ls | where type == "file" | get name | ^env TERM=xterm TERMINFO="" TERMINFO_DIRS="" sk --height 40% | str trim)
    if not ($file | is-empty) {
        hx $file
    }
}

# Helix Forge - fuzzy find any file in entire vault and open it
def hv [] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "This command requires sk and fd. Install with: brew install sk fd"
        return
    }
    
    let file = (
        fd --type f . $env.FORGE 
        | ^env TERM=xterm TERMINFO="" TERMINFO_DIRS="" sk --preview 'head -20 {}' --height 60%
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
    # Removed ll and la - defined below for consistency
    alias tree = eza --color=auto --icons --tree
}

# Compact ls commands for better SSH experience
# These work consistently across platforms without external tools
def ll [] { ls | select name size modified }
def la [] { ls -a | select name size modified }
def lc [] { ls | select name size }  # Even more compact

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

# 🔥 UNIVERSAL COMMAND-LINE SEARCH SUITE 🔥
# Replacements for Alt commands with universal clipboard support

# View Zellij keybindings cheat sheet in inlyne
def zkeys [] {
    if (which inlyne | is-empty) {
        print "inlyne not found. Install with: brew install inlyne or yay -S inlyne-bin"
        return
    }
    inlyne ~/dotfiles/zellij/KEYBINDINGS.md
}

# Theme switching (Linux only - uses dconf for system-wide dark/light mode)
def tdark [] {
    if (which dconf | is-empty) {
        print "⚠️  dconf not found (Linux/GNOME only)"
        return
    }
    dconf write /org/gnome/desktop/interface/color-scheme '"prefer-dark"'
    print "🌙 Switched to dark theme"
}

def tlight [] {
    if (which dconf | is-empty) {
        print "⚠️  dconf not found (Linux/GNOME only)"
        return
    }
    dconf write /org/gnome/desktop/interface/color-scheme '"prefer-light"'
    print "☀️  Switched to light theme"
}

# File search + open in Helix
def fsh [] {
    if (which fd | is-empty) or (which sk | is-empty) {
        print "fd and sk are required. Install with: brew install fd sk"
        return
    }
    if not ($env.FORGE? | is-empty) and ($env.FORGE | path exists) {
        let file = (fd . $env.FORGE --type f --hidden --exclude .git | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'mdcat --columns 80 {}' --preview-window 'right:60%' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📁 Forge File: " | str trim)
        if not ($file | is-empty) {
            print $"🚀 Opening ($file) in Helix..."
            hx $file
        }
    } else {
        print "❌ FORGE not set or doesn't exist"
    }
}

# Global file search (from current directory)
def gsh [] {
    if (which fd | is-empty) or (which sk | is-empty) {
        print "fd and sk are required. Install with: brew install fd sk"
        return
    }
    let file = (fd . --type f --hidden --exclude .git --exclude Library/CloudStorage/Dropbox | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'mdcat --columns 80 {}' --preview-window 'right:60%' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "🌍 Global Search: " | str trim)
    if not ($file | is-empty) {
        print $"🚀 Opening ($file) in Helix..."
        hx $file
    }
}

# Forge search + copy link to clipboard (universal)
def fsl [] {
    if not ($env.FORGE? | is-empty) and ($env.FORGE | path exists) {
        let file = (fd . $env.FORGE --type f --extension md | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'mdcat --columns 80 {}' --preview-window 'right:60%' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📝 Wiki Link: " | str trim)
        if not ($file | is-empty) {
            let filename = ($file | path basename | str replace ".md" "")
            let wikilink = $"[[($filename)]]"
            $wikilink | pbcopy
            print $"📋 Copied to clipboard: ($wikilink)"
            print "💡 Paste anywhere with Cmd+V"
        }
    } else {
        print "❌ FORGE not set or doesn't exist"
    }
}

# Citation + copy to clipboard (plain text)
def cit [] {
    print "🔍 Loading citations..."
    let citations_file = $"($env.FORGE?)/LIT/citations.md"
    if not ($citations_file | path exists) {
        print $"❌ Citations file not found: ($citations_file)"
        return
    }
    
    let citations = (open $citations_file | lines | where $it != "" | where ($it | str starts-with "#") == false | where ($it | str trim) != "")
    if ($citations | is-empty) {
        print "❌ No citations found"
        return
    }
    
    let selected = ($citations | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'echo {}' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📚 Citation: " | str trim)
    if not ($selected | is-empty) {
        # Extract clean key and title, removing the [ZoteroKey] part
        # Format: "CleanKey [ZoteroKey] Title - keywords" → "CleanKey Title"
        let citation_text = ($selected | parse --regex '^([^\[]+)\[([^\]]+)\]\s*(.*)$' | get -o 0?)

        if not ($citation_text | is-empty) {
            let clean_key = ($citation_text.capture0 | str trim)
            let title_and_keywords = ($citation_text.capture2 | str trim)
            let readable_citation = $"($clean_key) ($title_and_keywords)"

            $readable_citation | pbcopy
            print $"📋 Copied to clipboard: ($readable_citation)"
        } else {
            # Fallback: just copy the whole line if parsing fails
            $selected | pbcopy
            print $"📋 Copied to clipboard: ($selected)"
        }
        print "💡 Paste anywhere with Cmd+V"
    }
}

# Citation + copy wiki link to literature note
def cil [] {
    print "🔍 Loading citations..."
    let citations_file = $"($env.FORGE?)/LIT/citations.md"
    if not ($citations_file | path exists) {
        print $"❌ Citations file not found: ($citations_file)"
        return
    }

    let citations = (open $citations_file | lines | where $it != "" | where ($it | str starts-with "#") == false | where ($it | str trim) != "")
    if ($citations | is-empty) {
        print "❌ No citations found"
        return
    }

    let selected = ($citations | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'echo {}' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📚 Citation → Link: " | str trim)
    if not ($selected | is-empty) {
        # Extract clean key (e.g., "Zamoyski2009") for wiki link to literature note
        # Assumes literature notes are named with the citation key (e.g., Zamoyski2009.md)
        let clean_key = ($selected | split row ' ' | first)
        let wikilink = $"[[($clean_key)]]"

        $wikilink | pbcopy
        print $"📋 Copied wiki link to clipboard: ($wikilink)"
        print "💡 Links to your literature note file"
    }
}

# Citation + open PDF in Zotero
def ciz [] {
    print "🔍 Loading citations..."
    let citations_file = $"($env.FORGE?)/LIT/citations.md"
    let library_file = $"($env.FORGE?)/LIT/library.bib"

    if not ($citations_file | path exists) {
        print $"❌ Citations file not found: ($citations_file)"
        return
    }

    if not ($library_file | path exists) {
        print $"❌ Library file not found: ($library_file)"
        return
    }

    # Load citations (same as fcit for consistency)
    let citations = (open $citations_file | lines | where $it != "" | where ($it | str starts-with "#") == false | where ($it | str trim) != "")
    if ($citations | is-empty) {
        print "❌ No citations found"
        return
    }

    # Select citation
    let selected = ($citations | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'echo {}' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📚 Citation → PDF: " | str trim)

    if not ($selected | is-empty) {
        # Extract clean key and Zotero key from format: "CleanKey [ZoteroKey] Title"
        let clean_key = ($selected | split row ' ' | first)
        let zotero_key = ($selected | parse --regex '\[([^\]]+)\]' | get -o 0.capture0? | default "")

        if ($zotero_key | is-empty) {
            print "❌ Could not extract Zotero key from citation"
            return
        }

        print $"📄 Selected: ($clean_key) → ($zotero_key)"

        # Find PDF path in library.bib
        let entry_text = (rg -A 20 $"@\\w+\\{($zotero_key)," $library_file | str join ' ')
        let file_path = ($entry_text | parse --regex 'file\s*=\s*\{([^}]+)\}' | get -o 0.capture0? | default "")

        if not ($file_path | is-empty) and ($file_path | path exists) {
            print $"📂 Opening PDF: ($file_path)"
            # Use system open command to open in default PDF viewer
            if (sys host | get name) == "Darwin" {
                ^open $file_path
            } else {
                ^xdg-open $file_path
            }
        } else {
            print $"⚠️  PDF file not found at: ($file_path)"
            print $"🔗 Opening Zotero instead..."
            if (sys host | get name) == "Darwin" {
                ^open $"zotero://select/items/@($zotero_key)"
            } else {
                ^xdg-open $"zotero://select/items/@($zotero_key)"
            }
        }
    }
}

# Citation + copy Zotero link to clipboard (clickable markdown link)
def cizl [] {
    print "🔍 Loading citations..."
    let citations_file = $"($env.FORGE?)/LIT/citations.md"

    if not ($citations_file | path exists) {
        print $"❌ Citations file not found: ($citations_file)"
        return
    }

    # Load citations (same as fcit for consistency)
    let citations = (open $citations_file | lines | where $it != "" | where ($it | str starts-with "#") == false | where ($it | str trim) != "")
    if ($citations | is-empty) {
        print "❌ No citations found"
        return
    }

    # Select citation
    let selected = ($citations | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'echo {}' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📚 Citation → Link: " | str trim)

    if not ($selected | is-empty) {
        # Extract clean key and Zotero key from format: "CleanKey [ZoteroKey] Title"
        let citation_info = ($selected | parse --regex '^([^\[]+)\[([^\]]+)\]\s*(.*)$' | get -o 0?)

        if not ($citation_info | is-empty) {
            let clean_key = ($citation_info.capture0 | str trim)
            let zotero_key = ($citation_info.capture1 | str trim)
            let title = ($citation_info.capture2 | str trim)

            # Create markdown link: [CleanKey Title](zotero://select/items/@key)
            let zotero_link = $"[($clean_key) ($title)](zotero://select/items/@($zotero_key))"

            $zotero_link | pbcopy
            print $"📋 Copied Zotero link to clipboard:"
            print $"   ($zotero_link)"
            print "💡 Paste into your notes - clicking opens Zotero"
        } else {
            print "❌ Could not parse citation format"
        }
    }
}

# Forge semantic search + copy link to clipboard
def fsel [] {
    if ($env.OPENAI_API_KEY? | is-empty) {
        print "❌ OPENAI_API_KEY not set for semantic search"
        return
    }

    print "🧠 Semantic search in your vault..."
    let query = (input "🔍 Search concept: ")
    if ($query | is-empty) {
        return
    }

    print $"🔍 Finding notes related to: ($query)"
    let results = try {
        let output = (^semantic-query --text $query --limit 20 | complete)
        if $output.exit_code == 0 {
            # Filter to only keep lines starting with scores (not the # heading lines)
            $output.stdout | lines | where ($it =~ "^[0-9]\\.")
        } else {
            []
        }
    } catch {
        print "❌ Semantic search failed. Check if semantic-indexer is set up."
        return
    }
    
    if ($results | is-empty) {
        print "❌ No semantic matches found"
        return
    }
    
    let selected = ($results | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'title=$(echo {} | sd "^[0-9.]+[[:space:]]+" ""); file=$(fd -t f --full-path "$title.md" "$FORGE" | head -1); if [ -f "$file" ]; then mdcat --columns 80 "$file"; else echo "Title extracted: [$title]"; echo "Searching for: $title.md"; echo "In vault: $FORGE"; fd -t f "$title.md" "$FORGE"; fi' --preview-window 'right:60%' --prompt "🧠 Semantic: " | str trim)
    if not ($selected | is-empty) {
        # Extract filename from semantic search result
        # The selection is just the first line: "0.45  Title"
        let lines = ($selected | lines)

        # Extract title from the score line (format: "0.45  Title" or "1. 0.45  Title")
        # Strip both list numbers and scores
        let filename = ($lines | get 0 | sd '^\d+\.\s+' '' | sd '^[0-9.]+\s+' '' | str trim)

        let wikilink = $"[[($filename)]]"
        $wikilink | pbcopy
        print $"📋 Copied to clipboard: ($wikilink)"
        print "💡 Paste anywhere with Cmd+V"
    }
}

# Forge semantic search + open in Helix
def fseh [] {
    if ($env.OPENAI_API_KEY? | is-empty) {
        print "❌ OPENAI_API_KEY not set for semantic search"
        return
    }

    print "🧠 Semantic search in your vault..."
    let query = (input "🔍 Search concept: ")
    if ($query | is-empty) {
        return
    }

    print $"🔍 Finding notes related to: ($query)"
    let results = try {
        let output = (^semantic-query --text $query --limit 20 | complete)
        if $output.exit_code == 0 {
            # Filter to only keep lines starting with scores (not the # heading lines)
            $output.stdout | lines | where ($it =~ "^[0-9]\\.")
        } else {
            []
        }
    } catch {
        print "❌ Semantic search failed. Check if semantic-indexer is set up."
        return
    }
    
    if ($results | is-empty) {
        print "❌ No semantic matches found"
        return
    }
    
    let selected = ($results | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview 'title=$(echo {} | sd "^[0-9.]+[[:space:]]+" ""); file=$(fd -t f --full-path "$title.md" "$FORGE" | head -1); if [ -f "$file" ]; then mdcat --columns 80 "$file"; else echo "Title extracted: [$title]"; echo "Searching for: $title.md"; echo "In vault: $FORGE"; fd -t f "$title.md" "$FORGE"; fi' --preview-window 'right:60%' --prompt "🧠 Semantic: " | str trim)
    if not ($selected | is-empty) {
        # Extract filename from semantic search result
        # The selection is just the first line: "0.45  Title" or "1. 0.45  Title"
        # Strip both list numbers and scores
        let lines = ($selected | lines)
        let filename = ($lines | get 0 | sd '^\d+\.\s+' '' | sd '^[0-9.]+\s+' '' | str trim)

        # Find the full path and open in Helix
        let filepath = (fd -t f --full-path $"($filename).md" $env.FORGE | head -1)
        if not ($filepath | is-empty) {
            print $"🚀 Opening ($filename) in Helix..."
            hx $filepath
        } else {
            print $"❌ File not found: ($filename).md"
        }
    }
}

# Forge content search + copy link to clipboard
def fcl [] {
    if not ($env.FORGE? | is-empty) and ($env.FORGE | path exists) {
        let query = (input "🔍 Search content: ")
        if ($query | is-empty) {
            return
        }
        
        print $"🔍 Searching for: ($query)"
        let results = try {
            ^rg -i --type md -l $query $env.FORGE | lines | where $it != ""
        } catch {
            print "❌ Content search failed"
            return
        }
        
        if ($results | is-empty) {
            print "❌ No matches found"
            return
        }
        
        let selected = ($results | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview $"rg --color=always -i -C 3 '($query)' {}" --preview-window 'right:60%' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📄 Content: " | str trim)
        if not ($selected | is-empty) {
            let filename = ($selected | path basename | str replace ".md" "")
            let wikilink = $"[[($filename)]]"
            $wikilink | pbcopy
            print $"📋 Copied to clipboard: ($wikilink)"
            print "💡 Paste anywhere with Cmd+V"
        }
    } else {
        print "❌ FORGE not set or doesn't exist"
    }
}

# File content search + open in Helix
def fcsh [] {
    if not ($env.FORGE? | is-empty) and ($env.FORGE | path exists) {
        let query = (input "🔍 Search content: ")
        if ($query | is-empty) {
            return
        }
        
        print $"🔍 Searching for: ($query)"
        let results = try {
            ^rg -i --type md -l $query $env.FORGE | lines | where $it != ""
        } catch {
            print "❌ Content search failed"
            return
        }
        
        if ($results | is-empty) {
            print "❌ No matches found"
            return
        }
        
        let selected = ($results | str join "\n" | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --preview $"rg --color=always -i -C 3 '($query)' {}" --preview-window 'right:60%' --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📄 Content: " | str trim)
        if not ($selected | is-empty) {
            print $"🚀 Opening ($selected) in Helix..."
            hx $selected
        }
    } else {
        print "❌ FORGE not set or doesn't exist"
    }
}

# File duration processing (universal) - processes activity time spans
def fdur [file_path?: string] {
    let files_to_process = if ($file_path | is-empty) {
        # Process all .md files recursively that contain "t::"
        let activity_files = (ls **/*.md | where type == file | get name | where {|file|
            let content = (try { open $file --raw | str trim } catch { "" })
            ($content | str contains "t::")
        })
        
        if ($activity_files | is-empty) {
            print "❌ No activity files found in current directory or subdirectories"
            print "💡 Usage: fdur [file_path] or run in directory with .md files containing 't::'"
            print $"💡 Current directory: (pwd)"
            return
        }
        
        $activity_files
    } else {
        if not ($file_path | path exists) {
            print $"❌ File not found: ($file_path)"
            print $"💡 Usage: fdur [file_path] or run in directory with activity files"
            return
        }
        [$file_path]
    }
    
    for file in $files_to_process {
        print $"🔄 Processing activity durations in ($file)"
        # Small delay to ensure file system stability
        sleep 100ms
        activity-duration-processor $file
        print $"✅ Activity durations processed in ($file)"
    }
    
    if ($files_to_process | length) > 1 {
        print $"🎉 Processed ($files_to_process | length) files total"
    }
    print $"💡 Remember to reload files in your editor if they're open"
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
alias zj-save = ~/dotfiles/scripts/zellij-sync.nu save
alias zj-restore = ~/dotfiles/scripts/zellij-sync.nu restore
alias zj-remote = ~/dotfiles/scripts/zellij-sync.nu remote

# Quick aliases for common Zellij operations
alias zj-ls = zellij list-sessions
alias zj-kill = zellij kill-session
alias zj-work = zellij --session work

# Smart Zellij launcher (screen-aware layouts)
alias zj = ^zj

# Find project root with intelligent priority-based detection
# Detects: Obsidian vaults, Git repos, Dotter configs, Node.js/Python/Rust projects
# Returns: Record with path, type, priority, and marker information
def find-project-root-enhanced [start_path?: path] {
    let current_dir = if ($start_path == null) { (pwd) } else { $start_path | path expand }
    # Build list of directories to check (current + parents)
    mut dirs_to_check = [$current_dir]
    mut check_dir = $current_dir
    for $i in 1..10 {
        let parent = ($check_dir | path dirname)
        if $parent == $check_dir {
            break
        }
        $dirs_to_check = ($dirs_to_check | append $parent)
        $check_dir = $parent
    }
    
    # Project type detection (ordered by priority)
    let project_types = [
        {
            name: "Obsidian Vault",
            marker: ".obsidian",
            priority: 1
        },
        {
            name: "Git Repository",
            marker: ".git",
            priority: 2
        },
        {
            name: "Dotter Config",
            marker: ".dotter",
            priority: 3
        },
        {
            name: "Node.js Project",
            marker: "package.json",
            priority: 4
        },
        {
            name: "Python Project",
            marker: "pyproject.toml",
            priority: 5
        },
        {
            name: "Rust Project",
            marker: "Cargo.toml",
            priority: 6
        }
    ]
    
    # Find all possible project roots
    let found_projects = ($dirs_to_check | each { |dir|
        let project_info = ($project_types | each { |proj|
            let marker_path = ($dir | path join $proj.marker)
            if ($marker_path | path exists) {
                {
                    path: $dir,
                    type: $proj.name,
                    priority: $proj.priority,
                    marker: $proj.marker
                }
            } else {
                null
            }
        } | where $it != null)
        
        if ($project_info | length) > 0 {
            $project_info.0  # Return first (highest priority) match
        } else {
            null
        }
    } | where $it != null)
    
    # Return the highest priority project root found
    if ($found_projects | length) > 0 {
        $found_projects | sort-by priority | first
    } else {
        {
            path: $current_dir,
            type: "Current Directory",
            priority: 99,
            marker: null
        }
    }
}

# Display rich contextual information about the detected project
# Shows: project type, root path, file counts, git status, package info
def show-project-info [] {
    let project = (find-project-root-enhanced)
    
    print $"📁 Project: ($project.type)"
    print $"📍 Root: ($project.path)"
    
    # Show additional info based on project type
    match $project.type {
        "Obsidian Vault" => {
            let md_count = (do -i { 
                cd $project.path; 
                glob **/*.md | length 
            } | default 0)
            print $"📝 Notes: ($md_count) markdown files"
        },
        "Git Repository" => {
            let branch = (do -i { git -C $project.path branch --show-current } | complete)
            if $branch.exit_code == 0 {
                print $"🔀 Branch: ($branch.stdout | str trim)"
            }
            
            let status = (do -i { git -C $project.path status --porcelain } | complete)
            if $status.exit_code == 0 {
                let changes = ($status.stdout | str trim | lines | length)
                if $changes > 0 {
                    print $"📝 Changes: ($changes) modified files"
                } else {
                    print $"✅ Status: Clean working tree"
                }
            }
        },
        "Node.js Project" => {
            let package_json = ($project.path | path join "package.json")
            if ($package_json | path exists) {
                let pkg_info = (open $package_json | from json)
                if "name" in $pkg_info {
                    print $"📦 Package: ($pkg_info.name)"
                }
                if "version" in $pkg_info {
                    print $"🏷️  Version: ($pkg_info.version)"
                }
            }
        },
        "Python Project" => {
            let pyproject = ($project.path | path join "pyproject.toml")
            if ($pyproject | path exists) {
                print $"🐍 Python project with pyproject.toml"
            }
        },
        "Rust Project" => {
            let cargo = ($project.path | path join "Cargo.toml")
            if ($cargo | path exists) {
                print $"🦀 Rust project with Cargo.toml"
            }
        },
        _ => {
            let files = (ls $project.path | length)
            print $"📂 Files: ($files) items"
        }
    }
    
    print ""  # Empty line for readability
}

# Intelligent Yazi launcher with project-aware navigation
# No args: Opens at detected project root with context info  
# With args: Preserves explicit user intent (e.g., y ~/Downloads)
# Integrates with Alt+l wiki links and floating pane workflow
def --env y [...args] {
    let tmp = (mktemp -t "yazi-cwd.XXXXXX")
    
    # Determine the starting directory
    let start_dir = if ($args | length) > 0 {
        # If arguments provided, use them as-is (preserve explicit user intent)
        null  # Will use args directly
    } else {
        # No args - use intelligent project root detection
        let project = (find-project-root-enhanced)
        
        # Show project context info
        show-project-info
        
        $project.path
    }
    
    # Run yazi with appropriate starting directory
    if $start_dir != null {
        yazi $start_dir --cwd-file $tmp
    } else if ($args | length) > 0 {
        yazi ...$args --cwd-file $tmp
    } else {
        # Fallback (shouldn't happen with our logic above)
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

# Zoxide-powered directory navigation suite (Yazi-like CLI experience)

# Recent directories - show zoxide's frecency-ranked directories interactively
def --env zr [] {
    if (which sk | is-empty) {
        print "sk (skim) not found. Install with: brew install sk"
        return
    }
    
    let selected = (
        zoxide query -l 
        | str trim
        | str join "\n"
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk 
            --preview 'eza --tree --color=always --level=2 {} | head -20' 
            --preview-window 'right:60%' 
            --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' 
            --prompt "📂 Recent Dirs: "
        | str trim
    )
    
    if not ($selected | is-empty) {
        cd $selected
    }
}

# Project directories - smart detection for git/obsidian/dotter projects
def --env zp [] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "sk and fd required. Install with: brew install sk fd"
        return
    }
    
    let selected = (
        fd -H -t d '(\.git|\.obsidian|\.dotter|package\.json|Cargo\.toml)$' ~ --max-depth 4
        | lines
        | each { |p| $p | path dirname }
        | uniq
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk 
            --preview 'eza --tree --color=always --level=2 {} | head -20' 
            --preview-window 'right:60%' 
            --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' 
            --prompt "🚀 Projects: "
        | str trim
    )
    
    if not ($selected | is-empty) {
        cd $selected
    }
}

# Fuzzy find any directory from current location down
def --env zf [path?: string] {
    if (which sk | is-empty) or (which fd | is-empty) {
        print "sk and fd required. Install with: brew install sk fd"
        return
    }
    
    let search_path = if ($path | is-empty) { "." } else { $path }
    
    let selected = (
        fd -t d . $search_path
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk 
            --preview 'eza --tree --color=always --level=2 {} | head -20' 
            --preview-window 'right:60%' 
            --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' 
            --prompt "🔍 Find Dir: "
        | str trim
    )
    
    if not ($selected | is-empty) {
        cd $selected
    }
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

# Note: Zoxide functions (z, zi) are initialized at the top of the file
source ~/.config/nushell/zotero-commands.nu

# Link Manager aliases
alias links-status = link-service status
alias links-logs = link-service logs  
alias links-restart = link-service restart
alias links-test = link-service test

# Semantic Search System - AI-powered note discovery
# Find notes semantically related to current context

def related [...args] {
    # Find notes semantically similar to current file or specified file
    if ($args | is-empty) {
        # Try to find current file context - for now, use current directory
        let current_dir = (pwd)
        let md_files = (ls *.md | get name)
        
        if ($md_files | length) > 0 {
            let first_file = ($md_files | first)
            print $"Finding notes related to: ($first_file | path basename)"
            ^semantic-query --file $first_file
        } else {
            print "No markdown files in current directory. Use: related /path/to/file.md"
        }
    } else {
        let file_path = ($args | first)
        if ($file_path | path exists) {
            print $"Finding notes related to: ($file_path | path basename)"
            ^semantic-query --file $file_path
        } else {
            print $"File not found: ($file_path)"
        }
    }
}

def semantic [query: string, --limit: int = 10] {
    # Find notes by concept description using AI semantic search
    print $"Searching for concept: \"($query)\""
    if $limit != 10 {
        ^semantic-query --text $query --limit $limit
    } else {
        ^semantic-query --text $query
    }
}

def semantic-rebuild [] {
    # Rebuild the entire semantic search index (run after major vault changes)
    print "🔄 Rebuilding semantic search index..."
    print "⚠️  This will process all 6,000+ files and may cost ~$8-15 in OpenAI API usage"
    print "Continue? (y/N)"
    
    let response = (input)
    if ($response | str downcase) == "y" {
        print "🚀 Starting full rebuild. This may take 15-30 minutes..."
        ^semantic-indexer --rebuild
    } else {
        print "❌ Rebuild cancelled"
    }
}

def semantic-update [] {
    # Update semantic search index with only changed files (daily maintenance)
    print "🔄 Updating semantic search index with changed files..."
    ^semantic-indexer --update
}

def semantic-watch [] {
    # Start file watcher for automatic incremental updates
    print "👁️  Starting semantic search file watcher..."
    print "This will monitor your vault and update the index automatically."
    print "Press Ctrl+C to stop."
    ^semantic-indexer --watch
}

def semantic-status [] {
    # Show semantic search system status
    let index_path = $"($env.HOME)/.local/share/semantic-search/db/faiss_index.bin"
    let metadata_path = $"($env.HOME)/.local/share/semantic-search/db/file_metadata.json"
    let config_path = $"($env.HOME)/.local/share/semantic-search/config.yaml"
    
    print "📊 Semantic Search System Status"
    print "─" * 50
    
    if ($index_path | path exists) {
        let index_size = (ls $index_path | get size | first)
        print $"✅ Index exists: ($index_size | into string)"
    } else {
        print "❌ Index not found - run semantic-rebuild first"
    }
    
    if ($metadata_path | path exists) {
        let metadata = (open $metadata_path)
        let file_count = ($metadata.files | length)
        let last_updated = $metadata.last_updated
        print $"📁 Files indexed: ($file_count)"
        print $"🕐 Last updated: ($last_updated)"
    } else {
        print "❌ Metadata not found"
    }
    
    if ($config_path | path exists) {
        print "✅ Configuration found"
    } else {
        print "❌ Configuration missing"
    }
    
    # Check if OpenAI API key is set
    if ($env.OPENAI_API_KEY? | is-empty) {
        print "❌ OPENAI_API_KEY not set"
        print "   Set with: $env.OPENAI_API_KEY = 'your-key-here'"
    } else {
        print "✅ OpenAI API key configured"
    }
}

# Clipboard aliases - force external command execution to prevent Nushell interception
# Cross-platform clipboard aliases
alias pbcopy = cross-platform-clipboard copy
alias pbpaste = cross-platform-clipboard paste


# Helix integration functions (project-aware file opening)
def hx-smart-gf [] {
    let input_text = ($in | str trim)
    
    if ($input_text | is-empty) {
        print "No text provided"
        return
    }
    
    # Get project context
    let project_info = (show-project-info)
    let project_root = $project_info.project_root
    let project_type = $project_info.project_type
    
    # Clean input
    let clean_text = ($input_text | str replace --all '"' "" | str replace --all "'" "" | str trim)
    
    let target_file = if ($clean_text | str starts-with "http") {
        $clean_text
    } else if ($clean_text | str contains "[[") and ($clean_text | str contains "]]") {
        # Handle wiki links
        let wiki_content = ($clean_text | str replace "[[" "" | str replace "]]" "")
        
        if $project_type == "obsidian-vault" {
            let note_path = ($project_root | path join $"($wiki_content).md")
            if ($note_path | path exists) {
                $note_path
            } else {
                let search_results = (try {
                    ^fd -e md -g $"*($wiki_content)*" $project_root | lines | take 1
                } catch { [] })
                
                if ($search_results | length) > 0 {
                    $search_results | first
                } else {
                    $wiki_content
                }
            }
        } else {
            $wiki_content
        }
    } else {
        # Handle file paths
        let potential_paths = [
            ($project_root | path join $clean_text)
            ($env.PWD | path join $clean_text) 
            $clean_text
        ]
        
        let existing_path = ($potential_paths | where {|path| $path | path exists} | first)
        
        if ($existing_path | is-empty) {
            let search_results = (try {
                ^fd -t f -g $"*($clean_text)*" $project_root | lines | take 1
            } catch { [] })
            
            if ($search_results | length) > 0 {
                $search_results | first
            } else {
                $clean_text
            }
        } else {
            $existing_path
        }
    }
    
    # Write result
    $target_file | save -f /tmp/helix-gf-target.md
    print $"Resolved: ($input_text) -> ($target_file)"
}

# Wiki navigation - Universal tool for following wiki links
# Designed for Zellij workflow: Helix in left pane, run this in right pane
# Usage:
#   wiki-nav           - Uses file exported by Space+w, or auto-detects most recent
#   wiki-nav [file]    - Extracts links from specified file
# Hybrid approach: Space+w auto-exports current file for reliable detection
def wiki-nav [file?: string] {
    let vault = $"($env.HOME)/Forge"
    let target_file = if ($file | is-empty) {
        # Layer 1: Check helix-current-link.md symlink (most recent Space+w target)
        if ("/tmp/helix-current-link.md" | path exists) {
            let link_target = (^readlink /tmp/helix-current-link.md | str trim)
            if ($link_target | path exists) {
                print $"📖 Using recent link target: ($link_target | path basename)"
                $link_target
            } else {
                null
            }
        } else {
            null
        }
        | default (
            # Layer 2: Check exported file from Helix (set by Space+w)
            if ("/tmp/helix-current-file.txt" | path exists) {
                let exported_raw = (open /tmp/helix-current-file.txt | str trim)
                if not ($exported_raw | is-empty) {
                    # Ensure absolute path (exported might be relative like "Forge/file.md")
                    let exported = if ($exported_raw | path type) == "file" {
                        $exported_raw | path expand
                    } else {
                        # Relative path - expand from home
                        $"($env.HOME)/($exported_raw)" | path expand
                    }
                    print $"📖 Using exported file: ($exported | path basename)"
                    $exported
                } else {
                    null
                }
            } else {
                null
            }
        )
        | default (
            # Layer 3: Check current directory for .md files - most recently modified
            glob "*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | get 0?.name?
        )
        | default (
            # Layer 4: Fallback to most recently modified file in vault
            glob $"($vault)/**/*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | first | get name
        )
    } else if ($file | path type) == "file" {
        $file
    } else {
        # Treat as wiki link name
        let search_result = (fd -t f --full-path $"($file).md" $vault | lines | first)
        if ($search_result | is-empty) {
            print $"❌ File not found: ($file)"
            return
        }
        $search_result
    }

    # Extract all wiki links from the file (read from disk - always fresh!)
    let links = (open $target_file | rg -o '\[\[([^\]]+)\]\]' --replace '$1' | lines | uniq)

    if ($links | is-empty) {
        print "No wiki links found in file"
        return
    }

    # Let user pick a link with skim (fast fuzzy finding)
    print $"🔗 Found ($links | length) wiki links in ($target_file | path basename)"
    let selected = ($links | to text | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --prompt "Follow link: ")

    if ($selected | is-empty) {
        print "❌ No link selected"
        return
    }

    print $"➡️  Following: ($selected)"

    # Find or create the target file
    let clean_link = ($selected | str replace -r '[#|].*' '' | str trim)

    # Search for existing file by basename only (more reliable than --full-path)
    let existing_results = (fd -t f -e md $"^($clean_link).md$" $vault | lines)
    let existing = if ($existing_results | is-empty) { "" } else { $existing_results | first }

    let next_file = if not ($existing | is-empty) {
        $existing
    } else {
        # Create new file in root of vault (simple approach - no nested paths from link names)
        let new_path = $"($vault)/($clean_link).md"
        let dir = ($new_path | path dirname)
        mkdir $dir

        let today = (date now | format date "%Y-%m-%d")
        let now = (date now | format date "%H:%M")

        # Create file with frontmatter
        $"---
tags:
-
date created: ($today) ($now)
date modified: ($today) ($now)
---
# ($clean_link)



## Backlinks

" | save $new_path

        print $"✨ Created new file: ($clean_link)"
        $new_path
    }

    # Track this navigation in history before opening
    let history_file = "/tmp/wiki-nav-history.txt"

    # Add current file to history (before navigating away)
    if ($target_file | path exists) {
        $"($target_file)\n" | save --raw --append $history_file
    }

    # Open file - context-aware (Zellij or standalone)
    print $"📂 Opening: ($next_file | path basename)"

    if ($env.ZELLIJ? | is-not-empty) {
        # In Zellij: open in current editor instance (Helix assumed)
        hx $next_file
    } else {
        # Standalone terminal: launch $EDITOR
        let editor = ($env.EDITOR? | default "hx")
        ^$editor $next_file
    }
}

# Wiki navigation - right pane (creates new pane to the right)
# Usage: wiki-nav-right (or wnr)
# Opens selected link in a NEW pane to the right for rightward exploration chains
def wiki-nav-right [file?: string] {
    let vault = $"($env.HOME)/Forge"
    let target_file = if ($file | is-empty) {
        # Layer 1: Check helix-current-link.md symlink (most recent Space+w target)
        if ("/tmp/helix-current-link.md" | path exists) {
            let link_target = (^readlink /tmp/helix-current-link.md | str trim)
            if ($link_target | path exists) {
                print $"📖 Using recent link target: ($link_target | path basename)"
                $link_target
            } else {
                null
            }
        } else {
            null
        }
        | default (
            # Layer 2: Check exported file from Helix (set by Space+w)
            if ("/tmp/helix-current-file.txt" | path exists) {
                let exported_raw = (open /tmp/helix-current-file.txt | str trim)
                if not ($exported_raw | is-empty) {
                    # Ensure absolute path (exported might be relative like "Forge/file.md")
                    let exported = if ($exported_raw | path type) == "file" {
                        $exported_raw | path expand
                    } else {
                        # Relative path - expand from home
                        $"($env.HOME)/($exported_raw)" | path expand
                    }
                    print $"📖 Using exported file: ($exported | path basename)"
                    $exported
                } else {
                    null
                }
            } else {
                null
            }
        )
        | default (
            # Layer 3: Check current directory for .md files - most recently modified
            glob "*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | get 0?.name?
        )
        | default (
            # Layer 4: Fallback to most recently modified file in vault
            glob $"($vault)/**/*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | first | get name
        )
    } else if ($file | path type) == "file" {
        $file
    } else {
        # Treat as wiki link name
        let search_result = (fd -t f --full-path $"($file).md" $vault | lines | first)
        if ($search_result | is-empty) {
            print $"❌ File not found: ($file)"
            return
        }
        $search_result
    }

    # Extract all wiki links from the file (read from disk - always fresh!)
    let links = (open $target_file | rg -o '\[\[([^\]]+)\]\]' --replace '$1' | lines | uniq)

    if ($links | is-empty) {
        print "No wiki links found in file"
        return
    }

    # Let user pick a link with skim (fast fuzzy finding)
    print $"🔗 Found ($links | length) wiki links in ($target_file | path basename)"
    let selected = ($links | to text | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --prompt "Follow link: ")

    if ($selected | is-empty) {
        print "❌ No link selected"
        return
    }

    print $"➡️  Following: ($selected)"

    # Find or create the target file
    let clean_link = ($selected | str replace -r '[#|].*' '')
    let existing_results = (fd -t f --full-path $"($clean_link).md" $vault | lines)
    let existing = if ($existing_results | is-empty) { "" } else { $existing_results | first }

    let next_file = if not ($existing | is-empty) {
        $existing
    } else {
        # Create new file in appropriate location
        let new_path = $"($vault)/($clean_link).md"
        let dir = ($new_path | path dirname)
        mkdir $dir

        let today = (date now | format date "%Y-%m-%d")
        let now = (date now | format date "%H:%M")

        # Create file with frontmatter
        $"---
tags:
-
date created: ($today) ($now)
date modified: ($today) ($now)
---
# ($clean_link)



## Backlinks

" | save $new_path

        print $"✨ Created new file: ($clean_link)"
        $new_path
    }

    # Track this navigation in history before opening
    let history_file = "/tmp/wiki-nav-history.txt"

    # Add current file to history (before navigating away)
    if ($target_file | path exists) {
        $"($target_file)\n" | save --raw --append $history_file
    }

    # Create new pane to the right and open file there
    print $"📂 Opening in new pane to the right: ($next_file | path basename)"

    # Create new pane to the right, then send hx command to it
    zellij action new-pane --direction right
    sleep 0.2sec
    zellij action write-chars $"hx ($next_file)"
    zellij action write 13  # ENTER key
}

alias wnr = wiki-nav-right

# Wiki navigation - new tab (creates new tab with full screen width)
# Usage: wiki-nav-tab (or wnt)
# Opens selected link in a NEW tab for deep exploration with full width
def wiki-nav-tab [file?: string] {
    let vault = $"($env.HOME)/Forge"
    let target_file = if ($file | is-empty) {
        # Layer 1: Check helix-current-link.md symlink (most recent Space+w target)
        if ("/tmp/helix-current-link.md" | path exists) {
            let link_target = (^readlink /tmp/helix-current-link.md | str trim)
            if ($link_target | path exists) {
                print $"📖 Using recent link target: ($link_target | path basename)"
                $link_target
            } else {
                null
            }
        } else {
            null
        }
        | default (
            # Layer 2: Check exported file from Helix (set by Space+w)
            if ("/tmp/helix-current-file.txt" | path exists) {
                let exported_raw = (open /tmp/helix-current-file.txt | str trim)
                if not ($exported_raw | is-empty) {
                    # Ensure absolute path (exported might be relative like "Forge/file.md")
                    let exported = if ($exported_raw | path type) == "file" {
                        $exported_raw | path expand
                    } else {
                        # Relative path - expand from home
                        $"($env.HOME)/($exported_raw)" | path expand
                    }
                    print $"📖 Using exported file: ($exported | path basename)"
                    $exported
                } else {
                    null
                }
            } else {
                null
            }
        )
        | default (
            # Layer 3: Check current directory for .md files - most recently modified
            glob "*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | get 0?.name?
        )
        | default (
            # Layer 4: Fallback to most recently modified file in vault
            glob $"($vault)/**/*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | first | get name
        )
    } else if ($file | path type) == "file" {
        $file
    } else {
        # Treat as wiki link name
        let search_result = (fd -t f --full-path $"($file).md" $vault | lines | first)
        if ($search_result | is-empty) {
            print $"❌ File not found: ($file)"
            return
        }
        $search_result
    }

    # Extract all wiki links from the file (read from disk - always fresh!)
    let links = (open $target_file | rg -o '\[\[([^\]]+)\]\]' --replace '$1' | lines | uniq)

    if ($links | is-empty) {
        print "No wiki links found in file"
        return
    }

    # Let user pick a link with skim (fast fuzzy finding)
    print $"🔗 Found ($links | length) wiki links in ($target_file | path basename)"
    let selected = ($links | to text | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --prompt "Follow link: ")

    if ($selected | is-empty) {
        print "❌ No link selected"
        return
    }

    print $"➡️  Following: ($selected)"

    # Find or create the target file
    let clean_link = ($selected | str replace -r '[#|].*' '')
    let existing_results = (fd -t f --full-path $"($clean_link).md" $vault | lines)
    let existing = if ($existing_results | is-empty) { "" } else { $existing_results | first }

    let next_file = if not ($existing | is-empty) {
        $existing
    } else {
        # Create new file in appropriate location
        let new_path = $"($vault)/($clean_link).md"
        let dir = ($new_path | path dirname)
        mkdir $dir

        let today = (date now | format date "%Y-%m-%d")
        let now = (date now | format date "%H:%M")

        # Create file with frontmatter
        $"---
tags:
-
date created: ($today) ($now)
date modified: ($today) ($now)
---
# ($clean_link)



## Backlinks

" | save $new_path

        print $"✨ Created new file: ($clean_link)"
        $new_path
    }

    # Track this navigation in history before opening
    let history_file = "/tmp/wiki-nav-history.txt"

    # Add current file to history (before navigating away)
    if ($target_file | path exists) {
        $"($target_file)\n" | save --raw --append $history_file
    }

    # Create new tab and open file there
    print $"📂 Opening in new tab: ($next_file | path basename)"

    # Create new tab, then send hx command to it
    zellij action new-tab
    sleep 0.2sec
    zellij action write-chars $"hx ($next_file)"
    zellij action write 13  # ENTER key
}

alias wnt = wiki-nav-tab

# Wiki navigation - floating pane (creates overlay window)
# Usage: wiki-nav-floating (or wnf)
# Opens selected link in a FLOATING pane for quick preview without layout disruption
def wiki-nav-floating [file?: string] {
    let vault = $"($env.HOME)/Forge"
    let target_file = if ($file | is-empty) {
        # Layer 1: Check helix-current-link.md symlink (most recent Space+w target)
        if ("/tmp/helix-current-link.md" | path exists) {
            let link_target = (^readlink /tmp/helix-current-link.md | str trim)
            if ($link_target | path exists) {
                print $"📖 Using recent link target: ($link_target | path basename)"
                $link_target
            } else {
                null
            }
        } else {
            null
        }
        | default (
            # Layer 2: Check exported file from Helix (set by Space+w)
            if ("/tmp/helix-current-file.txt" | path exists) {
                let exported_raw = (open /tmp/helix-current-file.txt | str trim)
                if not ($exported_raw | is-empty) {
                    # Ensure absolute path (exported might be relative like "Forge/file.md")
                    let exported = if ($exported_raw | path type) == "file" {
                        $exported_raw | path expand
                    } else {
                        # Relative path - expand from home
                        $"($env.HOME)/($exported_raw)" | path expand
                    }
                    print $"📖 Using exported file: ($exported | path basename)"
                    $exported
                } else {
                    null
                }
            } else {
                null
            }
        )
        | default (
            # Layer 3: Check current directory for .md files - most recently modified
            glob "*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | get 0?.name?
        )
        | default (
            # Layer 4: Fallback to most recently modified file in vault
            glob $"($vault)/**/*.md" | each { |f| {name: $f, modified: (ls $f | get modified | first)} } | sort-by modified -r | first | get name
        )
    } else if ($file | path type) == "file" {
        $file
    } else {
        # Treat as wiki link name
        let search_result = (fd -t f --full-path $"($file).md" $vault | lines | first)
        if ($search_result | is-empty) {
            print $"❌ File not found: ($file)"
            return
        }
        $search_result
    }

    # Extract all wiki links from the file (read from disk - always fresh!)
    let links = (open $target_file | rg -o '\[\[([^\]]+)\]\]' --replace '$1' | lines | uniq)

    if ($links | is-empty) {
        print "No wiki links found in file"
        return
    }

    # Let user pick a link with skim (fast fuzzy finding)
    print $"🔗 Found ($links | length) wiki links in ($target_file | path basename)"
    let selected = ($links | to text | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --prompt "Follow link: ")

    if ($selected | is-empty) {
        print "❌ No link selected"
        return
    }

    print $"➡️  Following: ($selected)"

    # Find or create the target file
    let clean_link = ($selected | str replace -r '[#|].*' '')
    let existing_results = (fd -t f --full-path $"($clean_link).md" $vault | lines)
    let existing = if ($existing_results | is-empty) { "" } else { $existing_results | first }

    let next_file = if not ($existing | is-empty) {
        $existing
    } else {
        # Create new file in appropriate location
        let new_path = $"($vault)/($clean_link).md"
        let dir = ($new_path | path dirname)
        mkdir $dir

        let today = (date now | format date "%Y-%m-%d")
        let now = (date now | format date "%H:%M")

        # Create file with frontmatter
        $"---
tags:
-
date created: ($today) ($now)
date modified: ($today) ($now)
---
# ($clean_link)



## Backlinks

" | save $new_path

        print $"✨ Created new file: ($clean_link)"
        $new_path
    }

    # Track this navigation in history before opening
    let history_file = "/tmp/wiki-nav-history.txt"

    # Add current file to history (before navigating away)
    if ($target_file | path exists) {
        $"($target_file)\n" | save --raw --append $history_file
    }

    # Create floating pane and open file there
    print $"📂 Opening in floating pane: ($next_file | path basename)"

    # Create floating pane (80% width, 80% height) and run hx command directly
    zellij action new-pane --floating --width "80%" --height "80%" -- hx ($next_file)
}

alias wnf = wiki-nav-floating

# Wiki back - Navigate backwards through history
# Usage: wiki-back (or wb)
# Sends :open command directly to Helix pane via WezTerm CLI
def wiki-back [] {
    let history_file = "/tmp/wiki-nav-history.txt"

    if not ($history_file | path exists) {
        print "❌ No navigation history yet"
        return
    }

    let history = (open $history_file | lines)

    if ($history | is-empty) {
        print "❌ No previous files in history"
        return
    }

    # Get the last file from history
    let previous_file_raw = ($history | last)

    # Ensure we have an absolute path
    let previous_file = if ($previous_file_raw | path type) == "file" {
        $previous_file_raw | path expand
    } else {
        # If not absolute, make it relative to home/Forge
        $"($env.HOME)/($previous_file_raw)"
    }

    # Remove the last line from history
    let new_history = ($history | drop)

    if ($new_history | is-empty) {
        # If history is now empty, delete the file
        rm $history_file
    } else {
        # Save updated history
        $new_history | str join "\n" | save -f $history_file
    }

    # Open previous file - context-aware (Zellij or standalone)
    print $"⬅️  Going back to: ($previous_file | path basename)"

    if ($env.ZELLIJ? | is-not-empty) {
        # In Zellij: Send :open command to Helix pane via Zellij
        # Move focus to left pane (where Helix is), send command, then return focus
        zellij action move-focus left

        # Press ESC to ensure we're in normal mode, then type the command
        zellij action write 27  # ESC key
        sleep 0.1sec

        # Build the :open command with absolute path
        let open_cmd = $":open ($previous_file)"
        zellij action write-chars $open_cmd
        zellij action write 13  # ENTER key
        sleep 0.1sec

        zellij action move-focus right
    } else {
        # Standalone terminal: launch $EDITOR
        let editor = ($env.EDITOR? | default "hx")
        ^$editor $previous_file
    }
}

# File operations - Toggle todo checkbox on cursor line
# Usage: ftodo  (toggles the line where cursor currently is in most recent file)
# Requires: hx-toggle-todo script (processes entire file, toggles line under cursor)
# Toggle todo checkbox - Interactive picker with context detection
# Usage:
#   ftodo           - Auto-detect file, show picker to select line to toggle
#   ftodo file.md   - Show picker for specific file
# Interactive: presents all lines, select which to toggle (like wn for wiki links)
def ftodo [file?: string] {
    let vault = $"($env.HOME)/Forge"

    let target_file = if ($file | is-empty) {
        # Layer 1: Recent Helix action (exported by Space+w)
        if ("/tmp/helix-current-file.txt" | path exists) {
            let exported_raw = (open /tmp/helix-current-file.txt | str trim)
            if not ($exported_raw | is-empty) {
                let exported = if ($exported_raw | path type) == "file" {
                    $exported_raw | path expand
                } else {
                    $"($env.HOME)/($exported_raw)" | path expand
                }
                print $"📖 Using Helix context: ($exported | path basename)"
                $exported
            } else {
                null
            }
        } else {
            null
        }
        | default (
            # Layer 2: Current directory most recent .md
            glob "*.md"
            | each {|f| {name: $f, mod: (ls $f | get modified | first)}}
            | sort-by mod -r
            | get 0?.name?
        )
        | default (
            # Layer 3: Vault most recent .md
            fd -t f -e md . $vault
            | lines
            | each {|f| {file: $f, modified: (ls $f | get modified | first)}}
            | sort-by modified -r
            | first
            | get file?
        )
    } else {
        $file
    }

    if ($target_file | is-empty) {
        print "❌ No markdown files found"
        return
    }

    print $"📝 Select line to toggle in: ($target_file | path basename)"

    # Read file with line numbers
    let lines_with_numbers = (
        open $target_file
        | lines
        | enumerate
        | each {|item| $"($item.index + 1): ($item.item)"}
    )

    if ($lines_with_numbers | is-empty) {
        print "❌ File is empty"
        return
    }

    # Use skim to select line (with TERM override for compatibility)
    let selected = (
        $lines_with_numbers
        | str join "\n"
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --prompt "Select line to toggle> "
    )

    if ($selected | is-empty) {
        print "❌ No selection made"
        return
    }

    # Extract line number from selection (format: "123: content")
    let line_num = ($selected | parse "{num}: {content}" | get num.0 | into int)

    # Read all lines
    let all_lines = (open $target_file | lines)

    # Get the specific line (0-indexed, so subtract 1)
    let original_line = ($all_lines | get ($line_num - 1))

    # Show current line and prompt for desired state
    print $"\n📝 Current line ($line_num): ($original_line)"
    print "\nSelect desired state:"
    print "  1 - Plain text (no markers)"
    print "  2 - Unchecked todo: - [ ] text"
    print "  3 - Checked todo: - [x] text"
    print "  4 - Plain list item: - text"
    print "  5 - Cancel (no change)"
    print -n "\nEnter choice (1-5): "

    let choice = (input)

    let new_line = match $choice {
        "1" => {
            # Plain text - remove all markers
            $original_line | sd '^[[:space:]]*[-\*][[:space:]]*\[[x ]\][[:space:]]*' '' | sd '^[[:space:]]*[-\*][[:space:]]*' ''
        }
        "2" => {
            # Unchecked todo
            let stripped = ($original_line | sd '^[[:space:]]*[-\*][[:space:]]*\[[x ]\][[:space:]]*' '' | sd '^[[:space:]]*[-\*][[:space:]]*' '')
            $"- [ ] ($stripped)"
        }
        "3" => {
            # Checked todo
            let stripped = ($original_line | sd '^[[:space:]]*[-\*][[:space:]]*\[[x ]\][[:space:]]*' '' | sd '^[[:space:]]*[-\*][[:space:]]*' '')
            $"- [x] ($stripped)"
        }
        "4" => {
            # Plain list item
            let stripped = ($original_line | sd '^[[:space:]]*[-\*][[:space:]]*\[[x ]\][[:space:]]*' '' | sd '^[[:space:]]*[-\*][[:space:]]*' '')
            $"- ($stripped)"
        }
        "5" => {
            print "❌ Cancelled"
            return
        }
        _ => {
            print "❌ Invalid choice, no changes made"
            return
        }
    }

    # Rebuild file with new line
    let new_content = (
        $all_lines
        | enumerate
        | each {|item|
            if $item.index == ($line_num - 1) {
                $new_line
            } else {
                $item.item
            }
        }
        | str join "\n"
    )

    # Save back to file
    $new_content | save -f $target_file

    print $"✅ Updated line ($line_num): ($new_line)"
}

# Mark file as revisited - Universal tool with context detection
# Usage:
#   fmark           - Auto-detect file and mark as revisited today
#   fmark file.md   - Mark specific file as revisited
# Essential for scripting: glob "reading-list/*.md" | each {|f| fmark $f}
def fmark [file?: string] {
    let vault = $"($env.HOME)/Forge"

    let target_file = if ($file | is-empty) {
        # Layer 1: Recent Helix action (exported by Space+w)
        if ("/tmp/helix-current-file.txt" | path exists) {
            let exported_raw = (open /tmp/helix-current-file.txt | str trim)
            if not ($exported_raw | is-empty) {
                let exported = if ($exported_raw | path type) == "file" {
                    $exported_raw | path expand
                } else {
                    $"($env.HOME)/($exported_raw)" | path expand
                }
                print $"📖 Using Helix context: ($exported | path basename)"
                $exported
            } else {
                null
            }
        } else {
            null
        }
        | default (
            # Layer 2: Current directory most recent .md
            glob "*.md"
            | each {|f| {name: $f, mod: (ls $f | get modified | first)}}
            | sort-by mod -r
            | get 0?.name?
        )
        | default (
            # Layer 3: Vault most recent .md
            fd -t f -e md . $vault
            | lines
            | each {|f| {file: $f, modified: (ls $f | get modified | first)}}
            | sort-by modified -r
            | first
            | get file?
        )
    } else {
        $file
    }

    if ($target_file | is-empty) {
        print "❌ No markdown files found"
        return
    }

    print $"📅 Marking as revisited: ($target_file | path basename)"

    # Call existing hx-mark-revisit script
    ^hx-mark-revisit $target_file
    print "✅ Done"
}

# Date stamp - Output current date for piping/insertion
# Usage:
#   dstamp                    - Output: 2025-10-03
#   dstamp | pbcopy          - Copy to clipboard
#   echo (dstamp) >> file.md - Append to file
def dstamp [] {
    date now | format date "%Y-%m-%d"
}

# Time stamp - Output current time for piping/insertion
# Usage:
#   tstamp                    - Output: 14:30
#   tstamp | pbcopy          - Copy to clipboard
#   echo (tstamp) >> file.md - Append to file
def tstamp [] {
    date now | format date "%H:%M"
}

# DateTime stamp - Output current date and time for piping/insertion
# Usage:
#   dtstamp                      - Output: 2025-10-03 14:30
#   dtstamp | pbcopy            - Copy to clipboard
#   echo (dtstamp) >> file.md   - Append to file
def dtstamp [] {
    date now | format date "%Y-%m-%d %H:%M"
}

# ═══════════════════════════════════════════════════════════════════════════
# Wiki Navigation Aliases - Quick access
# ═══════════════════════════════════════════════════════════════════════════

alias wn = wiki-nav        # Navigate to wiki links from current file
alias wb = wiki-back       # Go back in navigation history
alias ft = ftodo           # Toggle todo checkbox
