#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Host Deploy (Raspberry Pi / Linux)
# Run from the repo root. Builds, installs, and registers the
# host daemon as a systemd service.
#
# Usage:
#   bash scripts/deploy-host.sh
#
# For a fresh Pi, first install prerequisites:
#   sudo apt-get update && sudo apt-get install -y libasound2-dev build-essential pkg-config git curl
#   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && source ~/.cargo/env
# ──────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

TOTAL=6
step()  { echo -e "\n${CYAN}[$1/$TOTAL]${NC} $2"; }
ok()    { echo -e "    ${GREEN}✓${NC} $1"; }
warn()  { echo -e "    ${YELLOW}!${NC} $1"; }
fail()  { echo -e "    ${RED}✗${NC} $1"; exit 1; }

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_DIR="$HOME/.midinet"
SERVICE_NAME="midinet-host"

# Sanity check
if [ ! -f "$REPO_DIR/Cargo.toml" ]; then
    fail "Run this script from the MIDInet repo root."
fi

cd "$REPO_DIR"

echo -e "${CYAN}"
echo "  ┌──────────────────────────────────────┐"
echo "  │   MIDInet — Host Deploy (Pi/Linux)   │"
echo "  │   Hakol Fine AV Services             │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

# ── 1. Check & install prerequisites ────────────────────────
step 1 "Checking prerequisites..."

# Rust
if ! command -v cargo &>/dev/null; then
    warn "Rust not found. Installing..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
    ok "Rust installed ($(rustc --version))"
else
    ok "Rust $(rustc --version | awk '{print $2}')"
fi

# ALSA dev headers + build tools (Linux only)
if [ "$(uname -s)" = "Linux" ]; then
    if ! pkg-config --exists alsa 2>/dev/null; then
        warn "ALSA development headers not found. Installing..."
        if command -v apt-get &>/dev/null; then
            sudo apt-get update -qq
            sudo apt-get install -y -qq libasound2-dev build-essential pkg-config
        elif command -v dnf &>/dev/null; then
            sudo dnf install -y alsa-lib-devel gcc pkg-config
        elif command -v pacman &>/dev/null; then
            sudo pacman -S --needed --noconfirm alsa-lib base-devel pkg-config
        else
            fail "Unknown package manager. Install libasound2-dev (or equivalent) manually."
        fi
        ok "ALSA development headers installed"
    else
        ok "ALSA development headers found"
    fi
fi

# ── 2. Stop existing service ────────────────────────────────
step 2 "Stopping existing service..."
systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true
pkill -f midinet-host 2>/dev/null || true
sleep 1
ok "Stopped"

# ── 3. Build ────────────────────────────────────────────────
step 3 "Building release binaries (this may take a while on Pi)..."
cargo build --release -p midi-host -p midi-cli -p midi-admin
ok "Build complete"

# ── 4. Install binaries + config ────────────────────────────
step 4 "Installing binaries and config..."
mkdir -p "$INSTALL_DIR/bin" "$INSTALL_DIR/config" "$HOME/.local/bin"

cp target/release/midi-host  "$INSTALL_DIR/bin/midinet-host"
cp target/release/midi-cli   "$INSTALL_DIR/bin/midinet-cli"
cp target/release/midi-admin "$INSTALL_DIR/bin/midinet-admin"
ln -sf "$INSTALL_DIR/bin/midinet-host"  "$HOME/.local/bin/midinet-host"
ln -sf "$INSTALL_DIR/bin/midinet-cli"   "$HOME/.local/bin/midinet-cli"
ln -sf "$INSTALL_DIR/bin/midinet-admin" "$HOME/.local/bin/midinet-admin"
ok "Binaries → $INSTALL_DIR/bin/"

if [ ! -f "$INSTALL_DIR/config/host.toml" ]; then
    cp config/host.toml "$INSTALL_DIR/config/host.toml"
    ok "Config → $INSTALL_DIR/config/host.toml"
    warn "Edit the config before starting: nano $INSTALL_DIR/config/host.toml"
    warn "  - Set host.id (1 = primary, 2 = standby)"
    warn "  - Set network.interface to your LAN interface (eth0, wlan0, etc.)"
    warn "  - Change admin.password from the default!"
else
    warn "Config already exists — not overwriting"
fi

# ── 5. Register systemd service ─────────────────────────────
step 5 "Registering systemd service..."
mkdir -p "$HOME/.config/systemd/user"

cat > "$HOME/.config/systemd/user/$SERVICE_NAME.service" << SERVICE
[Unit]
Description=MIDInet Host Daemon
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/bin/midinet-host --config $INSTALL_DIR/config/host.toml
Restart=always
RestartSec=2
Environment=RUST_LOG=info

# Real-time scheduling priority for MIDI
Nice=-10
LimitRTPRIO=99
LimitMEMLOCK=infinity

[Install]
WantedBy=default.target
SERVICE

systemctl --user daemon-reload
systemctl --user enable "$SERVICE_NAME"
ok "Systemd service registered (auto-start on boot)"

# Enable lingering so the service runs even when not logged in via SSH
loginctl enable-linger "$USER" 2>/dev/null || true
ok "User lingering enabled (service runs without active SSH session)"

# ── 6. Start ────────────────────────────────────────────────
step 6 "Starting host daemon..."
systemctl --user start "$SERVICE_NAME"
sleep 2

if systemctl --user is-active "$SERVICE_NAME" &>/dev/null; then
    ok "Host daemon running"
else
    warn "Service may still be starting — check with: systemctl --user status $SERVICE_NAME"
fi

# Check admin panel
ADMIN_PORT=$(grep -oP 'listen\s*=\s*"[^"]*:(\d+)"' "$INSTALL_DIR/config/host.toml" 2>/dev/null | grep -oP '\d+$' || echo "8080")
if curl -sf "http://127.0.0.1:$ADMIN_PORT" > /dev/null 2>&1; then
    ok "Admin panel responding on :$ADMIN_PORT"
fi

# ── Done ────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  MIDInet host deployed!${NC}"
echo -e "${GREEN}══════════════════════════════════════════════════${NC}"
echo ""
echo "  Config:  $INSTALL_DIR/config/host.toml"
echo "  Logs:    journalctl --user -u $SERVICE_NAME -f"
echo "  Admin:   http://$(hostname -I | awk '{print $1}'):$ADMIN_PORT"
echo ""
echo "  Commands:"
echo "    midinet-cli status                              # Check status"
echo "    systemctl --user stop  $SERVICE_NAME            # Stop"
echo "    systemctl --user start $SERVICE_NAME            # Start"
echo "    systemctl --user status $SERVICE_NAME           # Service status"
echo "    journalctl --user -u $SERVICE_NAME -f           # Live logs"
echo "    bash scripts/deploy-host.sh                     # Redeploy after changes"
echo ""
echo "  IMPORTANT: Edit the config if this is a fresh install:"
echo "    nano $INSTALL_DIR/config/host.toml"
echo ""
