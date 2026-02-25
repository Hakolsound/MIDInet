/// Synthetic MIDI load test generator.
///
/// Sends test MIDI packets via UDP multicast using the same wire format
/// as the host broadcaster.  Clients receive and forward them normally.
/// Latency is measured client-side using the `timestamp_us` field.
///
/// Test packets use `host_id = 254` so clients can distinguish them
/// from real host traffic (host IDs 1/2).

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use midi_protocol::packets::MidiDataPacket;

use crate::state::AppStateInner;

/// Reserved host ID for test traffic.
pub const TEST_HOST_ID: u8 = 254;

/// Test profile controls the type and rate of synthetic MIDI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestProfile {
    /// CC#1 sweep on channel 1, ~10 msg/sec
    Gentle,
    /// CC sweeps + Note On/Off across channels 1-4, ~50 msg/sec
    Normal,
    /// All 16 channels, CC + notes + pitch bend, ~200 msg/sec
    Stress,
    /// Linear ramp from Gentle rate to Stress rate
    Ramp,
}

impl TestProfile {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "gentle" => Self::Gentle,
            "normal" => Self::Normal,
            "stress" => Self::Stress,
            "ramp" => Self::Ramp,
            _ => Self::Gentle,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Gentle => "gentle",
            Self::Normal => "normal",
            Self::Stress => "stress",
            Self::Ramp => "ramp",
        }
    }
}

/// Configuration for a test run.
pub struct TestConfig {
    pub profile: TestProfile,
    /// 0 = run until stopped
    pub duration_secs: u32,
    /// Ramp profile: seconds over which to increase from Gentle to Stress
    pub ramp_duration_secs: u32,
    /// Multicast group to send on
    pub multicast_group: String,
    /// Multicast data port
    pub data_port: u16,
}

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Run the test generator.  Sends synthetic MIDI packets at the configured
/// rate until cancelled or duration expires.
pub async fn run(
    state: Arc<AppStateInner>,
    config: TestConfig,
    cancel: CancellationToken,
) {
    let multicast_addr: Ipv4Addr = match config.multicast_group.parse() {
        Ok(addr) => addr,
        Err(e) => {
            error!("Invalid multicast group '{}': {}", config.multicast_group, e);
            return;
        }
    };

    // Create multicast sender socket
    let socket = match create_multicast_sender(multicast_addr) {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to create multicast socket: {}", e);
            return;
        }
    };

    let dest = SocketAddrV4::new(multicast_addr, config.data_port);
    let mut sequence: u16 = 0;
    let mut send_buf = Vec::with_capacity(256);
    let start = std::time::Instant::now();

    // Mark test as running
    {
        let mut ts = state.test_state.write().await;
        ts.running = true;
        ts.profile = config.profile.as_str().to_string();
        ts.started_at = Some(now_ms());
        ts.packets_sent = 0;
        ts.duration_secs = config.duration_secs;
        ts.client_snapshots.clear();
    }
    state.test_packets_sent.store(0, Ordering::Relaxed);

    info!(
        profile = config.profile.as_str(),
        duration = config.duration_secs,
        multicast = %multicast_addr,
        port = config.data_port,
        "Test generator started"
    );

    // Pattern state
    let mut cc_value: u8 = 0;
    let mut cc_direction: bool = true; // true = ascending
    let mut note_index: u8 = 0;
    let mut channel_index: u8 = 0;
    let mut tick: u64 = 0;
    let mut last_sample = std::time::Instant::now();

    loop {
        if cancel.is_cancelled() {
            break;
        }

        // Check duration limit
        if config.duration_secs > 0 && start.elapsed().as_secs() >= config.duration_secs as u64 {
            break;
        }

        // Compute current rate based on profile
        let elapsed_secs = start.elapsed().as_secs_f32();
        let (interval_us, channels, send_notes, send_pitchbend) =
            profile_params(&config, elapsed_secs);

        // Generate MIDI data
        let midi_data = generate_midi(
            &mut cc_value,
            &mut cc_direction,
            &mut note_index,
            &mut channel_index,
            channels,
            send_notes,
            send_pitchbend,
            tick,
        );

        let packet = MidiDataPacket {
            sequence,
            timestamp_us: now_us(),
            host_id: TEST_HOST_ID,
            midi_data,
            journal: None,
        };

        packet.serialize(&mut send_buf);

        if let Err(e) = socket.send_to(&send_buf, dest).await {
            error!("Test send error: {}", e);
        }

        state.test_packets_sent.fetch_add(1, Ordering::Relaxed);
        sequence = sequence.wrapping_add(1);
        tick += 1;

        // Sample client metrics every second for min/max/avg tracking
        if last_sample.elapsed() >= std::time::Duration::from_secs(1) {
            last_sample = std::time::Instant::now();
            let clients = state.clients.read().await;
            let mut ts = state.test_state.write().await;
            for c in clients.iter() {
                let snap = ts.client_snapshots.entry(c.id).or_insert_with(|| {
                    crate::state::ClientTestSnapshot {
                        hostname: c.hostname.clone(),
                        latency_min_ms: c.latency_ms,
                        latency_max_ms: c.latency_ms,
                        latency_avg_ms: c.latency_ms,
                        latency_samples: 0,
                        packet_loss_percent: c.packet_loss_percent,
                    }
                });
                snap.hostname = c.hostname.clone();
                if c.latency_ms > 0.0 {
                    snap.latency_min_ms = snap.latency_min_ms.min(c.latency_ms);
                    snap.latency_max_ms = snap.latency_max_ms.max(c.latency_ms);
                    let n = snap.latency_samples as f32;
                    snap.latency_avg_ms = (snap.latency_avg_ms * n + c.latency_ms) / (n + 1.0);
                    snap.latency_samples += 1;
                }
                snap.packet_loss_percent = c.packet_loss_percent;
            }
        }

        // Sleep for the computed interval
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_micros(interval_us)) => {}
            _ = cancel.cancelled() => break,
        }
    }

    // Finalize test state
    let total = state.test_packets_sent.load(Ordering::Relaxed);
    {
        let mut ts = state.test_state.write().await;
        ts.running = false;
        ts.packets_sent = total;
    }

    info!(packets_sent = total, "Test generator stopped");
}

