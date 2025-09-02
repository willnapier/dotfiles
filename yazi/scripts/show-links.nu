#!/usr/bin/env nu

# Get current file from yazi
let current_file = $env.YAZI_FILE_ONE? | default ""

if ($current_file | is-empty) {
  print "No file selected"
  exit 1
}

# Check if file exists and is readable
if not ($current_file | path exists) {
  print $"File not found: ($current_file)"
  exit 1
}

# Extract all wiki-links from the file
let content = open $current_file | default ""
let links = (
  $content
  | str replace -ra '\[\[([^\]|]+)(\|[^\]]+)?\]\]' '$1'
  | lines
  | where { |line| $line | str contains "[[" }
  | each { |line|
    $line | str replace -ra '.*\[\[([^\]|]+)(\|[^\]]+)?\]\].*' '$1'
  }
  | uniq
)

# Alternative approach using rg
let links = (
  rg '\[\[([^\]|]+)(\|[^\]]+)?\]\]' $current_file --only-matching --no-line-number --replace '$1'
  | lines
  | uniq
  | each { |link|
    let file_path = $"~/Obsidian.nosync/Forge/($link).md" | path expand
    if ($file_path | path exists) {
      { link: $link, exists: true, path: $file_path }
    } else {
      { link: $link, exists: false, path: "" }
    }
  }
)

if ($links | length) == 0 {
  print $"No wiki links found in (($current_file | path basename))"
  exit 0
}

print $"Links in (($current_file | path basename)):"
print "─────────────────────────────────"
$links | each { |item|
  if $item.exists {
    print $"✅ ($item.link)"
  } else {
    print $"❌ ($item.link) \(missing\)"
  }
} | str join "\n"