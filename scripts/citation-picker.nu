#!/usr/bin/env nu

# Citation Picker - Native Nushell Re-nu Version
# Academic citation key picker for journal notation with structured data processing
# Re-nu Phase 2: Productivity Script Excellence
# Created: 2025-09-09

# Native Nushell vault detection with structured metadata
export def find-forge-directory [] {
    let forge_candidates = [
        $"($env.HOME)/Obsidian.nosync/Forge",
        $"($env.HOME)/Obsidian/Forge", 
        $"($env.HOME)/Documents/Forge",
        $"($env.HOME)/Forge"
    ]
    
    for candidate in $forge_candidates {
        if ($candidate | path exists) {
            return {
                path: $candidate,
                type: "found",
                method: "standard_search"
            }
        }
    }
    
    # Try finding from current working directory
    let cwd_forge = (find_forge_from_cwd)
    if $cwd_forge.type == "found" {
        return $cwd_forge
    }
    
    return {
        path: "",
        type: "not_found", 
        method: "exhaustive_search",
        error: "No Forge directory found"
    }
}

def find_forge_from_cwd [] {
    let current_dir = (pwd)
    mut check_dir = $current_dir
    
    # Walk up directory tree looking for Forge indicators
    loop {
        let forge_indicators = [
            ($check_dir | path join ".obsidian"),
            ($check_dir | path join "NapierianLogs"),
            ($check_dir | path join "Areas"),
            ($check_dir | path join "ZET")
        ]
        
        for indicator in $forge_indicators {
            if ($indicator | path exists) {
                return {
                    path: $check_dir,
                    type: "found",
                    method: "cwd_traversal"
                }
            }
        }
        
        let parent = ($check_dir | path dirname)
        if $parent == $check_dir {
            break
        }
        $check_dir = $parent
    }
    
    return {
        path: "",
        type: "not_found",
        method: "cwd_traversal"
    }
}

# Native Nushell citation file discovery with structured data
export def find-citations-file [vault_path: string] {
    let citation_candidates = [
        ($vault_path | path join "ZET" "citations.md"),
        ($vault_path | path join "citations.md"),
        ($vault_path | path join "references" "citations.md"),
        ($vault_path | path join "References" "citations.md")
    ]
    
    for candidate in $citation_candidates {
        if ($candidate | path exists) {
            return {
                path: $candidate,
                type: "found",
                location: ($candidate | str replace $vault_path "" | str trim --left --char "/")
            }
        }
    }
    
    return {
        path: "",
        type: "not_found",
        error: "No citations.md file found"
    }
}

# Native Nushell citation parsing with structured metadata
export def parse-citations [citations_file: string] {
    print "🔍 Loading citations database..."
    
    let raw_content = try { 
        open $citations_file --raw | lines 
    } catch {
        return {
            citations: [],
            error: "Failed to read citations file"
        }
    }
    
    let citations_data = (
        $raw_content
        | where {|it| not ($it | str starts-with "#")}  # Remove headers
        | where {|it| not ($it | str trim | is-empty)}  # Remove empty lines  
        | where {|it| not ($it | str contains "Total entries:")}  # Remove summary lines
        | each {|line|
            let parts = ($line | split row " ")
            let citation_key = ($parts | first)
            let full_text = $line
            
            # Extract basic metadata from citation text
            let has_author = ($line | str contains ",")
            let has_year = ($line =~ '\d{4}')
            let word_count = ($parts | length)
            
            {
                citation_key: $citation_key,
                full_text: $full_text,
                has_author: $has_author,
                has_year: $has_year,
                word_count: $word_count,
                length: ($line | str length),
                category: (categorize_citation $line)
            }
        }
    )
    
    return {
        citations: $citations_data,
        count: ($citations_data | length),
        error: null
    }
}

def categorize_citation [citation_text: string] {
    let lower_text = ($citation_text | str downcase)
    if ($lower_text =~ '(philosophy|ethics|epistemology)') {
        "philosophy"
    } else if ($lower_text =~ '(neuroscience|brain|cognitive)') {
        "neuroscience"  
    } else if ($lower_text =~ '(psychology|behavior|social)') {
        "psychology"
    } else if ($lower_text =~ '(academic|research|method)') {
        "academic"
    } else {
        "general"
    }
}

# Native Nushell input list integration - no external dependencies
def run-citation-picker [citations: table] {
    print -e ""
    print -e "🎯 Use arrow keys to navigate, Enter to select, Esc to cancel"
    print -e "💡 You can also type to search by author, title, keywords, or citation key"
    
    # Create display items for citation selection
    let display_items = (
        $citations 
        | each {|citation| 
            {
                display: $citation.full_text,
                citation_key: $citation.citation_key,
                category: $citation.category
            }
        }
    )
    
    # Use external skim with better key bindings
    let selected = try {
        $display_items 
        | get display
        | str join "\n"
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up' --prompt "📚 Select citation: "
        | str trim
    } catch {|err|
        print -e $"❌ Selection error: ($err.msg)"
        ""
    }
    
    if ($selected | is-empty) {
        return ""
    }
    
    # Return the selected citation's full text for processing
    return $selected
}