/// Returns (interval_us, num_channels, send_notes, send_pitchbend) for the current moment.
fn profile_params(config: &TestConfig, elapsed_secs: f32) -> (u64, u8, bool, bool) {
    match config.profile {
        TestProfile::Gentle => {
            // ~10 msg/sec = 100ms interval
            (100_000, 1, false, false)
        }
        TestProfile::Normal => {
            // ~50 msg/sec = 20ms interval
            (20_000, 4, true, false)
        }
        TestProfile::Stress => {
            // ~200 msg/sec = 5ms interval
            (5_000, 16, true, true)
        }
        TestProfile::Ramp => {
            let ramp = config.ramp_duration_secs.max(1) as f32;
            let t = (elapsed_secs / ramp).min(1.0); // 0.0 → 1.0
            // Interpolate from Gentle (100ms) to Stress (5ms)
            let interval = 100_000.0 - (95_000.0 * t);
            let channels = 1 + (15.0 * t) as u8;
            let send_notes = t > 0.3;
            let send_pitchbend = t > 0.7;
            (interval as u64, channels, send_notes, send_pitchbend)
        }
    }
}

/// Generate a batch of synthetic MIDI bytes.
fn generate_midi(
    cc_value: &mut u8,
    cc_direction: &mut bool,
    note_index: &mut u8,
    channel_index: &mut u8,
    channels: u8,
    send_notes: bool,
    send_pitchbend: bool,
    tick: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(16);
    let ch = *channel_index % channels;

    // CC#1 (mod wheel) sweep
    data.push(0xB0 | ch); // Control Change
    data.push(0x01);       // CC#1
    data.push(*cc_value);

    // Update CC sweep
    if *cc_direction {
        if *cc_value >= 127 {
            *cc_direction = false;
        } else {
            *cc_value += 1;
        }
    } else {
        if *cc_value == 0 {
            *cc_direction = true;
        } else {
            *cc_value -= 1;
        }
    }

    // Note On / Note Off pairs
    if send_notes && tick % 5 == 0 {
        let note = 48 + (*note_index % 24); // C3 to B4
        // Note On
        data.push(0x90 | ch);
        data.push(note);
        data.push(100); // velocity
        // Note Off (short note)
        data.push(0x80 | ch);
        data.push(note);
        data.push(0);
        *note_index = note_index.wrapping_add(1);
    }

    // Pitch bend sweep
    if send_pitchbend && tick % 3 == 0 {
        let bend = ((tick % 128) as u16) << 7;
        data.push(0xE0 | ch); // Pitch Bend
        data.push((bend & 0x7F) as u8);
        data.push(((bend >> 7) & 0x7F) as u8);
    }

    *channel_index = channel_index.wrapping_add(1);

    data
}

fn create_multicast_sender(_multicast_addr: Ipv4Addr) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_multicast_if_v4(&Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.set_multicast_loop_v4(true)?;

    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;

    let std_socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(std_socket)
}
