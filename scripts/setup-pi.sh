#!/bin/bash
# MIDInet Raspberry Pi Setup Script
# Configures a Raspberry Pi for optimal real-time MIDI hosting.
# Run as root: sudo bash setup-pi.sh

set -e

echo "=== MIDInet Raspberry Pi Setup ==="

# 1. System updates
echo "[1/7] Updating system packages..."
apt-get update && apt-get upgrade -y

# 2. Install real-time kernel (if available)
echo "[2/7] Installing real-time kernel..."
if apt-cache show linux-image-rt-arm64 &>/dev/null; then
    apt-get install -y linux-image-rt-arm64
    echo "Real-time kernel installed. Reboot to activate."
else
    echo "RT kernel not available in repos. Consider building from source."
    echo "See: https://wiki.linuxfoundation.org/realtime/documentation/howto/applications/preemptrt_setup"
fi

# 3. Install ALSA development libraries
echo "[3/7] Installing ALSA libraries..."
apt-get install -y alsa-utils libasound2-dev

# 4. Network tuning for low-latency multicast
echo "[4/7] Tuning network stack..."
cat >> /etc/sysctl.d/99-midinet.conf << 'EOF'
# MIDInet network tuning
# Increase socket buffer sizes
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.core.rmem_default = 1048576
net.core.wmem_default = 1048576

# Reduce network latency
net.ipv4.tcp_low_latency = 1

# Enable multicast
net.ipv4.igmp_max_memberships = 64
EOF

sysctl -p /etc/sysctl.d/99-midinet.conf

# 5. CPU governor: set to performance mode
echo "[5/7] Setting CPU governor to performance..."
apt-get install -y cpufrequtils
echo 'GOVERNOR="performance"' > /etc/default/cpufrequtils
systemctl restart cpufrequtils 2>/dev/null || true

# Also set immediately
for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
    echo "performance" > "$cpu" 2>/dev/null || true
done

# 6. Disable unnecessary services
echo "[6/7] Disabling unnecessary services..."
systemctl disable bluetooth 2>/dev/null || true
systemctl disable avahi-daemon 2>/dev/null || true  # We use our own mDNS
systemctl disable triggerhappy 2>/dev/null || true
systemctl disable hciuart 2>/dev/null || true

# 7. Create midinet user and directories
echo "[7/7] Creating midinet user and directories..."
if ! id midinet &>/dev/null; then
    useradd -r -s /bin/false -d /opt/midinet midinet
fi
mkdir -p /opt/midinet/{bin,config,logs}
chown -R midinet:midinet /opt/midinet

# Allow midinet user to set real-time priority
cat >> /etc/security/limits.d/99-midinet.conf << 'EOF'
midinet    -    rtprio    99
midinet    -    nice      -20
midinet    -    memlock   unlimited
EOF

echo ""
echo "=== Setup complete ==="
echo "Next steps:"
echo "  1. Copy midi-host binary to /opt/midinet/bin/"
echo "  2. Copy host.toml to /opt/midinet/config/"
echo "  3. Run: sudo bash install-service.sh"
echo "  4. Reboot if RT kernel was installed"