# Native Nushell citation processing with structured output
def process-citation-selection [selection: string, citations: table] {
    if ($selection | str trim | is-empty) {
        return {
            citation_key: "",
            wiki_link: "",
            success: false,
            error: "No selection made"
        }
    }
    
    # Find the matching citation record
    let matching_citation = (
        $citations 
        | where full_text == $selection 
        | first
    )
    
    if $matching_citation == null {
        return {
            citation_key: "",
            wiki_link: "",
            success: false,
            error: "Citation not found in database"
        }
    }
    
    let citation_key = $matching_citation.citation_key
    let wiki_link = $"[[$citation_key]]"
    
    return {
        citation_key: $citation_key,
        wiki_link: $wiki_link,
        success: true,
        category: $matching_citation.category,
        usage_examples: [
            $"r:: 30m [[$citation_key]] topic, keywords",
            $"w:: 45m [[$citation_key]] academic-writing"
        ]
    }
}

# Cross-platform clipboard with structured error handling  
def copy-to-clipboard [content: string] {
    let clipboard_result = try {
        if (which pbcopy | is-not-empty) {
            $content | pbcopy
            { success: true, method: "pbcopy" }
        } else if (which wl-copy | is-not-empty) {
            $content | wl-copy  
            { success: true, method: "wl-copy" }
        } else if (which xclip | is-not-empty) {
            $content | xclip -selection clipboard
            { success: true, method: "xclip" }
        } else {
            { success: false, method: "none", error: "No clipboard tool found" }
        }
    } catch {|err|
        { success: false, method: "error", error: $err.msg }
    }
    
    return $clipboard_result
}

# Main citation-picker function with full structured data processing
def main [] {
    print "🚀 Citation Picker (Native Nushell Re-nu Version)"
    print "📚 Using advanced structured data processing"
    
    # Structured vault detection
    let forge_info = (find-forge-directory)
    
    if $forge_info.type == "not_found" {
        print "❌ No Forge directory found"
        print "📝 Searched standard locations and current directory tree"  
        print "💡 Expected locations:"
        print "   • ~/Obsidian.nosync/Forge"
        print "   • ~/Obsidian/Forge" 
        print "   • ~/Documents/Forge"
        print "   • ~/Forge"
        exit 1
    }
    
    print $"✅ Found Forge: ($forge_info.path)"
    print $"🔍 Discovery method: ($forge_info.method)"
    
    # Structured citations file discovery
    let citations_file_info = (find-citations-file $forge_info.path)
    
    if $citations_file_info.type == "not_found" {
        print "❌ Citations file not found in vault"
        print "📝 Looking for: citations.md in ZET/, root, or references/ folders"
        exit 1
    }
    
    print $"✅ Found citations file: ($citations_file_info.location)"
    
    # Structured citation parsing with rich metadata
    let citations_result = (parse-citations $citations_file_info.path)
    
    if $citations_result.error != null {
        print $"❌ ($citations_result.error)"
        exit 1
    }
    
    let citations = $citations_result.citations
    print $"📚 Found ($citations_result.count) citations"
    
    # Analytics preview
    let category_stats = ($citations | group-by category | transpose category citations | each {|row| 
        { category: $row.category, count: ($row.citations | length) }
    })
    print "📊 Citation distribution by category:"
    print ($category_stats | table)
    
    print ""
    print "🎯 Select citation for wiki linking..."
    
    # Native Nushell skim integration  
    let selection = (run-citation-picker $citations)
    
    # Structured citation processing
    let result = (process-citation-selection $selection $citations)
    
    if not $result.success {
        print ""
        print $"❌ ($result.error)"
        sleep 1sec
        exit 0
    }
    
    # Output results
    print $result.wiki_link
    
    # Structured clipboard handling
    let clipboard_result = (copy-to-clipboard $result.wiki_link)
    
    if $clipboard_result.success {
        print $"📋 Copied to clipboard via ($clipboard_result.method)"
        print ""
        print $"✅ Citation wikilink ready: ($result.wiki_link)"
        print $"📝 Category: ($result.category)"
        print ""
        print "💡 Usage examples:"
        for example in $result.usage_examples {
            print $"   ($example)"
        }
        
        # Zellij floating pane feedback
        if ("ZELLIJ_SESSION_NAME" in $env) {
            print "✨ Ready to paste with Cmd+V/Ctrl+V"
            sleep 1.5sec
        }
    } else {
        print $"⚠️  Clipboard copy failed: ($clipboard_result.error)"
    }
    
    # Usage analytics (optional)
    let usage_data = {
        timestamp: (date now),
        vault_path: $forge_info.path,
        citations_scanned: ($citations | length),
        selected_key: $result.citation_key,
        category: $result.category,
        clipboard_method: $clipboard_result.method
    }
    
    # Could save analytics to structured log file for insights
    # $usage_data | to json | save --append ~/.local/share/citation-picker-analytics.json
}