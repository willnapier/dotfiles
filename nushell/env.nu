# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Environment Config File
# version = "0.106.1"

# --- Environment Conversions ---
$env.ENV_CONVERSIONS = {
    "PATH": {
        from_string: { |s| $s | split row (char esep) | path expand --no-symlink }
        to_string: { |v| $v | path expand --no-symlink | str join (char esep) }
    }
}

# --- Core Editor & Browser ---
$env.EDITOR = "hx"
$env.VISUAL = "hx"

let platform = (uname | get operating-system | str downcase)
$env.BROWSER = if $platform == "darwin" { "open" } else { "xdg-open" }

# --- Forge & System Paths ---
$env.FORGE = $"($env.HOME)/Forge"
$env.NOTMUCH_CONFIG = $"($env.HOME)/Mail/.notmuch-config"

# --- PATH Management (Cross-Platform) ---
let base_paths = if $platform == "darwin" {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/opt/homebrew/bin"
        "/opt/homebrew/sbin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
} else {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/usr/local/bin"
        "/usr/bin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
}

# Safely merge paths
let current_paths = ($env.PATH | split row (char esep))
$env.PATH = ($base_paths | append $current_paths | uniq)

# --- Google Cloud & ForgePodium Integration ---
# This fixes the "No such file or directory" error by using your active Python
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
if ($gcloud_root | path exists) {
    # Dynamically find the best Python 3 available
    let python_path = (which python3 | get 0.path)
    $env.CLOUDSDK_PYTHON = $python_path
    
    # Priority Identity for Gemini CLI
    $env.GOOGLE_CLOUD_PROJECT = "forgepodium"
    $env.GOOGLE_GENAI_USE_VERTEXAI = "true"
}

# --- OpenAI Keychain Retrieval ---
$env.OPENAI_API_KEY = (try {
    let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
    if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
} catch { "" })

# --- Theme & Visuals ---
$env.SYSTEM_THEME = if $platform == "darwin" {
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if ($theme_result.stdout | str contains "Dark") { "dark" } else { "light" }
} else { "dark" }

if (which vivid | is-not-empty) {
    let theme = if $env.SYSTEM_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}

# --- Tools Initialization ---
if (which zoxide | is-not-empty) {
    zoxide init nushell | save -f ~/.zoxide.nu
    # source ~/.zoxide.nu # Note: Usually sourced in config.nu
}

# Skim (sk) configuration
if (which sk | is-not-empty) {
    $env.SKIM_DEFAULT_OPTIONS = if $env.SYSTEM_THEME == "dark" { "--color=dark" } else { "--color=light" }
    $env.SKIM_DEFAULT_COMMAND = "fd --type f --strip-cwd-prefix --hidden --follow --exclude .git"
}

# --- Secrets & External Auth ---
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}

if ($"($env.HOME)/.claude/auth-token.txt" | path exists) {
    $env.CLAUDE_CODE_OAUTH_TOKEN = (open ~/.claude/auth-token.txt | str trim)
}# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
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

# Cross-platform browser detection
let platform = (uname | get operating-system | str downcase)
$env.BROWSER = if $platform == "darwin" {
    "open"
} else if ($platform | str contains "linux") {
    "xdg-open"
} else {
    "xdg-open"  # Default fallback
}

# Forge knowledge base path (cross-platform)
$env.FORGE = $"($env.HOME)/Forge"

# Notmuch mail config
$env.NOTMUCH_CONFIG = $"($env.HOME)/Mail/.notmuch-config"

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

# PATH management - cross-platform with bootstrap fix for Linux
let platform = (uname | get operating-system | str downcase)

# Bootstrap fix: Ensure essential paths are available first
if $platform == "linux" {
    # Force critical paths for Nushell and scripts to be available
    let bootstrap_paths = [
        $"($env.HOME)/.cargo/bin"
        $"($env.HOME)/.local/bin"
    ]
    let current_path = ($env.PATH | str join ":")
    let bootstrap_path_str = ($bootstrap_paths | str join ":")
    $env.PATH = ($bootstrap_path_str + ":" + $current_path | split row ":" | uniq | str join ":")
}

let paths_to_add = if $platform == "darwin" {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/opt/homebrew/bin"
        "/opt/homebrew/sbin"
    ]
} else {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/usr/local/bin"
    ]
}

