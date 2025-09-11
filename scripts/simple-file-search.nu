#!/usr/bin/env nu

# Simple file search with fd + skim + helix
# Runs directly in Zellij pane with full TTY support
# Created: 2025-09-10

def main [] {
    print "🔍 Simple File Search"
    print "Using fd + skim in native environment"
    print ""
    
    # Check if we're in a reasonable directory
    let current_dir = (pwd)
    print $"📁 Searching in: (($current_dir | path basename))"
    
    # Run fd + skim with preview, then open in Helix
    let selection = try {
        fd . --type f --hidden --exclude .git --exclude node_modules --exclude target --exclude .DS_Store
        | ^env TERM=xterm-256color TERMINFO="" TERMINFO_DIRS="" skim --preview 'bat --color=always --style=numbers --line-range=:50 {}' 
               --preview-window 'right:60%:wrap'
               --prompt='📁 File Search: '
               --header='↑↓ navigate • Tab preview toggle • Enter select • Esc cancel'
               --height=90%
               --bind 'up:up,down:down,ctrl-j:down,ctrl-k:up'
        | str trim
    } catch {
        print "❌ Search cancelled or failed"
        return
    }
    
    if ($selection | is-empty) {
        print "❌ No file selected"
        return
    }
    
    print $"✅ Selected: ($selection)"
    print "🚀 Opening in Helix..."
    
    # Open in new pane and close this one
    ^zellij action new-pane -- hx ($selection)
    sleep 200ms
    ^zellij action close-pane
}