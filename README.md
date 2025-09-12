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

## Automation Philosophy

This dotfiles repository demonstrates two complementary approaches to workflow automation:

### 🌍 Universal Tools - Work Anywhere

Tool-independent solutions that prioritize **portability** over deep integration. These functions work on any platform, in any terminal, with any editor:

- **`fcit`** - Universal citation picker (works anywhere, any platform)
- **`fcitz`** - Universal PDF finder & opener (cross-platform file access)
- **`fwl`** - Universal wiki link picker (knowledge navigation anywhere)
- **`fsem`** - Universal semantic search (AI-powered research discovery)
- **`fsh`** - Universal file search & open (editor integration anywhere) 
- **`fsearch`** - Universal content search (vault-wide text discovery)
- **`fdur`** - Universal file duration processing (converts "t:: 1430-45" → "t:: 15min 1430-1445")

**Key Benefits**: SSH-friendly, zero setup required, works on colleagues' machines, platform agnostic

### ⚙️ Stack Integration Examples - Deep Automation Within Chosen Tools

Solutions that leverage specific tool combinations for **maximum depth** within a chosen workflow:

- **Helix Activity Watcher** (`helix-activity-watcher-renu`) - Monitors Helix process state to trigger activity duration processing only when actively editing
- **WezTerm + Nushell Integration** - Deep terminal integration with structured data processing
- **Yazi + Helix Workflow** - File manager integration with modal editor keybindings

**Key Benefits**: Tighter integration, context-aware automation, richer functionality within chosen stack

### Trade-offs & Design Philosophy

**Universal Functions** excel when you need:
- Remote work capabilities (SSH, different machines)
- Team collaboration (works on anyone's setup)
- Platform flexibility (Linux, macOS, Windows)
- Minimal dependencies

**Stack Integrations** excel when you need:
- Maximum productivity within your chosen tools
- Context-aware automation (knowing what apps are running)
- Rich data exchange between integrated tools
- Sophisticated workflow orchestration

Both approaches are **complementary, not competing**. Universal functions provide the foundation for anywhere-access, while stack integrations provide the depth for daily productivity.

### 🔄 Dual Approach Example: Activity Duration Processing

The `fdur` function perfectly demonstrates how both approaches work together:

#### Universal Function Approach
```bash
# Process specific file (works anywhere with Nushell)
fdur ~/notes/today.md

# Bulk process all activity files in directory
cd ~/notes && fdur

# Works in SSH sessions
ssh remote-server "cd ~/notes && fdur"

# Works on any platform
fdur ./activity-notes.md  # Linux, macOS, Windows
```

#### Stack Integration Approach
- **Space+p in Helix** - Seamless single-keypress processing while editing
- **Helix Activity Watcher** - Context-aware automation that triggers only during active editing sessions
- **Real-time Processing** - Automatic duration processing integrated into your editing workflow

#### Why Both Matter
- **Universal `fdur`**: Essential for bulk processing, remote work, and platform independence
- **Helix Integration**: Optimal for single-file editing with immediate visual feedback
- **Activity Watcher**: Provides context-awareness and automated triggers

This dual approach means you're never limited by context - use the universal function for flexibility, leverage the stack integration for optimal daily workflow.

## Core Features

- **CLI-first workflow** - Powerful file management via Nushell
- **Helix + Colemak-DH** - Ergonomic text editing
- **Consistent theming** - Solarized across all tools
- **Smart file handling** - Yazi integration with proper keybindings