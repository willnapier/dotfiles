# 🐧 LINUX MIGRATION GUIDE - Enhanced with Comprehensive Dotter Management

**Status**: Ready for seamless cross-platform transition with bulletproof configuration management

## Overview

This guide covers migrating the entire macOS development environment to Arch Linux while maintaining 100% configuration consistency through the comprehensive Dotter management system.

## 🛡️ Pre-Migration: Dotter Management Verification

### Complete Configuration Coverage Achieved

Before migrating, we've established comprehensive Dotter management ensuring ZERO configuration drift during the transition:

**✅ FULLY PROTECTED CONFIGS (33+ files)**:
- **Editors**: Helix config, themes, scripts + Neovim complete setup
- **Shells**: Nushell config/env (identical syntax across platforms)
- **Terminal**: Zellij config + all layouts  
- **File Manager**: Yazi config, themes, plugins, scripts
- **Essential Scripts**: Daily notes, wiki links, citation picker, semantic search

### Migration-Safe Verification Commands

```bash
# Pre-migration verification
dotter-complete-audit                    # Verify ALL configs managed
dotter-enforce-compliance --dry-run      # Check for any drift
claude-diagnostic-auto "pre-migration" "dotter"  # Full system check

# Commit current state
cd ~/dotfiles && git add . && git commit -m "Pre-Linux migration state"
```

## 🔄 Cross-Platform Dotter Configuration

### Current Linux Support (Already Configured)

The Dotter configuration already includes comprehensive Linux support:

```toml
# Linux-specific configuration  
[linux]
depends = ["shared"]

[linux.files]
"shell/bashrc" = "~/.bashrc"
"wayland/river" = { target = "~/.config/river", type = "symbolic" }
"wayland/waybar" = { target = "~/.config/waybar", type = "symbolic" }
"wayland/foot" = { target = "~/.config/foot", type = "symbolic" }

[linux.variables]
opener_cmd = "xdg-open"
clipboard_copy = "wl-copy"
clipboard_paste = "wl-paste" 
terminal_emulator = "foot"
platform = "linux"
nushell_path = "/usr/bin/nu"
```

### Shared Cross-Platform Configs

All core configurations work identically on both platforms:

- **Nushell**: Identical syntax, just different binary path
- **Helix**: Full cross-platform editor configuration
- **Yazi**: File manager with same keybindings and themes
- **Zellij**: Terminal multiplexer with same layouts
- **Neovim**: Complete Lua configuration works identically

## 📦 Arch Linux Package Requirements

### Core Development Environment

```bash
# Essential development tools
sudo pacman -S base-devel git curl wget unzip

# Rust ecosystem
sudo pacman -S rustup
rustup default stable

# Nushell shell
sudo pacman -S nushell

# Modern terminal tools
sudo pacman -S helix          # Text editor
sudo pacman -S yazi           # File manager  
sudo pacman -S zellij         # Terminal multiplexer
sudo pacman -S starship       # Prompt
sudo pacman -S bat            # Enhanced cat
sudo pacman -S fd             # Enhanced find
sudo pacman -S ripgrep        # Enhanced grep
sudo pacman -S skim           # Fuzzy finder
sudo pacman -S zoxide         # Enhanced cd

# Wayland compositor and tools
sudo pacman -S river waybar   # Compositor and status bar
sudo pacman -S foot           # Wayland terminal
sudo pacman -S wl-clipboard   # Clipboard tools

# Media and preview tools
sudo pacman -S chafa ffmpegthumbnailer poppler p7zip unar
```

### AUR Packages (using yay)

```bash
# Install AUR helper
git clone https://aur.archlinux.org/yay.git
cd yay && makepkg -si

# Dotter configuration manager
yay -S dotter

# Additional development tools
yay -S neovim-git            # Latest Neovim
yay -S ghostty               # Modern terminal (if available)
```

## 🚀 Migration Process

### Step 1: Clone Dotfiles

```bash
# Clone dotfiles repository
git clone https://github.com/your-username/dotfiles.git ~/dotfiles
cd ~/dotfiles

# Verify Dotter configuration
dotter --help
```

### Step 2: Deploy Linux Configuration

```bash
# Deploy Linux-specific configuration
cd ~/dotfiles
dotter deploy linux

# Verify deployment
dotter-complete-audit  # Should show all files properly managed
```

### Step 3: Install Development Tools

```bash
# Install Rust toolchain
rustup default stable
rustup component add rust-src rust-analyzer

# Install Node.js (for Neovim LSP servers)
sudo pacman -S nodejs npm

# Python development
sudo pacman -S python python-pip python-pynvim

# Ruby (for Neovim provider)
sudo pacman -S ruby
gem install neovim
```

### Step 4: Configure Shell

```bash
# Set Nushell as default shell
chsh -s /usr/bin/nu

# Source configuration (should work immediately via Dotter)
exec nu
```

### Step 5: Verification

```bash
# Verify all tools work
helix --version
yazi --version  
zellij --version
starship --version

# Check Dotter management
dotter-complete-audit

# Verify no drift occurred during migration
claude-diagnostic-auto "post-migration" "all"
```

## 🎨 Wayland-Specific Configuration

### River Window Manager Setup

