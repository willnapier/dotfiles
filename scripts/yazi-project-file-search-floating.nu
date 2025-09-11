#!/usr/bin/env nu

# Yazi Project File Search Floating - Native Nushell Re-nu Version
# Floating pane file search with project detection and Helix integration
# Re-nu Phase 2: Productivity Script Excellence  
# Created: 2025-09-10

# Native Nushell project root detection with enhanced safety checks
export def find-project-root-enhanced [] {
    let project_indicators = [
        ".git",
        ".obsidian", 
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "go.mod",
        "NapierianLogs",
        "Areas",
        "ZET"
    ]
    
    let current_dir = (pwd)
    mut check_dir = $current_dir
    
    print -e "🔍 Detecting project root from current location..."
    
    # Walk up directory tree looking for project indicators
    loop {
        for indicator in $project_indicators {
            let indicator_path = ($check_dir | path join $indicator)
            if ($indicator_path | path exists) {
                return {
                    path: $check_dir,
                    type: "found",
                    method: "indicator_search",
                    indicator: $indicator,
                    safety_level: "safe"
                }
            }
        }
        
        let parent = ($check_dir | path dirname)
        if $parent == $check_dir {
            break
        }
        $check_dir = $parent
    }
    
    # Enhanced safety checks with structured decision making
    let home_dir = $env.HOME
    let dangerous_roots = ["/Users", "/home", "/"]
    
    # If we're at a dangerous root, implement intelligent fallback
    if $current_dir in $dangerous_roots {
        print -e "⚠️  Search scope too broad. Implementing intelligent fallback..."
        return (find-safe-search-scope $home_dir)
    }
    
    # If we're in home directory, try to find a sensible scope
    if ($current_dir | str starts-with $home_dir) {
        print -e "🏠 At home directory - finding appropriate search scope..."
        return (find-safe-search-scope $home_dir)
    }
    
    # Default to current directory if it seems reasonable
    return {
        path: $current_dir,
        type: "found",
        method: "current_dir_fallback",
        safety_level: "safe"
    }
}

def find-safe-search-scope [home_dir: string] {
    # Priority order for search scope selection
    let scope_candidates = [
        {
            path: ($home_dir | path join "Obsidian.nosync" "Forge"),
            name: "Obsidian Vault",
            reason: "Primary knowledge base"
        },
        {
            path: ($home_dir | path join "Documents"),
            name: "Documents",
            reason: "Common document location"
        },
        {
            path: ($home_dir | path join "Projects"),
            name: "Projects",
            reason: "Development workspace"
        },
        {
            path: ($home_dir | path join "Desktop"),
            name: "Desktop", 
            reason: "Active workspace"
        }
    ]
    
    for candidate in $scope_candidates {
        if ($candidate.path | path exists) {
            print -e $"📝 Found ($candidate.name), using: ($candidate.path)"
            print -e $"💡 Reason: ($candidate.reason)"
            return {
                path: $candidate.path,
                type: "found",
                method: "safe_scope_fallback",
                scope_type: $candidate.name,
                safety_level: "safe"
            }
        }
    }
    
    # Last resort: home directory with warning
    print -e "⚠️  Will search entire home directory - this may be slow!"
    return {
        path: $home_dir,
        type: "found", 
        method: "home_fallback",
        safety_level: "broad_but_safe"
    }
}


# Native Nushell file discovery with intelligent filtering and preview support
export def scan-project-files-for-search [project_root: string, --file-patterns: list<string> = ["*.md", "*.txt", "*.org", "*.rs", "*.py", "*.js", "*.ts"]] {
    print -e $"🔍 Scanning files in: (($project_root | path basename))"
    
    let exclusions = [
        ".git",
        ".obsidian", 
        "node_modules",
        "target",
        "dist",
        "build",
        ".DS_Store",
        "linked_media",
        ".trash",
        "Templates",
        ".vscode",
        "__pycache__",
        ".pytest_cache",
        "venv",
        ".env"
    ]
    
    mut all_files = []
    
    # Use native Nushell globbing with progress indication
    for pattern in $file_patterns {
        print -e $"  📁 Scanning pattern: ($pattern)"
        
        let pattern_files = try {
            glob ($project_root | path join "**" $pattern)
            | where {|file|
                let relative = ($file | str replace $project_root "")
                # Efficient exclusion check
                not ($exclusions | any {|exc| $relative | str contains $exc})
            }
            | sort
        } catch {
            print -e $"    ⚠️  Pattern ($pattern) failed, skipping..."
            []
        }
        
        let pattern_count = ($pattern_files | length)
        if $pattern_count > 0 {
            print -e $"    ✅ Found ($pattern_count) files for ($pattern)"
        }
        
        $all_files = ($all_files | append $pattern_files)
    }
    
    # Lightweight metadata - only what's needed for the picker  
    let files_with_metadata = (
        $all_files 
        | uniq
        | each {|file|
            let relative_path = ($file | str replace $project_root "" | str trim --left --char "/")
            let filename = ($file | path basename)
            
            {
                absolute_path: $file,
                relative_path: $relative_path,
                filename: $filename,
                display_name: $relative_path
            }
        }
    )
    
    print -e $"📝 Found ($files_with_metadata | length) total files with metadata"
    return $files_with_metadata
}


