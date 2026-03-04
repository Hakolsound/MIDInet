#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Raspberry Pi Network Setup
# Configures hostname, DHCP with static fallback, and Avahi
# for .local mDNS resolution.
#
# Usage:
#   sudo bash scripts/pi-network-setup.sh
#
# After running, the Pi will be reachable at:
#   <hostname>.local   (mDNS)
#   <DHCP IP>          (if a DHCP server is present)
#   <fallback IP>      (direct connect / no DHCP)
# ──────────────────────────────────────────────────────────────
set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

TOTAL_STEPS=4
step() { echo -e "\n${CYAN}[$1/$TOTAL_STEPS]${NC} $2"; }
ok()   { echo -e "    ${GREEN}done${NC} $1"; }
warn() { echo -e "    ${YELLOW}!${NC} $1"; }
fail() { echo -e "    ${RED}error${NC} $1"; exit 1; }

# ── Pre-flight ───────────────────────────────────────────────

if [ "$(id -u)" -ne 0 ]; then
    fail "This script must be run as root (sudo)"
fi

echo -e "${CYAN}"
echo "  ╔══════════════════════════════════════╗"
echo "  ║     MIDInet Pi Network Setup         ║"
echo "  ╚══════════════════════════════════════╝"
echo -e "${NC}"

# ── Auto-detect primary interface ────────────────────────────

DEFAULT_IFACE=$(ip route show default 2>/dev/null | awk '{print $5; exit}')
DEFAULT_IFACE="${DEFAULT_IFACE:-eth0}"

# ── Instructions ─────────────────────────────────────────────

echo -e "  ${YELLOW}IMPORTANT: Write these settings down!${NC}"
echo "  These are how you'll reach the Pi when there's no monitor."
echo "  Changing them later requires SSH or direct console access."
echo ""
echo "  How it works:"
echo "    1. DHCP first — Pi gets an IP from the router (if available)"
echo "    2. Fallback — if no DHCP server responds, Pi uses the static IP"
echo "    3. mDNS — Pi is always reachable at <hostname>.local"
echo ""

# Detect Companion co-installation
COMPANION_PRE_DETECTED=false
if systemctl list-unit-files 2>/dev/null | grep -q companion; then
    COMPANION_PRE_DETECTED=true
    echo -e "  ${GREEN}Bitfocus Companion detected.${NC}"
    echo "  No conflicts — Companion uses port 8000, MIDInet uses 8080."
    echo "  companion.local alias will be configured automatically."
    echo ""
fi

# ── Interactive prompts ──────────────────────────────────────

read -rp "  Hostname [midinet]: " HOSTNAME
HOSTNAME="${HOSTNAME:-midinet}"

read -rp "  Static fallback IP [192.168.50.1]: " STATIC_IP
STATIC_IP="${STATIC_IP:-192.168.50.1}"

read -rp "  Subnet mask CIDR [24]: " SUBNET
SUBNET="${SUBNET:-24}"

read -rp "  Network interface [${DEFAULT_IFACE}]: " IFACE
IFACE="${IFACE:-$DEFAULT_IFACE}"

echo ""
echo -e "  ${YELLOW}┌──────────────────────────────────────────────┐${NC}"
echo -e "  ${YELLOW}│  WRITE THIS DOWN — you'll need it on-site:   │${NC}"
echo -e "  ${YELLOW}│                                              │${NC}"
echo -e "  ${YELLOW}│  Hostname:    ${HOSTNAME}$(printf '%*s' $((30 - ${#HOSTNAME})) '')│${NC}"
echo -e "  ${YELLOW}│  mDNS:        ${HOSTNAME}.local$(printf '%*s' $((24 - ${#HOSTNAME})) '')│${NC}"
echo -e "  ${YELLOW}│  Fallback IP: ${STATIC_IP}/${SUBNET}$(printf '%*s' $((27 - ${#STATIC_IP} - ${#SUBNET})) '')│${NC}"
echo -e "  ${YELLOW}│  Admin:       http://${HOSTNAME}.local:8080$(printf '%*s' $((13 - ${#HOSTNAME})) '')│${NC}"
if [ "$COMPANION_PRE_DETECTED" = true ]; then
echo -e "  ${YELLOW}│  Companion:   http://companion.local:8000    │${NC}"
fi
echo -e "  ${YELLOW}└──────────────────────────────────────────────┘${NC}"
echo ""
read -rp "  Proceed? [Y/n] " CONFIRM
CONFIRM="${CONFIRM:-Y}"
if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
    echo "  Aborted."
    exit 0
fi

# ── Step 1: Set hostname ─────────────────────────────────────

step 1 "Setting hostname to '${HOSTNAME}'..."

hostnamectl set-hostname "$HOSTNAME"

# Update /etc/hosts
if grep -q "127.0.1.1" /etc/hosts; then
    sed -i "s/127\.0\.1\.1.*/127.0.1.1\t${HOSTNAME}/" /etc/hosts
