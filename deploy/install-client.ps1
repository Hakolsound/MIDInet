# MIDInet client installation / update script for Windows
#
# Run as Administrator:
#   powershell -ExecutionPolicy Bypass -File deploy\install-client.ps1
#
# Both first-time install and update are handled automatically.

param(
    [switch]$NoBuild,
    [string]$BinaryDir = "target\release"
)

$ErrorActionPreference = "Stop"

Write-Host "`n=== MIDInet Client Install/Update ===" -ForegroundColor Cyan

# ── Check admin ──
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "ERROR: Run this script as Administrator." -ForegroundColor Red
    Write-Host "  Right-click PowerShell -> Run as Administrator, then re-run this script." -ForegroundColor Yellow
    exit 1
}

$InstallDir = "C:\MIDInet"
$ServiceNames = @("MIDInetBridge", "MIDInetClient")

# ── Detect mode ──
$bridgeSvc = Get-Service -Name "MIDInetBridge" -ErrorAction SilentlyContinue
$clientSvc = Get-Service -Name "MIDInetClient" -ErrorAction SilentlyContinue
$firstInstall = ($null -eq $bridgeSvc) -or ($null -eq $clientSvc)

if ($firstInstall) {
    Write-Host "Mode: First-time install" -ForegroundColor Green
} else {
    Write-Host "Mode: Update" -ForegroundColor Yellow
}

# ── 1. Build ──
Write-Host "`n[1/4] Building..." -ForegroundColor White
if (-not $NoBuild) {
    cargo build --release -p midi-client -p midi-bridge
    if ($LASTEXITCODE -ne 0) {
        Write-Host "ERROR: Build failed." -ForegroundColor Red
        exit 1
    }
    $BinaryDir = "target\release"
} else {
    Write-Host "  Skipping build (-NoBuild), using binaries from $BinaryDir"
}

# Verify binaries exist
foreach ($bin in @("midi-client.exe", "midi-bridge.exe")) {
    if (-not (Test-Path "$BinaryDir\$bin")) {
        Write-Host "ERROR: $BinaryDir\$bin not found." -ForegroundColor Red
        exit 1
    }
}

# ── 2. Install directory + binaries ──
Write-Host "`n[2/4] Installing binaries..." -ForegroundColor White

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Write-Host "  Created $InstallDir"
}

# For updates: stop services before replacing binaries
if (-not $firstInstall) {
    Write-Host "  Stopping services for update..."
    Stop-Service -Name "MIDInetClient" -ErrorAction SilentlyContinue -Force
    # Bridge keeps running during binary swap to keep the device alive!
    # We stop it last and restart it first.
}

Copy-Item "$BinaryDir\midi-bridge.exe" "$InstallDir\midi-bridge.exe" -Force
Copy-Item "$BinaryDir\midi-client.exe" "$InstallDir\midi-client.exe" -Force
Write-Host "  Binaries copied to $InstallDir"

# Copy config if missing
if (-not (Test-Path "$InstallDir\client.toml")) {
    if (Test-Path "config\client.toml") {
        Copy-Item "config\client.toml" "$InstallDir\client.toml"
        Write-Host "  Default client config installed."
    }
}

# ── 3. Register Windows services ──
Write-Host "`n[3/4] Configuring services..." -ForegroundColor White

# Check for NSSM (preferred) or fall back to sc.exe
$nssm = Get-Command nssm -ErrorAction SilentlyContinue
if (-not $nssm) {
    # Try common install location
    $nssmPaths = @("C:\nssm\nssm.exe", "C:\tools\nssm\nssm.exe", "$env:ProgramFiles\nssm\nssm.exe")
    foreach ($p in $nssmPaths) {
        if (Test-Path $p) { $nssm = Get-Item $p; break }
    }
}

if ($nssm) {
    Write-Host "  Using NSSM for service management."
    $nssmExe = if ($nssm -is [System.Management.Automation.ApplicationInfo]) { $nssm.Source } else { $nssm.FullName }

    if ($firstInstall) {
        # Install bridge service
        & $nssmExe install MIDInetBridge "$InstallDir\midi-bridge.exe"
        & $nssmExe set MIDInetBridge DisplayName "MIDInet MIDI Bridge"
        & $nssmExe set MIDInetBridge Description "Owns the virtual MIDI device - survives client restarts"
        & $nssmExe set MIDInetBridge Start SERVICE_AUTO_START
        & $nssmExe set MIDInetBridge AppStdout "$InstallDir\bridge.log"
        & $nssmExe set MIDInetBridge AppStderr "$InstallDir\bridge.log"
        & $nssmExe set MIDInetBridge AppRotateFiles 1
        & $nssmExe set MIDInetBridge AppRotateBytes 1048576
        & $nssmExe set MIDInetBridge AppEnvironmentExtra "RUST_LOG=info"

        # Install client service (depends on bridge)
        & $nssmExe install MIDInetClient "$InstallDir\midi-client.exe" "--config $InstallDir\client.toml"
        & $nssmExe set MIDInetClient DisplayName "MIDInet Client"
        & $nssmExe set MIDInetClient Description "MIDI-over-network client daemon"
        & $nssmExe set MIDInetClient DependOnService MIDInetBridge
        & $nssmExe set MIDInetClient Start SERVICE_AUTO_START
        & $nssmExe set MIDInetClient AppStdout "$InstallDir\client.log"
        & $nssmExe set MIDInetClient AppStderr "$InstallDir\client.log"
        & $nssmExe set MIDInetClient AppRotateFiles 1
        & $nssmExe set MIDInetClient AppRotateBytes 1048576
        & $nssmExe set MIDInetClient AppEnvironmentExtra "RUST_LOG=info"

        Write-Host "  Services registered."
    } else {
        Write-Host "  Services already registered."
    }
} else {
    # Fallback: use sc.exe (basic, no log rotation)
    Write-Host "  NSSM not found - using sc.exe (install NSSM for better log management)."

    if ($firstInstall) {
        sc.exe create MIDInetBridge binPath= "`"$InstallDir\midi-bridge.exe`" --service" start= auto DisplayName= "MIDInet MIDI Bridge"
        sc.exe description MIDInetBridge "Owns the virtual MIDI device - survives client restarts"

        sc.exe create MIDInetClient binPath= "`"$InstallDir\midi-client.exe`" --service --config `"$InstallDir\client.toml`"" start= auto depend= MIDInetBridge DisplayName= "MIDInet Client"
        sc.exe description MIDInetClient "MIDI-over-network client daemon"

        Write-Host "  Services registered via sc.exe."
    } else {
        Write-Host "  Services already registered."
    }
}

# ── 4. Start / restart services ──
Write-Host "`n[4/4] Starting services..." -ForegroundColor White

if (-not $firstInstall) {
    # Update: restart bridge briefly, then start client
    Stop-Service -Name "MIDInetBridge" -ErrorAction SilentlyContinue -Force
    Start-Sleep -Seconds 1
}

Start-Service -Name "MIDInetBridge"
Write-Host "  Bridge started."
Start-Sleep -Seconds 1

Start-Service -Name "MIDInetClient"
Write-Host "  Client started."

Write-Host "`n=== Installation complete ===" -ForegroundColor Green

# Show status
Get-Service -Name $ServiceNames | Format-Table Name, Status, DisplayName -AutoSize

Write-Host "View logs:"
Write-Host "  Bridge: Get-Content $InstallDir\bridge.log -Tail 50 -Wait"
Write-Host "  Client: Get-Content $InstallDir\client.log -Tail 50 -Wait"
Write-Host ""
