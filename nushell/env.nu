# Nushell Environment Config File
# version = "0.106.1"

# Specifies how environment variables are:
# - converted from a string to a value on Nushell startup (from_string)
# - converted from a value back to a string when running external commands (to_string)
$env.ENV_CONVERSIONS = {
    "PATH": {
        from_string: { |s| $s | split row (char esep) | path expand --no-symlink }
        to_string: { |v| $v | path expand --no-symlink | str join (char esep) }
    }
}

# Environment variables
$env.EDITOR = "hx"
$env.VISUAL = "hx"
$env.BROWSER = "open"

# PATH management
let paths_to_add = [
    $"($env.HOME)/.local/bin"
    $"($env.HOME)/.cargo/bin"
    "/opt/homebrew/bin"
    "/opt/homebrew/sbin"
]

let existing_path = ($env.PATH | split row (char esep))
let new_paths = ($paths_to_add | where { |p| ($p | path exists) and ($p not-in $existing_path) })
$env.PATH = ($new_paths | append $existing_path)

# Development environment
$env.RUST_BACKTRACE = "1"

# Obsidian vault location
$env.OBSIDIAN_VAULT = $"($env.HOME)/Obsidian.nosync/Forge"

# Nushell specific
$env.NU_LIB_DIRS = [
    ($nu.default-config-dir | path join 'scripts')
    ($nu.data-dir | path join 'completions')
]

$env.NU_PLUGIN_DIRS = [
    ($nu.default-config-dir | path join 'plugins')
]

# Solarized theme detection (macOS)
def get_macos_theme [] {
    let theme_check = (do -i { 
        ^defaults read -g AppleInterfaceStyle 
    } | complete)
    
    if $theme_check.exit_code == 0 {
        "dark"
    } else {
        "light"
    }
}

# Set theme-based environment variables
$env.MACOS_THEME = (get_macos_theme)
$env.BAT_THEME = if $env.MACOS_THEME == "dark" { "Solarized (dark)" } else { "Solarized (light)" }

# Conditional tool initialization with modern approach
# Create cache directory if it doesn't exist
let cache_dir = $"($env.HOME)/.cache/nushell"
if not ($cache_dir | path exists) {
    mkdir $cache_dir
}

# Initialize Starship prompt using vendor autoload method
if (which starship | is-not-empty) {
    let starship_cache = $"($cache_dir)/starship-init.nu"
    
    # Generate init script if it doesn't exist or is older than 24 hours
    let should_regenerate = if ($starship_cache | path exists) {
        let file_modified = (ls $starship_cache | get modified.0)
        let hours_old = ((date now) - $file_modified | into int) / 3600000000000
        $hours_old > 24
    } else {
        true
    }
    
    if $should_regenerate {
        ^starship init nu | save -f $starship_cache
    }
}

# Initialize zoxide for smarter cd
if (which zoxide | is-not-empty) {
    let zoxide_cache = $"($cache_dir)/zoxide-init.nu"
    
    # Generate init script if it doesn't exist
    if not ($zoxide_cache | path exists) {
        ^zoxide init nushell | save -f $zoxide_cache
    }
}

# FZF configuration
if (which fzf | is-not-empty) {
    $env.FZF_DEFAULT_OPTS = '--height 40% --layout=reverse --border --inline-info'
    
    # Use fd for faster file finding if available
    if (which fd | is-not-empty) {
        $env.FZF_DEFAULT_COMMAND = 'fd --type f --strip-cwd-prefix --hidden --follow --exclude .git'
        $env.FZF_CTRL_T_COMMAND = $env.FZF_DEFAULT_COMMAND
    }
}

# Vivid for better LS_COLORS
if (which vivid | is-not-empty) {
    let theme = if $env.MACOS_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}