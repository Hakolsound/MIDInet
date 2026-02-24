/// Virtual MIDI device abstraction.
/// Creates a platform-specific virtual MIDI port that mimics the identity
/// of the physical controller connected to the host.

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
}

/// Create a platform-appropriate virtual MIDI device.
pub fn create_virtual_device() -> Box<dyn VirtualMidiDevice> {
    #[cfg(target_os = "linux")]
    {
        Box::new(crate::platform::linux::AlsaVirtualDevice::new())
    }

    #[cfg(target_os = "macos")]
    {
        Box::new(crate::platform::macos::CoreMidiVirtualDevice::new())
    }

    #[cfg(target_os = "windows")]
    {
        Box::new(crate::platform::windows::WindowsVirtualDevice::new())
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
