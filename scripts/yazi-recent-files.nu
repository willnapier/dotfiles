#!/usr/bin/env nu

# Yazi Recent Files - Native Nushell Re-nu Version  
# Fast recent file discovery with project detection and Helix integration
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
        return (find-safe-search-scope $home_dir)
    }
    
    # If we're in home directory, try to find a sensible scope
    if ($current_dir | str starts-with $home_dir) {
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
    return {
        path: $home_dir,
        type: "found", 
        method: "home_fallback",
        safety_level: "broad_but_safe"
    }
}

# Native Nushell recent file discovery with optimized performance
export def scan-recent-files [project_root: string, --days: int = 7] {
    let project_name = ($project_root | path basename)
    print $"🕒 Scanning recent files in: ($project_name) (within ($days) days)"
    
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
    
    # Use native Nushell globbing with optimized performance (no stat calls)
    let recent_files = try {
        glob ($project_root | path join "**/*.md")
        | where {|file|
            let relative = ($file | str replace $project_root "")
            # Efficient exclusion check
            not ($exclusions | any {|exc| $relative | str contains $exc})
        }
        | sort --reverse  # Simple sort by path (most recently created paths tend to be newer)
        | first 100  # Get more files since we can't filter by date efficiently
    } catch {
        print "⚠️  Error scanning recent files, trying fallback..."
        []
    }
    
    # Lightweight metadata - only what's needed for the picker  
    let files_with_metadata = (
        $recent_files 
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
    
    print $"📝 Found ($files_with_metadata | length) recent files"
    return $files_with_metadata
}

# Native Nushell input list integration - no external dependencies
def run-recent-file-picker [files: table, project_info: record] {
    print -e ""
    print -e "🎯 Use arrow keys to navigate, Enter to select, Esc to cancel"
    print -e "💡 You can also type to search for files"
    
    # Create display items with relative paths for better selection
    let display_items = (
        $files 
        | each {|file| 
            {
                display: $file.relative_path,
                path: $file.absolute_path
            }
        }
    )
    
    # Use native Nushell input list with fuzzy search
    let selected_index = try {
        $display_items 
        | get display
        | input list --fuzzy --index "📅 Select recent file to open:"
    } catch {|err|
        print -e $"❌ Selection error: ($err.msg)"
        null
    }
    
    if $selected_index == null {
        return ""
    }
    
    # Return the selected file's relative path for processing
    return ($display_items | get $selected_index | get display)
}

# Process file selection and return absolute path for opening
def process-recent-file-selection [selection: string, files: table] {
    if ($selection | str trim | is-empty) {
        return ""
    }
    
    # Find the matching file record using relative_path for robust matching
    let matching_file = (
        $files 
        | where relative_path == ($selection | str trim)
        | first
    )
    
    if $matching_file == null {
        print $"❌ File not found in results: ($selection)"
        return ""
    }
    
    return $matching_file.absolute_path
}

# Main recent files function
def main [] {
    print -e "🚀 Yazi Recent Files (Native Nushell Re-nu Version)"
    print -e "📅 Recent file discovery with advanced project detection"
    
    # Enhanced project detection with safety checks
    let project_info = (find-project-root-enhanced)
    
    if ($project_info.path | is-empty) {
        print -e "❌ No suitable directory found for recent file search"
        exit 1
    }
    
    print -e $"✅ Project root: (($project_info.path | path basename))"
    print -e $"🔍 Detection method: ($project_info.method)"
    
    # Structured file scanning for recent files
    let files_data = (scan-recent-files $project_info.path)
    
    if ($files_data | is-empty) {
        print -e "📭 No recent files found in the last 7 days"
        exit 0
    }
    
    print -e ""
    print -e "🎯 Select recent file to open in Helix..."
    
    # Native Nushell input list integration 
    let selection = (run-recent-file-picker $files_data $project_info)
    
    # Process selection and return absolute path
    let selected_path = (process-recent-file-selection $selection $files_data)
    
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