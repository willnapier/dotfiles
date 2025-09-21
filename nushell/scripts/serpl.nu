# ---- serpl.nu (Nushell-native with enhanced markers) ----
#
# Serpl integration for Helix-Zellij development environment
# 
# This module provides intelligent project root detection for the `serpl` 
# (search and replace) tool, automatically finding the appropriate project 
# root directory by looking for common markers like .git, .obsidian, 
# Cargo.toml, package.json, etc.
#
# Key Features:
# - Auto-detects project roots using configurable markers
# - Supports Forges, Git repos, Rust/Node.js/Python projects
# - Integrated with Zellij (Alt+r) and Helix (Space+r,r) keybindings
# - Includes safety features like git snapshots before bulk operations
#
# Available Commands:
# - serpl-anywhere [--from <path>] [--root <path>]  # Main command with options
# - serpl-at <path>                                 # Target specific path  
# - serpl-here                                      # Use current directory
# - serpl-any                                       # Alias for serpl-anywhere
#
# Configuration:
# - Root markers defined in $env.SERPL_ROOT_MARKERS
# - Customizable via Nushell config: $env.SERPL_ROOT_MARKERS = [".git", ".mymarker"]
#
# Integration:
# - Zellij: Alt+r opens serpl in floating pane
# - Helix: Space+r,r opens serpl with auto-reload and git safety
# - Standalone: ~/bin/serpl-anywhere.nu script
#

# Import unified root detection system
use project-root-detection.nu *

# Launch serpl with auto-detected root (or explicit --root)
# 
# Automatically detects the project root by walking up directories looking for
# common project markers (.git, .obsidian, Cargo.toml, package.json, etc.)
# and launches serpl from that root directory for project-wide search/replace.
#
# Examples:
#   serpl-anywhere                    # Auto-detect from current directory
#   serpl-anywhere --from ~/projects  # Auto-detect starting from ~/projects  
#   serpl-anywhere --root ~/myproject # Use specific root directory
export def serpl-anywhere [
  --from: path     # start directory for detection (defaults to $PWD)
  --root: path     # explicit root; bypass detection
]: nothing -> nothing {
  let start_dir = (if $from != null { $from | path expand } else { $env.PWD })
  let root_dir  = (if $root != null { $root | path expand } else { find-project-root $start_dir })
  cd $root_dir
  ^serpl
}

# Convenience: point at a path and detect root above it
#
# Useful when you want to run serpl on a specific file or directory
# but have it operate from the project root containing that path.
#
# Example:
#   serpl-at ~/myproject/src/main.rs  # Finds root above main.rs and runs serpl there
export def serpl-at [path: path]: nothing -> nothing {
  serpl-anywhere --from ($path | path expand)
}

# Enhanced: work with current directory (useful for Yazi integration)  
#
# Equivalent to serpl-anywhere but more explicit about using current directory.
# Particularly useful when called from file managers like Yazi.
#
# Example:
#   serpl-here  # Auto-detect project root from current directory
export def serpl-here []: nothing -> nothing {
  let current = $env.PWD
  serpl-anywhere --from $current
}