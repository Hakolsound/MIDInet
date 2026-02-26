#!/usr/bin/env bash
# MIDInet installation script for Raspberry Pi
# Run as root: sudo bash install.sh
set -euo pipefail

BINARY_DIR="${BINARY_DIR:-target/aarch64-unknown-linux-gnu/release}"
BUILD="${BUILD:-false}"

echo "=== MIDInet Install ==="

# 0. Install system dependencies
echo "[0/6] Checking system dependencies..."
DEPS=(libasound2-dev pkg-config libssl-dev libdbus-1-dev)
MISSING=()
for dep in "${DEPS[@]}"; do
    if ! dpkg -s "$dep" &>/dev/null; then
        MISSING+=("$dep")
    fi
done
if [ ${#MISSING[@]} -gt 0 ]; then
    echo "  Installing: ${MISSING[*]}"
    apt-get update -qq && apt-get install -y -qq "${MISSING[@]}"
else
    echo "  All dependencies present."
fi

# 0b. Build if requested
if [ "$BUILD" = "true" ]; then
    echo "[0b/7] Building release binaries..."
    sudo -u "$(logname 2>/dev/null || echo pi)" \
        cargo build --release -p midi-host -p midi-admin -p midi-cli -p midi-bridge
    BINARY_DIR="target/release"
fi

# 1. Create system user
if ! id midi &>/dev/null; then
    echo "[1/6] Creating midi user..."
    useradd -r -s /usr/sbin/nologin -d /nonexistent midi
else
    echo "[1/6] User midi already exists."
fi

# 2. Create directories
echo "[2/6] Creating directories..."
install -d -o midi -g midi -m 755 /etc/midinet
install -d -o midi -g midi -m 755 /var/lib/midinet

# 3. Install config (preserve existing)
echo "[3/6] Installing config..."
if [ -f config/host.toml ] && [ ! -f /etc/midinet/midinet.toml ]; then
    install -m 644 -o midi -g midi config/host.toml /etc/midinet/midinet.toml
    echo "  Installed default config."
else
    echo "  Config already exists, skipping."
fi

# 4. Install binaries
echo "[4/7] Installing binaries..."
install -m 755 "${BINARY_DIR}/midi-host"   /usr/local/bin/midi-host
install -m 755 "${BINARY_DIR}/midi-admin"  /usr/local/bin/midi-admin
install -m 755 "${BINARY_DIR}/midi-cli"    /usr/local/bin/midi-cli
install -m 755 "${BINARY_DIR}/midi-bridge" /usr/local/bin/midi-bridge

# 5. Install systemd service files
echo "[5/7] Installing systemd services..."
install -m 644 deploy/midinet-host.service   /etc/systemd/system/midinet-host.service
install -m 644 deploy/midinet-admin.service  /etc/systemd/system/midinet-admin.service
install -m 644 deploy/midinet-bridge.service /etc/systemd/system/midinet-bridge.service
systemctl daemon-reload

# 6. Enable and start services
echo "[6/7] Enabling and starting services..."
systemctl enable --now midinet-host.service
systemctl enable --now midinet-admin.service

# 7. Bridge is client-side only — install but don't auto-start on host
echo "[7/7] Bridge service installed (start on client machines with: systemctl enable --now midinet-bridge)"

echo ""
echo "=== Installation complete ==="
systemctl status midinet-host.service midinet-admin.service --no-pager || true
echo ""
echo "View logs: journalctl -u midinet-host -u midinet-admin -f"
