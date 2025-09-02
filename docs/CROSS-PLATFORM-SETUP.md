# Cross-Platform Dotfiles Setup Guide

## 🖥️ Dual Environment: macOS + Arch Linux

This guide covers deploying and maintaining dotfiles across both macOS and Arch Linux systems.

## Architecture Overview

**Shared Components (Identical on Both Platforms):**
- **Core Apps**: Helix, Yazi, Zellij, Nushell
- **Configs**: All main config files use same syntax/features
- **Scripts**: Nushell scripts work identically
- **Themes**: Solarized theme system works on both

**Platform-Specific Components:**
- **macOS**: Aerospace, WezTerm, Ghostty, macOS-specific scripts
- **Arch Linux**: River, Waybar, Foot, Linux-specific scripts

## Initial Setup on New Machine

### 1. Clone Dotfiles Repository
```bash
# Clone to standard location (works on both platforms)
git clone https://github.com/your-username/dotfiles.git ~/dotfiles
cd ~/dotfiles
```

### 2. Install Dotter
**macOS:**
```bash
brew install dotter
```

**Arch Linux:**
```bash
# Install from AUR or cargo
paru -S dotter
# OR
cargo install dotter
```

### 3. Install Core Dependencies

**Shared Dependencies (Both Platforms):**
```bash
# Nushell (package managers differ)
# Helix editor
# Yazi file manager  
# Zellij terminal multiplexer
# Git
# Starship prompt
```

**macOS Specific:**
```bash
brew install nushell helix yazi zellij starship git
brew install wezterm
brew install --cask ghostty
brew install nikitabobko/tap/aerospace
```

**Arch Linux Specific:**
```bash
sudo pacman -S nushell helix git starship
paru -S yazi zellij
sudo pacman -S river waybar foot
```

### 4. Deploy Platform-Appropriate Configurations
```bash
# Dotter automatically detects platform and deploys correct configs
dotter deploy

# Verify deployment worked
dotter-verify-protection-cross-platform
```

### 5. Apply Comprehensive Protection
```bash
# Protect all platform-appropriate files from Dotter Drift
dotter-protect-all-cross-platform

# Verify protection is complete
dotter-verify-protection-cross-platform
```

## Ongoing Sync Workflow

### Making Changes
1. **Always edit in dotfiles directory** - never edit symlinked files directly
2. **Test on current platform** - ensure changes work locally
3. **Commit changes**: `git add -A && git commit -m "Description"`
4. **Push to remote**: `git push`

### Syncing to Other Machine
1. **Pull changes**: `git pull`
2. **Deploy updates**: `dotter deploy`
3. **Verify protection**: `dotter-verify-protection-cross-platform`
4. **Test functionality** - ensure everything works on target platform

### Cross-Platform Testing Checklist
- [ ] Nushell config loads without errors
- [ ] Helix opens with correct keybindings and theme
- [ ] Yazi navigation works with expected keybindings
- [ ] Zellij loads with correct layouts
- [ ] All scripts in `~/.local/bin/` are executable
- [ ] Platform-specific apps work (WezTerm vs Foot, etc.)

## Platform Detection in Scripts

All cross-platform scripts use Nushell's `sys` command for platform detection:

```nushell
let os = (sys | get host.name)

if ($os | str contains "Darwin") {
    # macOS-specific code
} else {
    # Linux-specific code  
}
```

**Benefits:**
- Single codebase for both platforms
- Automatic platform-appropriate behavior
- No shell script fragmentation
- Identical functionality where possible

## File Organization

```
dotfiles/
├── shared configs/      # Work on both platforms
│   ├── helix/
│   ├── nushell/
│   ├── yazi/
│   ├── zellij/
│   └── git/
├── macos-specific/      # macOS only
│   ├── wezterm/
│   ├── ghostty/
│   └── aerospace/
├── linux-specific/      # Arch Linux only
│   ├── wayland/
│   │   ├── river/
│   │   ├── waybar/
│   │   └── foot/
│   └── shell/bashrc
└── scripts/             # Cross-platform Nushell/Rust scripts
```

## Troubleshooting

### Dotter Deploy Fails
```bash
# Check what's conflicting
dotter deploy --dry-run

# Force overwrite if needed
dotter deploy --force
```

### Protection Status Issues
```bash
# Check protection status
dotter-verify-protection-cross-platform

# Re-apply protection if needed
dotter-protect-all-cross-platform
```

### Platform Detection Issues
```bash
# Check detected platform
nu -c "sys | get host.name"
```

### Symlink Issues
```bash
# Check if target file is properly symlinked
ls -la ~/.config/helix/config.toml

# Should show: config.toml -> /home/user/dotfiles/helix/config.toml
```

## Key Advantages of This Approach

1. **Unified Workflow**: Same commands, same configs where possible
2. **Platform Optimization**: Best tools for each platform where they differ  
3. **Dotter Drift Prevention**: Protection works on both platforms
4. **Version Control**: All changes tracked in git across both machines
5. **Easy Migration**: Full setup on new machine in minutes
6. **Consistent Experience**: Core editing/navigation identical everywhere

## Future Extensions

- **Container Development**: Dotfiles work in Docker containers
- **Remote Servers**: Core configs deployable to remote Linux servers
- **Additional Platforms**: Easy to add Windows WSL2 support
- **Team Sharing**: Others can use platform-appropriate subsets

---

*This cross-platform approach eliminates tool fragmentation while maximizing platform-appropriate functionality.*