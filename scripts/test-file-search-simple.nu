#!/usr/bin/env nu

# Simple test version - just return the first file to test path resolution

def main [] {
    print -e "🚀 Simple File Search Test"
    
    let project_root = "$"(.HOME)/Forge""
    print -e $"📁 Project root: ($project_root)"
    
    # Get first few files for testing
    let files = (
        glob ($project_root | path join "**/*.md")
        | where {|file|
            let relative = ($file | str replace $project_root "")
            not ($relative | str contains ".obsidian")
        }
        | first 3
        | each {|file|
            let relative_path = ($file | str replace $project_root "" | str trim --left --char "/")
            {
                absolute_path: $file,
                relative_path: $relative_path
            }
        }
    )
    
    print -e "📝 Test files found:"
    $files | each {|f| print -e $"  - ($f.relative_path)"}
    
    # Just return the first file for testing
    let first_file = ($files | first)
    print -e $"✅ Returning: ($first_file.relative_path)"
    print -e $"📁 Absolute: ($first_file.absolute_path)"
    
    print $first_file.absolute_path
}