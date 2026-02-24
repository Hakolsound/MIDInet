# ──────────────────────────────────────────────────────────────
# MIDInet — Windows Client Installer (PowerShell)
# Clones from GitHub, builds natively, and installs as a startup task.
#
# Usage (run in PowerShell as Administrator):
#   irm https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/client-install-windows.ps1 | iex
#
# Or clone first:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet; .\scripts\client-install-windows.ps1
#
# Environment variables:
#   $env:MIDINET_BRANCH  — git branch (default: main)
# ──────────────────────────────────────────────────────────────

$ErrorActionPreference = "Stop"

$Branch = if ($env:MIDINET_BRANCH) { $env:MIDINET_BRANCH } else { "main" }
$RepoUrl = "https://github.com/Hakolsound/MIDInet.git"
$InstallDir = "$env:LOCALAPPDATA\MIDInet"
$SrcDir = "$InstallDir\src"
$BinDir = "$InstallDir\bin"
$ConfigDir = "$InstallDir\config"
$TaskName = "MIDInet Client"

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
Write-Host "    MIDInet - Windows Client Installer" -ForegroundColor Cyan
Write-Host "    Hakol Fine AV Services" -ForegroundColor Cyan
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host ""

$TotalSteps = 7

# ── Prerequisites ─────────────────────────────────────────────
Write-Step 1 $TotalSteps "Checking prerequisites..."

# Git
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Warn "Git not found. Installing via winget..."
    winget install --id Git.Git -e --source winget --accept-package-agreements --accept-source-agreements
    $env:PATH = "$env:ProgramFiles\Git\cmd;$env:PATH"
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Fail "Git installation failed. Install from https://git-scm.com and re-run."
    }
}
Write-Ok "Git available ($(git --version))"

# Rust
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Warn "Rust not found. Installing via rustup..."
    $rustupInit = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit
    & $rustupInit -y --default-toolchain stable
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Fail "Rust installation failed. Install from https://rustup.rs and re-run."
    }
    Write-Ok "Rust installed ($(rustc --version))"
} else {
    Write-Ok "Rust already installed ($(rustc --version))"
}

# Visual Studio Build Tools (C++ workload)
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vsWhere) {
    $vsInstall = & $vsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vsInstall) {
        Write-Ok "Visual Studio C++ Build Tools available"
    } else {
        Write-Warn "Visual Studio found but C++ workload missing."
        Write-Warn "Install 'Desktop development with C++' workload from Visual Studio Installer."
    }
} else {
    Write-Warn "Visual Studio Build Tools not detected."
    Write-Warn "If the build fails, install from: https://visualstudio.microsoft.com/visual-cpp-build-tools/"
    Write-Warn "Select 'Desktop development with C++' workload."
}

# ── teVirtualMIDI Driver ─────────────────────────────────────
Write-Step 2 $TotalSteps "Checking teVirtualMIDI driver..."

$teVmDll = "$env:SystemRoot\System32\teVirtualMIDI64.dll"
$teVmDll32 = "$env:SystemRoot\System32\teVirtualMIDI32.dll"

if ((Test-Path $teVmDll) -or (Test-Path $teVmDll32)) {
    Write-Ok "teVirtualMIDI driver found"
} else {
    Write-Warn "teVirtualMIDI driver NOT found."
    Write-Host ""
    Write-Host "    MIDInet requires the teVirtualMIDI driver to create virtual MIDI ports." -ForegroundColor Yellow
    Write-Host "    Download from: https://www.tobias-erichsen.de/software/virtualmidi.html" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "    Install the driver, then re-run this script." -ForegroundColor Yellow
    Write-Host "    (The client will start but log warnings without this driver.)" -ForegroundColor Yellow
    Write-Host ""

    $response = Read-Host "    Continue anyway? (y/N)"
    if ($response -ne 'y' -and $response -ne 'Y') {
        Write-Host "    Opening download page..."
        Start-Process "https://www.tobias-erichsen.de/software/virtualmidi.html"
        exit 0
    }
}

# ── Clone / Update ────────────────────────────────────────────
Write-Step 3 $TotalSteps "Fetching MIDInet source..."

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

if (Test-Path "$SrcDir\.git") {
    Set-Location $SrcDir
    git fetch origin
    git checkout $Branch
    git reset --hard "origin/$Branch"
    Write-Ok "Updated to latest $Branch"
} else {
    git clone --branch $Branch $RepoUrl $SrcDir
    Set-Location $SrcDir
    Write-Ok "Cloned $RepoUrl ($Branch)"
}

# ── Build ─────────────────────────────────────────────────────
Write-Step 4 $TotalSteps "Building midi-client (release mode — this may take a while)..."
Set-Location $SrcDir
cargo build --release -p midi-client -p midi-cli
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Build failed. Check errors above."
}
Write-Ok "Build complete"

# ── Install ───────────────────────────────────────────────────
Write-Step 5 $TotalSteps "Installing binaries..."

New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
Copy-Item "$SrcDir\target\release\midi-client.exe" "$BinDir\midinet-client.exe" -Force
Copy-Item "$SrcDir\target\release\midi-cli.exe"    "$BinDir\midinet-cli.exe" -Force
Write-Ok "Binaries installed to $BinDir"

# Add to PATH if not already there
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$BinDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
    $env:PATH = "$BinDir;$env:PATH"
    Write-Ok "Added $BinDir to user PATH"
} else {
    Write-Ok "Already on PATH"
}

# ── Config ────────────────────────────────────────────────────
Write-Step 6 $TotalSteps "Setting up configuration..."

New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null

if (-not (Test-Path "$ConfigDir\client.toml")) {
    Copy-Item "$SrcDir\config\client.toml" "$ConfigDir\client.toml"
    Write-Ok "Default config installed to $ConfigDir\client.toml"
} else {
    Write-Warn "Config already exists - not overwriting"
}

# ── Startup Task ──────────────────────────────────────────────
Write-Step 7 $TotalSteps "Installing startup task..."

# Remove existing task if present
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue

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
    -Description "MIDInet client daemon — receives MIDI over network and creates virtual devices" `
    | Out-Null

# Start it now
Start-ScheduledTask -TaskName $TaskName
Write-Ok "Scheduled task installed and started"

# ── Done ──────────────────────────────────────────────────────
Write-Host ""
Write-Host "  =================================================" -ForegroundColor Green
Write-Host "    MIDInet client installed!" -ForegroundColor Green
Write-Host "  =================================================" -ForegroundColor Green
Write-Host ""
Write-Host "  The client is running and will auto-discover hosts on your LAN."
Write-Host "  Virtual MIDI device will appear once a host is found."
Write-Host ""
Write-Host "  Config:   $ConfigDir\client.toml"
Write-Host "  Binaries: $BinDir"
Write-Host "  Source:   $SrcDir"
Write-Host ""
Write-Host "  Commands:"
Write-Host "    midinet-cli status                            # Check connection"
Write-Host "    midinet-cli focus                             # View/claim focus"
Write-Host "    Stop-ScheduledTask -TaskName '$TaskName'      # Stop"
Write-Host "    Start-ScheduledTask -TaskName '$TaskName'     # Start"
Write-Host ""
Write-Host "  Update: cd $SrcDir; .\scripts\client-install-windows.ps1"
Write-Host ""

if (-not ((Test-Path $teVmDll) -or (Test-Path $teVmDll32))) {
    Write-Host "  REMINDER: Install teVirtualMIDI driver for virtual MIDI ports:" -ForegroundColor Yellow
    Write-Host "  https://www.tobias-erichsen.de/software/virtualmidi.html" -ForegroundColor Yellow
    Write-Host ""
}
