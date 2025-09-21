# ---- project-root-detection.nu (Unified Intelligent Root Detection) ----
#
# Shared root detection system for all development tools
# 
# This module provides intelligent project boundary detection that works
# across the entire filesystem, enabling tools to automatically understand
# project scope regardless of where files are located.
#
# Supported Project Types:
# - Git repositories (.git)
# - Forges (.obsidian)  
# - Rust projects (Cargo.toml)
# - Node.js projects (package.json)
# - Python projects (pyproject.toml)
# - Go projects (go.mod)
# - Custom markers (.vault-root, .project-root)
#
# Key Functions:
# - find-project-root [path]     # Find root from any path
# - find-forge [path]   # Find specifically Forge root
# - find-git-root [path]         # Find specifically git repository root
# - get-project-type [path]      # Determine what type of project
# - list-project-markers         # Show all configured markers
#
# Integration:
# - Used by serpl for search/replace scoping
# - Used by wiki link picker for dynamic vault detection  
# - Used by gf/gx commands for project-aware file opening
# - Used by any tool needing intelligent project boundaries
#

# Default project markers - can be customized per user
export-env {
  if "PROJECT_ROOT_MARKERS" not-in $env {
    $env.PROJECT_ROOT_MARKERS = [
      # Version control systems
      ".git" ".hg" ".svn"
      
      # Knowledge management systems  
      ".obsidian" ".vault-root"
      
      # Programming language project roots
      "Cargo.toml"      # Rust
      "package.json"    # Node.js/JavaScript
      "pyproject.toml"  # Python
      "go.mod"          # Go
      "pom.xml"         # Java Maven
      "build.gradle"    # Java Gradle
      "composer.json"   # PHP
      "Gemfile"         # Ruby
      
      # Custom markers
      ".project-root"
      ".workspace-root"
    ]
  }
}

# Core root detection function - walks up directory tree
def find-project-root [start_path?: path]: nothing -> string {
  let start = ($start_path | default $env.PWD | path expand)
  
  mut current_dir = $start
  mut found = false
  
  while not $found {
    for marker in $env.PROJECT_ROOT_MARKERS {
      let marker_path = ($current_dir | path join $marker)
      if ($marker_path | path exists) {
        $found = true
        break
      }
    }
    
    if not $found {
      let parent = ($current_dir | path dirname)
      if $parent == $current_dir {
        # Hit filesystem root, return original path
        return $start
      }
      $current_dir = $parent
    }
  }
  
  $current_dir
}

# Find specifically an Forge root
export def find-forge [start_path?: path]: nothing -> string {
  let start = ($start_path | default $env.PWD | path expand)
  
  mut current_dir = $start
  mut found = false
  
  while not $found {
    let obsidian_path = ($current_dir | path join ".obsidian")
    if ($obsidian_path | path exists) and ($obsidian_path | path type) == "dir" {
      return $current_dir
    }
    
    let parent = ($current_dir | path dirname)
    if $parent == $current_dir {
      # Hit filesystem root, return empty to indicate no vault found
      return ""
    }
    $current_dir = $parent
  }
  
  "" # fallback
}

# Find specifically a git repository root
export def find-git-root [start_path?: path]: nothing -> string {
  let start = ($start_path | default $env.PWD | path expand)
  
  mut current_dir = $start
  mut found = false
  
  while not $found {
    let git_path = ($current_dir | path join ".git")
    if ($git_path | path exists) {
      return $current_dir
    }
    
    let parent = ($current_dir | path dirname)
    if $parent == $current_dir {
      # Hit filesystem root, return empty to indicate no git repo found
      return ""
    }
    $current_dir = $parent
  }
  
  "" # fallback
}

# Determine the project type based on markers found
export def get-project-type [start_path?: path]: nothing -> string {
  let root = (find-project-root $start_path)
  let start = ($start_path | default $env.PWD | path expand)
  
  # Check for specific markers in priority order
  let markers_with_types = [
    [".obsidian", "forge"]
    [".git", "git-repository"] 
    ["Cargo.toml", "rust-project"]
    ["package.json", "nodejs-project"]
    ["pyproject.toml", "python-project"]
    ["go.mod", "go-project"]
    ["pom.xml", "java-maven-project"]
    ["build.gradle", "java-gradle-project"]
    ["composer.json", "php-project"]
    ["Gemfile", "ruby-project"]
    [".vault-root", "custom-vault"]
    [".project-root", "custom-project"]
    [".workspace-root", "custom-workspace"]
  ]
  
  for marker_info in $markers_with_types {
    let marker = ($marker_info | get 0)
    let type = ($marker_info | get 1)
    let marker_path = ($root | path join $marker)
    
    if ($marker_path | path exists) {
      return $type
    }
  }
  
  "unknown-project"
}

# List all configured project markers
export def list-project-markers []: nothing -> table {
  $env.PROJECT_ROOT_MARKERS 
  | enumerate 
  | each {|item| {
      index: $item.index
      marker: $item.item
      description: (match $item.item {
        ".git" => "Git repository"
        ".hg" => "Mercurial repository" 
        ".svn" => "Subversion repository"
        ".obsidian" => "Forge"
        ".vault-root" => "Custom vault marker"
        "Cargo.toml" => "Rust project"
        "package.json" => "Node.js project"
        "pyproject.toml" => "Python project"
        "go.mod" => "Go project"
        "pom.xml" => "Java Maven project"
        "build.gradle" => "Java Gradle project"
        "composer.json" => "PHP project"
        "Gemfile" => "Ruby project"
        ".project-root" => "Custom project marker"
        ".workspace-root" => "Custom workspace marker"
        _ => "Custom marker"
      })
    }}
}

# Test function to show project detection for current location
export def show-project-info [path?: path]: nothing -> record {
  let target_path = ($path | default $env.PWD | path expand)
  let root = (find-project-root $target_path)
  let project_type = (get-project-type $target_path)
  let obsidian_vault = (find-forge $target_path)
  let git_root = (find-git-root $target_path)
  
  {
    current_path: $target_path
    project_root: $root
    project_type: $project_type
    obsidian_vault: (if $obsidian_vault == "" { null } else { $obsidian_vault })
    git_root: (if $git_root == "" { null } else { $git_root })
    is_obsidian_vault: ($obsidian_vault != "")
    is_git_repository: ($git_root != "")
  }
}

# Export main function for backward compatibility and convenience
export def find-root [path?: path]: nothing -> string {
  find-project-root $path
}