#!/opt/homebrew/bin/nu

let file_path = $env.FILE_PATH

if ($file_path | path exists) {
    let content = (open $file_path | lines)
    let file_info = (ls $file_path | get 0)
    let word_count = ($content | str join ' ' | str replace -a '\n' ' ' | split row ' ' | where $it != '' | length)
    
    print "📊 Note Statistics"
    print "══════════════════"
    
    # Simple approach - print each stat
    print $"📄 Note: ($file_path | path basename | str replace '.md' '')"
    print $"📏 Lines: ($content | length)"
    print $"📝 Words: ($word_count)"
    print $"📦 Size: ($file_info.size)"
    print $"📅 Modified: ($file_info.modified | format date '%Y-%m-%d')"
    
    print ""
    print "📖 Content Preview"
    print "══════════════════"
    
    # Show first 15 lines
    $content | first 15
} else {
    print "❌ Note not found"
}