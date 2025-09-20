#!/usr/bin/env nu

# Zellij-based skim search with full environment and automatic Helix opening
# Uses proper Zellij pane with full TTY for skim functionality
# Created: 2025-09-10

# Project root detection (reuse existing logic)
def find-project-root [] {
    let project_indicators = [
        ".git", ".obsidian", "package.json", "Cargo.toml", 
        "pyproject.toml", "go.mod", "NapierianLogs", "Areas", "ZET"
    ]
    
    mut check_dir = (pwd)
    
    loop {
        for indicator in $project_indicators {
            if ($check_dir | path join $indicator | path exists) {
                return $check_dir
            }
        }
        
        let parent = ($check_dir | path dirname)
        if $parent == $check_dir { break }
        $check_dir = $parent
    }
    
    # Fallback to sensible directories
    let home_dir = $env.HOME
    let candidates = [
        ($home_dir | path join "Obsidian.nosync" "Forge"),
        ($home_dir | path join "Documents"),
        ($home_dir | path join "Projects"),
        ($home_dir | path join "Desktop")
    ]
    
    for candidate in $candidates {
        if ($candidate | path exists) {
            return $candidate
        }
    }
    
    return $home_dir
}

# Scan files for search
def scan-files [project_root: string] {
    let patterns = ["*.md", "*.txt", "*.org", "*.rs", "*.py", "*.js", "*.ts", "*.toml", "*.json", "*.nu"]
    let exclusions = [".git", ".obsidian", "node_modules", "target", "dist", "build", ".DS_Store"]
    
    mut all_files = []
    
    for pattern in $patterns {
        let pattern_files = try {
            glob ($project_root | path join "**" $pattern)
            | where {|file|
                let relative = ($file | str replace $project_root "")
                not ($exclusions | any {|exc| $relative | str contains $exc})
            }
        } catch { [] }
        
        $all_files = ($all_files | append $pattern_files)
    }
    
    return ($all_files | uniq | each {|file|
        $file | str replace $project_root "" | str trim --left --char "/"
    })
}

# Main search function that runs in proper Zellij pane
def main [] {
    print "🚀 Zellij Skim Search - Full Environment Mode"
    
    let project_root = (find-project-root)
    print $"📁 Project: (($project_root | path basename))"
    
    let files = (scan-files $project_root)
    print $"📊 Found ($files | length) files"
    
    if ($files | is-empty) {
        print "❌ No files found"
        exit 1
    }
    
    # Create skim input
    let skim_input = ($files | str join "\n")
    
    print "🔍 Starting skim with full preview..."
    print "   • Fuzzy search through files"
    print "   • Preview with Ctrl+/ toggle"  
    print "   • Will auto-open selection in Helix"
    print ""
    print "DEBUG: About to start skim..."
    
    # Run skim with proper terminal environment
    let selection = try {
        print "DEBUG: Setting terminal environment..."
        
        # Set up proper terminal environment for skim
        $env.TERM = "xterm-256color"
        $env.COLORTERM = "truecolor"
        $env.TERMINFO = "/usr/share/terminfo"
        
        print "DEBUG: Running skim command with fixed environment..."
        print $"DEBUG: Project root is: ($project_root)"
        print $"DEBUG: Preview command will be: bat --style=numbers --color=always --line-range=:50 \"($project_root)/{{}}\" 2>/dev/null || head -n 20 \"($project_root)/{{}}\" 2>/dev/null || echo 'No preview available'"
        
        # Test if bat is available
        let bat_available = try { ^bat --version | complete | get exit_code } catch { 1 } == 0
        print $"DEBUG: bat available: ($bat_available)"
        
        let preview_cmd = if $bat_available {
            $"bat --style=numbers --color=always --line-range=:50 \"($project_root)/{{}}\" 2>/dev/null || echo 'No preview available'"
        } else {
            $"head -n 20 \"($project_root)/{{}}\" 2>/dev/null || echo 'No preview available'"
        }
        
        print $"DEBUG: Using preview command: ($preview_cmd)"
        
        let result = ($skim_input 
        | sk --ansi 
             --reverse 
             --prompt="📁 File Search: " 
             --preview=$preview_cmd
             --preview-window="right:60%:wrap"
             --bind="ctrl-/:toggle-preview"
             --header="Type to search • Ctrl+/ toggle preview • Enter selects • Esc cancels"
             --height=90%)
        print "DEBUG: Skim completed successfully"
        $result
    } catch {|err|
        print $"❌ Skim still failed: ($err.msg)"
        print "DEBUG: Falling back to native selection..."

        # Show first 20 files for quick selection
        let display_files = ($file_list | first 20)
        print "Available files (first 20):"
        for i in 0..(($display_files | length) - 1) {
            print $"($i + 1). ($display_files | get $i)"
        }

        print "Enter file number (1-20):"
        let choice = (input --numchar 2 | str trim | into int)
        $display_files | get ($choice - 1)
    }
    
    let selection = ($selection | str trim)
    
    if ($selection | is-empty) {
        print "❌ No selection made"
        exit 1
    }
    
    let full_path = ($project_root | path join $selection)
    
    print $"✅ Selected: ($selection)"
    print $"🚀 Opening in Helix..."
    
    # Open in new Helix pane
    zellij action new-pane -- hx $full_path
    
    print "✅ Helix opened in new pane"
    print "Press any key to close this search pane..."
    
    # Wait for user input before closing
    input --numchar 1
    
    # Close this search pane
    zellij action close-pane
}