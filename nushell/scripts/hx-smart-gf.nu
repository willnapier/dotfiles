# hx-smart-gf.nu - Project-aware "go to file" command for Helix
# Pure Nushell implementation with intelligent root detection

use project-root-detection.nu *

# Main function to handle file resolution
export def main [] {
    # Get text from stdin
    let input_text = ($in | str trim)
    
    if ($input_text | is-empty) {
        print "No text provided"
        exit 1
    }
    
    # Get project context
    let project_info = (show-project-info)
    let project_root = $project_info.project_root
    let project_type = $project_info.project_type
    
    # Resolve the file
    let target_file = (resolve-file-reference $input_text $project_root $project_type)
    
    # Write result for Helix
    $target_file | save -f /tmp/helix-gf-target.md
    
    print $"Resolved: ($input_text) -> ($target_file)"
}

# Function to resolve file references
def resolve-file-reference [text: string, root: string, type: string]: nothing -> string {
    # Clean up the input
    let clean_text = ($text | str replace --all '"' "" | str replace --all "'" "" | str trim)
    
    # Handle URLs
    if ($clean_text | str starts-with "http") {
        return $clean_text
    }
    
    # Handle wiki links [[content]]
    if ($clean_text | str contains "[[") and ($clean_text | str contains "]]") {
        let wiki_content = ($clean_text | str replace "[[" "" | str replace "]]" "")
        
        if $type == "obsidian-vault" {
            # Look for exact match first
            let note_path = ($root | path join $"($wiki_content).md")
            if ($note_path | path exists) {
                return $note_path
            }
            
            # Search for similar notes
            let search_results = (try {
                fd -e md -g $"*($wiki_content)*" $root | head 1
            } catch {
                []
            })
            
            if ($search_results | length) > 0 {
                return ($search_results | get 0)
            }
        }
        
        return $wiki_content
    }
    
    # Handle regular file paths
    let potential_paths = [
        ($root | path join $clean_text)
        ($env.PWD | path join $clean_text) 
        ($clean_text | path expand)
    ]
    
    # Check existing paths
    for path in $potential_paths {
        if ($path | path exists) {
            return $path
        }
    }
    
    # Search for similar files in project
    let search_results = (try {
        fd -t f -g $"*($clean_text)*" $root | head 1  
    } catch {
        []
    })
    
    if ($search_results | length) > 0 {
        return ($search_results | get 0)
    }
    
    # Fallback to original text
    $clean_text
}