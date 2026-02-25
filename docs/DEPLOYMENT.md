# MIDInet Deployment Guide

## Overview

MIDInet has two types of deployments:

| Component | Runs on | Purpose |
|-----------|---------|---------|
| **Host** (`midinet-host`) | Raspberry Pi (or any Linux box with USB MIDI) | Reads physical MIDI controller, broadcasts over network |
| **Client** (`midinet-client` + `midinet-tray`) | macOS, Windows, Linux workstations | Receives MIDI, creates virtual device for DAW/media server |

For redundancy, deploy **two hosts** (primary + standby) with the same MIDI controller model. Clients auto-discover both and failover in ~10ms.

---

## Quick Deploy Commands

All commands assume you've cloned the repo and are in the repo root.

### Host (Raspberry Pi)

```bash
bash scripts/deploy-host.sh
```

### Client (macOS / Linux)

```bash
bash scripts/deploy.sh
```

### Client (Windows — PowerShell as Administrator)

```powershell
.\scripts\deploy.ps1
```

---

## Host Deployment (Raspberry Pi)

### Prerequisites

- Raspberry Pi 4 or 5 (Pi 5 recommended for faster builds)
- Raspbian / Raspberry Pi OS (64-bit recommended)
- Ethernet connection to the same LAN as clients
- USB MIDI controller plugged in

### Step 1: Prepare the Pi

If running alongside Bitfocus Companion, set up Companion first, then SSH in.

```bash
# Install system dependencies
sudo apt-get update
sudo apt-get install -y libasound2-dev build-essential pkg-config git curl

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env
```

### Step 2: Clone and Deploy

```bash
git clone https://github.com/Hakolsound/MIDInet.git
cd MIDInet
bash scripts/deploy-host.sh
```

The script builds `midinet-host`, `midinet-cli`, and `midinet-admin`, then registers a systemd service that auto-starts on boot.

### Step 3: Configure

Edit `~/.midinet/config/host.toml`:

```bash
nano ~/.midinet/config/host.toml
```

Key settings to verify:

| Setting | Primary Host | Standby Host |
|---------|-------------|--------------|
| `host.id` | `1` | `2` |
| `host.name` | `"host-a"` | `"host-b"` |
| `network.multicast_group` | `"239.69.83.1"` | `"239.69.83.2"` |
| `network.interface` | `"eth0"` | `"eth0"` |
| `admin.password` | Change from default! | Change from default! |
| `midi.device` | `"auto"` or specific ALSA device | `"auto"` or specific |

After editing, restart:

```bash
systemctl --user restart midinet-host
```

### Step 4: Verify

```bash
# Check service status
systemctl --user status midinet-host

# Watch live logs
journalctl --user -u midinet-host -f

# Open admin panel (from any browser on the LAN)
# http://<pi-ip>:8080
```

### Host Management Commands

```bash
systemctl --user stop midinet-host       # Stop
systemctl --user start midinet-host      # Start
systemctl --user restart midinet-host    # Restart
systemctl --user status midinet-host     # Status
journalctl --user -u midinet-host -f     # Live logs
midinet-cli status                       # Connection status
```

---

## Client Deployment (macOS)

### Prerequisites

- macOS 12+ (Monterey or later)
- Xcode Command Line Tools (`xcode-select --install`)

### Deploy

```bash
git clone https://github.com/Hakolsound/MIDInet.git
cd MIDInet
bash scripts/deploy.sh
```

This installs:
- `midinet-client` — background daemon (LaunchAgent, auto-starts on login, auto-restarts on failure)
- `midinet-tray` — menu bar icon (LaunchAgent, auto-starts on login)
- `midinet-cli` — command-line tool

### What You'll See

- A **colored circle** in the macOS menu bar:
  - **Gray** — daemon starting / not connected
  - **Green** — connected, healthy
  - **Yellow** — connected with warnings (packet loss, single host)
  - **Red** — disconnected / both hosts unreachable
- Right-click the icon for live metrics and actions
- Desktop notifications on failover events

### Verify

