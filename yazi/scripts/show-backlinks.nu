#!/usr/bin/env nu

# Get the current file from yazi
let current_file = $env.YAZI_FILE_ONE? | default ""

if ($current_file | is-empty) {
  print "No file selected"
  exit 1
}

# Get just the filename without extension for matching
let note_name = ($current_file | path basename | str replace '.md' '')

# Find all files that contain links to this file
let backlinks = (
  rg --files-with-matches $"\\[\\[($note_name)\\]\\]|\\[\\[($note_name)\\|" ~/Obsidian.nosync/Forge/
  | lines
  | each { |file| 
    {
      file: ($file | path basename)
      path: $file
      modified: (ls $file | get 0.modified)
    }
  }
  | sort-by modified --reverse
)

if ($backlinks | length) == 0 {
  print $"No backlinks found for ($note_name)"
  exit 0
}

print $"Backlinks to ($note_name):"
print "─────────────────────────────────"
$backlinks | each { |item| 
  print $"📄 ($item.file)"
} | str join "\n"