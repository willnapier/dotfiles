#!/opt/homebrew/bin/nu

let file_path = $env.FILE_PATH

if ($file_path | path exists) {
    let file_info = (ls $file_path | get 0)
    
    print "📊 File Information"
    print "═══════════════════"
    
    # Display file info with emojis
    print $"📄 Name: ($file_path | path basename)"
    print $"📋 Type: (if ($file_path | str ends-with '.md') { 'Markdown' } else if ($file_path | str ends-with '.txt') { 'Text' } else { ($file_path | path parse | get extension) })"
    print $"📦 Size: ($file_info.size)"
    print $"📅 Modified: ($file_info.modified | format date '%m-%d %H:%M')"
    
    # Show line count for text files
    if (($file_path | str ends-with '.md') or ($file_path | str ends-with '.txt')) {
        let content = (open $file_path | lines)
        print $"📏 Lines: ($content | length)"
        
        print ""
        print "📖 Content Preview"
        print "══════════════════"
        $content | first 15
    } else {
        print ""
        print "📖 Content Preview"  
        print "══════════════════"
        if (which bat | length) > 0 {
            ^bat --style=plain --color=always --line-range=1:15 $file_path
        } else {
            ^head -15 $file_path
        }
    }
} else {
    print "❌ File not found"
}