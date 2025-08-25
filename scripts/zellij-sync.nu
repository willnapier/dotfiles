#!/usr/bin/env nu

# Zellij session synchronization script
# Saves and restores Zellij session state across machines

def main [
    action: string  # "save" or "restore" or "remote"
    --session (-s): string = "work"  # Session name
    --remote (-r): string = "nimbini"  # Remote host for SSH
] {
    match $action {
        "save" => save_session $session,
        "restore" => restore_session $session,
        "remote" => connect_remote $remote $session,
        _ => show_help
    }
}

# Save current Zellij session state
def save_session [session: string] {
    let config_dir = ($env.HOME | path join ".config" "zellij")
    let layouts_dir = ($config_dir | path join "layouts")
    
    # Create layouts directory if it doesn't exist
    mkdir $layouts_dir
    
    # Save current layout
    let layout_file = ($layouts_dir | path join $"($session).kdl")
    
    print $"💾 Saving Zellij session '($session)' layout..."
    
    # Dump current layout
    try {
        zellij action dump-layout | save -f $layout_file
        print $"✅ Layout saved to ($layout_file)"
    } catch {
        print "❌ Failed to save layout. Is Zellij running?"
        return
    }
    
    # Save session metadata
    let metadata = {
        session: $session,
        timestamp: (date now | format date "%Y-%m-%d %H:%M:%S"),
        machine: (sys host | get hostname),
        working_dirs: (get_working_directories)
    }
    
    let metadata_file = ($layouts_dir | path join $"($session)-metadata.json")
    $metadata | to json | save -f $metadata_file
    
    print $"📋 Session metadata saved to ($metadata_file)"
}

# Restore Zellij session from saved state
def restore_session [session: string] {
    let config_dir = ($env.HOME | path join ".config" "zellij") 
    let layouts_dir = ($config_dir | path join "layouts")
    let layout_file = ($layouts_dir | path join $"($session).kdl")
    let metadata_file = ($layouts_dir | path join $"($session)-metadata.json")
    
    if not ($layout_file | path exists) {
        print $"❌ No saved layout found for session '($session)'"
        print $"   Expected: ($layout_file)"
        return
    }
    
    print $"🔄 Restoring Zellij session '($session)'..."
    
    # Load metadata if available
    if ($metadata_file | path exists) {
        let metadata = (open $metadata_file | from json)
        print $"📅 Saved: ($metadata.timestamp) on ($metadata.machine)"
    }
    
    # Start Zellij with saved layout
    try {
        zellij --session $session --layout $layout_file
    } catch {
        print "❌ Failed to restore session. Check the layout file."
    }
}

# Connect to remote Zellij session via SSH
def connect_remote [remote: string, session: string] {
    print $"🔗 Connecting to remote Zellij session '($session)' on ($remote)..."
    
    # First, try to attach to existing session
    let attach_cmd = $"ssh ($remote) -t 'zellij attach ($session) 2>/dev/null || zellij --session ($session)'"
    
    print $"📡 Running: ($attach_cmd)"
    nu -c $attach_cmd
}

# Get working directories of all panes (placeholder)
def get_working_directories [] {
    # This would require Zellij to expose pane working directories
    # For now, return current directory
    [($env.PWD)]
}

# Show help
def show_help [] {
    print "Zellij Session Sync"
    print "==================="
    print ""
    print "Usage:"
    print "  zellij-sync save [--session work]       # Save current session"
    print "  zellij-sync restore [--session work]    # Restore saved session"  
    print "  zellij-sync remote [--remote nimbini]   # Connect to remote session"
    print ""
    print "Examples:"
    print "  zellij-sync save --session dev          # Save 'dev' session"
    print "  zellij-sync restore --session dev       # Restore 'dev' session"
    print "  zellij-sync remote --session work       # SSH to 'work' on nimbini"
}