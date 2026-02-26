# ──────────────────────────────────────────────────────────────
# MIDInet - Windows Client Uninstaller (PowerShell)
# Cleanly removes MIDInet client, tray, firewall rules,
# startup entries, and optionally the installation directory.
#
# Usage:
#   .\scripts\client-uninstall-windows.ps1
#   .\scripts\client-uninstall-windows.ps1 -KeepConfig
#
# Flags:
#   -KeepConfig   Keep config files (only remove binaries/startup)
# ──────────────────────────────────────────────────────────────

param(
    [switch]$KeepConfig
)

$InstallDir = "$env:LOCALAPPDATA\MIDInet"
$BinDir     = "$InstallDir\bin"
$ConfigDir  = "$InstallDir\config"
$TaskName   = "MIDInet Client"
$TrayRegName = "MIDInet Tray"

$IsAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# ── Helper Functions ─────────────────────────────────────────

function Write-Step($num, $total, $msg) {
    Write-Host "`n[$num/$total] $msg" -ForegroundColor Cyan
}
function Write-Ok($msg) {
    Write-Host "    [OK] $msg" -ForegroundColor Green
}
function Write-Warn($msg) {
    Write-Host "    [!] $msg" -ForegroundColor Yellow
}

$TotalSteps = 6
$Removed = @()

# ── Banner ────────────────────────────────────────────────────

Write-Host ""
Write-Host "  ========================================" -ForegroundColor Red
Write-Host "    MIDInet - Windows Client Uninstaller" -ForegroundColor Red
Write-Host "  ========================================" -ForegroundColor Red
Write-Host ""

if (Test-Path "$BinDir\version.txt") {
    $version = Get-Content "$BinDir\version.txt" -ErrorAction SilentlyContinue
    Write-Host "  Installed version: $version" -ForegroundColor DarkGray
} else {
    Write-Host "  No MIDInet installation found at $InstallDir" -ForegroundColor Yellow
    Write-Host "  Continuing anyway to clean up any remnants..." -ForegroundColor DarkGray
}
Write-Host ""

# ── 1. Stop Running Processes ────────────────────────────────
Write-Step 1 $TotalSteps "Stopping MIDInet processes..."

# Try graceful shutdown via health API first
try {
    $tcp = New-Object System.Net.Sockets.TcpClient
    $tcp.Connect("127.0.0.1", 5009)
    $stream = $tcp.GetStream()
    $request = "POST /shutdown HTTP/1.1`r`nHost: 127.0.0.1`r`nContent-Length: 0`r`nConnection: close`r`n`r`n"
    $bytes = [System.Text.Encoding]::ASCII.GetBytes($request)
    $stream.Write($bytes, 0, $bytes.Length)
    $tcp.Close()
    Write-Ok "Sent graceful shutdown to client"
    Start-Sleep -Seconds 2
} catch {
    # Health port not responding — client may not be running
}

# Force-kill any remaining processes
$procs = @("midinet-client", "midinet-tray", "midi-client", "midi-tray")
$killed = $false
foreach ($name in $procs) {
    $running = Get-Process -Name $name -ErrorAction SilentlyContinue
    if ($running) {
        $running | Stop-Process -Force -ErrorAction SilentlyContinue
        Write-Ok "Stopped $name (PID: $($running.Id -join ', '))"
        $killed = $true
    }
}
if (-not $killed) {
    Write-Ok "No running MIDInet processes found"
}

Start-Sleep -Milliseconds 500

# ── 2. Remove Scheduled Task (legacy) ────────────────────────
Write-Step 2 $TotalSteps "Removing scheduled task..."

$task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($task) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Ok "Removed scheduled task '$TaskName'"
    $Removed += "Scheduled task"
} else {
    Write-Ok "No scheduled task found"
}

# ── 3. Remove Tray Auto-Start (Registry Run key) ─────────────
Write-Step 3 $TotalSteps "Removing startup entries..."

$regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"

# Tray auto-start
$trayReg = Get-ItemProperty -Path $regPath -Name $TrayRegName -ErrorAction SilentlyContinue
if ($trayReg) {
    Remove-ItemProperty -Path $regPath -Name $TrayRegName -ErrorAction SilentlyContinue
    Write-Ok "Removed tray auto-start ($TrayRegName)"
    $Removed += "Tray auto-start"
} else {
    Write-Ok "No tray auto-start entry found"
}

