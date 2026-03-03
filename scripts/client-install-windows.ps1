# ──────────────────────────────────────────────────────────────
# MIDInet - Windows Client Installer (PowerShell)
# Clones from GitHub, builds natively, and installs the system tray
# wrapper that manages the MIDI client lifecycle.
# Safe for both fresh installs and updates — stops running processes
# before replacing binaries to prevent stale versions.
#
# Usage (one-liner - run in PowerShell as Administrator):
#   powershell -NoExit -Command "irm https://raw.githubusercontent.com/Hakolsound/MIDInet/v3.1/scripts/client-install-windows.ps1 | iex"
#
# Or clone first:
#   git clone https://github.com/Hakolsound/MIDInet.git
#   cd MIDInet; .\scripts\client-install-windows.ps1
#
# Environment variables:
#   $env:MIDINET_BRANCH  - git branch (default: v3.1)
# ──────────────────────────────────────────────────────────────

$Branch = if ($env:MIDINET_BRANCH) { $env:MIDINET_BRANCH } else { "v3.1" }
$RepoUrl = "https://github.com/Hakolsound/MIDInet.git"
$InstallDir = "$env:LOCALAPPDATA\MIDInet"
$SrcDir = "$InstallDir\src"
$BinDir = "$InstallDir\bin"
$ConfigDir = "$InstallDir\config"
$LogDir = "$InstallDir\log"
$TaskName = "MIDInet Client"
$TrayRegName = "MIDInet Tray"
$Errors = @()

$IsWin11 = ([Environment]::OSVersion.Version.Build -ge 22000)

# Ensure TLS 1.2 — fresh Windows 10 may default to older versions
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

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
function Write-Err($msg) {
    Write-Host "    [X] $msg" -ForegroundColor Red
    $script:Errors += $msg
}

function Stop-MidiNetProcesses {
    # Stop the scheduled task first (graceful)
    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($task -and $task.State -eq 'Running') {
        Write-Warn "Stopping scheduled task '$TaskName'..."
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 500
    }

    # Force-kill any running MIDInet processes
    $procs = @("midinet-client", "midinet-tray", "midi-client", "midi-tray")
    foreach ($name in $procs) {
        $running = Get-Process -Name $name -ErrorAction SilentlyContinue
        if ($running) {
            Write-Warn "Stopping $name (PID: $($running.Id -join ', '))..."
            $running | Stop-Process -Force -ErrorAction SilentlyContinue
        }
    }

    # Wait for file handles to release
    Start-Sleep -Milliseconds 500
}

function Copy-WithRetry($src, $dst) {
    for ($i = 0; $i -lt 3; $i++) {
        try {
            Copy-Item $src $dst -Force -ErrorAction Stop
            return
        } catch {
            if ($i -eq 2) { throw $_ }
            Write-Warn "File locked, retrying in 1s..."
            Start-Sleep -Seconds 1
        }
    }
}

# ── Banner ────────────────────────────────────────────────────

Write-Host ""
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host "    MIDInet - Windows Client Installer" -ForegroundColor Cyan
Write-Host "    Hakol Fine AV Services" -ForegroundColor Cyan
Write-Host "  ========================================" -ForegroundColor Cyan
Write-Host ""

if ($IsWin11) {
    Write-Host "  Detected: Windows 11 (build $([Environment]::OSVersion.Version.Build))" -ForegroundColor DarkGray
} else {
    Write-Host "  Detected: Windows 10 (build $([Environment]::OSVersion.Version.Build))" -ForegroundColor DarkGray
}

# Check for existing installation
if (Test-Path "$BinDir\version.txt") {
    $prevVersion = Get-Content "$BinDir\version.txt" -ErrorAction SilentlyContinue
    Write-Host "  Previous install: $prevVersion" -ForegroundColor DarkGray
} else {
    Write-Host "  Fresh installation" -ForegroundColor DarkGray
}
Write-Host ""

