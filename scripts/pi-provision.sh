#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Raspberry Pi Provisioning Script
# Clones the repo from GitHub, builds natively, and installs.
#
# Usage (run on the Pi over SSH):
#   curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/pi-provision.sh | sudo bash
#
# Or clone first, then run:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet && sudo bash scripts/pi-provision.sh
#
# Environment variables:
#   MIDINET_BRANCH  — git branch to build (default: main)
#   MIDINET_DIR     — clone directory (default: /opt/midinet/src)
#   SKIP_SETUP      — set to 1 to skip system tuning (for re-deploys)
# ──────────────────────────────────────────────────────────────
set -euo pipefail

BRANCH="${MIDINET_BRANCH:-main}"
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
    TOTAL_STEPS=5
else
    TOTAL_STEPS=8
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

    # Disable unnecessary services
    for svc in bluetooth avahi-daemon triggerhappy hciuart; do
        systemctl disable "$svc" 2>/dev/null || true
        systemctl stop "$svc" 2>/dev/null || true
    done
    ok "Unnecessary services disabled"

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

# Install systemd units (always update these)
install -m 644 "$MIDINET_DIR/deploy/midinet-host.service"  /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-admin.service" /etc/systemd/system/
systemctl daemon-reload
ok "Systemd services installed"

# ── Start services ───────────────────────────────────────────
STEP=$((STEP + 1))
step $STEP "Starting MIDInet services..."
systemctl enable midinet-host.service midinet-admin.service
systemctl start midinet-host.service
systemctl start midinet-admin.service
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
echo "  Dashboard: http://$(hostname -I | awk '{print $1}'):8080"
echo ""
echo "  Useful commands:"
echo "    journalctl -u midinet-host -f        # Host logs"
echo "    journalctl -u midinet-admin -f       # Admin logs"
echo "    systemctl status midinet-host        # Service status"
echo "    midinet-cli status                   # System status"
echo "    sudo midinet-update                  # Pull & rebuild"
echo ""
echo "  Edit config: sudo nano /etc/midinet/midinet.toml"
echo "  Then reload: sudo systemctl restart midinet-host midinet-admin"
echo ""
