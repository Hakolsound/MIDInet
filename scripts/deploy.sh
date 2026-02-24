#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Local Deploy (macOS / Linux)
# Run from the repo root. Builds, installs, and registers services.
#
# Usage:
#   bash scripts/deploy.sh
# ──────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

step()  { echo -e "\n${CYAN}[$1/$TOTAL]${NC} $2"; }
ok()    { echo -e "    ${GREEN}✓${NC} $1"; }
warn()  { echo -e "    ${YELLOW}!${NC} $1"; }
fail()  { echo -e "    ${RED}✗${NC} $1"; exit 1; }

OS="$(uname -s)"
REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_DIR="$HOME/.midinet"

# Sanity check — make sure we're in the repo
if [ ! -f "$REPO_DIR/Cargo.toml" ]; then
    fail "Run this script from the MIDInet repo root."
fi

cd "$REPO_DIR"

# ══════════════════════════════════════════════════════════════
#  macOS
# ══════════════════════════════════════════════════════════════
if [ "$OS" = "Darwin" ]; then

TOTAL=5
BIN_DIR="/usr/local/bin"
CLIENT_PLIST="$HOME/Library/LaunchAgents/co.hakol.midinet-client.plist"
TRAY_PLIST="$HOME/Library/LaunchAgents/co.hakol.midinet-tray.plist"

echo -e "${CYAN}"
echo "  ┌──────────────────────────────────────┐"
echo "  │   MIDInet — macOS Deploy             │"
echo "  │   Hakol Fine AV Services             │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

# ── 1. Stop existing ─────────────────────────────────────────
step 1 "Stopping existing services..."
launchctl unload "$TRAY_PLIST" 2>/dev/null || true
launchctl unload "$CLIENT_PLIST" 2>/dev/null || true
pkill -f midinet-tray 2>/dev/null || true
pkill -f midinet-client 2>/dev/null || true
sleep 1
ok "Stopped"

# ── 2. Build ─────────────────────────────────────────────────
step 2 "Building release binaries..."
cargo build --release -p midi-client -p midi-cli -p midi-tray
ok "Build complete"

# ── 3. Install binaries + config ─────────────────────────────
step 3 "Installing binaries and config..."
mkdir -p "$INSTALL_DIR/config"

if [ -w "$BIN_DIR" ]; then
    cp target/release/midi-client "$BIN_DIR/midinet-client"
    cp target/release/midi-cli    "$BIN_DIR/midinet-cli"
    cp target/release/midi-tray   "$BIN_DIR/midinet-tray"
else
    sudo cp target/release/midi-client "$BIN_DIR/midinet-client"
    sudo cp target/release/midi-cli    "$BIN_DIR/midinet-cli"
    sudo cp target/release/midi-tray   "$BIN_DIR/midinet-tray"
fi
ok "Binaries → $BIN_DIR/midinet-{client,cli,tray}"

if [ ! -f "$INSTALL_DIR/config/client.toml" ]; then
    cp config/client.toml "$INSTALL_DIR/config/client.toml"
    ok "Config → $INSTALL_DIR/config/client.toml"
else
    warn "Config already exists — not overwriting"
fi

# ── 4. Register LaunchAgents ─────────────────────────────────
step 4 "Registering launchd services..."
mkdir -p "$HOME/Library/LaunchAgents"

cat > "$CLIENT_PLIST" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>co.hakol.midinet-client</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN_DIR/midinet-client</string>
        <string>--config</string>
        <string>$INSTALL_DIR/config/client.toml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$INSTALL_DIR/midinet-client.log</string>
    <key>StandardErrorPath</key>
    <string>$INSTALL_DIR/midinet-client.err</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>info</string>
    </dict>
</dict>
</plist>
PLIST

cat > "$TRAY_PLIST" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>co.hakol.midinet-tray</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN_DIR/midinet-tray</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>$INSTALL_DIR/midinet-tray.log</string>
    <key>StandardErrorPath</key>
    <string>$INSTALL_DIR/midinet-tray.err</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>RUST_LOG</key>
        <string>info</string>
    </dict>
</dict>
</plist>
PLIST

ok "LaunchAgents registered"

# ── 5. Start ─────────────────────────────────────────────────
step 5 "Starting services..."
launchctl load "$CLIENT_PLIST"
sleep 1
launchctl load "$TRAY_PLIST"
ok "Client daemon and tray started"

# ── Verify ───────────────────────────────────────────────────
sleep 2
echo ""
if curl -sf http://127.0.0.1:5009/health > /dev/null 2>&1; then
    ok "Health endpoint responding on :5009"
fi

