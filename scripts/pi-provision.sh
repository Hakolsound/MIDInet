#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Raspberry Pi Provisioning Script
# Clones the repo from GitHub, builds natively, and installs.
#
# Usage (run on the Pi over SSH):
#   curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/pi-provision.sh | sudo bash
#
# Or clone first, then run:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet && sudo bash scripts/pi-provision.sh
#
# Environment variables:
#   MIDINET_BRANCH  — git branch to build (default: v3.1)
#   MIDINET_DIR     — clone directory (default: /opt/midinet/src)
#   SKIP_SETUP      — set to 1 to skip system tuning (for re-deploys)
# ──────────────────────────────────────────────────────────────
set -euo pipefail

BRANCH="${MIDINET_BRANCH:-v3.1}"
MIDINET_DIR="${MIDINET_DIR:-/opt/midinet/src}"
REPO_URL="https://github.com/Hakolsound/MIDInet.git"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

step() { echo -e "\n${CYAN}[$1/$TOTAL_STEPS]${NC} $2"; }
ok()   { echo -e "    ${GREEN}✓${NC} $1"; }
warn() { echo -e "    ${YELLOW}!${NC} $1"; }
fail() { echo -e "    ${RED}✗${NC} $1"; exit 1; }

# ── Pre-flight checks ────────────────────────────────────────
if [ "$(id -u)" -ne 0 ]; then
    fail "This script must be run as root (sudo)"
fi

ARCH=$(uname -m)
if [[ "$ARCH" != "aarch64" && "$ARCH" != "armv7l" ]]; then
    warn "Expected ARM architecture, got $ARCH. Continuing anyway..."
fi

if [ "${SKIP_SETUP:-0}" = "1" ]; then
    TOTAL_STEPS=6
else
    TOTAL_STEPS=9
fi

echo -e "${CYAN}"
echo "  ┌──────────────────────────────────────┐"
echo "  │   MIDInet — Raspberry Pi Provision    │"
echo "  │   Hakol Fine AV Services              │"
echo "  └──────────────────────────────────────┘"
echo -e "${NC}"
echo "  Branch:    $BRANCH"
echo "  Clone dir: $MIDINET_DIR"
echo ""

# ── Network setup prompts (collect upfront before long-running steps) ──

DEFAULT_IFACE=$(ip route show default 2>/dev/null | awk '{print $5; exit}')
DEFAULT_IFACE="${DEFAULT_IFACE:-eth0}"

echo -e "  ${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "  ${CYAN}  Network Configuration${NC}"
echo -e "  ${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo -e "  ${YELLOW}IMPORTANT: Write these settings down!${NC}"
echo "  These are how you'll reach the Pi when there's no monitor."
echo "  Changing them later requires SSH or direct console access."
echo ""
echo "  How it works:"
echo "    1. DHCP first — Pi gets an IP from the router (if available)"
echo "    2. Fallback — if no DHCP server responds, Pi uses the static IP"
echo "    3. mDNS — Pi is always reachable at <hostname>.local"
echo ""

# Detect if Companion is installed
COMPANION_DETECTED=false
if systemctl list-unit-files 2>/dev/null | grep -q companion; then
    COMPANION_DETECTED=true
    echo -e "  ${GREEN}Bitfocus Companion detected on this Pi.${NC}"
    echo "  No conflicts — Companion uses port 8000, MIDInet uses 8080."
    echo "  Both share the same hostname and static IP."
    echo ""
fi

read -rp "  Hostname [midinet]: " MIDINET_HOSTNAME
MIDINET_HOSTNAME="${MIDINET_HOSTNAME:-midinet}"

read -rp "  Static fallback IP [192.168.50.1]: " STATIC_IP
STATIC_IP="${STATIC_IP:-192.168.50.1}"

read -rp "  Subnet CIDR [24]: " SUBNET
SUBNET="${SUBNET:-24}"

read -rp "  Network interface [${DEFAULT_IFACE}]: " IFACE
IFACE="${IFACE:-$DEFAULT_IFACE}"

