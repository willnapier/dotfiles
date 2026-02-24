# ⚠️  CLAUDE CODE: THIS FILE IS DOTTER-MANAGED - EDIT HERE IN DOTFILES, NOT ~/.config/nushell/
# ⚠️  NEVER EDIT ~/.config/nushell/env.nu - IT'S A SYMLINK TO THIS FILE
# Nushell Environment Config File

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

let current_paths = ($env.PATH | split row (char esep))
$env.PATH = ($base_paths | append $current_paths | uniq)

# --- Google Cloud & ForgePodium Integration ---
let gcloud_root = $"($env.HOME)/google-cloud-sdk"
if ($gcloud_root | path exists) {
    # Use system python3 — the bundled python path does not exist on all installs
    let python_search = (which python3)
    if ($python_search | is-not-empty) {
        $env.CLOUDSDK_PYTHON = ($python_search | get 0.path)
    }

    # ForgePodium project identity for Gemini CLI
    $env.GOOGLE_CLOUD_PROJECT = "forgepodium"
}

# --- Gemini API Key Retrieval (macOS Keychain) ---
$env.GEMINI_API_KEY = (try {
    if $platform == "darwin" {
        let result = (^security find-generic-password -s "gemini-api-key" -a "forgepodium" -w | complete)
        if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
    } else {
        ""
    }
} catch {
    ""
})

# --- OpenAI Keychain Retrieval (macOS only) ---
$env.OPENAI_API_KEY = (try {
    if $platform == "darwin" {
        let result = (^security find-generic-password -s "openai-api-key" -a "semantic-search" -w | complete)
        if $result.exit_code == 0 { $result.stdout | str trim } else { "" }
    } else {
        ""
    }
} catch {
    ""
})

# --- Theme & Visuals ---
$env.SYSTEM_THEME = if $platform == "darwin" {
    let theme_result = (^defaults read -g AppleInterfaceStyle | complete)
    if ($theme_result.stdout | str contains "Dark") { "dark" } else { "light" }
} else {
    "dark"
}

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
}
