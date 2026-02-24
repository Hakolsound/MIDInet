/// USB MIDI reader using ALSA on Linux.
/// Reads raw MIDI data from a physical USB MIDI controller and pushes it
/// into the lock-free ring buffer for zero-alloc forwarding to the broadcaster.
///
/// On non-Linux platforms, this module provides a stub implementation.

#[cfg(target_os = "linux")]
pub mod platform {
    use alsa::rawmidi::Rawmidi;
    use alsa::Direction;
    use midi_protocol::ringbuf::MidiProducer;
    use std::ffi::CString;
    use tracing::{debug, error, info, warn};

    /// Open an ALSA rawmidi device and read MIDI data, pushing into the ring buffer.
    pub async fn run_midi_reader(
        device: &str,
        producer: MidiProducer,
    ) -> anyhow::Result<()> {
        let device_cstr = CString::new(device)?;

        // Spawn blocking read on a dedicated thread (ALSA rawmidi is blocking)
        let device_str = device.to_string();
        tokio::task::spawn_blocking(move || {
            let rawmidi = Rawmidi::open(&device_cstr, Direction::Capture, false).map_err(|e| {
                error!(device = %device_str, "Failed to open MIDI device: {}", e);
                anyhow::anyhow!("Failed to open MIDI device '{}': {}", device_str, e)
            })?;

            info!(device = %device_str, "MIDI device opened for reading");

            let mut buf = [0u8; 256];

            loop {
                match rawmidi.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        debug!(bytes = n, "Read MIDI data");
                        // Push into lock-free ring buffer — zero allocation
                        producer.push_overwrite(&buf[..n]);
                    }
                    Ok(_) => {
                        // Zero bytes read, brief pause
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(e) => {
                        error!("MIDI read error: {}", e);
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
        })
        .await?
    }
}

#[cfg(not(target_os = "linux"))]
pub mod platform {
    use midi_protocol::ringbuf::MidiProducer;
    use tracing::warn;

    /// Stub MIDI reader for non-Linux platforms.
    pub async fn run_midi_reader(
        device: &str,
        _producer: MidiProducer,
    ) -> anyhow::Result<()> {
        warn!(
            device = %device,
            "MIDI reader not implemented on this platform (Linux only for host)"
        );

        // Keep the task alive
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
}
