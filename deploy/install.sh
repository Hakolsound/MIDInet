#!/usr/bin/env bash
# MIDInet installation script for Raspberry Pi
# Run as root: sudo bash install.sh
set -euo pipefail

BINARY_DIR="${BINARY_DIR:-target/aarch64-unknown-linux-gnu/release}"

echo "=== MIDInet Install ==="

# 1. Create system user
if ! id midi &>/dev/null; then
    echo "[1/5] Creating midi user..."
    useradd -r -s /usr/sbin/nologin -d /nonexistent midi
else
    echo "[1/5] User midi already exists."
fi

# 2. Create directories
echo "[2/5] Creating directories..."
install -d -o midi -g midi -m 755 /etc/midinet
install -d -o midi -g midi -m 755 /var/lib/midinet

# 3. Install binaries
echo "[3/5] Installing binaries..."
install -m 755 "${BINARY_DIR}/midi-host"  /usr/local/bin/midi-host
install -m 755 "${BINARY_DIR}/midi-admin" /usr/local/bin/midi-admin
install -m 755 "${BINARY_DIR}/midi-cli"   /usr/local/bin/midi-cli

# 4. Install systemd service files
echo "[4/5] Installing systemd services..."
install -m 644 deploy/midinet-host.service  /etc/systemd/system/midinet-host.service
install -m 644 deploy/midinet-admin.service /etc/systemd/system/midinet-admin.service
systemctl daemon-reload

# 5. Enable and start services
echo "[5/5] Enabling and starting services..."
systemctl enable --now midinet-host.service
systemctl enable --now midinet-admin.service

echo ""
echo "=== Installation complete ==="
systemctl status midinet-host.service midinet-admin.service --no-pager || true
echo ""
echo "View logs: journalctl -u midinet-host -u midinet-admin -f"
