#!/usr/bin/env nu

# Yazi Search - Native Nushell Re-nu Version
# Unified search system for file and content discovery with structured data processing
# Re-nu Phase 2: Productivity Script Excellence
# Created: 2025-09-10

# Native Nushell project root detection with structured metadata
export def find-project-root [] {
    let project_indicators = [
        ".git",
        ".obsidian", 
        "package.json",
        "Cargo.toml",
        "pyproject.toml",
        "go.mod",
        "NapierianLogs",
        "Areas"
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
                    indicator: $indicator
                }
            }
        }
        
        let parent = ($check_dir | path dirname)
        if $parent == $check_dir {
            break
        }
        $check_dir = $parent
    }
    
    # Fallback to home directory with safety check
    let home_dir = $env.HOME
    if ($current_dir | str starts-with $home_dir) {
        # If we're in Documents, use Documents as root
        if ($current_dir | str contains "Documents") {
            let docs_path = ($home_dir | path join "Documents")
            if ($docs_path | path exists) {
                return {
                    path: $docs_path,
                    type: "found",
                    method: "documents_fallback"
                }
            }
        }
        
        # Check for Obsidian vault
        let vault_candidates = [
            ($home_dir | path join "Forge"),
            ($home_dir | path join "Obsidian" "Forge")
        ]
        
        for candidate in $vault_candidates {
            if ($candidate | path exists) {
                return {
                    path: $candidate,
                    type: "found", 
                    method: "vault_fallback"
                }
            }
        }
        
        return {
            path: $home_dir,
            type: "found",
            method: "home_fallback"
        }
    }
    
    return {
        path: $current_dir,
        type: "found",
        method: "current_dir_fallback"
    }
}

# Native Nushell file discovery with structured metadata and filtering
export def scan-project-files [project_root: string, --include-patterns: list<string> = ["*.md", "*.txt", "*.org"], --days: int = 30] {
    print $"🔍 Scanning project files in: (($project_root | path basename))"
    
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
        "Templates"
    ]
    
    mut all_files = []
    
    # Use native Nushell globbing for each pattern
    for pattern in $include_patterns {
        let pattern_files = try {
            glob ($project_root | path join "**" $pattern)
            | where {|file|
                let relative = ($file | str replace $project_root "")
                # Check if file path contains any exclusions
                not ($exclusions | any {|exc| $relative | str contains $exc})
            }
            | sort-by modified --reverse
        } catch {
            []
        }
        
        $all_files = ($all_files | append $pattern_files)
    }
    
    # Remove duplicates and add metadata
    let files_with_metadata = (
        $all_files 
        | uniq
        | each {|file|
            let stat_info = try { $file | stat } catch { null }
            let relative_path = try {
                $file | str replace $project_root "" | str trim --left --char "/"
            } catch { $file }
            
            let age_days = if $stat_info != null {
                let modified_date = $stat_info.modified
                let now = (date now)
                (($now - $modified_date) / 1day) | math round
            } else {
                999
            }
            
            {
                absolute_path: $file,
                relative_path: $relative_path,
                filename: ($file | path basename),
                extension: ($file | path parse | get extension),
                size: (if $stat_info != null { $stat_info.size } else { 0 }),
                modified: (if $stat_info != null { $stat_info.modified } else { "1970-01-01T00:00:00+00:00" | into datetime }),
                age_days: $age_days,
                is_recent: ($age_days <= $days),
                category: (categorize_file $relative_path),
                exists: ($file | path exists)
            }
        }
    )
    
    print $"📝 Found ($files_with_metadata | length) files with structured metadata"
    return $files_with_metadata
}

# Native Nushell content search with AND/OR support and structured results  
export def search-content [project_root: string, search_terms: string, files_data: table] {
    print $"🔍 Searching content for: ($search_terms)"
    
    # Parse search terms (simple AND/OR logic)
    let terms = if ($search_terms | str contains " OR ") {
        {
            type: "OR",
            terms: ($search_terms | split row " OR " | each {|term| $term | str trim})
        }
    } else if ($search_terms | str contains " AND ") {
        {
            type: "AND", 
            terms: ($search_terms | split row " AND " | each {|term| $term | str trim})
        }
    } else {
        {
            type: "SIMPLE",
            terms: [$search_terms]
        }
    }
    
    print $"🔍 Search mode: ($terms.type) with terms: ($terms.terms | str join ', ')"
    
    mut matching_files = []
    
    for file_info in $files_data {
        if not $file_info.exists {
            continue
        }
        
        let content = try {
            open $file_info.absolute_path --raw
        } catch {
            ""
        }
        
        let matches = match $terms.type {
            "OR" => {
                $terms.terms | any {|term| 
                    ($content | str downcase) | str contains ($term | str downcase)
                }
            },
            "AND" => {
                $terms.terms | all {|term|
                    ($content | str downcase) | str contains ($term | str downcase)
                }
            },
            "SIMPLE" => {
                ($content | str downcase) | str contains (($terms.terms | first) | str downcase)
            }
        }
        
        if $matches {
            let content_preview = ($content | lines | first 3 | str join " " | str substring 0..100)
            $matching_files = ($matching_files | append {
                ...$file_info,
                content_preview: $content_preview,
                search_terms: $search_terms,
                match_type: $terms.type
            })
        }
    }
    
    print $"📄 Found ($matching_files | length) files with matching content"
    return $matching_files
}

