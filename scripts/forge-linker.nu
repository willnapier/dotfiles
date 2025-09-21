#!/usr/bin/env nu

# Forge Link Picker - Native Nushell Re-nu Version
# Advanced structured data processing for wiki link insertion
# Re-nu Phase 2: Productivity Script Excellence
# Created: 2025-09-09

# Native Nushell structured data types
export def find-forge-directory [] {
    let forge_candidates = [
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
            ($check_dir | path join "Areas")
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

# Native Nushell file discovery with structured metadata
export def scan-forge-files [forge_path: string] {
    print "🔍 Scanning Forge for markdown files..."
    
    let files = (
        glob ($forge_path | path join "**/*.md") 
        | where ($it | path basename) != ".DS_Store"
        | where not ($it | str contains ".obsidian")
        | where not ($it | str contains ".trash") 
        | where not ($it | str contains "Templates")
        | where not ($it | str contains "linked_media")
        | sort
    )
    
    let files_with_metadata = (
        $files 
        | each {|file| 
            let relative_path = ($file | str replace $forge_path "" | str trim --left --char "/")
            let filename = ($file | path basename | str replace ".md" "")
            
            {
                absolute_path: $file,
                relative_path: $relative_path,
                filename: $filename,
                display_name: $filename
            }
        }
    )
    
    print $"📝 Found ($files_with_metadata | length) notes with structured metadata"
    return $files_with_metadata
}


# Native Nushell input list integration - no external dependencies
def run-skim-picker [files_data: table] {
    print -e ""
    print -e "🎯 Use arrow keys to navigate, Enter to select, Esc to cancel"
    print -e "💡 You can also type to search for files"
    print -e "📝 Multi-select: Use space to toggle selection, Enter to confirm"
    
    # Create display items with relative paths for better selection
    let display_items = (
        $files_data 
        | each {|file| 
            {
                display: $file.relative_path,
                filename: $file.filename
            }
        }
    )
    
    # Use external skim for better terminal compatibility in floating panes
    let selected = try {
        $display_items 
        | get display
        | str join "\n"
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" sk --multi --preview 'echo {}' --prompt "📝 Select notes: "
        | str trim
    } catch {|err|
        print -e $"❌ Selection error: ($err.msg)"
        ""
    }
    
    if ($selected | is-empty) {
        return ""
    }
    
    # Return the selected paths directly (skim already provides newline-separated output)
    return $selected
}

# Native Nushell wiki link processing with structured data
def process-selections [selections: string, files_data: table, forge_path: string] {
    let selection_list = ($selections | lines | where $it != "")
    mut wiki_links = []
    
    for selection in $selection_list {
        let existing_file = (
            $files_data 
            | where relative_path == $selection 
            | first 
        )
        
        if $existing_file != null {
            # Existing file - use filename without extension
            $wiki_links = ($wiki_links | append $"[[($existing_file.filename)]]")
        } else {
            # New file (from Ctrl+N) - add unresolved prefix
            let clean_name = ($selection | str replace ".md" "")
            $wiki_links = ($wiki_links | append $"?[[($clean_name)]]")
        }
    }
    
    return ($wiki_links | str join " ")
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

# Main forge-linker function with full structured data processing
def main [] {
    print "🚀 Forge Linker (Native Nushell Re-nu Version)"
    print "📊 Using advanced structured data processing"
    
    # Structured forge detection
    let forge_info = (find-forge-directory)
    
    if $forge_info.type == "not_found" {
        print "❌ No Forge directory found"
        print "📝 Searched standard locations and current directory tree"  
        print "💡 Expected locations:"
        print "   • ~/Forge"
        exit 1
    }
    
    print $"✅ Found Forge: ($forge_info.path)"
    print $"🔍 Discovery method: ($forge_info.method)"
    
    # Structured file scanning with rich metadata
    let files_data = (scan-forge-files $forge_info.path)
    
    if ($files_data | is-empty) {
        print "❌ No markdown files found in Forge"
        exit 1
    }
    
    print "🎯 Select note(s) for wiki linking..."
    
    # Native Nushell skim integration  
    let selections = (run-skim-picker $files_data)
    
    if ($selections | str trim | is-empty) {
        print ""
        print "❌ No selection made"
        sleep 1sec
        exit 0
    }
    
    # Structured wiki link processing
    let wiki_links = (process-selections $selections $files_data $forge_info.path)
    
    # Output results
    print $wiki_links
    
    # Structured clipboard handling
    let clipboard_result = (copy-to-clipboard $wiki_links)
    
    if $clipboard_result.success {
        print $"📋 Copied to clipboard via ($clipboard_result.method)"
        
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
        forge_path: $forge_info.path,
        files_scanned: ($files_data | length),
        selections_made: ($selections | lines | length),
        categories_used: 1,
        clipboard_method: $clipboard_result.method
    }
    
    # Could save analytics to structured log file for insights
    # $usage_data | to json | save --append ~/.local/share/forge-linker-analytics.json
}