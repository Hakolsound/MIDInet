#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Pi Update Script
# Pulls latest code from GitHub, rebuilds, and restarts services.
#
# Usage:  sudo midinet-update
#    or:  sudo bash /opt/midinet/src/scripts/pi-update.sh
#
# This is installed as /usr/local/bin/midinet-update by pi-provision.sh
# ──────────────────────────────────────────────────────────────
set -euo pipefail

MIDINET_DIR="${MIDINET_DIR:-/opt/midinet/src}"
BRANCH="${MIDINET_BRANCH:-main}"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

if [ "$(id -u)" -ne 0 ]; then
    echo -e "${RED}Run as root: sudo midinet-update${NC}"
    exit 1
fi

if [ ! -d "$MIDINET_DIR/.git" ]; then
    echo -e "${RED}Source not found at $MIDINET_DIR. Run pi-provision.sh first.${NC}"
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
    echo ""
    read -p "  Rebuild anyway? [y/N] " -n 1 -r
    echo ""
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "  Aborted."
        exit 0
    fi
else
    echo -e "    ${GREEN}✓${NC} Updated $BEFORE → $AFTER"
    echo ""
    echo "  Changes:"
    git log --oneline "$BEFORE..$AFTER" | head -10 | sed 's/^/    /'
    echo ""
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