$TotalSteps = 12
$IsAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# ── 1. Prerequisites ─────────────────────────────────────────
Write-Step 1 $TotalSteps "Checking prerequisites..."

# Git
if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    $gitInstalled = $false
    # Try winget first (may not exist on fresh Windows 10)
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Warn "Git not found. Installing via winget..."
        try {
            winget install --id Git.Git -e --source winget --accept-package-agreements --accept-source-agreements
            $env:PATH = "$env:ProgramFiles\Git\cmd;$env:PATH"
            if (Get-Command git -ErrorAction SilentlyContinue) { $gitInstalled = $true }
        } catch {}
    }
    # Fall back to direct download
    if (-not $gitInstalled) {
        Write-Warn "Git not found. Downloading installer from git-scm.com..."
        try {
            $gitInstallerUrl = "https://github.com/git-for-windows/git/releases/latest/download/Git-64-bit.exe"
            $gitInstaller = "$env:TEMP\git-installer.exe"
            Invoke-WebRequest -Uri $gitInstallerUrl -OutFile $gitInstaller -UseBasicParsing
            Write-Warn "Running Git installer (silent)..."
            Start-Process -FilePath $gitInstaller -ArgumentList "/VERYSILENT", "/NORESTART", "/NOCANCEL", "/SP-" -Wait
            Remove-Item $gitInstaller -Force -ErrorAction SilentlyContinue
            $env:PATH = "$env:ProgramFiles\Git\cmd;$env:PATH"
        } catch {
            Write-Warn "Direct download failed: $_"
        }
    }
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Err "Git installation failed. Install from https://git-scm.com and re-run."
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
        exit 1
    }
    Write-Ok "Rust installed ($(rustc --version))"
} else {
    Write-Ok "Rust already installed ($(rustc --version))"
}

# Visual Studio Build Tools (C++ workload) — required for Rust MSVC target
$HasVsCpp = $false
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vsWhere) {
    $vsInstall = & $vsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($vsInstall) {
        $HasVsCpp = $true
    }
}

if ($HasVsCpp) {
    Write-Ok "Visual Studio C++ Build Tools available"
} else {
    Write-Warn "Visual Studio C++ Build Tools not found (required for Rust to compile)."
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        Write-Warn "Installing minimal MSVC build tools via winget..."
        try {
            winget install Microsoft.VisualStudio.2022.BuildTools `
                --override "--wait --passive --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.Windows11SDK.26100" `
                --accept-package-agreements --accept-source-agreements
        } catch {}
    } else {
        Write-Warn "winget not available. Downloading VS Build Tools installer directly..."
        try {
            $vsbtInstaller = "$env:TEMP\vs_BuildTools.exe"
            Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vs_BuildTools.exe" -OutFile $vsbtInstaller -UseBasicParsing
            Write-Warn "Running VS Build Tools installer (this may take several minutes)..."
            Start-Process -FilePath $vsbtInstaller -ArgumentList `
                "--wait", "--passive", "--norestart", `
                "--add", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", `
                "--add", "Microsoft.VisualStudio.Component.Windows11SDK.26100" -Wait
            Remove-Item $vsbtInstaller -Force -ErrorAction SilentlyContinue
        } catch {
            Write-Warn "Direct download failed: $_"
        }
    }

    # Re-check after install attempt
    $vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vsWhere) {
        $vsInstall = & $vsWhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($vsInstall) {
            $HasVsCpp = $true
        }
    }

    if ($HasVsCpp) {
        Write-Ok "Visual Studio C++ Build Tools installed"
    } else {
        Write-Err "Visual Studio C++ Build Tools could not be installed automatically."
        Write-Host ""
        Write-Host "    Rust on Windows requires the MSVC linker (link.exe) which comes with" -ForegroundColor Red
        Write-Host "    Visual Studio Build Tools. Without it, the build WILL fail." -ForegroundColor Red
        Write-Host ""
        Write-Host "    Please install manually:" -ForegroundColor Yellow
        Write-Host "    1. Download from: https://visualstudio.microsoft.com/visual-cpp-build-tools/" -ForegroundColor Yellow
        Write-Host "    2. Select 'Desktop development with C++' workload" -ForegroundColor Yellow
        Write-Host "    3. Re-run this installer after installation completes" -ForegroundColor Yellow
        Write-Host ""
        exit 1
    }
}

