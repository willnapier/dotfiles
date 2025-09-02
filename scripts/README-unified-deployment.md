# Unified Development Environment Deployment

**Complete cross-platform deployment of intelligent project-aware development tools**

## What This Deploys

### 🦀 **Complete Rust Development Stack**
- **Core tools**: Nushell, Helix, Yazi, Zellij, WezTerm, Starship
- **Essential utilities**: serpl, ripgrep, fd, skim, bat, eza, zoxide
- **Productivity tools**: git-delta, tealdeer, tokei
- **Cross-platform compatibility** - Identical setup on macOS and Arch Linux

### 🧠 **Unified Intelligent Root Detection System**
- **Automatic project boundary detection** - Works with Git repos, Obsidian vaults, Rust/Node.js/Python projects, and custom markers
- **Cross-filesystem intelligence** - Tools adapt to YOUR organization, not the other way around
- **Configurable markers** - Add your own project types easily

### 🔧 **Integrated Tools**
- **serpl integration** - Project-scoped search and replace
- **Dynamic wiki links** - Find Obsidian vaults automatically from anywhere
- **Smart file opening** - Context-aware gf/gx commands in Helix
- **Citation management** - Auto-locate citations.md in any vault
- **Cross-platform URL handling** - Opens URLs correctly on macOS/Linux

### ⌨️ **Keybindings** 
- **Zellij**: `Alt+r` → serpl in floating pane
- **Helix**: `Space+r,r` → serpl with git safety and auto-reload
- **Helix**: `gf` → intelligent file opening (wiki links, file paths, URLs)
- **Helix**: `gx` → cross-platform URL opening
- **Helix**: `Alt+l` → dynamic wiki link picker (any vault, anywhere)

## Quick Start

### 1. **Deploy the System**
```bash
# Run the deployment script
~/dotfiles/scripts/deploy-unified-dev-environment
```

### 2. **Test the Installation**
```bash
# Verify everything works
~/dotfiles/scripts/test-unified-system
```

### 3. **Reload Your Shell**
```bash
# Start new Nushell session or source config
nu
# OR
source ~/.config/nushell/config.nu
```

## Platform Support

### ✅ **macOS** (Primary Platform)
- Full automatic dependency installation via Homebrew
- All features supported and tested
- Native clipboard and URL handling

### ✅ **Arch Linux** (Secondary Platform) 
- Automatic dependency installation via pacman
- All core features supported
- XDG URL handling
- **Pre-installed tools**: Keep existing Helix/WezTerm installations - the script uses `--needed` flag to avoid conflicts

### ⚠️ **Other Linux Distributions**
- Manual dependency installation required
- Core features will work once dependencies are met
- Generic Linux URL handling

## Pre-Existing Installations

### **If You Already Have Some Tools Installed**
**✅ Recommended**: Keep existing installations and run the deployment script

**Why this works safely:**
- Script uses `--needed` flags to skip already-installed packages
- Config integration works regardless of installation method
- Saves download/compilation time
- No package conflicts

**Example behavior:**
```bash
sudo pacman -S --needed helix  # → "helix is up to date -- skipping"
```

### **When You Might Want to Update First**
Only if existing versions are very outdated:
```bash
# Check current versions
helix --version
wezterm --version

# Update if desired (before deployment)
sudo pacman -Syu helix
yay -Syu wezterm
```

## Required Dependencies

### **Essential Tools** (auto-installed on macOS/Arch)
- `serpl` - Search and replace tool
- `ripgrep` (rg) - Fast text search
- `fd` - Fast file finder
- `nushell` - Shell and scripting language
- `helix` - Text editor
- `yazi` - File manager

### **Optional Tools** (enhanced experience)
- `skim` (sk) - Fuzzy finder for interactive selection
- `bat` - Syntax highlighting for previews
- `eza` - Enhanced directory listings
- `zoxide` - Smart directory jumping
- `starship` - Enhanced prompt

## Customization

### **Add Custom Project Markers**
Add to your `~/.config/nushell/config.nu`:
```nushell
$env.PROJECT_ROOT_MARKERS = [
    ".git" ".obsidian"           # Keep essentials
    ".my-workspace"              # Custom workspace marker
    ".research-project"          # Research directories  
    "justfile"                   # Just command runner projects
    "Cargo.toml" "package.json"  # Language projects
]
```

### **Test Your Customization**
```bash
nu -c "show-project-info /path/to/your/directory"
```

## Usage Examples

