/// Virtual MIDI device abstraction and platform-specific implementations.
///
/// This crate provides the `VirtualMidiDevice` trait and implementations for
/// macOS (CoreMIDI), Linux (ALSA), and Windows (teVirtualMIDI / MIDI Services).
///
/// Extracted into a shared crate so both `midi-client` and `midi-bridge` can
/// create and manage virtual MIDI devices.

pub mod platform;

use midi_protocol::identity::DeviceIdentity;

/// Trait for platform-specific virtual MIDI device implementations.
pub trait VirtualMidiDevice: Send + Sync {
    /// Create the virtual MIDI device with the given identity.
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()>;

    /// Send MIDI data out through the virtual device (host → client app).
    fn send(&self, data: &[u8]) -> anyhow::Result<()>;

    /// Receive MIDI data from the client app (for bidirectional feedback).
    /// Returns None if no data available.
    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>>;

    /// Close the virtual device.
    fn close(&mut self) -> anyhow::Result<()>;

    /// Get the device name as seen by the host application.
    fn device_name(&self) -> &str;

    /// Graceful shutdown: send All Sound Off + All Notes Off on all channels,
    /// then detach the device handle so it persists until the process exits.
    ///
    /// This prevents crashes in applications (like Resolume Arena) that hold
    /// open handles to the virtual MIDI port. The OS will clean up the handles
    /// when the process terminates, which MIDI drivers handle gracefully —
    /// unlike explicit close() which can trigger bugs in Windows MIDI Services.
    fn silence_and_detach(&mut self) -> anyhow::Result<()> {
        // Default: send silence then close normally (safe on macOS/Linux)
        self.send_all_off()?;
        self.close()
    }

    /// Send All Sound Off (CC 120) + All Notes Off (CC 123) on all 16 channels.
    fn send_all_off(&self) -> anyhow::Result<()> {
        for ch in 0u8..16 {
            let status = 0xB0 | ch;
            // CC 120 = All Sound Off
            self.send(&[status, 120, 0])?;
            // CC 123 = All Notes Off
            self.send(&[status, 123, 0])?;
        }
        Ok(())
    }
}

/// Create a platform-appropriate virtual MIDI device.
pub fn create_virtual_device() -> Box<dyn VirtualMidiDevice> {
    #[cfg(target_os = "linux")]
    {
        Box::new(platform::linux::AlsaVirtualDevice::new())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(platform::macos::CoreMidiVirtualDevice::new())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(platform::windows::WindowsVirtualDevice::new())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Box::new(StubVirtualDevice::new())
    }
}

/// Stub implementation for unsupported platforms
struct StubVirtualDevice {
    name: String,
}

#[allow(dead_code)]
impl StubVirtualDevice {
    fn new() -> Self {
        Self {
            name: String::new(),
        }
    }
}

impl VirtualMidiDevice for StubVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();
        tracing::warn!("Virtual MIDI device not supported on this platform");
        Ok(())
    }

    fn send(&self, _data: &[u8]) -> anyhow::Result<()> {
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(None)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        Ok(())
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}
