# ──────────────────────────────────────────────────────────────
# MIDInet — Local Deploy (Windows)
# Run from the repo root in PowerShell (Administrator recommended).
#
# Usage:
#   .\scripts\deploy.ps1
# ──────────────────────────────────────────────────────────────
$ErrorActionPreference = "Stop"

$RepoDir = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$InstallDir = "$env:LOCALAPPDATA\MIDInet"
$BinDir = "$InstallDir\bin"
$ConfigDir = "$InstallDir\config"
$TaskName = "MIDInet Client"
$TrayRegName = "MIDInet Tray"

function Write-Step($num, $total, $msg) {
    Write-Host "`n[$num/$total] $msg" -ForegroundColor Cyan
}
function Write-Ok($msg) {
    Write-Host "    [OK] $msg" -ForegroundColor Green
}
function Write-Warn($msg) {
    Write-Host "    [!] $msg" -ForegroundColor Yellow
}
function Write-Fail($msg) {
    Write-Host "    [X] $msg" -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host "    MIDInet - Windows Deploy" -ForegroundColor Cyan
Write-Host "    Hakol Fine AV Services" -ForegroundColor Cyan
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host ""

$TotalSteps = 5

# ── 1. Stop existing ────────────────────────────────────────
Write-Step 1 $TotalSteps "Stopping existing services..."

Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
Get-Process -Name "midinet-tray" -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process -Name "midinet-client" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 1
Write-Ok "Stopped"

# ── 2. Build ────────────────────────────────────────────────
Write-Step 2 $TotalSteps "Building release binaries..."
Set-Location $RepoDir
cargo build --release -p midi-client -p midi-cli -p midi-tray
if ($LASTEXITCODE -ne 0) { Write-Fail "Build failed." }
Write-Ok "Build complete"

# ── 3. Install binaries + config ────────────────────────────
Write-Step 3 $TotalSteps "Installing binaries and config..."

New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null

Copy-Item "$RepoDir\target\release\midi-client.exe" "$BinDir\midinet-client.exe" -Force
Copy-Item "$RepoDir\target\release\midi-cli.exe"    "$BinDir\midinet-cli.exe" -Force
Copy-Item "$RepoDir\target\release\midi-tray.exe"   "$BinDir\midinet-tray.exe" -Force
Write-Ok "Binaries -> $BinDir"

# Add to PATH if not already there
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
    $env:PATH = "$BinDir;$env:PATH"
    Write-Ok "Added $BinDir to user PATH"
} else {
    Write-Ok "Already on PATH"
}

if (-not (Test-Path "$ConfigDir\client.toml")) {
    Copy-Item "$RepoDir\config\client.toml" "$ConfigDir\client.toml"
    Write-Ok "Config -> $ConfigDir\client.toml"
} else {
    Write-Warn "Config already exists - not overwriting"
}

# ── 4. Register services ───────────────────────────────────
Write-Step 4 $TotalSteps "Registering startup services..."

# Client daemon — ScheduledTask (runs at logon, auto-restart)
$action = New-ScheduledTaskAction `
    -Execute "$BinDir\midinet-client.exe" `
    -Argument "--config `"$ConfigDir\client.toml`"" `
    -WorkingDirectory $InstallDir

$trigger = New-ScheduledTaskTrigger -AtLogon -User $env:USERNAME

$settings = New-ScheduledTaskSettingsSet `
    -AllowStartIfOnBatteries `
    -DontStopIfGoingOnBatteries `
    -RestartCount 3 `
    -RestartInterval (New-TimeSpan -Seconds 10) `
    -ExecutionTimeLimit (New-TimeSpan -Days 365)

Register-ScheduledTask `
    -TaskName $TaskName `
    -Action $action `
    -Trigger $trigger `
    -Settings $settings `
    -Description "MIDInet client daemon" `
    | Out-Null
Write-Ok "Client registered as ScheduledTask"

# Tray app — Registry Run key (starts at login)
$regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
Set-ItemProperty -Path $regPath -Name $TrayRegName -Value "`"$BinDir\midinet-tray.exe`""
Write-Ok "Tray registered in startup (Registry Run)"

# ── 5. Start ───────────────────────────────────────────────
Write-Step 5 $TotalSteps "Starting services..."

Start-ScheduledTask -TaskName $TaskName
Start-Sleep -Seconds 1
Start-Process -FilePath "$BinDir\midinet-tray.exe" -WindowStyle Hidden
Write-Ok "Client daemon and tray started"

# ── Verify ─────────────────────────────────────────────────
Start-Sleep -Seconds 2
try {
    $null = Invoke-WebRequest -Uri "http://127.0.0.1:5009/health" -UseBasicParsing -TimeoutSec 3
    Write-Ok "Health endpoint responding on :5009"
} catch {
    Write-Warn "Health endpoint not responding yet (daemon may still be starting)"
}

# ── Done ───────────────────────────────────────────────────
Write-Host ""
Write-Host "  =================================================" -ForegroundColor Green
Write-Host "    MIDInet deployed on Windows!" -ForegroundColor Green
Write-Host "  =================================================" -ForegroundColor Green
Write-Host ""
Write-Host "  Config:   $ConfigDir\client.toml"
Write-Host "  Binaries: $BinDir"
Write-Host "  Tray:     Look for the colored circle in your system tray"
Write-Host ""
Write-Host "  Commands:"
Write-Host "    midinet-cli status                             # Check connection"
Write-Host "    Stop-ScheduledTask -TaskName '$TaskName'       # Stop daemon"
Write-Host "    Start-ScheduledTask -TaskName '$TaskName'      # Start daemon"
Write-Host "    .\scripts\deploy.ps1                           # Redeploy after changes"
Write-Host ""

# Warn about teVirtualMIDI
$teVmDll = "$env:SystemRoot\System32\teVirtualMIDI64.dll"
$teVmDll32 = "$env:SystemRoot\System32\teVirtualMIDI32.dll"
if (-not ((Test-Path $teVmDll) -or (Test-Path $teVmDll32))) {
    Write-Host "  NOTE: Install teVirtualMIDI driver for virtual MIDI ports:" -ForegroundColor Yellow
    Write-Host "  https://www.tobias-erichsen.de/software/virtualmidi.html" -ForegroundColor Yellow
    Write-Host ""
}
