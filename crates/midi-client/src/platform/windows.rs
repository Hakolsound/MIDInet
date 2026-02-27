/// Windows virtual MIDI device — orchestrator with dual-backend fallback.
///
/// Strategy (Windows 11+):
/// 1. Try Windows MIDI Services first (native, no driver install needed)
/// 2. If that fails → try teVirtualMIDI (requires driver install)
///
/// Strategy (Windows 10 and below):
/// 1. Try teVirtualMIDI (requires driver install)
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
    /// Windows 11+: try MIDI Services first, fall back to teVirtualMIDI.
    fn create_win11(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        info!(name = %identity.name, "Windows 11 — attempting Windows MIDI Services backend...");
        let mut ms_device = MidiServicesDevice::new();
        match ms_device.create(identity) {
            Ok(()) => {
                info!(name = %identity.name, "Using Windows MIDI Services backend");
                self.backend = Backend::MidiServices(ms_device);
                return Ok(());
            }
            Err(e) => {
                warn!(
                    name = %identity.name,
                    error = %e,
                    "Windows MIDI Services failed, trying teVirtualMIDI..."
                );
            }
        }

        // Fallback: teVirtualMIDI
        let mut te_device = TeVirtualMidiDevice::new();
        match te_device.create(identity) {
            Ok(()) => {
                info!(name = %identity.name, "Using teVirtualMIDI backend (fallback)");
                self.backend = Backend::TeVirtualMidi(te_device);
                Ok(())
            }
            Err(e) => {
                error!(
                    name = %identity.name,
                    error = %e,
                    "Both backends failed. Options:\n\
                     1. Install Windows MIDI Services: winget install Microsoft.WindowsMIDIServicesSDK\n\
                     2. Install teVirtualMIDI: https://www.tobias-erichsen.de/software/virtualmidi.html"
                );
                Err(anyhow::anyhow!(
                    "No virtual MIDI backend available. \
                     Install Windows MIDI Services SDK or teVirtualMIDI."
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
