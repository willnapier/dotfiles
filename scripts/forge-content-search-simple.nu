#!/usr/bin/env nu
# Simple content search with wikilink output
# Streamlined for Zellij floating pane usage

# Set terminal compatibility
$env.TERM = "xterm"

def main [] {
    print "🔍 Content Search - Enter search terms:"
    print ""
    
    # Get search query from user
    let query = (input "> ")
    
    if ($query | str trim | is-empty) {
        print "No search terms provided."
        exit 0
    }
    
    print $"Searching for: ($query)"
    
    # Use ripgrep to search vault content
    let vault = "/Users/williamnapier/Forge"
    
    # Search with ripgrep and format for skim
    let results = (
        rg --type md --files-with-matches --follow $query $vault
        | lines
        | each {|file| 
            let relative = ($file | str replace $vault "" | str trim --left --char "/")
            let preview = (rg --type md -C 1 $query $file | head -3 | str join " ... ")
            $"($relative): ($preview)"
        }
    )
    
    if ($results | is-empty) {
        print $"No matches found for '($query)'"
        exit 0
    }
    
    print $"Found ($results | length) files"
    
    # Use skim for selection
    let selection = ($results | to text | sk --prompt="Select file: ")
    
    if ($selection | is-empty) {
        exit 0
    }
    
    # Extract filename and create wikilink
    let filename = ($selection | split row ":" | get 0 | path basename | str replace ".md" "")
    let wikilink = $"[[($filename)]]"
    
    print $wikilink
    
    # Copy to clipboard
    $wikilink | pbcopy
    print "📋 Copied to clipboard!"
    sleep 1sec
}

main