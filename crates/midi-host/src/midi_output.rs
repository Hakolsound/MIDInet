/// MIDI output writer for bidirectional feedback to physical controllers.
///
/// Writes MIDI data to one or more ALSA rawmidi devices in playback mode.
/// In dual-controller mode, feedback is sent to ALL connected controllers
/// simultaneously so LED state, displays, and motorized faders stay in sync
/// regardless of which controller is currently active for input.

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
        rawmidi: Rawmidi,
    }

    impl MidiOutputWriter {
        /// Open MIDI output devices. Devices that fail to open are logged
        /// and skipped — the writer continues with whatever devices are available.
        pub fn open(device_names: &[&str]) -> Self {
            let mut devices = Vec::new();

            for &name in device_names {
                if name.is_empty() {
                    continue;
                }

                match CString::new(name) {
                    Ok(cstr) => match Rawmidi::open(&cstr, Direction::Playback, false) {
                        Ok(rawmidi) => {
                            info!(device = %name, "MIDI output device opened");
                            devices.push(MidiOutputDevice {
                                name: name.to_string(),
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
        /// Errors on individual devices are logged but do not stop output to others.
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

        pub fn device_count(&self) -> usize {
            0
        }
    }
}
