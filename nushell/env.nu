# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
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

# Obsidian vault path for notes command
$env.OBSIDIAN_VAULT = "/Users/williamnapier/Obsidian.nosync/Forge"

# OpenAI API key for semantic search system (retrieved from keychain)
$env.OPENAI_API_KEY = (try {
    let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
    if $result.exit_code == 0 {
        $result.stdout | str trim
    } else {
        ""
    }
} catch {
    ""
})

# PATH management
let paths_to_add = [
    $"($env.HOME)/.local/bin"
    $"($env.HOME)/.cargo/bin"
    "/opt/homebrew/bin"
    "/opt/homebrew/sbin"
]

let existing_path = ($env.PATH | split row (char esep))
$env.PATH = ($paths_to_add | append $existing_path | uniq | str join (char esep))

# Cross-platform theme detection
let platform = (uname | str downcase)
$env.SYSTEM_THEME = if $platform == "darwin" {
    # macOS theme detection
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if $theme_result.exit_code == 0 {
        let theme_value = ($theme_result.stdout | str trim)
        if ($theme_value | str contains "Dark") { "dark" } else { "light" }
    } else {
        "light"  # Default to light if command fails
    }
} else {
    # Linux/other platforms - default to dark (can be enhanced later)
    "dark"
}

# Zoxide initialization
if (which zoxide | is-not-empty) {
    zoxide init nushell | save -f ~/.zoxide.nu
    source ~/.zoxide.nu
}

# Skim (sk) configuration - preferred fuzzy finder
if (which sk | is-not-empty) {
    let sk_base_command = "fd --type f --strip-cwd-prefix --hidden --follow --exclude .git"

    if $env.SYSTEM_THEME == "dark" {
        $env.SKIM_DEFAULT_OPTIONS = "--color=dark"
    } else {
        $env.SKIM_DEFAULT_OPTIONS = "--color=light"
    }

    $env.SKIM_DEFAULT_COMMAND = $sk_base_command
}

# Vivid for better LS_COLORS
if (which vivid | is-not-empty) {
    let theme = if $env.SYSTEM_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}