# ── 2. MIDI Driver Check ─────────────────────────────────────
Write-Step 2 $TotalSteps "Checking MIDI virtual device support..."

$teVmDll = "$env:SystemRoot\System32\teVirtualMIDI64.dll"
$teVmDll32 = "$env:SystemRoot\System32\teVirtualMIDI32.dll"
$HasTeVirtualMidi = (Test-Path $teVmDll) -or (Test-Path $teVmDll32)

if ($HasTeVirtualMidi) {
    Write-Ok "teVirtualMIDI driver found (primary backend)"
} elseif ($IsWin11) {
    Write-Ok "Windows 11 detected - Windows MIDI Services will be used as fallback"
    Write-Host "    teVirtualMIDI driver not found, but not required on Windows 11." -ForegroundColor DarkGray
    Write-Host "    The client uses Windows MIDI Services as a native alternative." -ForegroundColor DarkGray
} else {
    Write-Warn "teVirtualMIDI driver not found (required on Windows 10 for virtual MIDI ports)."
    Write-Warn "Downloading teVirtualMIDI SDK from tobias-erichsen.de..."
    $teVmZip = "$env:TEMP\teVirtualMIDISDK.zip"
    $teVmDir = "$env:TEMP\teVirtualMIDISDK"
    $teVmUrl = "https://www.tobias-erichsen.de/wp-content/uploads/2020/01/teVirtualMIDISDKSetup_1_3_0_43.zip"
    try {
        Invoke-WebRequest -Uri $teVmUrl -OutFile $teVmZip -UseBasicParsing
        Expand-Archive -Path $teVmZip -DestinationPath $teVmDir -Force
        $teVmSetup = Get-ChildItem -Path $teVmDir -Filter "*.exe" -Recurse | Select-Object -First 1
        if ($teVmSetup) {
            Write-Warn "Running teVirtualMIDI installer (may request admin approval)..."
            Start-Process -FilePath $teVmSetup.FullName -ArgumentList "/SILENT" -Wait
        }
        # Clean up temp files
        Remove-Item $teVmZip -Force -ErrorAction SilentlyContinue
        Remove-Item $teVmDir -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        Remove-Item $teVmZip -Force -ErrorAction SilentlyContinue
        Remove-Item $teVmDir -Recurse -Force -ErrorAction SilentlyContinue
    }

    # Re-check after install attempt
    $HasTeVirtualMidi = (Test-Path $teVmDll) -or (Test-Path $teVmDll32)
    if ($HasTeVirtualMidi) {
        Write-Ok "teVirtualMIDI driver installed"
    } else {
        Write-Warn "teVirtualMIDI driver could not be installed automatically."
        Write-Warn "Virtual MIDI ports won't work until the driver is installed."
        Write-Warn "Download manually from: https://www.tobias-erichsen.de/software/virtualmidi.html"
    }
}

# ── 3. Windows MIDI Services SDK (Win11 only) ────────────────
Write-Step 3 $TotalSteps "Checking Windows MIDI Services SDK..."

