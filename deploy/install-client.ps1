# MIDInet client installation / update script for Windows
#
# Run as Administrator:
#   powershell -ExecutionPolicy Bypass -File deploy\install-client.ps1
#
# Both first-time install and update are handled automatically.
#
# Uses Task Scheduler (not Windows services) to run in the user session.
# This is required because virtual MIDI devices must be created in the
# interactive session -- Session 0 services can't register MIDI In devices.

param(
    [switch]$NoBuild,
    [string]$BinaryDir = "target\release"
)

$ErrorActionPreference = "Stop"

Write-Host "`n=== MIDInet Client Install/Update ===" -ForegroundColor Cyan

# --Check admin --
$isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "ERROR: Run this script as Administrator." -ForegroundColor Red
    Write-Host "  Right-click PowerShell -> Run as Administrator, then re-run this script." -ForegroundColor Yellow
    exit 1
}

$InstallDir = "C:\MIDInet"
$TaskNames = @("MIDInetBridge", "MIDInetClient")

# Detect the current interactive user (for the logon trigger)
$CurrentUser = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name

# --Detect mode --
$bridgeTask = Get-ScheduledTask -TaskName "MIDInetBridge" -ErrorAction SilentlyContinue
$clientTask = Get-ScheduledTask -TaskName "MIDInetClient" -ErrorAction SilentlyContinue

# Check for legacy Windows services from previous versions
$bridgeSvc = Get-Service -Name "MIDInetBridge" -ErrorAction SilentlyContinue
$clientSvc = Get-Service -Name "MIDInetClient" -ErrorAction SilentlyContinue
$hasLegacyServices = ($null -ne $bridgeSvc) -or ($null -ne $clientSvc)

$firstInstall = ($null -eq $bridgeTask) -and ($null -eq $clientTask) -and (-not $hasLegacyServices)

if ($firstInstall) {
    Write-Host "Mode: First-time install" -ForegroundColor Green
} elseif ($hasLegacyServices) {
    Write-Host "Mode: Migration (services -> scheduled tasks)" -ForegroundColor Yellow
} else {
    Write-Host "Mode: Update" -ForegroundColor Yellow
}

# --1. Build --
Write-Host "`n[1/5] Building..." -ForegroundColor White
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

# --2. Remove legacy Windows services (if migrating) --
if ($hasLegacyServices) {
    Write-Host "`n[2/5] Removing legacy Windows services..." -ForegroundColor White
    if ($null -ne $clientSvc) {
        Stop-Service -Name "MIDInetClient" -ErrorAction SilentlyContinue -Force
        sc.exe delete MIDInetClient | Out-Null
        Write-Host "  Removed MIDInetClient service."
    }
    if ($null -ne $bridgeSvc) {
        Stop-Service -Name "MIDInetBridge" -ErrorAction SilentlyContinue -Force
        sc.exe delete MIDInetBridge | Out-Null
        Write-Host "  Removed MIDInetBridge service."
    }
    # Also try NSSM removal
    $nssm = Get-Command nssm -ErrorAction SilentlyContinue
    if (-not $nssm) {
        foreach ($p in @("C:\nssm\nssm.exe", "C:\tools\nssm\nssm.exe", "$env:ProgramFiles\nssm\nssm.exe")) {
            if (Test-Path $p) { $nssm = Get-Item $p; break }
        }
    }
    if ($nssm) {
        $nssmExe = if ($nssm -is [System.Management.Automation.ApplicationInfo]) { $nssm.Source } else { $nssm.FullName }
        & $nssmExe remove MIDInetClient confirm 2>$null
        & $nssmExe remove MIDInetBridge confirm 2>$null
    }
} else {
    Write-Host "`n[2/5] No legacy services to remove." -ForegroundColor DarkGray
}

# --3. Install directory + binaries --
Write-Host "`n[3/5] Installing binaries..." -ForegroundColor White

if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    Write-Host "  Created $InstallDir"
}