let existing_path = ($env.PATH | split row (char esep))
$env.PATH = ($paths_to_add | append $existing_path | uniq | str join (char esep))

# Cross-platform theme detection
let platform = (uname | get operating-system | str downcase)
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
$env.THEME_PREFERENCE = "dark"

# API Keys - Set these in your shell's secure environment configuration
# DO NOT commit API keys to git!
# Example: Add to ~/.zshenv or ~/.config/nushell/env-secret.nu (gitignored)
# $env.ANTHROPIC_API_KEY = "sk-ant-..."
# $env.OPENROUTER_API_KEY = "sk-or-..."
# Load secure environment variables
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}

# Claude Code OAuth token for headless/SSH sessions
if ($"($env.HOME)/.claude/auth-token.txt" | path exists) {
    $env.CLAUDE_CODE_OAUTH_TOKEN = (open ~/.claude/auth-token.txt | str trim)
}
# Point gcloud to its own bundled Python (adjust path if you installed elsewhere)
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
$env.PATH = ($env.PATH | append $"($gcloud_root)/bin" | uniq)

# This prevents gcloud from looking at your system Python 3.9 or 3.15
$env.CLOUDSDK_PYTHON = $"($gcloud_root)/platform/bundledpython/bin/python3"# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Environment Config File
# version = "0.106.1"

# --- Environment Conversions ---
$env.ENV_CONVERSIONS = {
    "PATH": {
        from_string: { |s| $s | split row (char esep) | path expand --no-symlink }
        to_string: { |v| $v | path expand --no-symlink | str join (char esep) }
    }
}

# --- Core Editor & Browser ---
$env.EDITOR = "hx"
$env.VISUAL = "hx"

let platform = (uname | get operating-system | str downcase)
$env.BROWSER = if $platform == "darwin" { "open" } else { "xdg-open" }

# --- Forge & System Paths ---
$env.FORGE = $"($env.HOME)/Forge"
$env.NOTMUCH_CONFIG = $"($env.HOME)/Mail/.notmuch-config"

# --- PATH Management (Cross-Platform) ---
let base_paths = if $platform == "darwin" {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/opt/homebrew/bin"
        "/opt/homebrew/sbin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
} else {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/usr/local/bin"
        "/usr/bin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
}

# Safely merge paths
let current_paths = ($env.PATH | split row (char esep))
$env.PATH = ($base_paths | append $current_paths | uniq)

# --- Google Cloud & ForgePodium Integration ---
# This fixes the "No such file or directory" error by using your active Python
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
if ($gcloud_root | path exists) {
    # Dynamically find the best Python 3 available
    let python_path = (which python3 | get 0.path)
    $env.CLOUDSDK_PYTHON = $python_path
    
    # Priority Identity for Gemini CLI
    $env.GOOGLE_CLOUD_PROJECT = "forgepodium"
    $env.GOOGLE_GENAI_USE_VERTEXAI = "true"
}

# --- OpenAI Keychain Retrieval ---
$env.OPENAI_API_KEY = (try {
    let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
    if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
} catch { "" })

# --- Theme & Visuals ---
$env.SYSTEM_THEME = if $platform == "darwin" {
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if ($theme_result.stdout | str contains "Dark") { "dark" } else { "light" }
} else { "dark" }

if (which vivid | is-not-empty) {
    let theme = if $env.SYSTEM_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}

# --- Tools Initialization ---
if (which zoxide | is-not-empty) {
    zoxide init nushell | save -f ~/.zoxide.nu
    # source ~/.zoxide.nu # Note: Usually sourced in config.nu
}

# Skim (sk) configuration
if (which sk | is-not-empty) {
    $env.SKIM_DEFAULT_OPTIONS = if $env.SYSTEM_THEME == "dark" { "--color=dark" } else { "--color=light" }
    $env.SKIM_DEFAULT_COMMAND = "fd --type f --strip-cwd-prefix --hidden --follow --exclude .git"
}

# --- Secrets & External Auth ---
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}

if ($"($env.HOME)/.claude/auth-token.txt" | path exists) {
    $env.CLAUDE_CODE_OAUTH_TOKEN = (open ~/.claude/auth-token.txt | str trim)
}# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Environment Config File
# version = "0.106.1"

# --- Environment Conversions ---
$env.ENV_CONVERSIONS = {
    "PATH": {
        from_string: { |s| $s | split row (char esep) | path expand --no-symlink }
        to_string: { |v| $v | path expand --no-symlink | str join (char esep) }
    }
}

