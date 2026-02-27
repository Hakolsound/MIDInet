#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Linux Client Installer
# Clones from GitHub, builds natively, and installs as a systemd user service.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/client-install-linux.sh | bash
#
# Or clone first:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet && bash scripts/client-install-linux.sh
#
# Environment variables:
#   MIDINET_BRANCH  — git branch (default: v3.1)
# ──────────────────────────────────────────────────────────────
set -euo pipefail

BRANCH="${MIDINET_BRANCH:-v3.1}"
REPO_URL="https://github.com/Hakolsound/MIDInet.git"
INSTALL_DIR="$HOME/.midinet"
SRC_DIR="$INSTALL_DIR/src"
SERVICE_NAME="midinet-client"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

step() { echo -e "\n${CYAN}[$1/8]${NC} $2"; }
ok()   { echo -e "    ${GREEN}✓${NC} $1"; }
warn() { echo -e "    ${YELLOW}!${NC} $1"; }
fail() { echo -e "    ${RED}✗${NC} $1"; exit 1; }

echo -e "${CYAN}"
echo "  ┌──────────────────────────────────────┐"
echo "  │   MIDInet — Linux Client Installer    │"
echo "  │   Hakol Fine AV Services              │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

# ── Prerequisites ─────────────────────────────────────────────
step 1 "Checking prerequisites..."

# Detect package manager
if command -v apt-get &>/dev/null; then
    PKG_MANAGER="apt"
elif command -v dnf &>/dev/null; then
    PKG_MANAGER="dnf"
elif command -v pacman &>/dev/null; then
    PKG_MANAGER="pacman"
else
    warn "Unknown package manager. Install manually: libasound2-dev, git, build-essential"
    PKG_MANAGER="unknown"
fi

# Install ALSA dev headers + build tools
if [ "$PKG_MANAGER" = "apt" ]; then
    if ! dpkg -s libasound2-dev &>/dev/null 2>&1; then
        echo "    Installing ALSA development libraries (needs sudo)..."
        sudo apt-get update -qq
        sudo apt-get install -y -qq libasound2-dev build-essential pkg-config git
    fi
    ok "System dependencies installed (apt)"
elif [ "$PKG_MANAGER" = "dnf" ]; then
    sudo dnf install -y alsa-lib-devel gcc git pkg-config 2>/dev/null
    ok "System dependencies installed (dnf)"
elif [ "$PKG_MANAGER" = "pacman" ]; then
    sudo pacman -S --needed --noconfirm alsa-lib base-devel git pkg-config 2>/dev/null
    ok "System dependencies installed (pacman)"
fi

# Rust
if ! command -v cargo &>/dev/null; then
    warn "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
    ok "Rust installed ($(rustc --version))"
else
    ok "Rust already installed ($(rustc --version))"
fi

# ── Clone / Update ────────────────────────────────────────────
step 2 "Fetching MIDInet source..."
mkdir -p "$INSTALL_DIR"

if [ -d "$SRC_DIR/.git" ]; then
    cd "$SRC_DIR"
    git fetch origin
    git checkout "$BRANCH"
    git reset --hard "origin/$BRANCH"
    ok "Updated to latest $BRANCH"
else
    git clone --branch "$BRANCH" "$REPO_URL" "$SRC_DIR"
    cd "$SRC_DIR"
    ok "Cloned $REPO_URL ($BRANCH)"
fi

# ── Build ─────────────────────────────────────────────────────
step 3 "Building midi-client and midi-tray (release mode)..."
cd "$SRC_DIR"
cargo build --release -p midi-client -p midi-cli -p midi-tray 2>&1 | tail -5
ok "Build complete"

# ── Install Binaries ──────────────────────────────────────────
step 4 "Installing binaries..."
mkdir -p "$INSTALL_DIR/bin"
cp "$SRC_DIR/target/release/midi-client" "$INSTALL_DIR/bin/midinet-client"
cp "$SRC_DIR/target/release/midi-cli"    "$INSTALL_DIR/bin/midinet-cli"
cp "$SRC_DIR/target/release/midi-tray"   "$INSTALL_DIR/bin/midinet-tray"

