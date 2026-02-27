#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Pi Update Script
# Pulls latest code from GitHub, rebuilds, and restarts services.
#
# Usage:  sudo midinet-update
#    or:  sudo bash scripts/pi-update.sh  (from repo root)
#
# This is installed as /usr/local/bin/midinet-update by pi-provision.sh
# ──────────────────────────────────────────────────────────────
set -euo pipefail

BRANCH="${MIDINET_BRANCH:-v3.1}"
FORCE=false
if [ "${1:-}" = "--force" ]; then
    FORCE=true
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

if [ "$(id -u)" -ne 0 ]; then
    echo -e "${RED}Run as root: sudo midinet-update${NC}"
    exit 1
fi

# Auto-detect source directory
if [ -n "${MIDINET_DIR:-}" ] && [ -d "$MIDINET_DIR/.git" ]; then
    : # explicit override, use as-is
elif [ -d "/opt/midinet/src/.git" ]; then
    MIDINET_DIR="/opt/midinet/src"
elif [ -d "/home/pi/MIDInet/.git" ]; then
    MIDINET_DIR="/home/pi/MIDInet"
else
    echo -e "${RED}Source not found. Checked /opt/midinet/src and /home/pi/MIDInet.${NC}"
    echo -e "${RED}Set MIDINET_DIR or run pi-provision.sh first.${NC}"
    exit 1
fi

echo -e "${CYAN}MIDInet Update${NC}"
echo ""

# Pull latest
echo -e "${CYAN}[1/4]${NC} Pulling latest from origin/$BRANCH..."
cd "$MIDINET_DIR"
BEFORE=$(git rev-parse --short HEAD)
git fetch origin
git checkout "$BRANCH"
git reset --hard "origin/$BRANCH"
AFTER=$(git rev-parse --short HEAD)

if [ "$BEFORE" = "$AFTER" ]; then
    echo -e "    ${GREEN}✓${NC} Already up-to-date ($AFTER)"
    if [ "$FORCE" = true ]; then
        echo "  --force: rebuilding anyway"
    else
        echo ""
        read -p "  Rebuild anyway? [y/N] " -n 1 -r
        echo ""
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            echo "  Aborted."
            exit 0
        fi
    fi
else
    echo -e "    ${GREEN}✓${NC} Updated $BEFORE → $AFTER"
    echo ""
    echo "  Changes:"
    git log --oneline "$BEFORE..$AFTER" | head -10 | sed 's/^/    /'
    echo ""
fi

# Ensure cargo is on PATH (sudo doesn't inherit user PATH)
for cargo_bin in /home/pi/.cargo/bin /root/.cargo/bin /usr/local/cargo/bin; do
    if [ -x "$cargo_bin/cargo" ]; then
        export PATH="$cargo_bin:$PATH"
        break
    fi
done
if ! command -v cargo &>/dev/null; then
    echo -e "${RED}cargo not found. Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh${NC}"
    exit 1
fi

# Build
echo -e "${CYAN}[2/4]${NC} Building release..."
cargo build --release 2>&1 | tail -3
echo -e "    ${GREEN}✓${NC} Build complete"

# Stop services
echo -e "${CYAN}[3/4]${NC} Stopping services..."
systemctl stop midinet-admin.service 2>/dev/null || true
systemctl stop midinet-host.service 2>/dev/null || true

# Install & restart
echo -e "${CYAN}[4/4]${NC} Installing and restarting..."
install -m 755 "$MIDINET_DIR/target/release/midi-host"  /usr/local/bin/midi-host
install -m 755 "$MIDINET_DIR/target/release/midi-admin" /usr/local/bin/midi-admin
install -m 755 "$MIDINET_DIR/target/release/midi-cli"   /usr/local/bin/midi-cli

# Update systemd units in case they changed
install -m 644 "$MIDINET_DIR/deploy/midinet-host.service"  /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-admin.service" /etc/systemd/system/
systemctl daemon-reload

systemctl start midinet-host.service
systemctl start midinet-admin.service

echo ""
echo -e "${GREEN}✓ MIDInet updated and running${NC}"
echo ""
systemctl status midinet-host.service midinet-admin.service --no-pager -l 2>/dev/null | head -20 || true
echo ""
echo "  Dashboard: http://$(hostname -I | awk '{print $1}'):8080"
echo "  Logs:      journalctl -u midinet-host -u midinet-admin -f"