# For updates: stop running processes before replacing binaries
if (-not $firstInstall) {
    Write-Host "  Stopping running tasks for update..."
    Stop-ScheduledTask -TaskName "MIDInetClient" -ErrorAction SilentlyContinue
    Stop-ScheduledTask -TaskName "MIDInetBridge" -ErrorAction SilentlyContinue
    # Give processes a moment to exit gracefully
    Start-Sleep -Seconds 2
    # Force-kill if still running
    Get-Process -Name "midi-client" -ErrorAction SilentlyContinue | Stop-Process -Force
    Get-Process -Name "midi-bridge" -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 1
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

# --4. Register scheduled tasks --
Write-Host "`n[4/5] Configuring scheduled tasks..." -ForegroundColor White
Write-Host "  (Task Scheduler runs in user session -- required for virtual MIDI devices)" -ForegroundColor DarkGray

# Remove existing tasks if updating
foreach ($name in $TaskNames) {
    $existing = Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
    if ($null -ne $existing) {
        Unregister-ScheduledTask -TaskName $name -Confirm:$false
        Write-Host "  Removed existing task: $name"
    }
}

# Bridge task: runs at logon, auto-restarts on failure
$bridgeAction = New-ScheduledTaskAction `
    -Execute "$InstallDir\midi-bridge.exe" `
    -Argument "--log-file `"$InstallDir\bridge.log`"" `
    -WorkingDirectory $InstallDir

$bridgeTrigger = New-ScheduledTaskTrigger -AtLogon -User $CurrentUser

$bridgeSettings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -DontStopOnIdleEnd `
    -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -RestartCount 999

$bridgePrincipal = New-ScheduledTaskPrincipal `
    -UserId $CurrentUser `
    -LogonType Interactive `
    -RunLevel Highest

Register-ScheduledTask `
    -TaskName "MIDInetBridge" `
    -Action $bridgeAction `
    -Trigger $bridgeTrigger `
    -Settings $bridgeSettings `
    -Principal $bridgePrincipal `
    -Description "MIDInet MIDI Bridge - owns the virtual MIDI device (must run in user session)" `
    | Out-Null

Write-Host "  Registered MIDInetBridge task."

# Client task: runs at logon (3s after bridge), auto-restarts on failure
$clientAction = New-ScheduledTaskAction `
    -Execute "$InstallDir\midi-client.exe" `
    -Argument "--config `"$InstallDir\client.toml`" --log-file `"$InstallDir\client.log`"" `
    -WorkingDirectory $InstallDir

$clientTrigger = New-ScheduledTaskTrigger -AtLogon -User $CurrentUser
# Delay client start so bridge has time to create the named pipe
$clientTrigger.Delay = "PT3S"

$clientSettings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -DontStopOnIdleEnd `
    -ExecutionTimeLimit (New-TimeSpan -Seconds 0) `
    -RestartInterval (New-TimeSpan -Minutes 1) `
    -RestartCount 999

$clientPrincipal = New-ScheduledTaskPrincipal `
    -UserId $CurrentUser `
    -LogonType Interactive `
    -RunLevel Highest

Register-ScheduledTask `
    -TaskName "MIDInetClient" `
    -Action $clientAction `
    -Trigger $clientTrigger `
    -Settings $clientSettings `
    -Principal $clientPrincipal `
    -Description "MIDInet Client - MIDI-over-network client daemon" `
    | Out-Null

Write-Host "  Registered MIDInetClient task."

# --5. Start tasks now --
Write-Host "`n[5/5] Starting tasks..." -ForegroundColor White

Start-ScheduledTask -TaskName "MIDInetBridge"
Write-Host "  Bridge started."
Start-Sleep -Seconds 2

Start-ScheduledTask -TaskName "MIDInetClient"
Write-Host "  Client started."

Write-Host "`n=== Installation complete ===" -ForegroundColor Green

# Show status
Get-ScheduledTask -TaskName $TaskNames | Format-Table TaskName, State, Description -AutoSize

Write-Host "Manage:"
Write-Host "  Stop:    Stop-ScheduledTask -TaskName MIDInetClient; Stop-ScheduledTask -TaskName MIDInetBridge"
Write-Host "  Start:   Start-ScheduledTask -TaskName MIDInetBridge; Start-ScheduledTask -TaskName MIDInetClient"
Write-Host "  Status:  Get-ScheduledTask -TaskName MIDInetBridge,MIDInetClient | ft TaskName,State"
Write-Host ""
Write-Host "View logs:"
Write-Host "  Bridge: Get-Content $InstallDir\bridge.log -Tail 50 -Wait"
Write-Host "  Client: Get-Content $InstallDir\client.log -Tail 50 -Wait"
Write-Host ""
