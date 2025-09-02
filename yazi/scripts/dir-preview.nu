#!/opt/homebrew/bin/nu

# Get directory path from environment variable
let dir_path = $env.FILE_PATH

if ($dir_path | path exists) {
    let contents = (ls $dir_path)
    let file_count = ($contents | where type == file | length)
    let dir_count = ($contents | where type == dir | length) 
    let total_size = ($contents | get size | math sum)
    
    print "📁 Directory Information"
    print "═══════════════════════"
    
    # Create and display directory stats table
    [{
        directory: ($dir_path | path basename),
        files: $file_count,
        folders: $dir_count, 
        total_size: $total_size
    }] | table
    
    print ""
    print "📂 Contents"
    print "═══════════"
    
    # Show contents as a table
    $contents 
    | select name type size modified
    | first 10 
    | table
} else {
    print "❌ Directory not found"
}