/// USB MIDI reader using ALSA on Linux.
/// Reads raw MIDI data from a physical USB MIDI controller and pushes it
/// into the lock-free ring buffer for zero-alloc forwarding to the broadcaster.
///
/// On non-Linux platforms, this module provides a stub implementation.

/// Health status reported by a MIDI input reader.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum InputHealth {
    /// Device opened and reading successfully
    Active,
    /// ALSA read error — device may be malfunctioning
    Error(String),
    /// Device could not be opened — disconnected or unavailable
    Disconnected(String),
}

#[cfg(target_os = "linux")]
pub mod platform {
    use alsa::rawmidi::Rawmidi;
    use alsa::Direction;
    use midi_protocol::ringbuf::MidiProducer;
    use std::ffi::CString;
    use tokio::sync::mpsc;
    use tracing::{debug, error, info};

    use super::InputHealth;

    /// Open an ALSA rawmidi device and read MIDI data, pushing into the ring buffer.
    /// Reports health events via the provided channel. Returns on error —
    /// the InputMux handles failover to the secondary controller.
    pub async fn run_midi_reader(
        device: &str,
        producer: MidiProducer,
        health_tx: mpsc::Sender<InputHealth>,
    ) -> anyhow::Result<()> {
        let device_cstr = CString::new(device)?;

        let device_str = device.to_string();
        tokio::task::spawn_blocking(move || {
            let rawmidi = match Rawmidi::open(&device_cstr, Direction::Capture, false) {
                Ok(r) => r,
                Err(e) => {
                    error!(device = %device_str, "Failed to open MIDI device: {}", e);
                    let _ = health_tx.blocking_send(InputHealth::Disconnected(format!(
                        "Failed to open '{}': {}", device_str, e
                    )));
                    return Err(anyhow::anyhow!("Failed to open MIDI device '{}': {}", device_str, e));
                }
            };

            info!(device = %device_str, "MIDI device opened for reading");
            let _ = health_tx.blocking_send(InputHealth::Active);

            let mut buf = [0u8; 256];

            loop {
                match rawmidi.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        debug!(bytes = n, "Read MIDI data");
                        producer.push_overwrite(&buf[..n]);
                    }
                    Ok(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(e) => {
                        error!(device = %device_str, "MIDI read error: {}", e);
                        let _ = health_tx.blocking_send(InputHealth::Error(format!(
                            "Read error on '{}': {}", device_str, e
                        )));
                        return Err(anyhow::anyhow!("MIDI read error on '{}': {}", device_str, e));
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
    use tokio::sync::mpsc;
    use tracing::warn;

    use super::InputHealth;

    /// Stub MIDI reader for non-Linux platforms.
    pub async fn run_midi_reader(
        device: &str,
        _producer: MidiProducer,
        health_tx: mpsc::Sender<InputHealth>,
    ) -> anyhow::Result<()> {
        warn!(
            device = %device,
            "MIDI reader not implemented on this platform (Linux only for host)"
        );

        let _ = health_tx.send(InputHealth::Disconnected(
            "Not supported on this platform".to_string(),
        )).await;

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
}
