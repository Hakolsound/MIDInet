/// MIDI output writer for bidirectional feedback to physical controllers.
///
/// Writes MIDI data to one or more ALSA rawmidi devices in playback mode.
/// In dual-controller mode (single/redundant), feedback is sent to ALL connected
/// controllers simultaneously so LED state, displays, and motorized faders stay
/// in sync regardless of which controller is currently active for input.
///
/// In multi-device mode, `write_to_device(device_id, data)` routes feedback to
/// the specific controller matching that device_id.

#[cfg(target_os = "linux")]
pub mod platform {
    use alsa::rawmidi::Rawmidi;
    use alsa::Direction;
    use std::ffi::CString;
    use std::io::Write;
    use tracing::{debug, error, info, warn};

    /// A handle to one or more ALSA rawmidi output devices.
    /// Writes are broadcast to all open devices.
    pub struct MidiOutputWriter {
        devices: Vec<MidiOutputDevice>,
    }

    struct MidiOutputDevice {
        name: String,
        /// device_id for multi-device routing (index in the devices list)
        device_id: u8,
        rawmidi: Rawmidi,
    }

    impl MidiOutputWriter {
        /// Open MIDI output devices for single/redundant mode.
        /// All devices receive the same feedback (broadcast).
        pub fn open(device_names: &[&str]) -> Self {
            let mut devices = Vec::new();

            for (idx, &name) in device_names.iter().enumerate() {
                if name.is_empty() {
                    continue;
                }

                match CString::new(name) {
                    Ok(cstr) => match Rawmidi::open(&cstr, Direction::Playback, false) {
                        Ok(rawmidi) => {
                            info!(device = %name, device_id = idx, "MIDI output device opened");
                            devices.push(MidiOutputDevice {
                                name: name.to_string(),
                                device_id: idx as u8,
                                rawmidi,
                            });
                        }
                        Err(e) => {
                            warn!(device = %name, "Failed to open MIDI output device: {}", e);
                        }
                    },
                    Err(e) => {
                        error!(device = %name, "Invalid device name: {}", e);
                    }
                }
            }

            if devices.is_empty() {
                warn!("No MIDI output devices available — feedback will be dropped");
            }

            Self { devices }
        }

        /// Write MIDI data to all open output devices.
        /// Used in single/redundant mode where all controllers mirror the same state.
        pub fn write_all(&self, data: &[u8]) {
            for dev in &self.devices {
                match dev.rawmidi.io().write(data) {
                    Ok(n) => {
                        debug!(device = %dev.name, bytes = n, "Wrote MIDI feedback");
                    }
                    Err(e) => {
                        error!(device = %dev.name, "MIDI output write error: {}", e);
                    }
                }
            }
        }

        /// Write MIDI data to a specific device by device_id.
        /// Used in multi-device mode to route feedback to the correct controller.
        /// Falls back to write_all if device_id is not found (single-device compat).
        pub fn write_to_device(&self, device_id: u8, data: &[u8]) {
            if let Some(dev) = self.devices.iter().find(|d| d.device_id == device_id) {
                match dev.rawmidi.io().write(data) {
                    Ok(n) => {
                        debug!(device = %dev.name, device_id, bytes = n, "Wrote MIDI feedback to device");
                    }
                    Err(e) => {
                        error!(device = %dev.name, device_id, "MIDI output write error: {}", e);
                    }
                }
            } else {
                // No device with matching ID — fall back to broadcast
                self.write_all(data);
            }
        }

        /// Number of open output devices.
        pub fn device_count(&self) -> usize {
            self.devices.len()
        }
    }

    // SAFETY: ALSA rawmidi handles are file-descriptor based and safe to send
    // across threads. The alsa crate doesn't impl Send/Sync because the raw
    // pointer isn't automatically Send, but the underlying fd is thread-safe.
    unsafe impl Send for MidiOutputWriter {}
    unsafe impl Sync for MidiOutputWriter {}
}

#[cfg(not(target_os = "linux"))]
pub mod platform {
    use tracing::warn;

    /// Stub MIDI output writer for non-Linux platforms.
    pub struct MidiOutputWriter;

    impl MidiOutputWriter {
        pub fn open(device_names: &[&str]) -> Self {
            if !device_names.is_empty() {
                warn!("MIDI output not supported on this platform (Linux only)");
            }
            Self
        }

        pub fn write_all(&self, _data: &[u8]) {}

        pub fn write_to_device(&self, _device_id: u8, _data: &[u8]) {}

        pub fn device_count(&self) -> usize {
            0
        }
    }
}
