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

/// Create and initialize a virtual MIDI device on a blocking thread with timeout.
///
/// This prevents Windows MIDI Services COM calls from blocking the tokio async
/// runtime. If `MidiSession::Create()` or `CreateVirtualDevice()` hangs (due to
/// stale midisrv.exe state), the timeout fires, we kill midisrv.exe to unblock
/// the hung COM call and reset state, then retry with a fresh MIDI Services session.
pub async fn create_virtual_device_async(
    identity: &DeviceIdentity,
) -> (Box<dyn VirtualMidiDevice>, bool) {
    let id = identity.clone();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            let mut dev = create_virtual_device();
            dev.create(&id).map(|_| dev)
        }),
    )
    .await;

    match result {
        Ok(Ok(Ok(device))) => return (device, true),
        Ok(Ok(Err(e))) => {
            tracing::error!(device = %identity.name, "Failed to create virtual device: {}", e);
            return (create_virtual_device(), false);
        }
        Ok(Err(e)) => {
            tracing::error!(device = %identity.name, "Virtual device creation panicked: {}", e);
            return (create_virtual_device(), false);
        }
        Err(_) => {
            tracing::warn!(
                device = %identity.name,
                "Virtual device creation timed out (15s) — MIDI Services COM call is hung"
            );

            // Kill midisrv.exe to unblock the hung COM call and reset MIDI Services.
            // Also remove the crash sentinel so the retry goes straight to MIDI Services
            // (not the kill-and-wait path which would add another 3s delay).
            #[cfg(target_os = "windows")]
            {
                crate::platform::windows::kill_midi_services();
                let sentinel = crate::platform::windows::WindowsVirtualDevice::crash_sentinel();
                let _ = std::fs::remove_file(&sentinel);
            }

            // Wait for Service Control Manager to restart midisrv.exe
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        }
    }

    // Retry with fresh midisrv.exe. On Windows 11 this retries MIDI Services
    // (the only viable backend). The killed midisrv.exe should have been
    // auto-restarted by SCM during the 3s wait above.
    let id = identity.clone();
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            let mut dev = create_virtual_device();
            dev.create(&id).map(|_| dev)
        }),
    )
    .await
    {
        Ok(Ok(Ok(device))) => {
            tracing::info!(device = %identity.name, "Virtual device created on retry (after midisrv.exe restart)");
            (device, true)
        }
        Ok(Ok(Err(e))) => {
            tracing::error!(device = %identity.name, "Virtual device creation failed on retry: {}", e);
            (create_virtual_device(), false)
        }
        _ => {
            tracing::error!(device = %identity.name, "Virtual device creation failed on retry (panic or timeout)");
            (create_virtual_device(), false)
        }
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
