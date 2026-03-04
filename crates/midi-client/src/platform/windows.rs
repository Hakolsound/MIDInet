/// Windows virtual MIDI device — orchestrator with backend selection.
///
/// Strategy (Windows 11+):
/// 1. Use Windows MIDI Services (native, no driver install needed)
/// 2. If midisrv.exe is hung → kill it, wait for restart, retry
///    (teVirtualMIDI does NOT work on Windows 11)
///
/// Strategy (Windows 10 and below):
/// 1. Use teVirtualMIDI (requires driver install)
/// 2. No MIDI Services fallback available
///
/// The selected backend is transparent to the rest of the codebase —
/// `WindowsVirtualDevice` implements `VirtualMidiDevice` regardless of which
/// backend is active underneath.

use crate::platform::midi_services::MidiServicesDevice;
use crate::platform::te_virtual_midi::TeVirtualMidiDevice;
use crate::virtual_device::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;
use tracing::{error, info, warn};

// ── Windows version detection ──

/// Detect Windows 11 (build >= 22000) using RtlGetVersion.
/// Unlike GetVersionEx, RtlGetVersion returns the real OS version
/// regardless of application manifest compatibility settings.
#[cfg(target_os = "windows")]
fn is_windows_11() -> bool {
    #[repr(C)]
    struct OsVersionInfoW {
        os_version_info_size: u32,
        major_version: u32,
        minor_version: u32,
        build_number: u32,
        platform_id: u32,
        sz_csd_version: [u16; 128],
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(lpVersionInformation: *mut OsVersionInfoW) -> i32;
    }

    let mut info: OsVersionInfoW = unsafe { std::mem::zeroed() };
    info.os_version_info_size = std::mem::size_of::<OsVersionInfoW>() as u32;
    unsafe { RtlGetVersion(&mut info) };

    let is_win11 = info.build_number >= 22000;
    info!(
        build = info.build_number,
        major = info.major_version,
        minor = info.minor_version,
        is_windows_11 = is_win11,
        "Windows version detected"
    );
    is_win11
}

#[cfg(not(target_os = "windows"))]
fn is_windows_11() -> bool {
    false
}

// ── MIDI Services recovery ──