def categorize_file [path: string] {
    if ($path | str contains "DayPages") {
        "daily"
    } else if ($path | str contains "WeeklyReviews") {
        "weekly"
    } else if ($path | str contains "Areas") {
        "area"
    } else if ($path | str contains "Projects") {
        "project"
    } else if ($path | str contains "Resources") {
        "resource"
    } else if ($path | str contains "ZET") {
        "zettelkasten"
    } else {
        "general"
    }
}

# Native Nushell skim integration with structured file picker
def run-file-picker [files: table, search_type: string] {
    let skim_input = (
        $files 
        | each {|file| 
            if $search_type == "content" {
                $"($file.relative_path) | ($file.content_preview)"
            } else {
                $file.relative_path
            }
        }
        | to text
    )
    
    let preview_cmd = if $search_type == "content" {
        $"echo 'Preview not available for content search results'"
    } else {
        $"$env.HOME/.local/bin/simple-file-preview {}"
    }
    
    let selected = (
        $skim_input 
        | sk --interactive
             --prompt=$"📁 Project ($search_type) Search [Enter=open, Tab=multi]: "
             --reverse
             --preview=$preview_cmd
             --preview-window="right:60%"
             --header=$"Found ($files | length) files - arrows=navigate, Enter=open in Helix"
    )
    
    return $selected
}

# Process selection and open in Helix
def process-file-selection [selection: string, files: table, search_type: string] {
    if ($selection | str trim | is-empty) {
        return {
            selected_file: "",
            success: false,
            error: "No selection made"
        }
    }
    
    # For content search, extract just the file path part
    let file_path = if $search_type == "content" {
        $selection | split row " | " | first | str trim
    } else {
        $selection | str trim
    }
    
    # Find the matching file record
    let matching_file = (
        $files 
        | where relative_path == $file_path 
        | first
    )
    
    if $matching_file == null {
        return {
            selected_file: "",
            success: false, 
            error: $"File not found in results: ($file_path)"
        }
    }
    
    return {
        selected_file: $matching_file.absolute_path,
        success: true,
        file_info: $matching_file
    }
}

# Main yazi-search function with multiple modes
def main [
    mode: string = "files",           # "files", "content", "recent"
    search_term?: string,             # Search term for content search
    --days: int = 7,                  # Days for recent files filter
    --patterns: list<string> = ["*.md", "*.txt", "*.org"],  # File patterns to include
    --floating                        # Run in floating pane mode
] {
    print $"🚀 Yazi Search (Native Nushell Re-nu Version)"
    print $"📊 Mode: ($mode) | Using advanced structured data processing"
    
    # Structured project detection
    let project_info = (find-project-root)
    print $"✅ Project root: (($project_info.path | path basename))"
    print $"🔍 Detection method: ($project_info.method)"
    
    if $project_info.type == "not_found" {
        print "❌ Could not determine project root"
        exit 1
    }
    
    # Structured file scanning
    let files_data = (scan-project-files $project_info.path --include-patterns $patterns --days $days)
    
    if ($files_data | is-empty) {
        print "❌ No files found in project"
        exit 1
    }
    
    # Analytics preview
    let file_stats = ($files_data | group-by category | transpose category files | each {|row| 
        { category: $row.category, count: ($row.files | length) }
    })
    print "📊 File distribution by category:"
    print ($file_stats | table)
    
    # Process based on mode
    let search_results = match $mode {
        "files" => {
            print ""
            print "🎯 Select file to open in Helix..."
            $files_data
        },
        "recent" => {
            let recent_files = ($files_data | where is_recent | sort-by modified --reverse)
            print ""
            print $"📅 Found ($recent_files | length) recent files \(last ($days) days\)..."
            $recent_files
        },
        "content" => {
            if $search_term == null {
                print "❌ Content search requires search term"
                exit 1
            }
            search-content $project_info.path $search_term $files_data
        },
        _ => {
            print $"❌ Unknown mode: ($mode). Use: files, content, recent"
            exit 1
        }
    }
    
    if ($search_results | is-empty) {
        print "❌ No results found"
        exit 0
    }
    
    # Native Nushell skim integration
    let selection = (run-file-picker $search_results $mode)
    
    # Process selection
    let result = (process-file-selection $selection $search_results $mode)
    
    if not $result.success {
        print $"❌ ($result.error)"
        sleep 1sec
        exit 0
    }
    
    # Output the selected file path for yazi integration
    print $result.selected_file
    
    # Usage analytics (optional)
    let usage_data = {
        timestamp: (date now),
        mode: $mode,
        project_path: $project_info.path,
        files_scanned: ($files_data | length),
        results_found: ($search_results | length),
        selected_file: $result.selected_file,
        search_term: $search_term
    }
    
    # Could save analytics to structured log file for insights
    # $usage_data | to json | save --append ~/.local/share/yazi-search-analytics.json
}