# --- Core Editor & Browser ---
$env.EDITOR = "hx"
$env.VISUAL = "hx"

let platform = (uname | get operating-system | str downcase)
$env.BROWSER = if $platform == "darwin" { "open" } else { "xdg-open" }

# --- Forge & System Paths ---
$env.FORGE = $"($env.HOME)/Forge"
$env.NOTMUCH_CONFIG = $"($env.HOME)/Mail/.notmuch-config"

# --- PATH Management (Cross-Platform) ---
let base_paths = if $platform == "darwin" {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/opt/homebrew/bin"
        "/opt/homebrew/sbin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
} else {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/usr/local/bin"
        "/usr/bin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
}

# Safely merge paths
let current_paths = ($env.PATH | split row (char esep))
$env.PATH = ($base_paths | append $current_paths | uniq)

# --- Google Cloud & ForgePodium Integration ---
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
if ($gcloud_root | path exists) {
    # Use system python3 to avoid "bundledpython" errors
    let python_path = (which python3 | get 0.path)
    $env.CLOUDSDK_PYTHON = $python_path
    
    # Priority Identity for Gemini CLI
    $env.GOOGLE_CLOUD_PROJECT = "forgepodium"
    $env.GOOGLE_GENAI_USE_VERTEXAI = "true"
}

# --- OpenAI Keychain Retrieval ---
$env.OPENAI_API_KEY = (try {
    let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
    if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
} catch { "" })

# --- Theme & Visuals ---
$env.SYSTEM_THEME = if $platform == "darwin" {
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if ($theme_result.stdout | str contains "Dark") { "dark" } else { "light" }
} else { "dark" }

if (which vivid | is-not-empty) {
    let theme = if $env.SYSTEM_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}

# --- Tools Initialization ---
if (which zoxide | is-not-empty) {
    zoxide init nushell | save -f ~/.zoxide.nu
}

# Skim (sk) configuration
if (which sk | is-not-empty) {
    $env.SKIM_DEFAULT_OPTIONS = if $env.SYSTEM_THEME == "dark" { "--color=dark" } else { "--color=light" }
    $env.SKIM_DEFAULT_COMMAND = "fd --type f --strip-cwd-prefix --hidden --follow --exclude .git"
}

# --- Secrets & External Auth ---
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}

if ($"($env.HOME)/.claude/auth-token.txt" | path exists) {
    $env.CLAUDE_CODE_OAUTH_TOKEN = (open ~/.claude/auth-token.txt | str trim)
# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Environment Config File
# version = "0.106.1"

# --- Environment Conversions ---
$env.ENV_CONVERSIONS = {
    "PATH": {
        from_string: { |s| $s | split row (char esep) | path expand --no-symlink }
        to_string: { |v| $v | path expand --no-symlink | str join (char esep) }
    }
}

# --- Core Editor & Browser ---
$env.EDITOR = "hx"
$env.VISUAL = "hx"

let platform = (uname | get operating-system | str downcase)
$env.BROWSER = if $platform == "darwin" { "open" } else { "xdg-open" }

# --- Forge & System Paths ---
$env.FORGE = $"($env.HOME)/Forge"
$env.NOTMUCH_CONFIG = $"($env.HOME)/Mail/.notmuch-config"

# --- PATH Management (Cross-Platform) ---
let base_paths = if $platform == "darwin" {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/opt/homebrew/bin"
        "/opt/homebrew/sbin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
} else {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/usr/local/bin"
        "/usr/bin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
}

# Safely merge paths
let current_paths = ($env.PATH | split row (char esep))
$env.PATH = ($base_paths | append $current_paths | uniq)

# --- Google Cloud & ForgePodium Integration ---
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
if ($gcloud_root | path exists) {
    # Dynamically find system python3 to avoid "bundledpython" errors
    let python_search = (which python3)
    if ($python_search | is-not-empty) {
        $env.CLOUDSDK_PYTHON = ($python_search | get 0.path)
    }
    
    # Priority Identity for Gemini CLI
    $env.GOOGLE_CLOUD_PROJECT = "forgepodium"
    $env.GOOGLE_GENAI_USE_VERTEXAI = "true"
}

# --- OpenAI Keychain Retrieval ---
$env.OPENAI_API_KEY = (try {
    if $platform == "darwin" {
        let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
        if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
    } else { "" }
} catch { "" })

# --- Theme & Visuals ---
$env.SYSTEM_THEME = if $platform == "darwin" {
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if ($theme_result.stdout | str contains "Dark") { "dark" } else { "light" }
} else { "dark" }

if (which vivid | is-not-empty) {
    let theme = if $env.SYSTEM_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}

# --- Tools Initialization ---
if (which zoxide | is-not-empty) {
    zoxide init nushell | save -f ~/.zoxide.nu
}

# Skim (sk) configuration
if (which sk | is-not-empty) {
    $env.SKIM_DEFAULT_OPTIONS = if $env.SYSTEM_THEME == "dark" { "--color=dark" } else { "--color=light" }
    $env.SKIM_DEFAULT_COMMAND = "fd --type f --strip-cwd-prefix --hidden --follow --exclude .git"
}

# --- Secrets & External Auth ---
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}

