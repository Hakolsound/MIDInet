# MIDInet

**Real-time MIDI over network with redundant failover.**

Distribute MIDI from physical controllers to any number of clients over LAN. Each client creates virtual MIDI devices with the exact same identity as the originals — existing mappings and scripts work unchanged.

The host runs on a **Raspberry Pi** — a $80 device that sits on your network rack  on the same Pi as **Bitfocus Companion**, giving you both MIDI distribution and StreamDeck control from one tiny box.

Built for live production environments where reliability is non-negotiable.

🌐 **[midinet.io](https://midinet.io)** — Website & docs
💬 **[Discord Community](https://discord.gg/4s2ZjkB7N3)** — Support & discussion

*By [Hakol Fine AV Services](https://www.fineavservices.com)*

---

## What's New in v3.1

| Feature | Details |
|---------|---------|
| **Multi-Device Highways** | Up to 16 independent MIDI controllers on a single host. Each gets its own virtual port on every client. Protocol v2. |
| **Three Operational Modes** | Single (one controller), Redundant (primary + backup), Multi-Device (up to 16). Switch live from the admin dashboard with safety checks. |
| **Native System Tray** | Color-coded health icon, focus control, update checks, protected app detection, auto-start — on macOS, Windows, and Linux. |
| **Self-Update System** | One-click updates from the admin dashboard or system tray. Real-time progress streaming. Version mismatch detection across host and clients. |
| **Protected Apps** | Blocks mode changes and restarts when show-critical apps (Resolume, Ableton, QLab, etc.) are running. 27-app catalog. |
| **Pi Network CLI** | Interactive setup for hostname, static fallback IP, Avahi mDNS, and Companion integration. |
| **Discord Bot** | Community server management with roles, channels, and react-to-role. |
| **Load Test Suite** | Latency, throughput, burst, failover, soak, and pipeline benchmarks over real UDP multicast. |

---

## How It Works

A Raspberry Pi reads your physical MIDI controller(s) via USB and broadcasts over your existing LAN. Three operational modes, each with optional dual-host redundancy:

### Single Mode
```
 [APC-40] ──USB──▶ [Host A — Primary]  ──UDP Multicast──▶  [Client 1]  [Client 2]  [Client N]
                    [Host B — Standby]  ──UDP Multicast──▶  (same clients, ~10ms failover)
```

### Redundant Mode (SMPTE ST 2022-7)
```
 [APC-40 — Primary]  ──USB──▶ [Host A — Primary]   ═══ Stream 1 ═══▶  ┌───────────┐
 [APC-40 — Backup]   ──USB──▶ [Host B — Standby]   ═══ Stream 2 ═══▶  │ Client 1  │
                                                                        │ Client 2  │
           Hardware failover        Dual-stream broadcast               │ Client N  │
           (activity-based)         (~10ms client-side switch)          └───────────┘
```

### Multi-Device Mode
```
 [APC-40]      ─┐                                          ┌─ Virtual: "Akai APC40"
 [Launchpad]   ─┤ USB ──▶ [Host A — Primary]  ──UDP──▶    ├─ Virtual: "Launchpad"
 [Fader Bank]  ─┤          [Host B — Standby]              ├─ Virtual: "Fader Bank"
 [Drum Pad]    ─┘          (up to 16 devices)              └─ Virtual: "Drum Pad"
                                                            × every client on the LAN
```

**Key features:**

- **<5ms latency** — USB read to virtual device output, well under the 15ms perceptible threshold
- **~10ms failover** — Dual-stream redundancy inspired by Dante/SMPTE ST 2022-7. Both hosts broadcast simultaneously; clients switch on 3 missed heartbeats
- **Up to 16 devices** — Multi-device highways with independent virtual ports per controller
- **Zero-config** — mDNS/DNS-SD discovery (like AirPlay/NDI). Plug in a client and it finds hosts automatically
- **Identity cloning** — Virtual devices match the physical controller's name so Resolume Arena sees "Akai APC40", not a generic port
- **Bidirectional MIDI** — LED feedback and fader sync back to the controller via switchable focus
- **Cross-platform** — Host runs on Raspberry Pi (Linux/ALSA). Clients run on macOS (CoreMIDI), Windows (teVirtualMIDI / Windows MIDI Services), and Linux (ALSA)
- **System tray app** — Native tray icon with health monitoring, focus control, update checks on macOS, Windows, and Linux
- **Self-updating** — One-click updates from dashboard or tray with real-time progress and version mismatch detection

---

## Architecture

| Crate | Purpose |
|-------|---------|
| `midi-protocol` | Shared types: packets (v2), MIDI state, journal, pipeline, identity, operational modes |
| `midi-host` | Host daemon — reads physical MIDI controller(s), broadcasts via UDP multicast, multi-device highway manager |
| `midi-client` | Client daemon — receives multicast, creates virtual MIDI device(s), failover monitor |
| `midi-admin` | Web dashboard — REST API, WebSocket live updates, metrics, alerting, mode selector, protected apps |
| `midi-cli` | Management CLI — status, focus control, failover triggers, network setup |
| `midi-tray` | Native system tray — health icon, focus control, update checks, protected apps (macOS/Windows/Linux) |
| `midi-loadtest` | QA suite — latency, throughput, burst, failover, soak, and pipeline benchmarks |
| `discord-bot` | Discord community bot — roles, channels, react-to-role, welcome messages (Python) |

### Protocol (v2)

Custom UDP multicast with RTP-like framing. Protocol v2 adds `device_id` throughout for multi-device support. Backward-compatible with v1 clients.

| Packet | Multicast Group | Port | Interval | v2 Changes |
|--------|----------------|------|----------|------------|
| MIDI Data | 239.69.83.{1,2} | 5004 | Event-driven | `device_id` byte, `FLAG_V2` flag |
| Heartbeat | 239.69.83.{1,2} | 5005 | 3ms | `device_mask` bitmask (16-bit) |
| Identity | 239.69.83.100 | 5006 | 5s + on connect | Per-device identity, version marker |
| Focus | 239.69.83.100 | 5007 | On demand | Mode byte + `device_id` |
| Discovery | 239.69.83.100 | 5008 | On demand | `device_count`, `operational_mode` |

### Failover

Both hosts broadcast simultaneously on separate multicast groups. Failover is a **client-side decision** — no election protocol needed.

```
T+0ms     Primary host goes down
T+3ms     First missed heartbeat
T+6ms     Second missed heartbeat
T+9ms     Third missed heartbeat → switch to standby
T+10ms    All Notes Off + state reconciliation from journal
```

Failover presets: `safe_defaults`, `rock_solid` (5ms/5 miss, 10s lockout), `low_latency` (2ms/2 miss, 3s lockout), `rehearsal` (auto switch-back, triggers enabled).

---

## Three Operational Modes

### Single

One controller, one host. Simplest setup — plug in and go.

```toml
[host]
mode = "single"

[midi]
device = "auto"
```

### Redundant

Primary + backup controller with activity-based auto-switch. Full InputMux failover.

```toml
[host]
mode = "redundant"

[midi]
device = "auto"
secondary_device = "auto:Launchpad"
```

### Multi-Device

Up to 16 independent controllers on one host. Each gets its own highway — independent USB reader, ring buffer, and broadcaster pipeline. Packets distinguished by `device_id`.

```toml
[host]
mode = "multi"

[[midi.devices]]
name = "APC40"
device = "auto:APC40"

[[midi.devices]]
name = "nanoKONTROL2"
device = "auto:nanoKONTROL2"

[[midi.devices]]
name = "Launchpad"
device = "auto:Launchpad"
```

### Host Redundancy (independent flag)

Primary/Standby host-level failover can be combined with **any** mode:

```toml
[host]
mode = "multi"
host_redundancy = true    # Enables P/S dual-host broadcasting
```

Change modes live via the admin dashboard or API:

```bash
# Via API
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -d '{"mode":"multi"}' http://host:8080/api/system/mode

# Via CLI
midinet-cli mode set multi
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
# Auto-discover hosts via mDNS, create virtual device(s)
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
midinet-cli mode                # Show current mode
midinet-cli mode set multi      # Change operational mode
midinet-cli metrics             # MIDI throughput stats
midinet-cli metrics --system    # CPU, memory, temperature
midinet-cli alerts              # Active alerts
```

---

## Raspberry Pi Deployment

### One-Command Setup

SSH into your Pi and run:

```bash
curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/pi-provision.sh | sudo bash
```

This will:
1. Install system dependencies (ALSA, build tools, Avahi)
2. Install the Rust toolchain
3. Tune the system for real-time performance (CPU governor, network buffers, RT scheduling)
4. Clone the repo to `/opt/midinet/src`
5. Build all binaries in release mode
6. Install binaries to `/usr/local/bin/`
7. Install and start systemd services
8. Install the `midinet-update` command

### Network Setup

Interactive setup for hostname, static fallback IP, and mDNS:

```bash
sudo bash scripts/pi-network-setup.sh
```

Configures:
- Custom hostname (e.g., `midinet.local`)
- DHCP with static fallback IP
- Avahi mDNS for zero-config discovery
- Bitfocus Companion alias detection (if installed)

After setup, the admin panel is reachable at `http://midinet.local:8080`.

### Running Alongside Bitfocus Companion

MIDInet is designed to coexist with [Bitfocus Companion](https://bitfocus.io/companion) on the same Raspberry Pi. One device handles both MIDI distribution and StreamDeck button control for your entire show.

```
┌─────────────────────────────────────────────┐
│              Raspberry Pi 5                  │
│                                              │
│  MIDInet Host ─── MIDI to all clients        │
│  midinet.local:8080 (admin dashboard)        │
│                                              │
│  Companion ───── StreamDeck / button pages   │
│  companion.local:8000                        │
│                                              │
│  Both reachable via .local mDNS              │
└─────────────────────────────────────────────┘
```

The `pi-network-setup.sh` script auto-detects Companion and publishes a `companion.local` Avahi alias so both services are reachable at their own `.local` hostnames without changing the system hostname.

Companion can also trigger MIDInet failover, focus switching, and mode changes via the REST API — wire a StreamDeck button to `POST /api/failover/switch` for instant manual failover from your control surface.

### Updating

```bash
sudo midinet-update
```

Or trigger from the web dashboard or system tray. Real-time progress streaming via WebSocket.

### Remote Management (from your Mac)

```bash
make provision PI_HOST=pi@192.168.1.50     # First-time provision
make update PI_HOST=pi@192.168.1.50        # Remote update
make logs PI_HOST=pi@192.168.1.50          # View live logs
make status PI_HOST=pi@192.168.1.50        # Check service status
```

### File Locations on Pi

| Path | Contents |
|------|----------|
| `/usr/local/bin/midi-*` | Binaries |
| `/etc/midinet/midinet.toml` | Configuration (edit this) |
| `/var/lib/midinet/` | Runtime data (metrics DB, update logs) |
| `/opt/midinet/src/` | Git clone (for rebuilds) |
| `/etc/midinet/avahi-aliases.conf` | Additional .local hostnames |

### Systemd Services

```bash
sudo systemctl status midinet-host          # Host daemon
sudo systemctl status midinet-admin         # Admin panel
sudo systemctl status midinet-avahi-alias   # Avahi alias publisher
sudo systemctl restart midinet-host         # Restart after config change
journalctl -u midinet-host -f              # Live host logs
```

The host daemon runs with `SCHED_FIFO` priority 80 and locked memory for real-time MIDI processing.

---

## Client Installation

One-command installers for each platform. Each script clones the repo, builds natively, installs as a background service, and sets up auto-start.

### macOS

```bash
curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/client-install-macos.sh | bash
```

Installs:
- `midinet-client` and `midinet-cli` to `/usr/local/bin/`
- `midinet-tray` — menu bar app with health icon and focus control
- LaunchAgent for auto-start at login
- Config at `~/.midinet/config/client.toml`

### Windows

Run in PowerShell (as Administrator):

```powershell
powershell -NoExit -Command "irm https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/client-install-windows.ps1 | iex"
```

**MIDI virtual device support:**
- **Windows 11** — Works out of the box via Windows MIDI Services (auto-installed)
- **Windows 10** — Requires [teVirtualMIDI driver](https://www.tobias-erichsen.de/software/virtualmidi.html) (auto-detected)
- On Windows 11 with teVirtualMIDI installed, the driver is used as primary with MIDI Services as fallback

Installs:
- `midinet-client.exe` — background daemon (Scheduled Task, auto-restarts on failure)
- `midinet-tray.exe` — system tray icon (auto-starts at logon)
- `midinet-cli.exe` — command-line tool
- All binaries to `%LOCALAPPDATA%\MIDInet\bin\` (added to PATH)
- Config at `%LOCALAPPDATA%\MIDInet\config\client.toml`

The script is **update-safe** — re-running stops processes, replaces binaries, and restarts cleanly.

### Linux

```bash
curl -sSL https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/client-install-linux.sh | bash
```

Installs:
- `midinet-client` and `midinet-cli` to `~/.midinet/bin/`
- `midinet-tray` with XDG autostart desktop entry
- Systemd user service for auto-start
- Config at `~/.midinet/config/client.toml`

### Client Configuration

Client config is **optional** — mDNS discovery handles everything automatically.

---

## System Tray App

Native system tray application on macOS (menu bar), Windows (notification area), and Linux (AppIndicator).

### Color-Coded Health Icon

| Color | Meaning |
|-------|---------|
| 🟢 Green (blinking) | Connected, healthy |
| 🟡 Yellow | Degraded / warning |
| 🔴 Red | Error / disconnected |
| ⚪ Gray | Starting / not connected |

### Menu Items

- **Status** — Connected to Primary/Standby, mode, active controller names
- **Metrics** — Messages/s in and out, packet loss percentage
- **Focus** — Claim/Release focus (context-sensitive)
- **Open Admin Dashboard** — Opens browser to host's web UI
- **Version Mismatch Warning** — Directional guidance: "Host outdated — update via dashboard" or "Client outdated — check for updates"
- **Check for Updates** — Background check with native dialog showing changelog
- **Restart Client** — Restarts the background daemon
- **Start at Login** — Toggle auto-start
- **Quit MIDInet** — With confirmation; blocked when protected apps are running

### Tooltip

Shows at a glance: `MIDInet v3.1 (abc1234) | Redundant | Primary | APC40 | 142 in 0 out msg/s | 0.0% loss`

### Desktop Notifications

- Failover occurred
- All hosts unreachable
- Reconnected after outage
- 5-second cooldown prevents notification spam during flapping

---

## Web Dashboard

The admin panel serves an embedded single-page dashboard at `http://<host>:8080`.

**Features:**
- Live system overview with health score
- **Cockpit-style mode selector** (Single / Redundant / Multi-Device) with safety checks
- **Interactive device highway editor** for multi-device mode
- **Protected apps management** with 27-app catalog
- Host and client status with connection metrics
- MIDI activity monitor with real-time sparkline and traffic filter
- MIDI sniffer with feedback channel visibility
- Failover controls with confirmation modal
- System metrics (CPU, memory, temperature, disk)
- Alert configuration with webhook support
- Pipeline configuration (channel filter, CC remap, velocity curves)
- **Self-update** with real-time progress streaming
- **Version mismatch detection** across all components
- Full REST API for automation

### API Authentication

```bash
MIDINET_API_TOKEN=your-secret-token midi-admin --listen 0.0.0.0:8080
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

POST /api/system/mode                   Change operational mode
GET  /api/system/host-redundancy        Host redundancy status
POST /api/system/host-redundancy        Toggle host redundancy

GET  /api/settings/device-highways      Current device highway config
PUT  /api/settings/device-highways      Update device highways (max 16)

GET  /api/settings/protected-apps       Protected app catalog + enabled list
PUT  /api/settings/protected-apps       Update enabled protected apps

GET  /api/system/update-check           Check for available updates
POST /api/system/update                 Trigger self-update (with safety checks)
GET  /api/system/update-status          Stream update log

GET  /api/config              Full MIDInet config
PUT  /api/config              Update config

WS   /ws/status               Real-time status (1s push)
WS   /ws/midi                 Real-time MIDI stream
WS   /ws/alerts               Real-time alert notifications
WS   /ws/update               Real-time update log stream
```

---

## Configuration

### Host (`/etc/midinet/midinet.toml`)

```toml
[host]
id = 1                              # Lower = higher priority for primary
name = "host-a"
mode = "single"                     # "single" | "redundant" | "multi"
host_redundancy = false             # Enable dual-host P/S broadcasting

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

# Multi-device mode: replace [midi] device with [[midi.devices]]
# [[midi.devices]]
# name = "APC40"
# device = "auto:APC40"
# [[midi.devices]]
# name = "nanoKONTROL2"
# device = "auto:nanoKONTROL2"

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
listen_port = 5588
address = "/midinet/failover/switch"
allowed_sources = ["192.168.1.0/24"]

[safety]
protected_apps = ["resolume-arena"]
# custom_processes = ["MyApp.exe"]
```

### Client (`config/client.toml`)

Client config is **optional** — mDNS discovery handles everything automatically:

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

## Dual-Host Setup (Redundancy)

For production redundancy, run two Raspberry Pis with identical MIDI controllers:

**Host A** (`/etc/midinet/midinet.toml`):
```toml
[host]
id = 1                          # Primary (lower ID wins)
name = "host-a"
host_redundancy = true

[network]
multicast_group = "239.69.83.1"
```

**Host B** (`/etc/midinet/midinet.toml`):
```toml
[host]
id = 2                          # Standby
name = "host-b"
host_redundancy = true

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
| **OSC** | Send `/midinet/failover/switch` to port 5588 |
| **Tray** | Right-click → context menu (when available) |

Safety measures prevent accidental triggers:
- **Lockout period** — blocks rapid switching (default: 5s)
- **Confirmation mode** — optional double-trigger requirement
- **Guard note** — hold a safety button while pressing switch
- **Standby health gate** — refuses to switch if standby is unhealthy
- **Protected apps gate** — blocks when show-critical apps are running

---

## Protected Apps

Blocks mode changes, highway reconfiguration, and client restarts when show-critical MIDI applications are running on connected clients.

**27-app catalog across 4 categories:**

| Category | Applications |
|----------|-------------|
| **VJ / Media Server** | Resolume Arena, Resolume Avenue, TouchDesigner, MadMapper, VDMX, Millumin, Disguise (d3), Notch |
| **DAW** | Ableton Live, Logic Pro, Cubase, FL Studio, Reaper, Bitwig Studio, Pro Tools |
| **Lighting** | grandMA3, grandMA2, Hog 4 PC, ChamSys MagicQ, ETC Eos, Capture, ONYX, Chroma-Q Vista, Lightkey, QLC+ |
| **Playback** | QLab, PlayBack Pro |

Configure via dashboard or API:

```bash
curl -X PUT -H "Authorization: Bearer $TOKEN" \
  -d '{"enabled":["resolume-arena","ableton-live"],"custom_processes":["MyApp.exe"]}' \
  http://host:8080/api/settings/protected-apps
```

---

## Load Testing

The `midi-loadtest` binary tests the real UDP multicast path without interfering with live traffic:

```bash
midi-loadtest latency              # Send→receive loopback (min, mean, max, p50, p95, p99, p99.9)
midi-loadtest throughput           # Saturate link, measure max msg/s and MB/s
midi-loadtest burst                # Realistic MIDI burst patterns (drum rolls, chords)
midi-loadtest heartbeat            # Verify 3ms heartbeat timing accuracy
midi-loadtest failover             # Simulate primary failure, measure failover time
midi-loadtest soak --duration 3600 # Long-duration: packet loss, jitter, memory stability
midi-loadtest pipeline             # Benchmark pipeline processing throughput
midi-loadtest journal              # Benchmark journal encode/decode + state reconciliation
midi-loadtest all                  # Run all tests with combined report
```

---

## Development

```bash
cargo build                         # Build all crates
cargo test --workspace              # Run tests
RUST_LOG=debug cargo run -p midi-host -- --config config/host.toml
cargo check --workspace
```

### Cross-Compile for Pi (from macOS/Linux)

```bash
rustup target add aarch64-unknown-linux-gnu
make build-pi
make deploy PI_HOST=pi@192.168.1.50
```

---

## Project Structure

```
MIDInet/
├── Cargo.toml                    # Workspace root
├── Makefile                      # Build & deploy targets
├── crates/
│   ├── midi-protocol/            # Shared types & serialization (protocol v2)
│   │   └── src/
│   │       ├── packets.rs        # v2 packets with device_id + device_mask
│   │       ├── midi_state.rs     # 16-channel MIDI state model
│   │       ├── journal.rs        # Compact state journal for failover
│   │       ├── pipeline.rs       # Filter, remap, transpose, velocity curves
│   │       ├── identity.rs       # Device identity cloning
│   │       ├── modes.rs          # OperationalMode enum (Single/Redundant/Multi)
│   │       └── ringbuf.rs        # Lock-free ring buffer
│   │
│   ├── midi-host/                # Host daemon (Raspberry Pi)
│   │   └── src/
│   │       ├── main.rs           # Orchestration & task spawning
│   │       ├── usb_reader.rs     # ALSA MIDI input
│   │       ├── broadcaster.rs    # UDP multicast sender + heartbeat
│   │       ├── multi_device.rs   # Multi-device highway manager (up to 16)
│   │       ├── discovery.rs      # mDNS service advertisement
│   │       ├── failover.rs       # Primary/standby role management
│   │       ├── feedback.rs       # Bidirectional: network → controller
│   │       ├── osc_listener.rs   # OSC command handler (port 5588)
│   │       └── metrics.rs        # Internal metrics collection
│   │
│   ├── midi-client/              # Client daemon
│   │   └── src/
│   │       ├── main.rs           # Discovery → receive → virtual device(s)
│   │       ├── receiver.rs       # Multicast receiver + pipeline
│   │       ├── discovery.rs      # mDNS browser + mode-aware auto-connect
│   │       ├── failover.rs       # Dual-stream heartbeat monitor
│   │       ├── focus.rs          # Bidirectional focus + feedback
│   │       ├── virtual_device.rs # Platform abstraction trait
│   │       └── platform/
│   │           ├── macos.rs          # CoreMIDI virtual ports
│   │           ├── linux.rs          # ALSA sequencer virtual ports
│   │           ├── windows.rs        # Windows orchestrator (auto-selects backend)
│   │           ├── te_virtual_midi.rs # teVirtualMIDI FFI (Win10/11)
│   │           └── midi_services.rs  # Windows MIDI Services (Win11)
│   │
│   ├── midi-admin/               # Web admin panel
│   │   └── src/
│   │       ├── main.rs           # Axum server + config loading
│   │       ├── api/              # REST endpoints (mode, highways, protected apps, update)
│   │       ├── websocket.rs      # WebSocket hub (status, MIDI, alerts, update log)
│   │       ├── auth.rs           # Bearer token middleware
│   │       ├── collector.rs      # System metrics sampler (1Hz)
│   │       ├── alerting.rs       # Threshold alerts + webhooks
│   │       └── static/           # Embedded dashboard (HTML/CSS/JS)
│   │
│   ├── midi-tray/                # Native system tray app
│   │   └── src/
│   │       ├── main.rs           # Platform-specific event loops
│   │       ├── health.rs         # Health polling + color-coded icon
│   │       ├── updater.rs        # Self-update via git ls-remote + installer
│   │       └── platform/         # macOS (NSApp), Windows (Win32), Linux (GTK)
│   │
│   ├── midi-cli/                 # Management CLI
│   │   └── src/main.rs           # HTTP client → admin API
│   │
│   └── midi-loadtest/            # QA test suite
│       └── src/main.rs           # Latency, throughput, burst, failover, soak tests
│
├── discord-bot/                  # Discord community bot (Python)
│   ├── bot.py                    # Main bot + server setup
│   └── cogs/                     # Welcome, roles, moderation
│
├── config/
│   ├── host.toml                 # Host configuration template
│   └── client.toml               # Client configuration template
│
├── scripts/
│   ├── pi-provision.sh           # Full Pi provisioning
│   ├── pi-update.sh              # Pull + rebuild + restart
│   ├── pi-network-setup.sh       # Interactive network + hostname setup
│   ├── midinet-avahi-alias.sh    # Avahi alias publisher service
│   ├── client-install-macos.sh   # macOS client + tray installer
│   ├── client-install-linux.sh   # Linux client + tray installer
│   ├── client-install-windows.ps1 # Windows client + tray installer
│   └── setup-pi.sh               # System tuning (RT kernel, sysctl)
│
└── deploy/
    ├── midinet-host.service      # Systemd unit (RT priority)
    ├── midinet-admin.service     # Systemd unit (admin panel)
    └── midinet-avahi-alias.service # Systemd unit (alias publisher)
```

---

## License

MIT

---

*Built by [Hakol Fine AV Services](https://hakol.co.il) for production live events.*
*Visit [midinet.io](https://midinet.io) for documentation, case studies, and community.*