# Also check for a generic "MIDInet" key (legacy)
$genericReg = Get-ItemProperty -Path $regPath -Name "MIDInet" -ErrorAction SilentlyContinue
if ($genericReg) {
    Remove-ItemProperty -Path $regPath -Name "MIDInet" -ErrorAction SilentlyContinue
    Write-Ok "Removed legacy 'MIDInet' auto-start entry"
    $Removed += "Legacy auto-start"
}

# ── 4. Remove from PATH ──────────────────────────────────────
Write-Step 4 $TotalSteps "Cleaning up PATH..."

try {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -like "*$BinDir*") {
        $newPath = ($userPath -split ";" | Where-Object { $_ -ne $BinDir -and $_ -ne "" }) -join ";"
        [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
        Write-Ok "Removed $BinDir from user PATH"
        $Removed += "PATH entry"
    } else {
        Write-Ok "MIDInet not found on PATH"
    }
} catch {
    Write-Warn "Could not update PATH: $_"
}

# ── 5. Remove Windows Firewall Rules ─────────────────────────
Write-Step 5 $TotalSteps "Removing firewall rules..."

if ($IsAdmin) {
    $fwRules = Get-NetFirewallRule -DisplayName "MIDInet*" -ErrorAction SilentlyContinue
    if ($fwRules) {
        $fwRules | Remove-NetFirewallRule -ErrorAction SilentlyContinue
        Write-Ok "Removed $($fwRules.Count) MIDInet firewall rule(s)"
        $Removed += "Firewall rules"
    } else {
        Write-Ok "No MIDInet firewall rules found"
    }
} else {
    Write-Warn "Not running as Administrator - cannot remove firewall rules"
    Write-Warn "To remove manually: Get-NetFirewallRule -DisplayName 'MIDInet*' | Remove-NetFirewallRule"
}

# ── 6. Remove Installation Directory ─────────────────────────
Write-Step 6 $TotalSteps "Removing installation files..."

if (Test-Path $InstallDir) {
    if ($KeepConfig) {
        # Remove everything except config
        if (Test-Path $BinDir) {
            Remove-Item $BinDir -Recurse -Force -ErrorAction SilentlyContinue
            Write-Ok "Removed binaries ($BinDir)"
        }
        $logDir = "$InstallDir\log"
        if (Test-Path $logDir) {
            Remove-Item $logDir -Recurse -Force -ErrorAction SilentlyContinue
            Write-Ok "Removed logs"
        }
        $srcDir = "$InstallDir\src"
        if (Test-Path $srcDir) {
            Remove-Item $srcDir -Recurse -Force -ErrorAction SilentlyContinue
            Write-Ok "Removed source ($srcDir)"
        }
        Write-Warn "Config preserved at $ConfigDir"
        $Removed += "Binaries, logs, source"
    } else {
        Remove-Item $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
        Write-Ok "Removed $InstallDir"
        $Removed += "Installation directory"
    }
} else {
    Write-Ok "No installation directory found at $InstallDir"
}

# ── Summary ───────────────────────────────────────────────────
Write-Host ""
Write-Host "  =================================================" -ForegroundColor Green
Write-Host "    MIDInet has been uninstalled" -ForegroundColor Green
Write-Host "  =================================================" -ForegroundColor Green
Write-Host ""

if ($Removed.Count -gt 0) {
    Write-Host "  Removed:" -ForegroundColor DarkGray
    foreach ($item in $Removed) {
        Write-Host "    - $item" -ForegroundColor DarkGray
    }
} else {
    Write-Host "  Nothing to remove - MIDInet was not installed." -ForegroundColor DarkGray
}

if (-not $IsAdmin) {
    Write-Host ""
    Write-Host "  NOTE: Firewall rules were not removed (requires Administrator)." -ForegroundColor Yellow
    Write-Host "  Run as Admin to fully clean up, or remove manually:" -ForegroundColor Yellow
    Write-Host "    Get-NetFirewallRule -DisplayName 'MIDInet*' | Remove-NetFirewallRule" -ForegroundColor Yellow
}

Write-Host ""