### **Global Search and Replace**
```bash
# From anywhere in a project - automatically scopes to project root
Alt+r (in Zellij) → opens serpl
Space+r,r (in Helix) → opens serpl with git safety
```

### **Wiki Link Creation** 
```bash
# From anywhere in any Obsidian vault
Alt+l (in Zellij) → shows fuzzy finder with all notes
# Paste result with regular paste command
```

### **Smart File Opening**
```bash
# In Helix, select text and press 'gf':
Areas                    # → Opens ~/vault/Areas/ directory
[[My Note]]             # → Opens ~/vault/My Note.md  
src/main.rs             # → Opens relative to project root
https://example.com     # → Opens in browser
```

### **Project Information**
```bash
# See what type of project you're in
nu -c "show-project-info"

# List all configured markers  
nu -c "list-project-markers"

# Find project root from any subdirectory
nu -c "find-project-root"
```

## How It Enables Filesystem Freedom

### **Before**: Tool-Centric Organization
```
/Users/you/Obsidian.nosync/Vault/  ← LOCKED location
├── notes/
├── projects/
└── research/
```

### **After**: YOU-Centric Organization  
```
~/knowledge/
├── personal-vault/          ← .obsidian detected automatically
├── work-research/           ← .obsidian detected automatically  
└── project-notes/           ← .vault-root custom marker

~/projects/  
├── rust-tools/              ← Cargo.toml detected automatically
├── web-apps/                ← package.json detected automatically
└── scripts/                 ← .git detected automatically
```

**All tools automatically adapt to YOUR structure!**

## Troubleshooting

### **"Command not found" errors**
```bash
# Reload Nushell configuration
source ~/.config/nushell/config.nu

# Or start new shell session
nu
```

### **"No vault found" for wiki links**
```bash
# Make sure you're in an Obsidian vault directory
# Or create custom marker: touch .vault-root
```

### **serpl not launching**  
```bash
# Check if serpl is installed
which serpl

# Install manually if needed:
# macOS: brew install serpl
# Arch: sudo pacman -S serpl
```

### **Helix keybindings not working**
```bash
# Check if config was updated
grep -A 5 "Space+r,r" ~/.config/helix/config.toml
```

## Architecture

```
┌─────────────────────────────────────────┐
│ PROJECT ROOT DETECTION (Pure Nushell)  │
├─────────────────────────────────────────┤
│ • Configurable markers                 │
│ • Cross-platform path handling         │  
│ • Project type classification          │
└─────────────────────────────────────────┘
                    │
                    ▼ Used by
┌─────────────────────────────────────────┐
│ INTEGRATED TOOLS                        │
├─────────────────────────────────────────┤
│ • serpl (search/replace)                │
│ • Wiki link picker (dynamic)           │
│ • File opener (gf command)             │
│ • URL opener (gx command)              │
│ • Citation finder                      │
└─────────────────────────────────────────┘
                    │
                    ▼ Controlled by
┌─────────────────────────────────────────┐
│ EDITOR INTEGRATION                      │
├─────────────────────────────────────────┤
│ • Helix keybindings                     │
│ • Zellij floating panes                │
│ • Cross-platform compatibility         │
└─────────────────────────────────────────┘
```

---

## Cross-Platform Equivalence Guarantee

### 🎯 **Identical Experience on All Platforms**

This deployment script **guarantees 100% identical functionality** across platforms:

**✅ macOS → Arch Linux Equivalence:**
1. **Copy dotfiles** - `git pull` your dotfiles repository
2. **Run deployment** - `~/dotfiles/scripts/deploy-unified-dev-environment`  
3. **Test system** - `~/dotfiles/scripts/test-unified-system`
4. **Use identically** - All keybindings and features work the same

**🔧 Platform-Specific Handling (Automatic):**
- **Dependencies**: `brew install` (macOS) ↔ `sudo pacman -S` (Arch)
- **URL Opening**: `open` (macOS) ↔ `xdg-open` (Linux)
- **Paths**: Handles different filesystem conventions automatically
- **Keybindings**: Identical across all platforms

**📊 What Works Identically:**
- ✅ Project root detection system
- ✅ serpl integration (`Alt+r`, `Space+r,r`)
- ✅ Dynamic vault detection (`Alt+l`)
- ✅ Smart file opening (`gf`, `gx`)
- ✅ All Nushell functions and commands
- ✅ Configuration customization
- ✅ Filesystem reorganization freedom

**Result**: Your development workflow is **truly portable** - learn once, use everywhere!

---

*The deployment creates a unified, intelligent development environment that adapts to your filesystem organization rather than forcing you to adapt to tool limitations.*