/// Kill midisrv.exe to reset stale MIDI Services state.
/// Service Control Manager will auto-restart it after a few seconds.
/// Called when MIDI Services device creation hangs or the crash sentinel exists.
#[cfg(target_os = "windows")]
pub(crate) fn kill_midi_services() {
    use std::os::windows::process::CommandExt;
    info!("Killing midisrv.exe to reset MIDI Services state...");
    let output = std::process::Command::new("taskkill")
        .args(["/F", "/IM", "midisrv.exe"])
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
    match output {
        Ok(o) if o.status.success() => info!("midisrv.exe killed successfully"),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!("taskkill midisrv.exe exit {}: {}", o.status, stderr.trim());
        }
        Err(e) => warn!("Failed to run taskkill: {}", e),
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn kill_midi_services() {}

// ── Backend enum ──

enum Backend {
    TeVirtualMidi(TeVirtualMidiDevice),
    MidiServices(MidiServicesDevice),
    /// No backend initialized yet — `create()` hasn't been called
    Uninit,
}

// ── WindowsVirtualDevice (public API) ──

pub struct WindowsVirtualDevice {
    backend: Backend,
}

unsafe impl Send for WindowsVirtualDevice {}
unsafe impl Sync for WindowsVirtualDevice {}

impl WindowsVirtualDevice {
    pub fn new() -> Self {
        Self {
            backend: Backend::Uninit,
        }
    }
}

impl WindowsVirtualDevice {
    /// Path of the crash-sentinel file.  Written before attempting MIDI Services
    /// device creation and deleted on success.  If it exists at startup, the
    /// previous attempt crashed or hung → kill midisrv.exe before retrying.
    /// Uses %LOCALAPPDATA%\MIDInet — Program Files is read-only.
    pub(crate) fn crash_sentinel() -> std::path::PathBuf {
        let dir = std::env::var("LOCALAPPDATA")
            .map(|d| std::path::PathBuf::from(d).join("MIDInet"))
            .unwrap_or_else(|_| {
                std::env::current_exe()
                    .ok()
                    .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
            });
        let _ = std::fs::create_dir_all(&dir);
        dir.join(".midinet-midi-services-crash")
    }

    /// Windows 11+: MIDI Services only (teVirtualMIDI does NOT work on Win 11).
    /// If the previous attempt crashed or hung, kill midisrv.exe to reset state.
    fn create_win11(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        let sentinel = Self::crash_sentinel();

        // If crash sentinel exists or env var is set, the previous attempt failed
        // or hung. Kill midisrv.exe to reset stale state before retrying.
        // teVirtualMIDI is NOT available on Windows 11 — we must use MIDI Services.
        if sentinel.exists()
            || std::env::var("MIDINET_SKIP_MIDI_SERVICES").as_deref() == Ok("1")
        {
            warn!(
                name = %identity.name,
                "Previous MIDI Services attempt failed — killing midisrv.exe to reset state"
            );
            kill_midi_services();
            let _ = std::fs::remove_file(&sentinel);
            // Wait for Service Control Manager to restart midisrv.exe
            std::thread::sleep(std::time::Duration::from_secs(3));
        }

        info!(name = %identity.name, "Windows 11 — attempting Windows MIDI Services backend...");

        // Write crash sentinel BEFORE attempting creation.
        // Deleted on success or recoverable error. If we crash or hang, it persists.
        let _ = std::fs::write(&sentinel, b"crash during MIDI Services init");

        let mut ms_device = MidiServicesDevice::new();
        match ms_device.create(identity) {
            Ok(()) => {
                let _ = std::fs::remove_file(&sentinel);
                info!(name = %identity.name, "Using Windows MIDI Services backend");
                self.backend = Backend::MidiServices(ms_device);
                Ok(())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&sentinel);
                error!(
                    name = %identity.name,
                    error = %e,
                    "Windows MIDI Services failed. Options:\n\
                     1. Restart MIDI Services: taskkill /F /IM midisrv.exe (then retry)\n\
                     2. Install Windows MIDI Services SDK: winget install Microsoft.WindowsMIDIServicesSDK"
                );
                Err(anyhow::anyhow!(
                    "Windows MIDI Services failed. \
                     MIDI Services may need to be restarted (taskkill /F /IM midisrv.exe)."
                ))
            }
        }
    }

    /// Windows 10 and below: teVirtualMIDI only (no MIDI Services available).
    fn create_win10(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        info!(name = %identity.name, "Attempting teVirtualMIDI backend...");
        let mut te_device = TeVirtualMidiDevice::new();
        match te_device.create(identity) {
            Ok(()) => {
                info!(name = %identity.name, "Using teVirtualMIDI backend");
                self.backend = Backend::TeVirtualMidi(te_device);
                Ok(())
            }
            Err(e) => {
                error!(
                    name = %identity.name,
                    error = %e,
                    "teVirtualMIDI unavailable. \
                     Install the driver from: \
                     https://www.tobias-erichsen.de/software/virtualmidi.html"
                );
                Err(anyhow::anyhow!(
                    "Virtual MIDI device creation failed. \
                     Install teVirtualMIDI from https://www.tobias-erichsen.de/software/virtualmidi.html"
                ))
            }
        }
    }
}

impl VirtualMidiDevice for WindowsVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        if is_windows_11() {
            // Windows 11+: prefer MIDI Services (native, no driver needed)
            self.create_win11(identity)
        } else {
            // Windows 10 and below: teVirtualMIDI only
            self.create_win10(identity)
        }
    }

    fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        match &self.backend {
            Backend::TeVirtualMidi(dev) => dev.send(data),
            Backend::MidiServices(dev) => dev.send(data),
            Backend::Uninit => Ok(()),
        }
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        match &self.backend {
            Backend::TeVirtualMidi(dev) => dev.receive(),
            Backend::MidiServices(dev) => dev.receive(),
            Backend::Uninit => Ok(None),
        }
    }

    fn close(&mut self) -> anyhow::Result<()> {
        match &mut self.backend {
            Backend::TeVirtualMidi(dev) => dev.close(),
            Backend::MidiServices(dev) => dev.close(),
            Backend::Uninit => Ok(()),
        }
    }

    fn silence_and_detach(&mut self) -> anyhow::Result<()> {
        match &mut self.backend {
            Backend::TeVirtualMidi(dev) => dev.silence_and_detach(),
            Backend::MidiServices(dev) => dev.silence_and_detach(),
            Backend::Uninit => Ok(()),
        }
    }

    fn device_name(&self) -> &str {
        match &self.backend {
            Backend::TeVirtualMidi(dev) => dev.device_name(),
            Backend::MidiServices(dev) => dev.device_name(),
            Backend::Uninit => "",
        }
    }
}

impl Drop for WindowsVirtualDevice {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