else
    echo -e "127.0.1.1\t${HOSTNAME}" >> /etc/hosts
fi

ok "Hostname set to ${HOSTNAME}"

# ── Step 2: Configure DHCP with static fallback ─────────────

step 2 "Configuring DHCP with static fallback on ${IFACE}..."

DHCPCD_CONF="/etc/dhcpcd.conf"
MARKER_START="# >>> MIDInet static fallback"
MARKER_END="# <<< MIDInet static fallback"

if [ ! -f "$DHCPCD_CONF" ]; then
    warn "dhcpcd.conf not found — your Pi may use NetworkManager instead."
    warn "Skipping DHCP fallback config. Set a static IP manually via:"
    warn "  nmcli con mod 'Wired connection 1' ipv4.addresses ${STATIC_IP}/${SUBNET}"
    warn "  nmcli con mod 'Wired connection 1' ipv4.method auto"
else
    # Remove any previous MIDInet block
    if grep -q "$MARKER_START" "$DHCPCD_CONF"; then
        sed -i "/$MARKER_START/,/$MARKER_END/d" "$DHCPCD_CONF"
    fi

    cat >> "$DHCPCD_CONF" << EOF

${MARKER_START}
profile static_${IFACE}
static ip_address=${STATIC_IP}/${SUBNET}

interface ${IFACE}
fallback static_${IFACE}
${MARKER_END}
EOF

    ok "DHCP with fallback ${STATIC_IP}/${SUBNET} on ${IFACE}"
fi

# ── Step 3: Install and configure Avahi ──────────────────────

step 3 "Configuring Avahi for mDNS (.local) resolution..."

if ! command -v avahi-daemon &>/dev/null; then
    echo "    Installing avahi-daemon..."
    apt-get install -y -qq avahi-daemon avahi-utils libnss-mdns > /dev/null 2>&1
fi

systemctl enable avahi-daemon
systemctl start avahi-daemon

ok "Avahi running — ${HOSTNAME}.local reachable via mDNS"

# Set up .local hostname aliases (companion.local)
COMPANION_DETECTED=false
if systemctl list-unit-files 2>/dev/null | grep -q companion; then
    COMPANION_DETECTED=true
    install -d -m 755 /etc/midinet 2>/dev/null || true
    echo "# MIDInet Avahi hostname aliases (one per line)" > /etc/midinet/avahi-aliases.conf
    echo "# These extra .local names all point to this Pi" >> /etc/midinet/avahi-aliases.conf
    echo "companion" >> /etc/midinet/avahi-aliases.conf
    # Restart alias service if installed
    systemctl restart midinet-avahi-alias.service 2>/dev/null || true
    ok "Alias: companion.local -> this Pi"
elif [ ! -f /etc/midinet/avahi-aliases.conf ]; then
    install -d -m 755 /etc/midinet 2>/dev/null || true
    echo "# MIDInet Avahi hostname aliases (one per line)" > /etc/midinet/avahi-aliases.conf
    echo "# These extra .local names all point to this Pi" >> /etc/midinet/avahi-aliases.conf
    echo "# Uncomment below if Bitfocus Companion is installed:" >> /etc/midinet/avahi-aliases.conf
    echo "# companion" >> /etc/midinet/avahi-aliases.conf
fi

# ── Step 4: Apply changes ────────────────────────────────────

step 4 "Applying changes..."

if [ -f "$DHCPCD_CONF" ]; then
    systemctl restart dhcpcd 2>/dev/null || true
fi

echo ""
echo -e "${GREEN}Network setup complete!${NC}"
echo ""
echo -e "  ${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "  ${YELLOW}  SAVE THIS — How to reach the Pi on-site${NC}"
echo -e "  ${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  Hostname:     ${HOSTNAME}"
echo "  Fallback IP:  ${STATIC_IP}/${SUBNET}"
echo "  Interface:    ${IFACE}"
echo ""
echo "  MIDInet Admin Panel:"
echo "    http://${HOSTNAME}.local:8080     (via mDNS — any network)"
echo "    http://${STATIC_IP}:8080          (direct connect / no router)"
if [ "$COMPANION_DETECTED" = true ]; then
echo ""
echo "  Bitfocus Companion:"
echo "    http://companion.local:8000       (dedicated .local alias)"
echo "    http://${HOSTNAME}.local:8000     (same Pi, port 8000)"
fi
echo ""
echo "  From any client running MIDInet:"
echo "    http://<client-ip>:5009/admin     (redirects to admin panel)"
echo ""
echo "  Reconfigure later (requires SSH or console):"
echo "    sudo midinet-cli network set-ip <new-ip>"
echo "    sudo midinet-cli network set-hostname <new-name>"
echo ""
echo -e "  ${YELLOW}A reboot is recommended:${NC} sudo reboot"