echo ""
echo -e "  ${YELLOW}┌──────────────────────────────────────────────┐${NC}"
echo -e "  ${YELLOW}│  WRITE THIS DOWN — you'll need it on-site:   │${NC}"
echo -e "  ${YELLOW}│                                              │${NC}"
echo -e "  ${YELLOW}│  Hostname:    ${MIDINET_HOSTNAME}$(printf '%*s' $((30 - ${#MIDINET_HOSTNAME})) '')│${NC}"
echo -e "  ${YELLOW}│  mDNS:        ${MIDINET_HOSTNAME}.local$(printf '%*s' $((24 - ${#MIDINET_HOSTNAME})) '')│${NC}"
echo -e "  ${YELLOW}│  Fallback IP: ${STATIC_IP}/${SUBNET}$(printf '%*s' $((27 - ${#STATIC_IP} - ${#SUBNET})) '')│${NC}"
echo -e "  ${YELLOW}│  Interface:   ${IFACE}$(printf '%*s' $((30 - ${#IFACE})) '')│${NC}"
echo -e "  ${YELLOW}│                                              │${NC}"
echo -e "  ${YELLOW}│  Admin panel:                                │${NC}"
echo -e "  ${YELLOW}│    http://${MIDINET_HOSTNAME}.local:8080$(printf '%*s' $((20 - ${#MIDINET_HOSTNAME})) '')│${NC}"
echo -e "  ${YELLOW}│    http://${STATIC_IP}:8080$(printf '%*s' $((28 - ${#STATIC_IP})) '')│${NC}"
if [ "$COMPANION_DETECTED" = true ]; then
echo -e "  ${YELLOW}│                                              │${NC}"
echo -e "  ${YELLOW}│  Companion (separate .local alias):          │${NC}"
echo -e "  ${YELLOW}│    http://companion.local:8000               │${NC}"
echo -e "  ${YELLOW}│    http://${MIDINET_HOSTNAME}.local:8000$(printf '%*s' $((20 - ${#MIDINET_HOSTNAME})) '')│${NC}"
fi
echo -e "  ${YELLOW}└──────────────────────────────────────────────┘${NC}"
echo ""
read -rp "  Ready to proceed? [Y/n] " CONFIRM
CONFIRM="${CONFIRM:-Y}"
if [[ ! "$CONFIRM" =~ ^[Yy]$ ]]; then
    echo "  Aborted."
    exit 0
fi
echo ""

STEP=0

# ── System Setup (skippable for re-deploys) ──────────────────
if [ "${SKIP_SETUP:-0}" != "1" ]; then

    STEP=$((STEP + 1))
    step $STEP "Installing system dependencies..."
    apt-get update -qq
    apt-get install -y -qq \
        build-essential \
        pkg-config \
        libasound2-dev \
        alsa-utils \
        git \
        curl \
        cpufrequtils \
        > /dev/null 2>&1
    ok "System packages installed"

    STEP=$((STEP + 1))
    step $STEP "Installing Rust toolchain..."
    if command -v rustup &>/dev/null; then
        ok "Rust already installed ($(rustc --version))"
        sudo -u midi rustup update stable 2>/dev/null || rustup update stable 2>/dev/null || true
    else
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
        source "$HOME/.cargo/env" 2>/dev/null || true
        ok "Rust toolchain installed ($(rustc --version))"
    fi
    # Ensure cargo is on PATH for the rest of this script
    export PATH="$HOME/.cargo/bin:/usr/local/cargo/bin:$PATH"

    STEP=$((STEP + 1))
    step $STEP "Tuning system for real-time performance..."

    # Network tuning
    cat > /etc/sysctl.d/99-midinet.conf << 'SYSCTL'
# MIDInet network tuning for low-latency multicast
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.core.rmem_default = 1048576
net.core.wmem_default = 1048576
net.ipv4.tcp_low_latency = 1
net.ipv4.igmp_max_memberships = 64
SYSCTL
    sysctl -p /etc/sysctl.d/99-midinet.conf > /dev/null 2>&1
    ok "Network stack tuned"

    # CPU governor
    echo 'GOVERNOR="performance"' > /etc/default/cpufrequtils
    for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
        echo "performance" > "$cpu" 2>/dev/null || true
    done
    ok "CPU governor set to performance"

    # Disable unnecessary services (avahi kept for .local hostname resolution)
    for svc in bluetooth triggerhappy hciuart; do
        systemctl disable "$svc" 2>/dev/null || true
        systemctl stop "$svc" 2>/dev/null || true
    done
    ok "Unnecessary services disabled"

fi

# ── Network identity (hostname + DHCP fallback + Avahi) ──────
STEP=$((STEP + 1))
step $STEP "Configuring network identity..."

# Set hostname
hostnamectl set-hostname "$MIDINET_HOSTNAME"
if grep -q "127.0.1.1" /etc/hosts; then
    sed -i "s/127\.0\.1\.1.*$/127.0.1.1\t${MIDINET_HOSTNAME}/" /etc/hosts
else
    echo -e "127.0.1.1\t${MIDINET_HOSTNAME}" >> /etc/hosts
fi
ok "Hostname: ${MIDINET_HOSTNAME} (${MIDINET_HOSTNAME}.local)"

