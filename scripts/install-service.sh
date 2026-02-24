#!/bin/bash
# Install MIDInet as a systemd service
# Run as root: sudo bash install-service.sh

set -e

echo "=== Installing MIDInet systemd service ==="

# Create systemd service file
cat > /etc/systemd/system/midinet-host.service << 'EOF'
[Unit]
Description=MIDInet Host Daemon
After=network-online.target sound.target
Wants=network-online.target

[Service]
Type=simple
User=midinet
Group=midinet
ExecStart=/opt/midinet/bin/midi-host --config /opt/midinet/config/host.toml
Restart=always
RestartSec=2
WatchdogSec=30

# Real-time priority
Nice=-20
CPUSchedulingPolicy=fifo
CPUSchedulingPriority=80

# Resource limits
LimitRTPRIO=99
LimitMEMLOCK=infinity
LimitNOFILE=65536

# Security hardening
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/midinet/logs
NoNewPrivileges=false
PrivateTmp=true

# Logging
StandardOutput=journal
StandardError=journal
SyslogIdentifier=midinet-host

[Install]
WantedBy=multi-user.target
EOF

# Create admin panel service
cat > /etc/systemd/system/midinet-admin.service << 'EOF'
[Unit]
Description=MIDInet Web Admin Panel
After=midinet-host.service
Requires=midinet-host.service

[Service]
Type=simple
User=midinet
Group=midinet
ExecStart=/opt/midinet/bin/midi-admin --listen 0.0.0.0:8080
Restart=always
RestartSec=5

# Lower priority than host daemon
Nice=0

# Security
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/midinet/logs /opt/midinet/config
NoNewPrivileges=true
PrivateTmp=true

StandardOutput=journal
StandardError=journal
SyslogIdentifier=midinet-admin

[Install]
WantedBy=multi-user.target
EOF

# Reload systemd and enable services
systemctl daemon-reload
systemctl enable midinet-host.service
systemctl enable midinet-admin.service

echo "Services installed and enabled."
echo "Start with: sudo systemctl start midinet-host"
echo "View logs:  sudo journalctl -u midinet-host -f"
