#!/usr/bin/env nu
# obsidian-content-search.nu - Content search with wikilink output
# Alt+s: Search file contents and get pasteable wikilinks
# Part of the Re-Nu/Oxidise initiative - pure Nushell + Rust tools

# Terminal compatibility for skim
$env.TERM = "xterm"
$env.RUST_BACKTRACE = "0"

# Detect Obsidian vault - use direct path for reliability in Zellij context
print "🔍 Detecting Forge..."

# Check known vault locations directly
let possible_vaults = [
    "/Users/williamnapier/Forge"
    "/Users/williamnapier/Obsidian/Forge" 
    ($env.HOME | path join "Documents/Obsidian/Forge")
]

let vault_path = (
    $possible_vaults 
    | where {|vault| ($vault | path join ".obsidian" | path exists)} 
    | first 
    | default ""
)

if ($vault_path | is-empty) {
    print "❌ No Obsidian vault found"
    print "📝 Searched from current location and checked default vaults"
    print "💡 Create a .obsidian folder to mark a vault root"
    exit 1
}

print $"✅ Found Obsidian vault: ($vault_path)"

# Content search function using ripgrep (Rust tool)
def search-content [query: string, vault: string] {
    print $"🔍 Searching for: '($query)'"
    print ""
    
    # Use ripgrep to search content with file context
    let search_results = (
        rg --type md 
           --line-number 
           --with-filename 
           --context 2
           --color never
           --follow  # Follow symlinks (like Claude-Docs)
           --exclude-dir ".obsidian"
           --exclude-dir "linked_media" 
           --exclude-dir ".trash"
           $query $vault
        | lines 
        | parse "{file}:{line}:{content}"
        | group-by file
        | transpose file matches
        | each {|group| 
            {
                file: ($group.file | path relative-to $vault)
                match_count: ($group.matches | length)
                preview: ($group.matches | get content | str join " ... ")
            }
        }
    )
    
    if ($search_results | is-empty) {
        print $"❌ No matches found for '($query)'"
        exit 0
    }
    
    print $"📝 Found ($search_results | length) files with matches"
    
    # Format for skim selection
    $search_results 
    | each {|item| 
        $"($item.file) \(($item.match_count) matches\): ($item.preview | str substring 0..100)..."
    }
}

# Preview function for skim
def preview-file [file_line: string, vault: string] {
    let file = ($file_line | split row " " | get 0)
    let full_path = ($vault | path join $file)
    
    if ($full_path | path exists) {
        # Use bat if available, otherwise head
        if (which bat | is-not-empty) {
            bat --style=plain --color=always --line-range=:20 $full_path
        } else {
            open $full_path | lines | first 20 | str join "\n"
        }
    } else {
        $"📄 File: ($file)\n📁 Path: ($full_path)"
    }
}

# Interactive search using skim (Rust tool)
def main [query?: string] {
    let vault = $vault_path
    
    # Get search query
    let search_query = if ($query | is-not-empty) {
        $query
    } else {
        input "🔍 Enter search terms: "
    }
    
    if ($search_query | str trim | is-empty) {
        print "❌ No search query provided"
        exit 1
    }
    
    # Perform content search
    let results = (search-content $search_query $vault)
    
    # Interactive selection with skim
    let selection = (
        $results 
        | to text
        | sk --prompt="📝 Select file to link: "
           --preview=$"nu ($env.FILE_PWD)/obsidian-content-search.nu preview '{}' ($vault)"
           --preview-window="right:60%"
           --bind="enter:accept"
           --header="Enter=select and copy wikilink, Esc=cancel"
    )
    
    if ($selection | is-empty) {
        print "❌ No selection made"
        exit 0
    }
    
    # Extract filename from selection
    let selected_file = ($selection | split row " " | get 0)
    let filename = ($selected_file | path basename | str replace ".md" "")
    let wikilink = $"[[($filename)]]"
    
    # Output the wikilink
    print $wikilink
    
    # Copy to clipboard (cross-platform)
    if (which pbcopy | is-not-empty) {
        # macOS
        $wikilink | save --raw /dev/stdout | pbcopy
    } else if (which wl-copy | is-not-empty) {
        # Wayland Linux
        $wikilink | wl-copy
    } else if (which xclip | is-not-empty) {
        # X11 Linux
        $wikilink | xclip -selection clipboard
    }
    
    print "📋 Copied to clipboard! You can now paste with Cmd+V"
    sleep 1.5sec
}

# Main execution
main