# DHCP with static fallback
DHCPCD_CONF="/etc/dhcpcd.conf"
MARKER_START="# >>> MIDInet static fallback"
MARKER_END="# <<< MIDInet static fallback"

if [ -f "$DHCPCD_CONF" ]; then
    # Remove any previous MIDInet block
    if grep -q "$MARKER_START" "$DHCPCD_CONF"; then
        sed -i "/$MARKER_START/,/$MARKER_END/d" "$DHCPCD_CONF"
    fi

    cat >> "$DHCPCD_CONF" << DHCP

${MARKER_START}
profile static_${IFACE}
static ip_address=${STATIC_IP}/${SUBNET}

interface ${IFACE}
fallback static_${IFACE}
${MARKER_END}
DHCP
    ok "DHCP fallback: ${STATIC_IP}/${SUBNET} on ${IFACE}"
else
    warn "dhcpcd.conf not found (NetworkManager?). Set static IP manually."
fi

# Install and enable Avahi for .local mDNS resolution
if ! command -v avahi-daemon &>/dev/null; then
    apt-get install -y -qq avahi-daemon avahi-utils libnss-mdns > /dev/null 2>&1
fi
systemctl enable avahi-daemon 2>/dev/null || true
systemctl start avahi-daemon 2>/dev/null || true
ok "Avahi enabled (${MIDINET_HOSTNAME}.local reachable via mDNS)"

# Set up .local hostname aliases (companion.local, etc.)
# Always create the config — companion.local is added if Companion is detected
install -d -m 755 /etc/midinet 2>/dev/null || true
if [ "$COMPANION_DETECTED" = true ]; then
    echo "# MIDInet Avahi hostname aliases (one per line)" > /etc/midinet/avahi-aliases.conf
    echo "# These extra .local names all point to this Pi" >> /etc/midinet/avahi-aliases.conf
    echo "companion" >> /etc/midinet/avahi-aliases.conf
    ok "Alias: companion.local -> this Pi (Companion detected)"
else
    if [ ! -f /etc/midinet/avahi-aliases.conf ]; then
        echo "# MIDInet Avahi hostname aliases (one per line)" > /etc/midinet/avahi-aliases.conf
        echo "# These extra .local names all point to this Pi" >> /etc/midinet/avahi-aliases.conf
        echo "# Uncomment below if Bitfocus Companion is installed:" >> /etc/midinet/avahi-aliases.conf
        echo "# companion" >> /etc/midinet/avahi-aliases.conf
        ok "Alias config created (companion.local commented out — enable if needed)"
    fi
fi

# ── Create user & directories ────────────────────────────────
STEP=$((STEP + 1))
step $STEP "Creating midi user and directories..."
if ! id midi &>/dev/null; then
    useradd -r -s /usr/sbin/nologin -m -d /opt/midinet midi
    ok "Created system user: midi"
else
    ok "User midi already exists"
fi

install -d -o midi -g midi -m 755 /etc/midinet
install -d -o midi -g midi -m 755 /var/lib/midinet
install -d -o midi -g midi -m 755 /opt/midinet/src

# RT priority limits
cat > /etc/security/limits.d/99-midinet.conf << 'LIMITS'
midi    -    rtprio    99
midi    -    nice      -20
midi    -    memlock   unlimited
LIMITS
ok "Directories and permissions configured"

# Add midi user to audio group for ALSA access
usermod -aG audio midi 2>/dev/null || true

# ── Clone / update repository ────────────────────────────────
STEP=$((STEP + 1))
step $STEP "Fetching MIDInet source from GitHub..."
if [ -d "$MIDINET_DIR/.git" ]; then
    cd "$MIDINET_DIR"
    git fetch origin
    git checkout "$BRANCH"
    git reset --hard "origin/$BRANCH"
    ok "Updated to latest $BRANCH"
else
    git clone --branch "$BRANCH" "$REPO_URL" "$MIDINET_DIR"
    cd "$MIDINET_DIR"
    ok "Cloned $REPO_URL ($BRANCH)"
fi
chown -R midi:midi /opt/midinet

# ── Build ────────────────────────────────────────────────────
STEP=$((STEP + 1))
step $STEP "Building MIDInet (release mode — this may take a while)..."
cd "$MIDINET_DIR"

# Build as midi user if possible, otherwise as root
if sudo -u midi bash -c "source \$HOME/.cargo/env 2>/dev/null; cargo build --release" 2>/dev/null; then
    ok "Build complete"
else
    # Fallback: build as root (e.g. if Rust installed for root only)
    cargo build --release
    ok "Build complete (as root)"
fi

# ── Install binaries & services ──────────────────────────────
STEP=$((STEP + 1))
step $STEP "Installing binaries and systemd services..."

