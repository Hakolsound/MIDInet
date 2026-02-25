# ──────────────────────────────────────────────────────────────
# MIDInet — Windows Client Installer (PowerShell)
# Clones from GitHub, builds natively, and installs as a startup task.
#
# Usage (one-liner — run in PowerShell):
#   powershell -NoExit -Command "irm https://raw.githubusercontent.com/Hakolsound/MIDInet/main/scripts/client-install-windows.ps1 | iex"
#
# Or clone first:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet; .\scripts\client-install-windows.ps1
#
# Environment variables:
#   $env:MIDINET_BRANCH  — git branch (default: main)
# ──────────────────────────────────────────────────────────────

$Branch = if ($env:MIDINET_BRANCH) { $env:MIDINET_BRANCH } else { "main" }
$RepoUrl = "https://github.com/Hakolsound/MIDInet.git"
$InstallDir = "$env:LOCALAPPDATA\MIDInet"
$SrcDir = "$InstallDir\src"
$BinDir = "$InstallDir\bin"
$ConfigDir = "$InstallDir\config"
$LogDir = "$InstallDir\log"
$TaskName = "MIDInet Client"
$Errors = @()

function Write-Step($num, $total, $msg) {
    Write-Host "`n[$num/$total] $msg" -ForegroundColor Cyan
}
function Write-Ok($msg) {
    Write-Host "    [OK] $msg" -ForegroundColor Green
}
function Write-Warn($msg) {
    Write-Host "    [!] $msg" -ForegroundColor Yellow
}
function Write-Err($msg) {
    Write-Host "    [X] $msg" -ForegroundColor Red
    $script:Errors += $msg
}

Write-Host ""
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host "    MIDInet - Windows Client Installer" -ForegroundColor Cyan
Write-Host "    Hakol Fine AV Services" -ForegroundColor Cyan
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host ""

$TotalSteps = 8

# ── 1. Prerequisites ─────────────────────────────────────────
Write-Step 1 $TotalSteps "Checking prerequisites..."

# Git
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    Write-Warn "Git not found. Installing via winget..."
    try {
        winget install --id Git.Git -e --source winget --accept-package-agreements --accept-source-agreements
        $env:PATH = "$env:ProgramFiles\Git\cmd;$env:PATH"
    } catch {}
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Err "Git installation failed. Install from https://git-scm.com and re-run."
        Write-Host "`nCannot continue without Git. Press Enter to exit..." -ForegroundColor Red
        Read-Host
        exit 1
    }
}
Write-Ok "Git available ($(git --version))"

# Rust
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Warn "Rust not found. Installing via rustup..."
    try {
        $rustupInit = "$env:TEMP\rustup-init.exe"
        Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit
        & $rustupInit -y --default-toolchain stable
        $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
    } catch {}
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Write-Err "Rust installation failed. Install from https://rustup.rs and re-run."
        Write-Host "`nCannot continue without Rust. Press Enter to exit..." -ForegroundColor Red
        Read-Host
        exit 1
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

# ── 2. teVirtualMIDI Driver ─────────────────────────────────
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
    Write-Host "    The client will build and install, but virtual MIDI ports won't work" -ForegroundColor Yellow
    Write-Host "    until the driver is installed." -ForegroundColor Yellow
    Write-Host ""

    $response = Read-Host "    Continue anyway? (Y/n)"
    if ($response -eq 'n' -or $response -eq 'N') {
        Write-Host "    Opening download page..."
        Start-Process "https://www.tobias-erichsen.de/software/virtualmidi.html"
        exit 0
    }
}

# ── 3. Clone / Update ───────────────────────────────────────
Write-Step 3 $TotalSteps "Fetching MIDInet source..."

try {
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
} catch {
    Write-Err "Failed to fetch source: $_"
    Write-Host "`nCannot continue without source. Press Enter to exit..." -ForegroundColor Red
    Read-Host
    exit 1
}

# ── 4. Build ─────────────────────────────────────────────────
Write-Step 4 $TotalSteps "Building midi-client (release mode — this may take a while)..."
Set-Location $SrcDir
cargo build --release -p midi-client -p midi-cli -p midi-tray
if ($LASTEXITCODE -ne 0) {
    Write-Err "Build failed. Check errors above."
    Write-Host "`nCannot continue without a successful build. Press Enter to exit..." -ForegroundColor Red
    Read-Host
    exit 1
}
Write-Ok "Build complete"

