# Dotfiles

My personal dotfiles managed with [Dotter](https://github.com/SuperCuber/dotter).

## Included Configurations

- **Nushell** - Enhanced shell with powerful data manipulation
- **Helix** - Modal text editor with LSP support
- **WezTerm** - GPU-accelerated terminal emulator
- **Yazi** - File manager with image previews
- **Git** - Version control configuration
- **Zsh** - Shell configuration

## Usage

### Prerequisites

```bash
# Install Dotter
brew install dotter
```

### Deploy

```bash
git clone https://github.com/yourusername/dotfiles.git ~/dotfiles
cd ~/dotfiles
dotter deploy
```

### Update

Edit files in the `~/dotfiles` directory, then:

```bash
cd ~/dotfiles
git add .
git commit -m "Update configs"
git push
```

Changes are automatically reflected in your system via symlinks.

## Structure

- `.dotter/` - Dotter configuration
- `nushell/` - Nushell shell configuration
- `helix/` - Helix editor configuration  
- `wezterm/` - WezTerm terminal configuration
- `yazi/` - Yazi file manager configuration
- `git/` - Git configuration
- `shell/` - Zsh shell configuration

## Features

- **CLI-first workflow** - Powerful file management via Nushell
- **Helix + Colemak-DH** - Ergonomic text editing
- **Consistent theming** - Solarized across all tools
- **Smart file handling** - Yazi integration with proper keybindings