if ($IsWin11) {
    # Check if Windows MIDI Services is installed
    $midiSvcInstalled = $false
    try {
        $wingetList = winget list --id Microsoft.WindowsMIDIServices --accept-source-agreements 2>&1
        if ($wingetList -match "Microsoft.WindowsMIDIServices") {
            $midiSvcInstalled = $true
        }
    } catch {}

    if ($midiSvcInstalled) {
        Write-Ok "Windows MIDI Services SDK already installed"
    } else {
        if (-not $HasTeVirtualMidi) {
            Write-Warn "Installing Windows MIDI Services SDK (required for virtual MIDI on Win11 without teVirtualMIDI)..."
        } else {
            Write-Host "    Installing Windows MIDI Services SDK (recommended fallback)..." -ForegroundColor DarkGray
        }
        try {
            winget install Microsoft.WindowsMIDIServices --accept-package-agreements --accept-source-agreements 2>&1 | Out-Null
            Write-Ok "Windows MIDI Services SDK installed"
        } catch {
            if (-not $HasTeVirtualMidi) {
                Write-Err "Failed to install Windows MIDI Services SDK: $_"
                Write-Warn "Virtual MIDI may not work. Install manually: winget install Microsoft.WindowsMIDIServices"
            } else {
                Write-Warn "Could not install Windows MIDI Services SDK (teVirtualMIDI will be used instead)"
            }
        }
    }
} else {
    Write-Ok "Skipped (Windows MIDI Services requires Windows 11)"
}

# ── 4. Stop Running MIDInet Processes ─────────────────────────
Write-Step 4 $TotalSteps "Stopping any running MIDInet processes..."

$hadRunning = $false
$procs = @("midinet-client", "midinet-tray", "midi-client", "midi-tray")
foreach ($name in $procs) {
    if (Get-Process -Name $name -ErrorAction SilentlyContinue) {
        $hadRunning = $true
        break
    }
}

if ($hadRunning) {
    Stop-MidiNetProcesses
    Write-Ok "All MIDInet processes stopped"
} else {
    Write-Ok "No running MIDInet processes found"
}

# ── 5. Clone / Update Source ──────────────────────────────────
Write-Step 5 $TotalSteps "Fetching MIDInet source..."

try {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

    if (Test-Path "$SrcDir\.git") {
        Set-Location $SrcDir
        git fetch origin
        # Use -B to force branch checkout (avoids checking out tag when both tag + branch
        # exist with the same name, which causes detached HEAD)
        git checkout -B $Branch "origin/$Branch"
        Write-Ok "Updated to latest $Branch"
    } else {
        git clone --branch $Branch $RepoUrl $SrcDir
        Set-Location $SrcDir
        Write-Ok "Cloned $RepoUrl ($Branch)"
    }
} catch {
    Write-Err "Failed to fetch source: $_"
    exit 1
}

# ── 6. Build ──────────────────────────────────────────────────
Write-Step 6 $TotalSteps "Building MIDInet (release mode - this may take a while)..."
Set-Location $SrcDir
cargo build --release -p midi-client -p midi-cli -p midi-tray
if ($LASTEXITCODE -ne 0) {
    Write-Err "Build failed. Check errors above."
    exit 1
}
Write-Ok "Build complete"

# ── 7. Install Binaries ──────────────────────────────────────
Write-Step 7 $TotalSteps "Installing binaries..."

# Ensure processes are stopped before overwriting
Stop-MidiNetProcesses

