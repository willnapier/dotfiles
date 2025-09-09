#!/usr/bin/env nu

# Word-wrapped file preview using Nushell
# Usage: word-wrap-preview.nu filename.md [width]

def main [file_path: string, width: int = 80] {
    if not ($file_path | path exists) {
        print $"File not found: ($file_path)"
        return
    }
    
    # Calculate words per line based on average word length + spaces
    let words_per_line = if $width < 48 { 8 } else { ($width / 6 | math round) }
    
    # Process file content with intelligent word wrapping (safer first N lines)
    let content = (try { 
        open $file_path | lines | take 30 
    } catch { 
        ["Error reading file"] 
    })
    
    $content | each { |line|
        # Handle different line types
        if ($line | str trim | is-empty) {
            # Preserve empty lines
            ""
        } else if ($line | str starts-with "#") {
            # Headers - preserve but truncate if needed
            if ($line | str length) > $width {
                ($line | str substring 0..$width) + "..."
            } else {
                $line
            }
        } else if (($line | str starts-with "- ") or ($line | str starts-with "* ")) {
            # List items - preserve indent, wrap content
            let indent = if ($line | str starts-with "- ") { "- " } else { "* " }
            let content = ($line | str replace --regex "^[*-] *" "")
            if ($content | str length) <= ($width - 2) {
                $line  # Keep as-is if short enough
            } else {
                # Wrap list content safely
                try {
                    let words = ($content | split words)
                    let chunk_size = if $words_per_line > 1 { ($words_per_line - 1 | math round) } else { 1 }
                    $words 
                    | chunks $chunk_size  # Account for "- " prefix
                    | enumerate
                    | each { |chunk|
                        if $chunk.index == 0 {
                            $indent + ($chunk.item | str join " ")
                        } else {
                            "  " + ($chunk.item | str join " ")  # Indent continuation
                        }
                    }
                    | str join "\n"
                } catch {
                    $line  # Fallback to original line
                }
            }
        } else {
            # Regular paragraphs - word wrap
            if ($line | str length) <= $width {
                $line  # Keep short lines as-is
            } else {
                # Safe word wrapping with error handling
                try {
                    let words = ($line | split words)
                    if ($words | is-empty) {
                        $line  # Keep original if no words
                    } else {
                        $words
                        | chunks $words_per_line 
                        | each { str join " " } 
                        | str join "\n"
                    }
                } catch {
                    $line  # Fallback to original line
                }
            }
        }
    } | str join "\n"
}