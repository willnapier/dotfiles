#!/opt/homebrew/bin/nu

# Get file path from environment variable
let file_path = $env.FILE_PATH

if ($file_path | path exists) {
    # Get match count with ripgrep
    let match_count = (try { ^rg -c "." $file_path | into int } catch { 0 })
    let file_info = (ls $file_path | get 0)
    
    print "🔍 Search Results"
    print "════════════════"
    
    # Create and display search stats table
    [{
        file: ($file_path | path basename),
        matches: $match_count,
        size: ($file_info.size),
        modified: ($file_info.modified | format date "%Y-%m-%d")
    }] | table
    
    print ""
    print "📖 Content with Context"
    print "══════════════════════"
    
    # Show content with ripgrep context
    ^rg --color=always --context=3 "." $file_path | lines | first 20
} else {
    print "❌ File not found"
}