try {
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null

    Copy-WithRetry "$SrcDir\target\release\midi-client.exe" "$BinDir\midinet-client.exe"
    Copy-WithRetry "$SrcDir\target\release\midi-cli.exe"    "$BinDir\midinet-cli.exe"
    Copy-WithRetry "$SrcDir\target\release\midi-tray.exe"   "$BinDir\midinet-tray.exe"

    # Write version info
    $gitHash = (git -C $SrcDir rev-parse --short HEAD 2>$null)
    $buildTime = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    "$gitHash ($buildTime)" | Set-Content "$BinDir\version.txt"

    Write-Ok "Binaries installed to $BinDir"
    Write-Ok "Version: $gitHash ($buildTime)"
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

# Copy icon file for shortcuts
try {
    Copy-Item "$SrcDir\assets\icons\midinet.ico" "$BinDir\midinet.ico" -Force
} catch {
    Write-Warn "Could not copy icon file: $_"
}

# ── 8. Desktop Shortcuts ──────────────────────────────────────
Write-Step 8 $TotalSteps "Creating desktop shortcuts..."

try {
    $WshShell = New-Object -ComObject WScript.Shell

    # Start Menu shortcut
    $StartMenuDir = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\MIDInet"
    New-Item -ItemType Directory -Path $StartMenuDir -Force | Out-Null

    $StartMenuLink = $WshShell.CreateShortcut("$StartMenuDir\MIDInet.lnk")
    $StartMenuLink.TargetPath = "$BinDir\midinet-tray.exe"
    $StartMenuLink.WorkingDirectory = $BinDir
    $StartMenuLink.Description = "MIDInet - Real-time MIDI over network"
    if (Test-Path "$BinDir\midinet.ico") {
        $StartMenuLink.IconLocation = "$BinDir\midinet.ico"
    }
    $StartMenuLink.Save()
    Write-Ok "Start Menu shortcut created"

    # Desktop shortcut
    $DesktopLink = $WshShell.CreateShortcut("$env:USERPROFILE\Desktop\MIDInet.lnk")
    $DesktopLink.TargetPath = "$BinDir\midinet-tray.exe"
    $DesktopLink.WorkingDirectory = $BinDir
    $DesktopLink.Description = "MIDInet - Real-time MIDI over network"
    if (Test-Path "$BinDir\midinet.ico") {
        $DesktopLink.IconLocation = "$BinDir\midinet.ico"
    }
    $DesktopLink.Save()
    Write-Ok "Desktop shortcut created"
} catch {
    Write-Warn "Could not create shortcuts: $_"
}

# ── 9. Config ─────────────────────────────────────────────────
Write-Step 9 $TotalSteps "Setting up configuration..."

try {
    New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null
    New-Item -ItemType Directory -Path $LogDir -Force | Out-Null

    if (-not (Test-Path "$ConfigDir\client.toml")) {
        Copy-Item "$SrcDir\config\client.toml" "$ConfigDir\client.toml"
        Write-Ok "Default config installed to $ConfigDir\client.toml"
    } else {
        Write-Warn "Config already exists - not overwriting"
    }
} catch {
    Write-Err "Failed to set up config: $_"
}

# ── 9. Windows Firewall Rules ────────────────────────────────
Write-Step 10 $TotalSteps "Configuring Windows Firewall rules..."

if ($IsAdmin) {
    $fwRules = @(
        @{ Name = "MIDInet MIDI Data (UDP 5004)";   Port = 5004; Desc = "MIDInet multicast MIDI data" },
        @{ Name = "MIDInet Heartbeat (UDP 5005)";    Port = 5005; Desc = "MIDInet host heartbeat" },
        @{ Name = "MIDInet Control (UDP 5006)";      Port = 5006; Desc = "MIDInet focus/control channel" },
        @{ Name = "MIDInet mDNS (UDP 5353)";         Port = 5353; Desc = "mDNS service discovery" }
    )

    foreach ($rule in $fwRules) {
        $existing = Get-NetFirewallRule -DisplayName $rule.Name -ErrorAction SilentlyContinue
        if (-not $existing) {
            New-NetFirewallRule -DisplayName $rule.Name `
                -Direction Inbound -Protocol UDP -LocalPort $rule.Port `
                -Action Allow -Profile Private,Domain `
                -Description $rule.Desc -ErrorAction SilentlyContinue | Out-Null
            Write-Ok "Created: $($rule.Name)"
        } else {
            Write-Ok "Already exists: $($rule.Name)"
        }
    }
} else {
    Write-Warn "Not running as Administrator - skipping firewall rules"
    Write-Warn "If MIDI doesn't connect, re-run this script as Administrator, or manually allow"
    Write-Warn "inbound UDP ports 5004, 5005, 5006, and 5353 in Windows Firewall."
}

# ── 11. Remove legacy scheduled task (tray now manages client)
Write-Step 11 $TotalSteps "Cleaning up legacy startup..."

# Remove the old standalone client task — the tray spawns the client now
$oldTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
if ($oldTask) {
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Write-Ok "Removed legacy scheduled task '$TaskName' (tray manages client now)"
} else {
    Write-Ok "No legacy scheduled task to remove"
}

# ── 12. Tray Auto-Start ──────────────────────────────────────
Write-Step 12 $TotalSteps "Installing tray application (auto-start at login)..."

try {
    # Register tray in user startup via Registry Run key
    $regPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    Set-ItemProperty -Path $regPath -Name $TrayRegName -Value "`"$BinDir\midinet-tray.exe`""
    Write-Ok "Tray registered to start at login"

    # Start a single tray instance
    Start-Process -FilePath "$BinDir\midinet-tray.exe" -WindowStyle Hidden
    Start-Sleep -Milliseconds 500

    # Verify single instance
    $trayCount = (Get-Process -Name "midinet-tray" -ErrorAction SilentlyContinue | Measure-Object).Count
    if ($trayCount -eq 1) {
        Write-Ok "Tray running (single instance)"
    } elseif ($trayCount -gt 1) {
        Write-Warn "Multiple tray instances detected - killing extras..."
        Get-Process -Name "midinet-tray" | Select-Object -Skip 1 | Stop-Process -Force
        Write-Ok "Cleaned up to single tray instance"
    } else {
        Write-Warn "Tray process not detected - it may take a moment to start"
    }
} catch {
    Write-Err "Failed to set up tray: $_"
}

# ── Summary ───────────────────────────────────────────────────
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
$installedVersion = if (Test-Path "$BinDir\version.txt") { Get-Content "$BinDir\version.txt" } else { "unknown" }
Write-Host "  Version:  $installedVersion"
Write-Host ""
Write-Host "  The client will auto-discover hosts on your LAN."
Write-Host "  Virtual MIDI device will appear once a host is found."

if ($IsWin11 -and -not $HasTeVirtualMidi) {
    Write-Host ""
    Write-Host "  MIDI Backend: Windows MIDI Services (native)" -ForegroundColor DarkGray
} elseif ($HasTeVirtualMidi) {
    Write-Host ""
    Write-Host "  MIDI Backend: teVirtualMIDI (driver)" -ForegroundColor DarkGray
}

Write-Host ""
Write-Host "  Config:   $ConfigDir\client.toml"
Write-Host "  Binaries: $BinDir"
Write-Host "  Logs:     $LogDir"
Write-Host "  Source:   $SrcDir"
Write-Host ""
Write-Host "  Management:"
Write-Host "    Right-click the MIDInet tray icon for focus control and status."
Write-Host "    The tray auto-starts the client and restarts it on crash."
Write-Host ""
Write-Host "  Commands:"
Write-Host "    midinet-cli status                            # Check connection"
Write-Host "    midinet-cli focus                             # View/claim focus"
Write-Host ""
Write-Host "  Update:"
Write-Host "    cd $SrcDir; .\scripts\client-install-windows.ps1"
Write-Host ""
Write-Host "  Uninstall:"
Write-Host "    .\scripts\client-uninstall-windows.ps1"
Write-Host ""
if (-not $IsAdmin) {
    Write-Host "  NOTE: Firewall rules were skipped (not running as Administrator)." -ForegroundColor Yellow
    Write-Host "  If the client can't discover hosts, re-run as Admin or manually open" -ForegroundColor Yellow
    Write-Host "  inbound UDP ports 5004, 5005, 5006, 5353 in Windows Firewall." -ForegroundColor Yellow
    Write-Host ""
}

if (-not $IsWin11 -and -not $HasTeVirtualMidi) {
    Write-Host "  REMINDER: Install teVirtualMIDI driver for virtual MIDI ports:" -ForegroundColor Yellow
    Write-Host "  https://www.tobias-erichsen.de/software/virtualmidi.html" -ForegroundColor Yellow
    Write-Host ""
}