# Stop services if running (ignore failures on first install)
systemctl stop midinet-admin.service 2>/dev/null || true
systemctl stop midinet-host.service 2>/dev/null || true

# Install binaries
install -m 755 "$MIDINET_DIR/target/release/midi-host"  /usr/local/bin/midi-host
install -m 755 "$MIDINET_DIR/target/release/midi-admin" /usr/local/bin/midi-admin
install -m 755 "$MIDINET_DIR/target/release/midi-cli"   /usr/local/bin/midi-cli
# Stamp installed version for update detection
git -C "$MIDINET_DIR" rev-parse --short HEAD > /usr/local/bin/.midinet-version
# Write source directory marker so the admin service can find it for update checks
echo "$MIDINET_DIR" > /var/lib/midinet/src-dir
ok "Binaries installed to /usr/local/bin/"

# Install the update command
install -m 755 "$MIDINET_DIR/scripts/pi-update.sh" /usr/local/bin/midinet-update
ok "Update command installed (run: sudo midinet-update)"

# Install config if not present (never overwrite existing config)
if [ ! -f /etc/midinet/midinet.toml ]; then
    install -m 644 -o midi -g midi "$MIDINET_DIR/config/host.toml" /etc/midinet/midinet.toml
    ok "Default config installed to /etc/midinet/midinet.toml"
else
    warn "Config already exists at /etc/midinet/midinet.toml — not overwriting"
fi

# Install Avahi alias publisher
install -m 755 "$MIDINET_DIR/scripts/midinet-avahi-alias.sh" /usr/local/bin/midinet-avahi-alias
ok "Avahi alias publisher installed"

# Install systemd units (always update these)
install -m 644 "$MIDINET_DIR/deploy/midinet-host.service"       /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-admin.service"      /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-avahi-alias.service" /etc/systemd/system/
systemctl daemon-reload
ok "Systemd services installed"

# ── Start services ───────────────────────────────────────────
STEP=$((STEP + 1))
step $STEP "Starting MIDInet services..."
systemctl enable midinet-host.service midinet-admin.service midinet-avahi-alias.service
systemctl start midinet-host.service
systemctl start midinet-admin.service
systemctl start midinet-avahi-alias.service
ok "Services started"

# ── Done ─────────────────────────────────────────────────────
echo ""
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo -e "${GREEN}  MIDInet installation complete!${NC}"
echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
echo ""
echo "  Binaries:  /usr/local/bin/midi-{host,admin,cli}"
echo "  Config:    /etc/midinet/midinet.toml"
echo "  Data:      /var/lib/midinet/"
echo "  Source:    $MIDINET_DIR"
echo ""
echo -e "  ${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo -e "  ${YELLOW}  SAVE THIS — How to reach the Pi on-site${NC}"
echo -e "  ${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  Hostname:     ${MIDINET_HOSTNAME}"
echo "  Fallback IP:  ${STATIC_IP}/${SUBNET}"
echo ""
echo "  MIDInet Admin Panel:"
echo "    http://${MIDINET_HOSTNAME}.local:8080     (via mDNS — any network)"
CURRENT_IP=$(hostname -I 2>/dev/null | awk '{print $1}')
if [ -n "$CURRENT_IP" ]; then
echo "    http://${CURRENT_IP}:8080                 (current DHCP IP)"
fi
echo "    http://${STATIC_IP}:8080                  (direct connect / no router)"
if [ "$COMPANION_DETECTED" = true ]; then
echo ""
echo "  Bitfocus Companion:"
echo "    http://companion.local:8000               (dedicated .local alias)"
echo "    http://${MIDINET_HOSTNAME}.local:8000     (same Pi, port 8000)"
fi
echo ""
echo "  From any client machine running MIDInet:"
echo "    http://<client-ip>:5009/admin             (redirects to admin panel)"
echo ""
echo -e "  ${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
echo ""
echo "  Useful commands:"
echo "    journalctl -u midinet-host -f        # Host logs"
echo "    journalctl -u midinet-admin -f       # Admin logs"
echo "    systemctl status midinet-host        # Service status"
echo "    midinet-cli status                   # System status"
echo "    midinet-cli network                  # Show network config"
echo "    sudo midinet-update                  # Pull & rebuild"
echo ""
echo "  Edit config: sudo nano /etc/midinet/midinet.toml"
echo "  Then reload: sudo systemctl restart midinet-host midinet-admin"
echo ""
echo "  Reconfigure network later (requires SSH or console access):"
echo "    sudo midinet-cli network set-ip <new-ip>"
echo "    sudo midinet-cli network set-hostname <new-name>"
echo "    Or re-run: sudo bash scripts/pi-network-setup.sh"
echo ""
