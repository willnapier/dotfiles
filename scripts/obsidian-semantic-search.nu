#!/usr/bin/env nu
# obsidian-semantic-search.nu - Semantic search with wikilink output  
# Alt+e: Search concepts/ideas and get pasteable wikilinks
# Part of the Re-Nu/Oxidise initiative - Nushell + existing semantic search

# Terminal compatibility for skim
$env.TERM = "xterm"
$env.RUST_BACKTRACE = "0"

# Check if semantic search is available and configured
def check-semantic-setup [] {
    let semantic_dir = ($env.HOME | path join ".local/share/semantic-search")
    let venv_python = ($semantic_dir | path join "venv/bin/python3")
    
    if not ($semantic_dir | path exists) {
        return {status: "error", message: "Semantic search not installed"}
    }
    
    if not ($venv_python | path exists) {
        return {status: "error", message: "Virtual environment not found. Run: semantic-indexer --rebuild"}
    }
    
    if ($env.OPENAI_API_KEY? | is-empty) {
        return {status: "error", message: "OPENAI_API_KEY not set. Export your OpenAI API key."}
    }
    
    return {status: "ok", message: "Semantic search ready"}
}

# Run semantic query and parse results
def run-semantic-query [query: string] {
    print $"🧠 Searching semantically for: '($query)'"
    print ""
    
    # Run semantic-query and capture output
    let results = try {
        semantic-query $query
        | complete
    } catch {
        return {status: "error", message: "Semantic query failed"}
    }
    
    if $results.exit_code != 0 {
        return {status: "error", message: $"Semantic query error: ($results.stderr)"}
    }
    
    # Parse semantic query output 
    # Expected format: filename.md (similarity: 0.xx) - preview text
    let parsed_results = (
        $results.stdout
        | lines
        | where {|line| $line | str contains ".md"}
        | where {|line| $line | str contains "similarity:"}
        | each {|line|
            # Extract filename, similarity, and preview
            let parts = ($line | parse --regex '(.+\.md)\s+\(similarity:\s+([\d\.]+)\)\s*-?\s*(.*)')
            if ($parts | length) > 0 {
                let parsed = ($parts | get 0)
                {
                    file: ($parsed.capture0 | str trim)
                    similarity: ($parsed.capture1 | into float)
                    preview: ($parsed.capture2 | str trim | str substring 0..100)
                }
            }
        }
        | where {|item| $item.file | is-not-empty}
    )
    
    if ($parsed_results | is-empty) {
        return {status: "empty", message: $"No semantic matches found for '($query)'"}
    }
    
    return {status: "ok", results: $parsed_results}
}

# Format results for skim selection
def format-for-selection [results: list] {
    $results
    | each {|item|
        let similarity_pct = ($item.similarity * 100 | math round)
        $"($item.file) \(($similarity_pct)% match\): ($item.preview)..."
    }
}

# Preview function for skim
def preview-semantic-file [selection: string] {
    let filename = ($selection | split row " " | get 0)
    let vault_path = "/Users/williamnapier/Obsidian.nosync/Forge"
    let full_path = ($vault_path | path join $filename)
    
    if ($full_path | path exists) {
        # Use bat if available, otherwise head
        if (which bat | is-not-empty) {
            bat --style=plain --color=always --line-range=:25 $full_path
        } else {
            open $full_path | lines | first 25 | str join "\n"
        }
    } else {
        $"📄 File: ($filename)\n📁 Path: ($full_path)\n❌ File not found"
    }
}

# Main function
def main [query?: string] {
    print "🧠 Semantic Search with Wikilink Output"
    print "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    
    # Check setup
    let setup_check = (check-semantic-setup)
    if $setup_check.status != "ok" {
        print $"❌ ($setup_check.message)"
        exit 1
    }
    
    print "✅ Semantic search system ready"
    print ""
    
    # Get search query
    let search_query = if ($query | is-not-empty) {
        $query
    } else {
        input "🧠 Enter conceptual search terms: "
    }
    
    if ($search_query | str trim | is-empty) {
        print "❌ No search query provided"
        exit 1
    }
    
    # Run semantic search
    let search_result = (run-semantic-query $search_query)
    
    if $search_result.status == "error" {
        print $"❌ ($search_result.message)"
        exit 1
    }
    
    if $search_result.status == "empty" {
        print $search_result.message
        print "💡 Try broader or different conceptual terms"
        exit 0
    }
    
    let results = $search_result.results
    print $"📝 Found ($results | length) semantically similar files"
    print ""
    
    # Format for skim selection
    let formatted_results = (format-for-selection $results)
    
    # Interactive selection with skim
    let selection = (
        $formatted_results
        | to text
        | sk --prompt="🧠 Select file to link: "
           --preview="nu /Users/williamnapier/.local/bin/obsidian-semantic-search.nu preview '{}'"
           --preview-window="right:60%"
           --bind="enter:accept"
           --header="Semantic search results - Enter=select and copy wikilink, Esc=cancel"
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
    
    let similarity = ($results | where {|r| $r.file == $selected_file} | get similarity | get 0)
    let similarity_pct = ($similarity * 100 | math round)
    
    print $"📋 Copied to clipboard! Semantic similarity: ($similarity_pct)%"
    print "💡 You can now paste with Cmd+V"
    sleep 1.5sec
}

# Main execution  
main