```bash
# Health endpoint (JSON)
curl http://127.0.0.1:5009/health

# CLI status
midinet-cli status

# Check the virtual MIDI device
# Open Audio MIDI Setup → Window → Show MIDI Studio
```

### Client Management Commands (macOS)

```bash
# Daemon
launchctl unload ~/Library/LaunchAgents/co.hakol.midinet-client.plist   # Stop
launchctl load   ~/Library/LaunchAgents/co.hakol.midinet-client.plist   # Start

# Tray
launchctl unload ~/Library/LaunchAgents/co.hakol.midinet-tray.plist     # Stop
launchctl load   ~/Library/LaunchAgents/co.hakol.midinet-tray.plist     # Start

# Logs
cat ~/.midinet/midinet-client.log
cat ~/.midinet/midinet-tray.log
```

---

## Client Deployment (Windows)

### Prerequisites

- Windows 10 or Windows 11
- [Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) — select "Desktop development with C++"
- Git and Rust are auto-installed by the script if missing

**MIDI virtual device backend (auto-selected):**
- **Windows 11** — No third-party driver needed. The installer auto-installs Windows MIDI Services SDK, which provides native virtual MIDI support.
- **Windows 10** — Requires the [teVirtualMIDI driver](https://www.tobias-erichsen.de/software/virtualmidi.html). The installer prompts if it's missing.
- If both teVirtualMIDI and Windows MIDI Services are available, teVirtualMIDI is used as the primary backend with automatic fallback.

### Deploy (One-Liner)

Open **PowerShell** (Administrator recommended):

```powershell
powershell -NoExit -Command "irm https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/client-install-windows.ps1 | iex"
```

Or clone first:

```powershell
git clone https://github.com/Hakolsound/MIDInet.git
cd MIDInet
.\scripts\client-install-windows.ps1
```

### What Gets Installed

| Component | Location | Auto-Start |
|-----------|----------|------------|
| `midinet-client.exe` | `%LOCALAPPDATA%\MIDInet\bin\` | Scheduled Task (at logon, auto-restarts) |
| `midinet-tray.exe` | `%LOCALAPPDATA%\MIDInet\bin\` | Registry Run key (at logon) |
| `midinet-cli.exe` | `%LOCALAPPDATA%\MIDInet\bin\` | Manual (added to PATH) |
| Config | `%LOCALAPPDATA%\MIDInet\config\client.toml` | — |
| Logs | `%LOCALAPPDATA%\MIDInet\log\` | — |

### Updating

Re-run the same install script. It is **update-safe**:
1. Stops all running MIDInet processes (daemon + tray)
2. Pulls latest source and rebuilds
3. Replaces binaries (with retry logic for file locks)
4. Re-registers Scheduled Task and restarts all services
5. Ensures exactly one tray instance is running

```powershell
cd $env:LOCALAPPDATA\MIDInet\src
.\scripts\client-install-windows.ps1
```

### What You'll See

- A **colored circle** in the Windows system tray:
  - **Gray** — daemon starting / not connected to daemon
  - **Green (blinking)** — connected, healthy, MIDI flowing
  - **Yellow** — connected with warnings (packet loss, unhealthy task)
  - **Red** — disconnected / both hosts unreachable
- Right-click the icon for live metrics and actions (claim/release focus, open dashboard)
- Desktop notifications on failover events

### Task Supervisor

The client daemon includes an internal watchdog that monitors its core tasks (discovery, receiver, failover, focus). If any task crashes:
- It is automatically restarted with exponential backoff (2s, 4s, ... up to 30s)
- The virtual MIDI device stays open (it lives at process level, unaffected by task restarts)
- The tray icon turns yellow to indicate an unhealthy task, then green once recovered

If the entire process crashes, the Windows Scheduled Task automatically restarts it (3 retries, 1 minute interval). The virtual MIDI device is recreated with the same name, so Resolume / your DAW will reconnect automatically — similar to unplugging and replugging a USB MIDI controller.

### Client Management Commands (Windows)

```powershell
# Daemon
Stop-ScheduledTask  -TaskName 'MIDInet Client'    # Stop
Start-ScheduledTask -TaskName 'MIDInet Client'    # Start

# Check status
midinet-cli status

# Health endpoint
Invoke-WebRequest http://127.0.0.1:5009/health

# Check installed version
Get-Content $env:LOCALAPPDATA\MIDInet\bin\version.txt
```

---

## Client Deployment (Linux)

### Prerequisites

- Desktop Linux with system tray support (GNOME + AppIndicator, KDE, XFCE, etc.)
- ALSA development headers:
  ```bash
  # Debian/Ubuntu
  sudo apt-get install -y libasound2-dev build-essential pkg-config git

  # Fedora
  sudo dnf install -y alsa-lib-devel gcc git pkg-config

  # Arch
  sudo pacman -S --needed alsa-lib base-devel git pkg-config
  ```

### Deploy

```bash
git clone https://github.com/Hakolsound/MIDInet.git
cd MIDInet
bash scripts/deploy.sh
```

This installs:
- `midinet-client` — background daemon (systemd user service, auto-starts, auto-restarts)
- `midinet-tray` — system tray icon (XDG autostart, launches at login)
- `midinet-cli` — command-line tool

### Client Management Commands (Linux)

```bash
systemctl --user stop  midinet-client      # Stop
systemctl --user start midinet-client      # Start
systemctl --user status midinet-client     # Status
journalctl --user -u midinet-client -f     # Live logs
midinet-cli status                         # Connection status
```

---

## Configuration

### Client Config

Location: `~/.midinet/config/client.toml` (macOS/Linux) or `%LOCALAPPDATA%\MIDInet\config\client.toml` (Windows)

The client works with **zero configuration** via mDNS discovery. The config file is optional and only needed to override defaults.

### Host Config

Location: `~/.midinet/config/host.toml`

**Must be edited** for each host. See the template at `config/host.toml` in the repo.

---

## Network Requirements

| Port | Protocol | Direction | Purpose |
|------|----------|-----------|---------|
| 5004 | UDP multicast | Host → Client | MIDI data stream |
| 5005 | UDP multicast | Host → Client | Heartbeat packets (3ms interval) |
| 5006 | UDP multicast | Bidirectional | Control: identity, focus management |
| 5009 | TCP localhost | Client internal | Health WebSocket (tray ↔ daemon) |
| 5353 | UDP multicast | Bidirectional | mDNS discovery |
| 8080 | TCP | Host → Browser | Admin panel (configurable) |

Multicast groups: `239.69.83.1` (primary), `239.69.83.2` (standby), `239.69.83.100` (control).

Ensure multicast is enabled on your network switches. Most managed switches support IGMP snooping — enable it for efficient multicast routing.

---

## Updating

To update after pulling new code, just re-run the deploy script:

```bash
git pull
bash scripts/deploy.sh        # Client (macOS/Linux)
bash scripts/deploy-host.sh   # Host (Pi)
```

```powershell
git pull
.\scripts\deploy.ps1          # Client (Windows)
```

The scripts stop services, rebuild, reinstall, and restart automatically.

---

## Troubleshooting

### Tray icon not visible (macOS)

The tray icon requires macOS to process its native event loop. If you see the process running but no icon, check:
```bash
cat ~/.midinet/midinet-tray.err
```

### No MIDI device found (Host)

```bash
# List ALSA MIDI devices
arecordmidi -l

# Check if the controller is detected
lsusb | grep -i midi
```

Set `midi.device` in host.toml to the specific ALSA device (e.g., `"hw:1,0,0"`) or use `"auto:ControllerName"` to auto-detect by name.

### Client can't discover hosts

- Verify hosts and clients are on the same subnet
- Check that multicast is not blocked by firewall: `sudo ufw allow proto udp to 239.69.83.0/24`
- On macOS, check that the correct network interface is active

### Health endpoint not responding

```bash
curl http://127.0.0.1:5009/health
```

If this fails, the daemon may not be running:
```bash
# macOS
launchctl list | grep midinet

# Linux
systemctl --user status midinet-client

# Windows
Get-ScheduledTask -TaskName 'MIDInet Client'
```
