/// Virtual MIDI device abstraction — re-exports from the `midi-device` crate.
///
/// Platform-specific implementations live in `midi-device` so they can be
/// shared with `midi-bridge`. This module provides the bridge adapter and
/// the auto-detecting factory that chooses between bridge mode and direct mode.

pub use midi_device::{VirtualMidiDevice, create_virtual_device};

mod bridge;
pub use bridge::BridgeVirtualDevice;

/// Well-known bridge socket paths per platform.
#[cfg(unix)]
pub const BRIDGE_SOCKET_PATH: &str = "/tmp/midinet-bridge.sock";
#[cfg(windows)]
pub const BRIDGE_SOCKET_PATH: &str = r"\\.\pipe\midinet-bridge";

/// Check if the bridge process is available.
pub fn bridge_available() -> bool {
    #[cfg(unix)]
    {
        std::path::Path::new(BRIDGE_SOCKET_PATH).exists()
    }
    #[cfg(windows)]
    {
        // On Windows, named pipes don't show up in the filesystem.
        // Try a quick connect to check availability.
        use std::fs::OpenOptions;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(BRIDGE_SOCKET_PATH)
            .is_ok()
    }
}

/// Create the best available virtual MIDI device.
///
/// If a bridge process is running (socket exists), returns a `BridgeVirtualDevice`
/// that proxies through the bridge — the device survives client restarts.
/// Otherwise falls back to a direct platform-native device.
pub fn create_device() -> Box<dyn VirtualMidiDevice> {
    if bridge_available() {
        tracing::info!("Bridge detected at {}, using bridge mode (device survives restarts)", BRIDGE_SOCKET_PATH);
        Box::new(BridgeVirtualDevice::new(BRIDGE_SOCKET_PATH))
    } else {
        tracing::info!("No bridge detected, using direct device mode");
        create_virtual_device()
    }
}
