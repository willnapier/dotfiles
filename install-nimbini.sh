#!/usr/bin/env bash

# Nimbini Linux Setup Script
# Installs and configures the cross-platform Rust development environment

set -e

echo "🦀 Setting up Cross-Platform Rust Development Environment on Linux"
echo "================================================================="

# Detect package manager
if command -v pacman &> /dev/null; then
    PACKAGE_MANAGER="pacman"
    INSTALL_CMD="sudo pacman -S --needed"
    AUR_HELPER="yay"
elif command -v apt &> /dev/null; then
    PACKAGE_MANAGER="apt"
    INSTALL_CMD="sudo apt install -y"
elif command -v dnf &> /dev/null; then
    PACKAGE_MANAGER="dnf"  
    INSTALL_CMD="sudo dnf install -y"
else
    echo "❌ Unsupported package manager"
    exit 1
fi

echo "📦 Detected package manager: $PACKAGE_MANAGER"

# Install essential packages
echo "Installing core packages..."
case $PACKAGE_MANAGER in
    pacman)
        $INSTALL_CMD base-devel git curl wget
        $INSTALL_CMD helix nushell yazi fd ripgrep bat exa zoxide starship
        $INSTALL_CMD foot waybar river
        $INSTALL_CMD firefox tailscale
        ;;
    apt)
        $INSTALL_CMD build-essential git curl wget
        # Most Rust tools need to be installed via cargo on Debian/Ubuntu
        ;;
    dnf)
        $INSTALL_CMD @development-tools git curl wget  
        $INSTALL_CMD helix nushell
        ;;
esac

# Install Rust if not present
if ! command -v rustc &> /dev/null; then
    echo "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

# Install Rust tools if not available through package manager
echo "Installing Rust-based tools..."
RUST_TOOLS=("helix-term" "nu" "yazi-fm" "fd-find" "ripgrep" "bat" "exa" "zoxide" "starship")

for tool in "${RUST_TOOLS[@]}"; do
    case $tool in
        helix-term)
            if ! command -v hx &> /dev/null; then
                cargo install helix-term --locked
            fi
            ;;
        nu)
            if ! command -v nu &> /dev/null; then
                cargo install nu --locked
            fi
            ;;
        yazi-fm)
            if ! command -v yazi &> /dev/null; then
                cargo install --locked yazi-fm yazi-cli
            fi
            ;;
        fd-find)
            if ! command -v fd &> /dev/null; then
                cargo install fd-find
            fi
            ;;
        *)
            if ! command -v "${tool%-*}" &> /dev/null; then
                cargo install "$tool"
            fi
            ;;
    esac
done

# Install Zellij
if ! command -v zellij &> /dev/null; then
    echo "Installing Zellij..."
    cargo install --locked zellij
fi

# Install Dotter (dotfiles manager)
if ! command -v dotter &> /dev/null; then
    echo "Installing Dotter..."
    cargo install dotter
fi

# Create necessary directories
echo "Creating directories..."
mkdir -p "$HOME/.config"
mkdir -p "$HOME/.local/bin"
mkdir -p "$HOME/Obsidian/Forge"

# Clone dotfiles repository
DOTFILES_DIR="$HOME/.dotfiles"
if [ ! -d "$DOTFILES_DIR" ]; then
    echo "Cloning dotfiles..."
    git clone https://github.com/willnapier/dotfiles.git "$DOTFILES_DIR"
fi

# Set up Dotter for Linux
echo "Configuring dotfiles for Linux..."
cd "$DOTFILES_DIR"

# Create Linux-specific local.toml
cat > .dotter/local.toml << EOF
includes = []
packages = ["linux"]

[files]

[variables]
EOF

# Deploy dotfiles
dotter deploy

# Install cross-platform scripts
echo "Installing cross-platform scripts..."
cp "$HOME/.local/bin/hx-smart-gf-cross" "$HOME/.local/bin/hx-smart-gf-rust"
cp "$HOME/.local/bin/hx-open-system-cross" "$HOME/.local/bin/hx-open-system-rust"
cp "$HOME/.local/bin/hx-open-url-rust" "$HOME/.local/bin/" 2>/dev/null || true

chmod +x "$HOME/.local/bin/hx-"*

# Set up Wayland session
echo "Setting up Wayland session..."
if [ "$PACKAGE_MANAGER" = "pacman" ]; then
    # Enable required services
    sudo systemctl enable --now tailscaled
fi

# Configure git (if not already done)
if [ -z "$(git config --global user.name)" ]; then
    read -p "Enter your git username: " git_username
    read -p "Enter your git email: " git_email
    git config --global user.name "$git_username"
    git config --global user.email "$git_email"
fi

# Set up Tailscale
echo "Setting up Tailscale..."
if ! tailscale status &> /dev/null; then
    echo "Please run: sudo tailscale up"
    echo "Then authenticate in your browser"
fi

echo ""
echo "🎉 Installation complete!"
echo ""
echo "Next steps:"
echo "1. Reboot or log out and back in"
echo "2. Set up Tailscale: sudo tailscale up"
echo "3. Start River compositor: river"
echo "4. Open foot terminal and run: nu"
echo "5. Test cross-platform tools: hx, yazi, zellij"
echo ""
echo "SSH from Mac: ssh nimbini"
echo "Local development: Launch foot terminal → nu → hx/yazi/zellij"