# Symlink to ~/.local/bin (usually on PATH)
mkdir -p "$HOME/.local/bin"
ln -sf "$INSTALL_DIR/bin/midinet-client" "$HOME/.local/bin/midinet-client"
ln -sf "$INSTALL_DIR/bin/midinet-cli"    "$HOME/.local/bin/midinet-cli"
ln -sf "$INSTALL_DIR/bin/midinet-tray"   "$HOME/.local/bin/midinet-tray"
ok "Installed to $INSTALL_DIR/bin/ (symlinked to ~/.local/bin/)"

# ── Config ────────────────────────────────────────────────────
step 5 "Setting up configuration..."
mkdir -p "$INSTALL_DIR/config"

if [ ! -f "$INSTALL_DIR/config/client.toml" ]; then
    cp "$SRC_DIR/config/client.toml" "$INSTALL_DIR/config/client.toml"
    ok "Default config installed to $INSTALL_DIR/config/client.toml"
else
    warn "Config already exists — not overwriting"
fi

# ── Add to audio group ────────────────────────────────────────
step 6 "Configuring ALSA access..."
if groups "$USER" | grep -q '\baudio\b'; then
    ok "User already in audio group"
else
    sudo usermod -aG audio "$USER"
    ok "Added $USER to audio group (log out and back in to take effect)"
fi

# ── Systemd User Service ─────────────────────────────────────
step 7 "Installing systemd user service..."

# Stop existing service if running
systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true

mkdir -p "$HOME/.config/systemd/user"
cat > "$HOME/.config/systemd/user/$SERVICE_NAME.service" << SERVICE
[Unit]
Description=MIDInet Client Daemon
After=sound.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/bin/midinet-client --config $INSTALL_DIR/config/client.toml
Restart=always
RestartSec=2

Environment=RUST_LOG=info

[Install]
WantedBy=default.target
SERVICE

systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME"
systemctl --user start "$SERVICE_NAME"
ok "Systemd user service installed and started"

# Enable lingering so the service runs even when not logged in
loginctl enable-linger "$USER" 2>/dev/null || true

# ── Tray Autostart ───────────────────────────────────────────
step 8 "Installing tray application (autostart on login)..."

mkdir -p "$HOME/.config/autostart"
cat > "$HOME/.config/autostart/midinet-tray.desktop" << DESKTOP
[Desktop Entry]
Type=Application
Name=MIDInet Tray
Comment=MIDInet system tray health monitor
Exec=$INSTALL_DIR/bin/midinet-tray
Icon=midi
Terminal=false
StartupNotify=false
X-GNOME-Autostart-enabled=true
DESKTOP
ok "Tray autostart entry installed (requires system tray / AppIndicator support)"

# ── Done ──────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  MIDInet client installed!${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo "  The client is running and will auto-discover hosts on your LAN."
echo "  Virtual MIDI device will appear once a host is found."
echo ""
echo "  Config:  $INSTALL_DIR/config/client.toml"
echo "  Logs:    journalctl --user -u $SERVICE_NAME -f"
echo "  Source:  $SRC_DIR"
echo ""
echo "  Commands:"
echo "    midinet-cli status                           # Check connection"
echo "    midinet-cli focus                            # View/claim focus"
echo "    systemctl --user stop $SERVICE_NAME          # Stop"
echo "    systemctl --user start $SERVICE_NAME         # Start"
echo "    systemctl --user status $SERVICE_NAME        # Status"
echo ""
echo "  Update:  bash $SRC_DIR/scripts/client-install-linux.sh"
echo ""

# Warn if ~/.local/bin is not on PATH
if ! echo "$PATH" | grep -q "$HOME/.local/bin"; then
    warn "Add ~/.local/bin to your PATH: echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc"
fi
