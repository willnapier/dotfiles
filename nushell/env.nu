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

# --- Platform Detection ---
let platform = (uname | get operating-system | str downcase)

# --- SSH Agent (systemd socket-activated) ---
if $platform != "darwin" {
    $env.SSH_AUTH_SOCK = $"/run/user/(id -u | str trim)/ssh-agent.socket"
}

# --- Core Editor & Browser ---
$env.EDITOR = "hx"
$env.VISUAL = "hx"

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

# --- API Keys (cached from keychain) ---
# Direct `security find-generic-password` calls cost ~8s each on this Mac
# (permanent SEP IPC degradation from a removed VPN kernel extension).
# Calling them at every shell startup made new windows hang for ~20s.
# Instead, load from a TOML cache written by the `api-key-cache-refresh`
# script. Regenerate the cache after rotating a key.
let api_key_cache = ($env.HOME | path join ".cache" "shell" "api-keys.toml")
$env.GEMINI_API_KEY = ""
$env.OPENAI_API_KEY = ""
if ($api_key_cache | path exists) {
    let cached = (try { open $api_key_cache } catch { {} })
    $env.GEMINI_API_KEY = ($cached | get -o gemini_api_key | default "")
    $env.OPENAI_API_KEY = ($cached | get -o openai_api_key | default "")
} else if (which api-key-cache-refresh | is-not-empty) {
    # First run: populate the cache (slow, but only once). Swallow failures
    # so a broken keychain never blocks shell startup.
    try { ^api-key-cache-refresh out+err> /dev/null } catch { null }
    if ($api_key_cache | path exists) {
        let cached = (try { open $api_key_cache } catch { {} })
        $env.GEMINI_API_KEY = ($cached | get -o gemini_api_key | default "")
        $env.OPENAI_API_KEY = ($cached | get -o openai_api_key | default "")
    }
}

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

# --- Clinical Tools ---
$env.CLINICAL_NOTIFY_EMAIL = "pa@willnapier.com"

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
