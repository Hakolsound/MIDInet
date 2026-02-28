#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — macOS Client Installer
# Clones from GitHub, builds natively, and installs as a launchd service.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/client-install-macos.sh | bash
#
# Or clone first:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet && bash scripts/client-בין אביו, רוני רון, למתנדבת נוצרייה בשםinstall-macos.sh
#
# Environment variables:
#   MIDINET_BRANCH  — git branch (default: v3.1)
# ──────────────────────────────────────────────────────────────
set -euo pipefail

BRANCH="${MIDINET_BRANCH:-v3.1}"
REPO_URL="https://github.com/Hakolsound/MIDInet.git"
INSTALL_DIR="$HOME/.midinet"
SRC_DIR="$INSTALL_DIR/src"
BIN_DIR="/usr/local/bin"
PLIST_NAME="co.hakol.midinet-client"
PLIST_PATH="$HOME/Library/LaunchAgents/$PLIST_NAME.plist"

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
echo "  │   MIDInet — macOS Client Installer    │"
echo "  │   Hakol Fine AV Services              │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"

# ── Prerequisites ─────────────────────────────────────────────
step 1 "Checking prerequisites..."

# Xcode Command Line Tools (needed for CoreMIDI headers)
if ! xcode-select -p &>/dev/null; then
    warn "Installing Xcode Command Line Tools..."
    xcode-select --install
    echo "    Press any key after installation completes..."
    read -n 1 -s
fi
ok "Xcode Command Line Tools available"

# Rust
if ! command -v cargo &>/dev/null; then
    warn "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    source "$HOME/.cargo/env"
    ok "Rust installed ($(rustc --version))"
else
    ok "Rust already installed ($(rustc --version))"
fi

# Git
if ! command -v git &>/dev/null; then
    fail "Git not found. Install Xcode Command Line Tools first."
fi

# ── Clone / Update ────────────────────────────────────────────
step 2 "Fetching MIDInet source..."
mkdir -p "$INSTALL_DIR"

if [ -d "$SRC_DIR/.git" ]; then
    cd "$SRC_DIR"
    git fetch origin
    # Use -B to force branch checkout (avoids checking out tag when both tag + branch
    # exist with the same name, which causes detached HEAD)
    git checkout -B "$BRANCH" "origin/$BRANCH"
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

# ── Install ───────────────────────────────────────────────────
step 4 "Installing binaries..."

# May need sudo for /usr/local/bin
if [ -w "$BIN_DIR" ]; then
    cp "$SRC_DIR/target/release/midi-client" "$BIN_DIR/midinet-client"
    cp "$SRC_DIR/target/release/midi-cli"    "$BIN_DIR/midinet-cli"
    cp "$SRC_DIR/target/release/midi-tray"   "$BIN_DIR/midinet-tray"
else
    sudo cp "$SRC_DIR/target/release/midi-client" "$BIN_DIR/midinet-client"
    sudo cp "$SRC_DIR/target/release/midi-cli"    "$BIN_DIR/midinet-cli"
    sudo cp "$SRC_DIR/target/release/midi-tray"   "$BIN_DIR/midinet-tray"
fi
ok "Installed midinet-client, midinet-cli, and midinet-tray to $BIN_DIR"

# ── App Bundle (.app for Spotlight / Launchpad / Finder) ─────
step 5 "Creating MIDInet.app application bundle..."

APP_DIR="/Applications/MIDInet.app"
APP_CONTENTS="$APP_DIR/Contents"
APP_MACOS="$APP_CONTENTS/MacOS"
APP_RESOURCES="$APP_CONTENTS/Resources"

# Determine if we need sudo for /Applications
SUDO=""
if [ ! -w "/Applications" ]; then
    SUDO="sudo"
fi

$SUDO rm -rf "$APP_DIR" 2>/dev/null || true
$SUDO mkdir -p "$APP_MACOS" "$APP_RESOURCES"

# Copy icon
$SUDO cp "$SRC_DIR/assets/icons/midinet.icns" "$APP_RESOURCES/midinet.icns"