echo ""
echo -e "${GREEN}══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  MIDInet deployed on macOS!${NC}"
echo -e "${GREEN}══════════════════════════════════════════════════${NC}"
echo ""
echo "  Config:  $INSTALL_DIR/config/client.toml"
echo "  Logs:    $INSTALL_DIR/midinet-client.log"
echo "  Tray:    Look for the colored circle in your menu bar"
echo ""
echo "  Commands:"
echo "    midinet-cli status                            # Check connection"
echo "    launchctl unload $CLIENT_PLIST                # Stop daemon"
echo "    launchctl load   $CLIENT_PLIST                # Start daemon"
echo "    bash scripts/deploy.sh                        # Redeploy after changes"
echo ""

# ══════════════════════════════════════════════════════════════
#  Linux
# ══════════════════════════════════════════════════════════════
else

TOTAL=5
SERVICE_NAME="midinet-client"

echo -e "${CYAN}"
echo "  ┌──────────────────────────────────────┐"
echo "  │   MIDInet — Linux Deploy             │"
echo "  │   Hakol Fine AV Services             │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

# ── 1. Stop existing ─────────────────────────────────────────
step 1 "Stopping existing services..."
systemctl --user stop "$SERVICE_NAME" 2>/dev/null || true
pkill -f midinet-tray 2>/dev/null || true
sleep 1
ok "Stopped"

# ── 2. Build ─────────────────────────────────────────────────
step 2 "Building release binaries..."
cargo build --release -p midi-client -p midi-cli -p midi-tray
ok "Build complete"

# ── 3. Install binaries + config ─────────────────────────────
step 3 "Installing binaries and config..."
mkdir -p "$INSTALL_DIR/bin" "$INSTALL_DIR/config" "$HOME/.local/bin"

cp target/release/midi-client "$INSTALL_DIR/bin/midinet-client"
cp target/release/midi-cli    "$INSTALL_DIR/bin/midinet-cli"
cp target/release/midi-tray   "$INSTALL_DIR/bin/midinet-tray"
ln -sf "$INSTALL_DIR/bin/midinet-client" "$HOME/.local/bin/midinet-client"
ln -sf "$INSTALL_DIR/bin/midinet-cli"    "$HOME/.local/bin/midinet-cli"
ln -sf "$INSTALL_DIR/bin/midinet-tray"   "$HOME/.local/bin/midinet-tray"
ok "Binaries → $INSTALL_DIR/bin/ (symlinked to ~/.local/bin/)"

if [ ! -f "$INSTALL_DIR/config/client.toml" ]; then
    cp config/client.toml "$INSTALL_DIR/config/client.toml"
    ok "Config → $INSTALL_DIR/config/client.toml"
else
    warn "Config already exists — not overwriting"
fi

# ── 4. Register systemd + autostart ─────────────────────────
step 4 "Registering systemd service and tray autostart..."
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
ok "Systemd service registered"

mkdir -p "$HOME/.config/autostart"
cat > "$HOME/.config/autostart/midinet-tray.desktop" << DESKTOP
[Desktop Entry]
Type=Application
Name=MIDInet Tray
Comment=MIDInet system tray health monitor
Exec=$INSTALL_DIR/bin/midinet-tray
Terminal=false
StartupNotify=false
X-GNOME-Autostart-enabled=true
DESKTOP
ok "Tray autostart entry created"

loginctl enable-linger "$USER" 2>/dev/null || true

# ── 5. Start ─────────────────────────────────────────────────
step 5 "Starting services..."
systemctl --user start "$SERVICE_NAME"
sleep 1
nohup "$INSTALL_DIR/bin/midinet-tray" &>/dev/null &
ok "Client daemon and tray started"

# ── Verify ───────────────────────────────────────────────────
sleep 2
echo ""
if curl -sf http://127.0.0.1:5009/health > /dev/null 2>&1; then
    ok "Health endpoint responding on :5009"
fi

echo ""
echo -e "${GREEN}══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  MIDInet deployed on Linux!${NC}"
echo -e "${GREEN}══════════════════════════════════════════════════${NC}"
echo ""
echo "  Config:  $INSTALL_DIR/config/client.toml"
echo "  Logs:    journalctl --user -u $SERVICE_NAME -f"
echo "  Tray:    Look for the colored circle in your system tray"
echo ""
echo "  Commands:"
echo "    midinet-cli status                              # Check connection"
echo "    systemctl --user stop  $SERVICE_NAME            # Stop daemon"
echo "    systemctl --user start $SERVICE_NAME            # Start daemon"
echo "    bash scripts/deploy.sh                          # Redeploy after changes"
echo ""

fi