if ($"($env.HOME)/.claude/auth-token.txt" | path exists) {
    $env.CLAUDE_CODE_OAUTH_TOKEN = (open ~/.claude/auth-token.txt | str trim)
# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Environment Config File
# version = "0.106.1"

# --- Environment Conversions ---
$env.ENV_CONVERSIONS = {
    "PATH": {
        from_string: { |s| $s | split row (char esep) | path expand --no-symlink }
        to_string: { |v| $v | path expand --no-symlink | str join (char esep) }
    }
}

# --- Core Editor & Browser ---
$env.EDITOR = "hx"
$env.VISUAL = "hx"

let platform = (uname | get operating-system | str downcase)
$env.BROWSER = if $platform == "darwin" { "open" } else { "xdg-open" }

# --- Forge & System Paths ---
$env.FORGE = $"($env.HOME)/Forge"
$env.NOTMUCH_CONFIG = $"($env.HOME)/Mail/.notmuch-config"

# --- PATH Management (Cross-Platform) ---
let base_paths = if $platform == "darwin" {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/opt/homebrew/bin"
        "/opt/homebrew/sbin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
} else {
    [
        $"($env.HOME)/.local/bin"
        $"($env.HOME)/.cargo/bin"
        "/usr/local/bin"
        "/usr/bin"
        $"($env.HOME)/google-cloud-sdk/bin"
    ]
}

# Safely merge paths
let current_paths = ($env.PATH | split row (char esep))
$env.PATH = ($base_paths | append $current_paths | uniq)

# --- Google Cloud & ForgePodium Integration ---
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
if ($gcloud_root | path exists) {
    # Dynamically find system python3 to avoid "bundledpython" errors
    let python_search = (which python3)
    if ($python_search | is-not-empty) {
        $env.CLOUDSDK_PYTHON = ($python_search | get 0.path)
    }
    
    # Priority Identity for Gemini CLI
    $env.GOOGLE_CLOUD_PROJECT = "forgepodium"
    $env.GOOGLE_GENAI_USE_VERTEXAI = "true"
}

# --- OpenAI Keychain Retrieval ---
$env.OPENAI_API_KEY = (try {
    if $platform == "darwin" {
        let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
        if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
    } else { "" }
} catch { "" })

# --- Theme & Visuals ---
$env.SYSTEM_THEME = if $platform == "darwin" {
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if ($theme_result.stdout | str contains "Dark") { "dark" } else { "light" }
} else { "dark" }

if (which vivid | is-not-empty) {
    let theme = if $env.SYSTEM_THEME == "dark" { "solarized-dark" } else { "solarized-light" }
    $env.LS_COLORS = (^vivid generate $theme)
}

# --- Tools Initialization ---
if (which zoxide | is-not-empty) {
    zoxide init nushell | save -f ~/.zoxide.nu
}

# Skim (sk) configuration
if (which sk | is-not-empty) {
    $env.SKIM_DEFAULT_OPTIONS = if $env.SYSTEM_THEME == "dark" { "--color=dark" } else { "--color=light" }
    $env.SKIM_DEFAULT_COMMAND = "fd --type f --strip-cwd-prefix --hidden --follow --exclude .git"
}

# --- Secrets & External Auth ---
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}

if ($"($env.HOME)/.claude/auth-token.txt" | path exists) {
    $env.CLAUDE_CODE_OAUTH_TOKEN = (open ~/.claude/auth-token.txt | str trim)
}}}