# Create launcher script (delegates to installed binary, with single-instance guard)
LAUNCHER_TMP=$(mktemp)
cat > "$LAUNCHER_TMP" << 'LAUNCHER'
#!/bin/bash
if pgrep -x "midinet-tray" > /dev/null 2>&1; then
    osascript -e 'display notification "MIDInet tray is already running — look for the status icon in your menu bar." with title "MIDInet"' 2>/dev/null
    exit 0
fi
exec /usr/local/bin/midinet-tray "$@"
LAUNCHER
$SUDO cp "$LAUNCHER_TMP" "$APP_MACOS/MIDInet"
$SUDO chmod +x "$APP_MACOS/MIDInet"
rm -f "$LAUNCHER_TMP"

# Create Info.plist
GIT_HASH=$(git -C "$SRC_DIR" rev-parse --short HEAD 2>/dev/null || echo "unknown")
PLIST_TMP=$(mktemp)
cat > "$PLIST_TMP" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>MIDInet</string>
    <key>CFBundleDisplayName</key>
    <string>MIDInet</string>
    <key>CFBundleIdentifier</key>
    <string>co.hakol.midinet</string>
    <key>CFBundleVersion</key>
    <string>$GIT_HASH</string>
    <key>CFBundleShortVersionString</key>
    <string>3.1</string>
    <key>CFBundleExecutable</key>
    <string>MIDInet</string>
    <key>CFBundleIconFile</key>
    <string>midinet</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSUIElement</key>
    <true/>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST
$SUDO cp "$PLIST_TMP" "$APP_CONTENTS/Info.plist"
rm -f "$PLIST_TMP"

# Refresh Finder icon cache
/usr/bin/touch "$APP_DIR"
ok "MIDInet.app installed to /Applications/"
ok "  Launch via Spotlight, Launchpad, or Finder"

# ── Config ────────────────────────────────────────────────────
step 6 "Setting up configuration..."
mkdir -p "$INSTALL_DIR/config"

if [ ! -f "$INSTALL_DIR/config/client.toml" ]; then
    cp "$SRC_DIR/config/client.toml" "$INSTALL_DIR/config/client.toml"
    ok "Default config installed to $INSTALL_DIR/config/client.toml"
else
    warn "Config already exists — not overwriting"
fi

# ── LaunchAgent (auto-start) ─────────────────────────────────
step 7 "Installing launchd service..."

# Stop existing service if running
launchctl unload "$PLIST_PATH" 2>/dev/null || true

mkdir -p "$HOME/Library/LaunchAgents"
cat > "$PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$PLIST_NAME</string>

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

launchctl load "$PLIST_PATH"
ok "LaunchAgent installed and started"

# ── Tray LaunchAgent (auto-start at login) ───────────────────
step 8 "Installing tray application..."

TRAY_PLIST_NAME="co.hakol.midinet-tray"
TRAY_PLIST_PATH="$HOME/Library/LaunchAgents/$TRAY_PLIST_NAME.plist"

launchctl unload "$TRAY_PLIST_PATH" 2>/dev/null || true

cat > "$TRAY_PLIST_PATH" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$TRAY_PLIST_NAME</string>

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

launchctl load "$TRAY_PLIST_PATH"
ok "Tray LaunchAgent installed and started"

# ── Done ──────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  MIDInet client installed!${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo "  The client is running and will auto-discover hosts on your LAN."
echo "  Open Audio MIDI Setup to see the virtual MIDI device."
echo ""
echo "  Config:  $INSTALL_DIR/config/client.toml"
echo "  Logs:    $INSTALL_DIR/midinet-client.log"
echo "  Source:  $SRC_DIR"
echo ""
echo "  Commands:"
echo "    midinet-cli status                 # Check connection"
echo "    midinet-cli focus                  # View/claim focus"
echo "    launchctl unload $PLIST_PATH       # Stop"
echo "    launchctl load $PLIST_PATH         # Start"
echo ""
echo "  Update:  bash $SRC_DIR/scripts/client-install-macos.sh"
echo ""
