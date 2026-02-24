# MIDInet

**Real-time MIDI over network with redundant failover.**

Distribute MIDI from a single physical controller to any number of clients over LAN. Each client creates a virtual MIDI device with the exact same identity as the original controller — existing mappings and scripts work unchanged.

Built for live production environments where reliability is non-negotiable.

*By [Hakol Fine AV Services](https://hakol.co.il)*

---

## How It Works

```
                         LAN (UDP Multicast)
                      ┌──────────────────────────┐
 [APC-40] ──USB──▶ [Host A — Primary]            │
                      │  Broadcasts on 239.69.83.1│
                      └──────────┬────────────────┘
                                 │
                  ┌──────────────┼──────────────┐
                  │              │              │
            ┌─────▼─────┐ ┌─────▼─────┐ ┌─────▼─────┐
            │ Client 1  │ │ Client 2  │ │ Client N  │
            │ Resolume  │ │ Resolume  │ │ Media Srv │
            │           │ │           │ │           │
            │ Virtual:  │ │ Virtual:  │ │ Virtual:  │
            │ "Akai     │ │ "Akai     │ │ "Akai     │
            │  APC40"   │ │  APC40"   │ │  APC40"   │
            └───────────┘ └───────────┘ └───────────┘
                  │
 [APC-40] ──USB──▶ [Host B — Standby]
                      │  Broadcasts on 239.69.83.2
                      └──────────────────────────┘
```

**Key features:**

- **~3ms latency** — USB read to virtual device output, well under the 15ms perceptible threshold
- **~10ms failover** — Dual-stream redundancy inspired by Dante/SMPTE ST 2022-7. Both hosts broadcast simultaneously; clients switch on 3 missed heartbeats
- **Zero-config** — mDNS/DNS-SD discovery (like AirPlay/NDI). Plug in a client and it finds hosts automatically
- **Identity cloning** — Virtual devices match the physical controller's name so Resolume Arena sees "Akai APC40", not a generic port
- **Bidirectional MIDI** — LED feedback and fader sync back to the controller via switchable focus
- **Cross-platform** — Host runs on Raspberry Pi (Linux/ALSA). Clients run on macOS (CoreMIDI), Windows (teVirtualMIDI), and Linux (ALSA)

---

## Architecture

| Crate | Purpose |
|-------|---------|
| `midi-protocol` | Shared types: packets, MIDI state, journal, pipeline, identity |
| `midi-host` | Host daemon — reads physical MIDI controller, broadcasts via UDP multicast |
| `midi-client` | Client daemon — receives multicast, creates virtual MIDI device |
| `midi-admin` | Web dashboard — REST API, WebSocket live updates, metrics, alerting |
| `midi-cli` | Management CLI — status, focus control, failover triggers |

### Protocol

Custom UDP multicast with RTP-like framing. Not Network MIDI 2.0 — that standard lacks multicast, redundancy, and open-source Linux support as of 2025.

| Packet | Multicast Group | Port | Interval |
|--------|----------------|------|----------|
| MIDI Data | 239.69.83.{1,2} | 5004 | Event-driven |
| Heartbeat | 239.69.83.{1,2} | 5005 | 3ms |
| Identity | 239.69.83.100 | 5006 | 5s + on connect |
| Focus | 239.69.83.100 | 5007 | On demand |

### Failover

Both hosts broadcast simultaneously on separate multicast groups. Failover is a **client-side decision** — no election protocol needed.

```
T+0ms     Primary host goes down
T+3ms     First missed heartbeat
T+6ms     Second missed heartbeat
T+9ms     Third missed heartbeat → switch to standby
T+10ms    All Notes Off + state reconciliation from journal
```

---

## Quick Start

### Prerequisites

- Rust toolchain (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`)
- Linux: `libasound2-dev` (`sudo apt install libasound2-dev`)
- macOS: Xcode Command Line Tools

### Build

```bash
git clone https://github.com/Hakolsound/MIDInet.git
cd MIDInet
cargo build --release
```

### Run the Host (Raspberry Pi / Linux)

```bash
# Auto-detect MIDI controller, broadcast as primary
./target/release/midi-host --config config/host.toml
```

### Run a Client (macOS / Linux / Windows)

```bash
# Auto-discover hosts via mDNS, create virtual device
./target/release/midi-client --config config/client.toml
```

### Open the Dashboard

```bash
# Start the admin panel
./target/release/midi-admin --listen 0.0.0.0:8080 --config midinet.toml

# Open in browser
open http://localhost:8080
```

### CLI

```bash
midinet-cli status              # System health overview
midinet-cli hosts               # List discovered hosts
midinet-cli clients             # List connected clients
midinet-cli focus               # Show current focus holder
midinet-cli focus 1             # Assign focus to client 1
midinet-cli failover            # Trigger manual failover
midinet-cli failover --status   # Show failover state
midinet-cli metrics             # MIDI throughput stats
midinet-cli metrics --system    # CPU, memory, temperature
midinet-cli alerts              # Active alerts
```

---

## Raspberry Pi Deployment

### One-Command Setup

SSH into your Pi and run:

```bash
curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/pi-provision.sh | sudo bash
```

This will:
1. Install system dependencies (ALSA, build tools)
2. Install the Rust toolchain
3. Tune the system for real-time performance (CPU governor, network buffers, RT scheduling)
4. Clone the repo to `/opt/midinet/src`
5. Build all binaries in release mode
6. Install binaries to `/usr/local/bin/`
7. Install and start systemd services
8. Install the `midinet-update` command

### Updating

After the initial setup, updating is one command:

```bash
sudo midinet-update
```

This pulls the latest code from GitHub, rebuilds, and restarts the services. Your config at `/etc/midinet/midinet.toml` is never overwritten.

### Remote Management (from your Mac)

```bash
# First-time provision (Pi must have SSH + internet)
make provision PI_HOST=pi@192.168.1.50

# Trigger a remote update
make update PI_HOST=pi@192.168.1.50

# View live logs
make logs PI_HOST=pi@192.168.1.50

# Check service status
make status PI_HOST=pi@192.168.1.50
```

### File Locations on Pi

| Path | Contents |
|------|----------|
| `/usr/local/bin/midi-*` | Binaries |
| `/etc/midinet/midinet.toml` | Configuration (edit this) |
| `/var/lib/midinet/` | Runtime data (metrics DB) |
| `/opt/midinet/src/` | Git clone (for rebuilds) |

### Systemd Services

```bash
sudo systemctl status midinet-host     # Host daemon
sudo systemctl status midinet-admin    # Admin panel
sudo systemctl restart midinet-host    # Restart after config change
journalctl -u midinet-host -f          # Live host logs
journalctl -u midinet-admin -f         # Live admin logs
```

The host daemon runs with `SCHED_FIFO` priority 80 and locked memory for real-time MIDI processing. The admin panel runs at normal priority.

---

## Client Installation

One-command installers for each platform. Each script clones the repo, builds natively, installs as a background service, and sets up auto-start.

### macOS

```bash
curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/client-install-macos.sh | bash
```

Installs:
- `midinet-client` and `midinet-cli` to `/usr/local/bin/`
- LaunchAgent for auto-start at login
- Config at `~/.midinet/config/client.toml`

The virtual MIDI device appears in Audio MIDI Setup once a host is discovered.

```bash
# Manage the service
launchctl unload ~/Library/LaunchAgents/co.hakol.midinet-client.plist   # Stop
launchctl load ~/Library/LaunchAgents/co.hakol.midinet-client.plist     # Start

# View logs
tail -f ~/.midinet/midinet-client.log

# Update to latest
bash ~/.midinet/src/scripts/client-install-macos.sh
```

### Windows

Run in PowerShell (as Administrator):

```powershell
irm https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/client-install-windows.ps1 | iex
```

**Prerequisite:** Install the [teVirtualMIDI driver](https://www.tobias-erichsen.de/software/virtualmidi.html) for virtual MIDI port creation. The installer will prompt you if it's missing.

Installs:
- `midinet-client.exe` and `midinet-cli.exe` to `%LOCALAPPDATA%\MIDInet\bin\` (added to PATH)
- Scheduled Task for auto-start at logon
- Config at `%LOCALAPPDATA%\MIDInet\config\client.toml`

```powershell
# Manage the service
Stop-ScheduledTask -TaskName "MIDInet Client"     # Stop
Start-ScheduledTask -TaskName "MIDInet Client"     # Start

# Update to latest
cd $env:LOCALAPPDATA\MIDInet\src; .\scripts\client-install-windows.ps1
```

### Linux

```bash
curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/client-install-linux.sh | bash
```

Installs:
- `midinet-client` and `midinet-cli` to `~/.midinet/bin/` (symlinked to `~/.local/bin/`)
- Systemd user service for auto-start
- Config at `~/.midinet/config/client.toml`
- Adds user to `audio` group for ALSA access

```bash
# Manage the service
systemctl --user stop midinet-client       # Stop
systemctl --user start midinet-client      # Start
systemctl --user status midinet-client     # Status

# View logs
journalctl --user -u midinet-client -f

# Update to latest
bash ~/.midinet/src/scripts/client-install-linux.sh
```

### Client Configuration

Client config is **optional** — mDNS discovery handles everything automatically. The client will auto-discover hosts on the LAN and connect without any configuration.

Edit the config only if you need to override defaults (e.g., force a specific network interface or device name):

```bash
# macOS
nano ~/.midinet/config/client.toml

# Linux
nano ~/.midinet/config/client.toml

# Windows
notepad %LOCALAPPDATA%\MIDInet\config\client.toml
```

---

## Configuration

### Host (`/etc/midinet/midinet.toml`)

```toml
[host]
id = 1                              # Lower = higher priority for primary
name = "host-a"

[network]
multicast_group = "239.69.83.1"     # .1 for primary, .2 for standby
data_port = 5004
heartbeat_port = 5005
control_group = "239.69.83.100"
control_port = 5006
interface = "eth0"

[heartbeat]
interval_ms = 3                     # 333 heartbeats/sec
miss_threshold = 3                  # Failover after ~9ms

[midi]
device = "auto"                     # Or "hw:1,0,0" for specific device

[failover]
auto_enabled = true
switch_back_policy = "manual"       # Don't auto-switch-back during a show
lockout_seconds = 5                 # Prevent rapid oscillation
confirmation_mode = "immediate"     # Or "confirm" for double-trigger

[failover.triggers.midi]
enabled = false
channel = 16
note = 127
velocity_threshold = 100
guard_note = 0                      # Hold this note as safety lock

[failover.triggers.osc]
enabled = false
listen_port = 8000
address = "/midinet/failover/switch"
allowed_sources = ["192.168.1.0/24"]
```

### Client (`config/client.toml`)

Client config is **optional** — mDNS discovery handles everything automatically. Use it only to override defaults:

```toml
[network]
# primary_group = "239.69.83.1"    # Override mDNS discovery
# standby_group = "239.69.83.2"
interface = "eth0"

[midi]
# device_name = "Akai APC40"       # Override cloned name

[failover]
jitter_buffer_us = 0               # 0 for wired LAN, 2000 for WiFi

[focus]
auto_claim = true                   # Claim focus on startup
```

---

## Web Dashboard

The admin panel serves an embedded single-page dashboard at `http://<host>:8080`.

**Features:**
- Live system overview with health score
- Host and client status with connection metrics
- MIDI activity monitor with real-time sparkline
- Failover controls with confirmation modal
- System metrics (CPU, memory, temperature, disk)
- Alert configuration with webhook support
- Pipeline configuration (channel filter, CC remap, velocity curves)
- Full REST API for automation

### API Authentication

Set a bearer token to protect API endpoints:

```bash
# Via environment variable
MIDINET_API_TOKEN=your-secret-token midi-admin --listen 0.0.0.0:8080

# Via CLI flag
midi-admin --api-token your-secret-token
```

Static files and WebSocket connections are always accessible (so the dashboard works without a token). Only `/api/*` routes require authentication:

```bash
curl -H "Authorization: Bearer your-secret-token" http://host:8080/api/status
```

### API Endpoints

```
GET  /api/status              System health + stats
GET  /api/hosts               Discovered hosts
GET  /api/clients             Connected clients

GET  /api/devices             Available MIDI devices

GET  /api/pipeline            Pipeline config
PUT  /api/pipeline            Update pipeline (hot reload)

GET  /api/metrics/system      CPU, memory, temp, disk
GET  /api/metrics/midi        Throughput, message counts
GET  /api/metrics/history     Historical metrics

GET  /api/focus               Current focus holder

GET  /api/failover            Failover state
POST /api/failover/switch     Trigger manual failover
PUT  /api/failover/auto       Enable/disable auto-failover

GET  /api/alerts              Active alerts
GET  /api/alerts/config       Alert thresholds
PUT  /api/alerts/config       Update alert config

GET  /api/config              Full MIDInet config
PUT  /api/config              Update config

WS   /ws/status               Real-time status (1s push)
WS   /ws/midi                 Real-time MIDI stream
WS   /ws/alerts               Real-time alert notifications
```

---

## Dual-Host Setup (Redundancy)

For production redundancy, run two Raspberry Pis with identical MIDI controllers:

**Host A** (`/etc/midinet/midinet.toml`):
```toml
[host]
id = 1                          # Primary (lower ID wins)
name = "host-a"

[network]
multicast_group = "239.69.83.1"
```

**Host B** (`/etc/midinet/midinet.toml`):
```toml
[host]
id = 2                          # Standby
name = "host-b"

[network]
multicast_group = "239.69.83.2"
```

Clients discover both via mDNS and subscribe to both streams. No client configuration needed.

---

## Manual Failover Triggers

| Method | How |
|--------|-----|
| **Dashboard** | Click "Switch Host" button in the web UI |
| **API** | `POST /api/failover/switch` |
| **CLI** | `midinet-cli failover` |
| **MIDI Note** | Press configured note (default: Ch16, Note 127, Vel>100) |
| **OSC** | Send `/midinet/failover/switch` to configured port |

Safety measures prevent accidental triggers:
- **Lockout period** — blocks rapid switching (default: 5s)
- **Confirmation mode** — optional double-trigger requirement
- **Guard note** — hold a safety button while pressing switch
- **Standby health gate** — refuses to switch if standby is unhealthy

---

## Development

```bash
# Build all crates
cargo build

# Run tests
cargo test --workspace

# Run with debug logging
RUST_LOG=debug cargo run -p midi-host -- --config config/host.toml

# Check without building
cargo check --workspace
```

### Cross-Compile for Pi (from macOS/Linux)

```bash
# Install cross-compilation toolchain
# macOS: brew install arm-linux-gnueabihf-binutils
# Ubuntu: sudo apt install gcc-aarch64-linux-gnu

# Add Rust target
rustup target add aarch64-unknown-linux-gnu

# Build
make build-pi

# Deploy to Pi via SCP
make deploy PI_HOST=pi@192.168.1.50
```

---

## Project Structure

```
MIDInet/
├── Cargo.toml                    # Workspace root
├── Makefile                      # Build & deploy targets
├── crates/
│   ├── midi-protocol/            # Shared types & serialization
│   │   ├── src/
│   │   │   ├── packets.rs        # Packet types (MIDI, heartbeat, identity, focus)
│   │   │   ├── midi_state.rs     # 16-channel MIDI state model
│   │   │   ├── journal.rs        # Compact state journal for failover
│   │   │   ├── pipeline.rs       # MIDI processing (filter, remap, curves)
│   │   │   ├── identity.rs       # Device identity cloning
│   │   │   └── ringbuf.rs        # Lock-free ring buffer
│   │   └── tests/
│   │       └── integration.rs    # 36 protocol tests
│   │
│   ├── midi-host/                # Host daemon (Raspberry Pi)
│   │   └── src/
│   │       ├── main.rs           # Orchestration & task spawning
│   │       ├── usb_reader.rs     # ALSA MIDI input
│   │       ├── broadcaster.rs    # UDP multicast sender + heartbeat
│   │       ├── discovery.rs      # mDNS service advertisement
│   │       ├── pipeline.rs       # Re-export from midi-protocol
│   │       ├── failover.rs       # Primary/standby role management
│   │       ├── feedback.rs       # Bidirectional: network → controller
│   │       ├── osc_listener.rs   # OSC command handler
│   │       └── metrics.rs        # Internal metrics collection
│   │
│   ├── midi-client/              # Client daemon
│   │   └── src/
│   │       ├── main.rs           # Discovery → receive → virtual device
│   │       ├── receiver.rs       # Multicast receiver + pipeline
│   │       ├── discovery.rs      # mDNS browser + auto-connect
│   │       ├── failover.rs       # Dual-stream heartbeat monitor
│   │       ├── focus.rs          # Bidirectional focus + feedback
│   │       ├── virtual_device.rs # Platform abstraction trait
│   │       └── platform/
│   │           ├── macos.rs      # CoreMIDI virtual ports
│   │           ├── linux.rs      # ALSA sequencer virtual ports
│   │           └── windows.rs    # teVirtualMIDI FFI
│   │
│   ├── midi-admin/               # Web admin panel
│   │   └── src/
│   │       ├── main.rs           # Axum server + config loading
│   │       ├── api/              # REST endpoints
│   │       ├── websocket.rs      # WebSocket hub
│   │       ├── auth.rs           # Bearer token middleware
│   │       ├── collector.rs      # System metrics sampler (1Hz)
│   │       ├── alerting.rs       # Threshold alerts + webhooks
│   │       ├── metrics_store.rs  # Ring buffer + SQLite retention
│   │       ├── state.rs          # Shared application state
│   │       └── static/           # Embedded dashboard (HTML/CSS/JS)
│   │
│   └── midi-cli/                 # Management CLI
│       └── src/main.rs           # HTTP client → admin API
│
├── config/
│   ├── host.toml                 # Host configuration template
│   └── client.toml               # Client configuration template
│
├── deploy/
│   ├── midinet-host.service      # Systemd unit (RT priority)
│   ├── midinet-admin.service     # Systemd unit (admin panel)
│   └── install.sh                # Binary installation script
│
└── scripts/
    ├── pi-provision.sh           # Full Pi provisioning (git clone + build)
    ├── pi-update.sh              # Pull + rebuild + restart
    ├── client-install-macos.sh   # macOS client installer (LaunchAgent)
    ├── client-install-linux.sh   # Linux client installer (systemd user)
    ├── client-install-windows.ps1 # Windows client installer (Scheduled Task)
    ├── setup-pi.sh               # System tuning (RT kernel, sysctl)
    └── install-service.sh        # Systemd service setup
```

---

## License

MIT

---

*Built by [Hakol Fine AV Services](https://hakol.co.il) for production live events.*