# Adaptive skim integration for different Zellij environments
def run-floating-file-picker [files: table, project_info: record] {
    # Detect Zellij environment
    let zellij_session = $env.ZELLIJ_SESSION_NAME? | default ""
    let is_zellij = not ($zellij_session | is-empty)
    
    if $is_zellij {
        print -e "🎯 Zellij detected - testing skim with optimized terminal settings"
    } else {
        print -e "🎯 Testing skim in standard terminal environment"
    }
    
    # Create skim input with relative paths
    let skim_input = (
        $files 
        | each {|file| $file.relative_path }
        | str join "\n"
    )
    
    # Adaptive terminal environment based on context
    if $is_zellij {
        $env.TERM = "screen-256color"  # Better for tmux/zellij compatibility
        $env.COLORTERM = "truecolor"
    } else {
        $env.TERM = "xterm-256color"   # Standard terminal
        $env.COLORTERM = "truecolor"
    }
    $env.RUST_BACKTRACE = "0"
    
    # Try skim first, but with a working fallback when TTY fails
    let selected = try {
        $skim_input 
        | sk --ansi 
             --reverse 
             --prompt="📁 File Search: " 
             --preview=$"bat --style=numbers --color=always --line-range=:50 '($project_info.path)/{}' 2>/dev/null || head -n 20 '($project_info.path)/{}' 2>/dev/null || echo 'No preview available'" 
             --preview-window="right:60%:wrap"
             --bind="ctrl-/:toggle-preview"
             --header="Type to search, Ctrl+/ toggles preview, Enter selects"
             --height=90%
    } catch {|err|
        print -e $"⚠️  Skim failed in Zellij context: ($err.msg)"
        print -e "🔧 Using Nushell fallback with preview..."
        
        # Show numbered list with preview capability
        let file_list = ($files | enumerate | each {|item| 
            $"($item.index + 1). ($item.item.relative_path)"
        } | str join "\n")
        
        print -e $file_list
        print -e ""
        print -e "Enter file number to preview and select:"
        
        let selection = (input --numchar 3 | str trim)
        
        try {
            let index = ($selection | into int) - 1
            let selected_file = ($files | get $index)
            
            print -e ""
            print -e $"📖 Preview: ($selected_file.relative_path)"
            print -e "─" * 50
            
            let preview = try {
                open $selected_file.absolute_path | lines | first 15 | str join "\n"
            } catch { "Could not preview this file" }
            
            print -e $preview
            print -e "─" * 50
            print -e ""
            print -e "Press Enter to open in Helix, or 'q' to cancel:"
            
            let confirm = (input --numchar 1)
            if $confirm != "q" {
                return $selected_file.relative_path
            } else {
                return ""
            }
        } catch {
            print -e "❌ Invalid selection"
            return ""
        }
    }
    
    return ($selected | str trim)
}

# Process file selection and return absolute path (for shell integration)
def process-file-selection-and-return [selection: string, files: table] {
    if ($selection | str trim | is-empty) {
        return ""
    }
    
    let clean_selection = ($selection | str trim)
    
    # Find the matching file record using relative_path for robust matching
    let matching_file = (
        $files 
        | where relative_path == $clean_selection
        | first
    )
    
    if $matching_file == null {
        return ""
    }
    
    return $matching_file.absolute_path
}

# Main floating file search function
def main [] {
    print -e "🚀 Yazi Project File Search (Native Nushell Re-nu Version)"
    print -e "📊 Floating pane file search with advanced project detection"
    print -e "🔧 DEBUG: Script started successfully"
    
    # Enhanced project detection with safety checks
    let project_info = (find-project-root-enhanced)
    
    print -e $"✅ Project root: (($project_info.path | path basename))"
    print -e $"🔍 Detection method: ($project_info.method)"
    print -e $"🛡️  Safety level: ($project_info.safety_level)"
    
    if "indicator" in $project_info {
        print -e $"📍 Found via indicator: ($project_info.indicator)"
    }
    
    # Structured file scanning with multiple patterns
    let files_data = (scan-project-files-for-search $project_info.path)
    
    if ($files_data | is-empty) {
        print -e "❌ No files found in project"
        print -e "🔍 Listing directory contents for debugging:"
        try {
            ls $project_info.path | first 5 | table | print -e
        } catch {
            print -e "Could not list directory contents"
        }
        exit 1
    }
    
    print -e ""
    
    print -e "🎯 Select file to open in Helix..."
    
    # Native Nushell skim integration 
    let selection = (run-floating-file-picker $files_data $project_info)
    
    # Process selection and return absolute path (for shell integration)
    let selected_path = (process-file-selection-and-return $selection $files_data)
    
    # Open the file directly in Helix instead of returning path
    if not ($selected_path | is-empty) {
        print -e $"✅ Opening: ($selected_path | path basename)"
        # Open in new Zellij pane with Helix
        ^zellij action new-pane -- hx $selected_path
    } else {
        print -e "❌ No file selected"
        sleep 1sec
    }
}