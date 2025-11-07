# API Keys Setup Guide

## ⚠️ NEVER Commit API Keys to Git!

API keys should be stored in secure environment files that are gitignored.

## 📖 Complete Documentation

For comprehensive setup instructions, cross-platform details, troubleshooting, and architecture:

**See**: [`~/Assistants/shared/API-KEYS-CROSS-PLATFORM-SETUP.md`](../Assistants/shared/API-KEYS-CROSS-PLATFORM-SETUP.md)

## Quick Setup (Nushell)

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

1. Get new key from provider website
2. Update `env-secret.nu` on **all machines**
3. Reload shell: `exec nu`
4. Delete old key from provider dashboard

## Troubleshooting

**Keys not loading?**
- Verify file exists and has correct permissions (600)
- Check `env.nu` has the source command (should be in git already)
- Reload shell: `exec nu`

**Goose keyring errors?**
- Add `keyring: false` to `~/.config/goose/config.yaml`
- Run Goose from Nushell: `nu -l -c "goose session"`

For detailed troubleshooting, see the complete documentation.

