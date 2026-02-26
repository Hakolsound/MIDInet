#!/usr/bin/env bash
# MIDInet client installation / update script
#
# First-time install:  sudo bash install-client.sh
# Update:              sudo bash install-client.sh
#
# Both cases are handled automatically — the script detects whether
# services already exist and does the right thing.
set -euo pipefail

BINARY_DIR="${BINARY_DIR:-target/release}"
BUILD="${BUILD:-true}"

echo "=== MIDInet Client Install/Update ==="

# ── Detect mode ──
FIRST_INSTALL=false
if ! systemctl list-unit-files midinet-bridge.service &>/dev/null || \
   ! systemctl list-unit-files midinet-client.service &>/dev/null; then
    FIRST_INSTALL=true
fi

if [ "$FIRST_INSTALL" = true ]; then
    echo "Mode: First-time install"
else
    echo "Mode: Update"
fi

# ── 1. System dependencies ──
echo ""
echo "[1/5] Checking system dependencies..."
DEPS=(libasound2-dev pkg-config libssl-dev libdbus-1-dev)
MISSING=()
for dep in "${DEPS[@]}"; do
    if ! dpkg -s "$dep" &>/dev/null 2>&1; then
        MISSING+=("$dep")
    fi
done
if [ ${#MISSING[@]} -gt 0 ]; then
    echo "  Installing: ${MISSING[*]}"
    apt-get update -qq && apt-get install -y -qq "${MISSING[@]}"
else
    echo "  All dependencies present."
fi

# ── 2. Build ──
if [ "$BUILD" = "true" ]; then
    echo ""
    echo "[2/5] Building release binaries..."
    BUILD_USER="$(logname 2>/dev/null || echo "${SUDO_USER:-$(whoami)}")"
    sudo -u "$BUILD_USER" cargo build --release -p midi-client -p midi-bridge
    BINARY_DIR="target/release"
else
    echo ""
    echo "[2/5] Skipping build (BUILD=false), using binaries from ${BINARY_DIR}"
fi

# Verify binaries exist
for bin in midi-client midi-bridge; do
    if [ ! -f "${BINARY_DIR}/${bin}" ]; then
        echo "ERROR: ${BINARY_DIR}/${bin} not found. Build first or set BINARY_DIR."
        exit 1
    fi
done

# ── 3. System user + directories ──
echo ""
echo "[3/5] Setting up system user and directories..."
if ! id midi &>/dev/null; then
    useradd -r -s /usr/sbin/nologin -d /nonexistent midi
    echo "  Created midi user."
else
    echo "  User midi already exists."
fi
install -d -o midi -g midi -m 755 /etc/midinet
install -d -o midi -g midi -m 755 /var/lib/midinet

# Install default client config if missing
if [ -f config/client.toml ] && [ ! -f /etc/midinet/client.toml ]; then
    install -m 644 -o midi -g midi config/client.toml /etc/midinet/client.toml
    echo "  Installed default client config."
fi

# ── 4. Install binaries + services ──
echo ""
echo "[4/5] Installing binaries and services..."

# For updates: stop services before replacing binaries
if [ "$FIRST_INSTALL" = false ]; then
    echo "  Stopping services for update..."
    systemctl stop midinet-client.service 2>/dev/null || true
    # Bridge keeps running during binary swap to keep the device alive!
    # We'll restart it after installing the new binary.
fi

install -m 755 "${BINARY_DIR}/midi-bridge" /usr/local/bin/midi-bridge
install -m 755 "${BINARY_DIR}/midi-client" /usr/local/bin/midi-client
echo "  Binaries installed."

install -m 644 deploy/midinet-bridge.service /etc/systemd/system/midinet-bridge.service
install -m 644 deploy/midinet-client.service /etc/systemd/system/midinet-client.service
systemctl daemon-reload
echo "  Service files installed."

# ── 5. Start / restart services ──
echo ""
echo "[5/5] Starting services..."

if [ "$FIRST_INSTALL" = true ]; then
    # First install: enable and start both
    systemctl enable --now midinet-bridge.service
    # Brief pause to let bridge create the socket
    sleep 1
    systemctl enable --now midinet-client.service
    echo "  Services enabled and started."
else
    # Update: restart bridge (brief device blip), then start client
    systemctl restart midinet-bridge.service
    sleep 1
    systemctl start midinet-client.service
    echo "  Services restarted."
fi

echo ""
echo "=== Installation complete ==="
systemctl status midinet-bridge.service midinet-client.service --no-pager || true
echo ""
echo "View logs: journalctl -u midinet-bridge -u midinet-client -f"