```bash
# River configuration (already managed by Dotter)
# Files: ~/dotfiles/wayland/river/

# Start River session
echo 'exec river' >> ~/.xprofile

# Configure autostart applications
# (Configuration files deployed via Dotter)
```

### Waybar Status Bar

```bash
# Waybar configuration (already managed by Dotter)
# Files: ~/dotfiles/wayland/waybar/

# Auto-launches with River
# Styled to match Solarized theme system
```

### Foot Terminal Emulator

```bash
# Foot configuration (already managed by Dotter)  
# Files: ~/dotfiles/wayland/foot/

# Features automatic Solarized theme switching
# Integrates with system-wide theme management
```

## 📊 Cross-Platform Theme Management

### Automated Theme Switching

The Linux configuration includes the same intelligent theme switching:

```bash
# System theme detection (cross-platform)
get-system-theme    # Works on both macOS and Linux

# Manual theme switching
sync-themes         # Updates all applications

# Automatic switching
# (Configured via systemd user services on Linux)
```

### Solarized Consistency

All applications maintain consistent Solarized theming:
- **Terminal**: Foot with Solarized colors
- **Editor**: Helix with Solarized themes
- **File Manager**: Yazi with Solarized flavors
- **Multiplexer**: Zellij with Solarized themes
- **Status Bar**: Waybar with Solarized colors

## 🔧 Platform-Specific Differences

### File Paths

```bash
# macOS → Linux mappings (handled automatically by Dotter)
~/Library/Application\ Support/  → ~/.config/
pbcopy/pbpaste                   → wl-copy/wl-paste
open                             → xdg-open
/opt/homebrew/bin/nu             → /usr/bin/nu
```

### Clipboard Integration

```bash
# Linux clipboard (Wayland)
echo "text" | wl-copy      # Copy to clipboard  
wl-paste                   # Paste from clipboard

# Scripts automatically detect platform and use correct commands
```

### Application Launchers

```bash
# macOS: System applications
open file.pdf              → xdg-open file.pdf
open /Applications/App.app  → gtk-launch app-name

# Platform detection built into scripts
```

## 🛠️ Development Environment

### LSP Servers (Cross-Platform)

```bash
# Language servers (identical installation on both platforms)
# Rust
rustup component add rust-analyzer

# Nushell  
cargo install nu-lsp

# Python
pip install python-lsp-server

# Configuration managed by Dotter - works identically
```

### Git Configuration

```bash
# Git config (identical via Dotter)
# File: ~/dotfiles/git/gitconfig

# SSH keys (manual migration)
scp user@mac:~/.ssh/id_* ~/.ssh/
chmod 600 ~/.ssh/id_*
```

## 🚀 Post-Migration Optimization

### Performance Tuning

```bash
# SSD optimization
echo 'noatime,discard' >> /etc/fstab  # (requires root)

# Zsh/Nu shell completion caching  
# (Automatically configured via dotfiles)

# Neovim lazy loading
# (Already optimized in dotfiles configuration)
```

### System Integration

```bash
# Systemd user services for automation
systemctl --user enable theme-switcher.service
systemctl --user start theme-switcher.service

# Desktop file associations  
# (Configured via Dotter-managed files)
```

## 🔍 Troubleshooting

### Common Migration Issues

```bash
# Config not found
dotter-complete-audit              # Check Dotter coverage
dotter deploy linux --force       # Force re-deployment

# Command not found  
claude-diagnostic-auto "command not found: cmd" "cmd"

# Theme not switching
sync-themes                       # Manual theme sync
systemctl --user restart theme-switcher
```

### Verification Commands

```bash
# Complete system check
dotter-complete-audit             # All configs managed
dotter-enforce-compliance --dry-run  # No drift detected
claude-diagnostic-auto "system-check" "all"  # No issues

# Application functionality
helix ~/.config/helix/config.toml  # Editor works
yazi ~/                           # File manager works  
zellij --version                  # Multiplexer works
```

## 📋 Migration Checklist

### Pre-Migration
- [ ] Run `dotter-complete-audit` on macOS
- [ ] Commit all dotfiles changes to git
- [ ] Export essential data (GPG keys, SSH keys, etc.)
- [ ] Document any manual configurations not in dotfiles

### During Migration  
- [ ] Install Arch Linux with desired desktop environment
- [ ] Install essential packages via pacman/AUR
- [ ] Clone dotfiles repository
- [ ] Deploy Linux configuration via Dotter
- [ ] Import SSH/GPG keys

### Post-Migration
- [ ] Run `dotter-complete-audit` on Linux
- [ ] Verify all applications work correctly
- [ ] Test cross-platform scripts and functions
- [ ] Commit any Linux-specific configuration additions

## 🎯 Expected Outcome

**Complete Environment Consistency**: The Linux system will be functionally identical to macOS thanks to:

1. **Identical Configuration**: All dotfiles managed by Dotter work identically
2. **Same Workflow**: Nushell functions, Helix bindings, Yazi navigation unchanged
3. **Consistent Theming**: Solarized themes across all applications  
4. **Automated Management**: Same drift prevention and monitoring tools
5. **Zero Reconfiguration**: Development environment works immediately

The migration will be seamless because every relevant configuration file is already under comprehensive Dotter management, ensuring perfect consistency across platforms.

---

*Last updated: 2025-09-02*
*Migration-ready with comprehensive Dotter management system*