# ── 5. Install ───────────────────────────────────────────────
Write-Step 5 $TotalSteps "Installing binaries..."

try {
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null
    Copy-Item "$SrcDir\target\release\midi-client.exe" "$BinDir\midinet-client.exe" -Force
    Copy-Item "$SrcDir\target\release\midi-cli.exe"    "$BinDir\midinet-cli.exe" -Force
    Copy-Item "$SrcDir\target\release\midi-tray.exe"   "$BinDir\midinet-tray.exe" -Force
    Write-Ok "Binaries installed to $BinDir"
} catch {
    Write-Err "Failed to install binaries: $_"
}

# Add to PATH if not already there
try {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($userPath -notlike "*$BinDir*") {
        [Environment]::SetEnvironmentVariable("Path", "$userPath;$BinDir", "User")
        $env:PATH = "$BinDir;$env:PATH"
        Write-Ok "Added $BinDir to user PATH"
    } else {
        Write-Ok "Already on PATH"
    }
} catch {
    Write-Warn "Could not update PATH: $_"
}

# ── 6. Config ────────────────────────────────────────────────
Write-Step 6 $TotalSteps "Setting up configuration..."

try {
    New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null
    New-Item -ItemType Directory -Path $LogDir -Force | Out-Null

    if (-not (Test-Path "$ConfigDir\client.toml")) {
        Copy-Item "$SrcDir\config\client.toml" "$ConfigDir\client.toml"
        Write-Ok "Default config installed to $ConfigDir\client.toml"
    } else {
        Write-Warn "Config already exists — not overwriting"
    }
} catch {
    Write-Err "Failed to set up config: $_"
}

# ── 7. Startup Task ─────────────────────────────────────────
Write-Step 7 $TotalSteps "Installing startup task..."

try {
    # Remove existing task if present
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue

    $action = New-ScheduledTaskAction `
        -Execute "powershell.exe" `
        -Argument "-WindowStyle Hidden -Command `"& '$BinDir\midinet-client.exe' --config '$ConfigDir\client.toml'`"" `
        -WorkingDirectory $InstallDir

    $trigger = New-ScheduledTaskTrigger -AtLogon -User $env:USERNAME

    $settings = New-ScheduledTaskSettingsSet `
        -AllowStartIfOnBatteries `
        -DontStopIfGoingOnBatteries `
        -RestartCount 3 `
        -RestartInterval (New-TimeSpan -Minutes 1) `
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
} catch {
    Write-Err "Failed to register scheduled task: $_"
    Write-Warn "You can start the client manually: midinet-client --config `"$ConfigDir\client.toml`""
}

# ── 8. Tray Auto-Start ─────────────────────────────────────
Write-Step 8 $TotalSteps "Installing tray application (auto-start at login)..."

try {
    # Register tray in user startup via Registry Run key
    $regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    Set-ItemProperty -Path $regPath -Name "MIDInet Tray" -Value "`"$BinDir\midinet-tray.exe`""
    Write-Ok "Tray registered to start at login"

    # Start tray now
    Start-Process -FilePath "$BinDir\midinet-tray.exe" -WindowStyle Hidden
    Write-Ok "Tray started"
} catch {
    Write-Err "Failed to set up tray: $_"
}

# ── Summary ──────────────────────────────────────────────────
Write-Host ""
if ($Errors.Count -eq 0) {
    Write-Host "  =================================================" -ForegroundColor Green
    Write-Host "    MIDInet client installed successfully!" -ForegroundColor Green
    Write-Host "  =================================================" -ForegroundColor Green
} else {
    Write-Host "  =================================================" -ForegroundColor Yellow
    Write-Host "    MIDInet client installed with warnings" -ForegroundColor Yellow
    Write-Host "  =================================================" -ForegroundColor Yellow
    Write-Host ""
    Write-Host "  Issues encountered:" -ForegroundColor Yellow
    foreach ($err in $Errors) {
        Write-Host "    - $err" -ForegroundColor Red
    }
}
Write-Host ""
Write-Host "  The client will auto-discover hosts on your LAN."
Write-Host "  Virtual MIDI device will appear once a host is found."
Write-Host ""
Write-Host "  Config:   $ConfigDir\client.toml"
Write-Host "  Binaries: $BinDir"
Write-Host "  Logs:     $LogDir"
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
