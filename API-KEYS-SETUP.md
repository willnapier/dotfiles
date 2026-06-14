# API Keys Setup Guide

## ⚠️ NEVER Commit API Keys to Git!

API keys should be stored in secure environment files that are gitignored.

## 📖 Complete Documentation

For comprehensive setup instructions, cross-platform details, troubleshooting, and architecture:

**See**: [`~/Assistants/shared/API-KEYS-CROSS-PLATFORM-SETUP.md`](../Assistants/shared/API-KEYS-CROSS-PLATFORM-SETUP.md)

## Preferred: macOS Keychain / gnome-keyring (static API keys)

Static API keys (OpenRouter, OpenAI, Gemini) are **not** stored in `env-secret.nu`
as plaintext. They live in the OS keyring and are loaded via a small cache:

```
keyring (security / secret-tool)  ->  api-key-cache-refresh  ->  ~/.cache/shell/api-keys.toml (chmod 600)  ->  env.nu
```

`env.nu` loads from the cache at shell startup (a direct `security` read costs
roughly 8s on this Mac, so the cache avoids hanging every new window).

**Store / rotate a key**:
```bash
# macOS
security add-generic-password -s "openrouter-api-key" -a "goose" -w "sk-or-v1-..." -U
# Linux
secret-tool store --label="openrouter-api-key" service "openrouter-api-key" account "goose"

# then refresh the cache (run after any rotation):
api-key-cache-refresh
```

**Keyring service/account pairs** currently wired into `api-key-cache-refresh`:

| Env var | service | account |
|---------|---------|---------|
| `GEMINI_API_KEY` | `gemini-api-key` | `forgepodium` |
| `OPENAI_API_KEY` | `openai-api-key` | `semantic-search` |
| `OPENROUTER_API_KEY` | `openrouter-api-key` | `goose` |

To add a new static key: store it in the keyring, then add a fetch line +
`printf` to `scripts/api-key-cache-refresh` and a load line to `nushell/env.nu`.

> **Note on pizauth**: pizauth is an *OAuth2* token broker (mail stack). It does
> not apply to static API keys like these — use the keyring-cache pattern above.

## Legacy: env-secret.nu (OAuth-less plaintext — being phased out)

`env-secret.nu` is still sourced by `env.nu`, but should no longer hold live
static keys. It remains only for the (currently disabled) `ANTHROPIC_API_KEY`
fallback. Prefer the keyring pattern above for anything new.

### File Locations

**macOS**: `~/Library/Application Support/nushell/env-secret.nu`
**Linux**: `~/.config/nushell/env-secret.nu`

### Quick Start

**On macOS**:
```bash
hx "~/Library/Application Support/nushell/env-secret.nu"
```

**On Linux**:
```bash
hx ~/.config/nushell/env-secret.nu
```

**Add your keys** (paste real values):
```nushell
# Secure API Keys - This file should be gitignored
$env.ANTHROPIC_API_KEY = "sk-ant-api03-YOUR-ACTUAL-KEY"
$env.OPENROUTER_API_KEY = "sk-or-v1-YOUR-ACTUAL-KEY"
```

**Secure the file**:
```bash
# macOS
chmod 600 "~/Library/Application Support/nushell/env-secret.nu"

# Linux
chmod 600 ~/.config/nushell/env-secret.nu
```

**Test**:
```bash
exec nu  # Reload Nushell
echo $env.ANTHROPIC_API_KEY | str substring 0..15
```

## Integration

The `env.nu` file (in this dotfiles repo) automatically sources `env-secret.nu`:

```nushell
# Load secure environment variables
if ($"($nu.default-config-dir)/env-secret.nu" | path exists) {
    source $"($nu.default-config-dir)/env-secret.nu"
}
```

This is **already configured** in `nushell/env.nu` - no manual changes needed!

## Get API Keys

- **Anthropic**: https://console.anthropic.com/settings/keys
- **OpenRouter**: https://openrouter.ai/settings/keys

## Services Using These Keys

- **Goose** (AI assistant): Uses both Anthropic and OpenRouter
- **Custom scripts**: Any tool that reads these environment variables
- **Universal tools**: Semantic search and other AI-powered functions

## Key Rotation

**Static keys (OpenRouter/OpenAI/Gemini — keyring pattern):**
1. Get new key from provider website
2. `security add-generic-password -s <service> -a <account> -w <new-key> -U` (macOS)
   or `secret-tool store ...` (Linux), per the table above — **per machine**
3. `api-key-cache-refresh` to rewrite the cache
4. Reload shell: `exec nu`
5. Delete old key from provider dashboard

**ANTHROPIC (legacy env-secret.nu fallback, if ever re-enabled):** update
`env-secret.nu` directly, or migrate it to the keyring pattern too.

## Troubleshooting

**Keys not loading?**
- Verify file exists and has correct permissions (600)
- Check `env.nu` has the source command (should be in git already)
- Reload shell: `exec nu`

**Goose keyring errors?**
- Add `keyring: false` to `~/.config/goose/config.yaml`
- Run Goose from Nushell: `nu -l -c "goose session"`

For detailed troubleshooting, see the complete documentation.

