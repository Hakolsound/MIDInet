/// USB MIDI reader using ALSA on Linux.
/// Reads raw MIDI data from a physical USB MIDI controller and pushes it
/// into the lock-free ring buffer for zero-alloc forwarding to the broadcaster.
///
/// Includes a supervised retry loop for hot-plug reconnection: when a device
/// disconnects, the reader retries with exponential backoff until it comes
/// back online (or the task is cancelled).
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
    /// Reader is retrying connection after failure
    Reconnecting,
}

/// Backoff configuration for hot-plug reconnection.
#[cfg(target_os = "linux")]
const RETRY_INITIAL_MS: u64 = 100;
#[cfg(target_os = "linux")]
const RETRY_MAX_MS: u64 = 5000;
#[cfg(target_os = "linux")]
const RETRY_MULTIPLIER: f64 = 2.0;

#[cfg(target_os = "linux")]
pub mod platform {
    use alsa::rawmidi::Rawmidi;
    use alsa::Direction;
    use midi_protocol::ringbuf::MidiProducer;
    use std::ffi::CString;
    use std::io::Read;
    use tokio::sync::mpsc;
    use tracing::{debug, error, info, warn};

    use super::{InputHealth, RETRY_INITIAL_MS, RETRY_MAX_MS, RETRY_MULTIPLIER};

    /// Supervised MIDI reader with hot-plug reconnection.
    ///
    /// The entire retry loop runs inside a single `spawn_blocking` call to
    /// avoid repeated producer moves across the async/blocking boundary.
    /// On disconnect or read error, reports `InputHealth::Reconnecting`, then
    /// retries with exponential backoff (100ms → 200ms → ... → 5s cap).
    /// On successful reconnect, resets backoff and reports `InputHealth::Active`.
    ///
    /// This function only returns if the health channel closes (task cancelled).
    pub async fn run_midi_reader(
        device: &str,
        producer: MidiProducer,
        health_tx: mpsc::Sender<InputHealth>,
    ) -> anyhow::Result<()> {
        let device_owned = device.to_string();

        tokio::task::spawn_blocking(move || {
            let mut backoff_ms = RETRY_INITIAL_MS;

            loop {
                // If our supervisor is gone, exit cleanly
                if health_tx.is_closed() {
                    info!(device = %device_owned, "Health channel closed — reader exiting");
                    return Ok(());
                }

                let device_cstr = match CString::new(device_owned.as_str()) {
                    Ok(c) => c,
                    Err(e) => return Err(anyhow::anyhow!("Invalid device name: {}", e)),
                };

                // --- Try to open the device ---
                let rawmidi = match Rawmidi::open(&device_cstr, Direction::Capture, false) {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = format!("Failed to open '{}': {}", device_owned, e);
                        debug!(device = %device_owned, "Device not available: {}", e);
                        let _ = health_tx.blocking_send(InputHealth::Disconnected(msg));
                        let _ = health_tx.blocking_send(InputHealth::Reconnecting);

                        // Backoff and retry
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                        backoff_ms = ((backoff_ms as f64 * RETRY_MULTIPLIER) as u64)
                            .min(RETRY_MAX_MS);
                        continue;
                    }
                };

                // --- Device opened successfully ---
                info!(device = %device_owned, "MIDI device opened for reading");
                let _ = health_tx.blocking_send(InputHealth::Active);
                backoff_ms = RETRY_INITIAL_MS; // Reset backoff on success

                let mut buf = [0u8; 256];

                // --- Read loop ---
                let read_err = loop {
                    if health_tx.is_closed() {
                        info!(device = %device_owned, "Health channel closed — reader exiting");
                        return Ok(());
                    }

                    match rawmidi.io().read(&mut buf) {
                        Ok(n) if n > 0 => {
                            debug!(bytes = n, "Read MIDI data");
                            producer.push_overwrite(&buf[..n]);
                        }
                        Ok(_) => {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        Err(e) => {
                            break e;
                        }
                    }
                };

                // --- Read error — prepare to retry ---
                let msg = format!("Read error on '{}': {}", device_owned, read_err);
                error!(device = %device_owned, "MIDI read error: {}", read_err);
                let _ = health_tx.blocking_send(InputHealth::Error(msg));

                warn!(
                    device = %device_owned,
                    backoff_ms = backoff_ms,
                    "Will retry in {}ms", backoff_ms
                );
                let _ = health_tx.blocking_send(InputHealth::Reconnecting);

                // Drop rawmidi before sleeping (close the ALSA handle)
                drop(rawmidi);

                std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                backoff_ms = ((backoff_ms as f64 * RETRY_MULTIPLIER) as u64).min(RETRY_MAX_MS);
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
