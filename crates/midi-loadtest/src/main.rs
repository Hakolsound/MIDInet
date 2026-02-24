/// MIDInet Load Test & Latency Measurement Suite
///
/// Real-world QA tests that exercise the actual UDP multicast path,
/// packet serialization, heartbeat timing, and failover behavior.
///
/// Usage:
///   midi-loadtest latency              Measure send→receive latency over loopback multicast
///   midi-loadtest throughput            Saturate the link and measure max sustained throughput
///   midi-loadtest burst                 Send realistic MIDI burst patterns (drum rolls, chord stabs)
///   midi-loadtest heartbeat             Verify heartbeat timing accuracy at 3ms intervals
///   midi-loadtest failover              Simulate primary failure and measure failover time
///   midi-loadtest soak                  Long-duration soak test (packet loss, jitter, memory)
///   midi-loadtest pipeline              Benchmark pipeline processing throughput
///   midi-loadtest journal               Benchmark journal encode/decode + state reconciliation
///   midi-loadtest all                   Run all tests sequentially with a final report

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use midi_protocol::journal::{decode_journal, encode_journal};
use midi_protocol::midi_state::MidiState;
use midi_protocol::packets::{HeartbeatPacket, HostRole, MidiDataPacket};
use midi_protocol::pipeline::PipelineConfig;

// ── Test Configuration ───────────────────────────────────────

/// Dedicated test multicast group (avoids interfering with live traffic)
const TEST_MCAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 69, 83, 250);
const TEST_DATA_PORT: u16 = 15004;
const TEST_HB_PORT: u16 = 15005;

fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

// ── CLI ──────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "midi-loadtest", about = "MIDInet load test and latency measurement suite")]
struct Args {
    #[command(subcommand)]
    command: Command,

    /// Network interface to bind to
    #[arg(short, long, default_value = "0.0.0.0", global = true)]
    interface: String,
}

#[derive(Subcommand)]
enum Command {
    /// Measure UDP round-trip latency (send → loopback receive)
    Latency {
        /// Number of packets to send
        #[arg(short, long, default_value = "10000")]
        count: u64,
    },
    /// Saturate the link — max sustained packet rate
    Throughput {
        /// Test duration in seconds
        #[arg(short, long, default_value = "10")]
        duration: u64,
    },
    /// Realistic MIDI burst patterns (drum rolls, chord stabs, CC sweeps)
    Burst,
    /// Verify heartbeat timing accuracy at 3ms intervals
    Heartbeat {
        /// Number of heartbeats to measure
        #[arg(short, long, default_value = "3000")]
        count: u64,
    },
    /// Simulate primary failure and measure client-side failover time
    Failover,
    /// Long-duration soak test (packet loss, jitter, memory stability)
    Soak {
        /// Soak duration in seconds
        #[arg(short, long, default_value = "60")]
        duration: u64,
        /// Target messages per second
        #[arg(short, long, default_value = "1000")]
        rate: u64,
    },
    /// Benchmark MIDI pipeline processing throughput
    Pipeline,
    /// Benchmark journal encode/decode and state reconciliation
    Journal,
    /// Run all tests sequentially
    All,
}

// ── Socket Helpers ───────────────────────────────────────────

fn create_sender(interface: Ipv4Addr) -> anyhow::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "macos")]
    socket.set_reuse_port(true)?;
    socket.set_multicast_if_v4(&interface)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.set_multicast_loop_v4(true)?; // Loopback enabled for self-test
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
    socket.bind(&addr.into())?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

fn create_receiver(mcast: Ipv4Addr, port: u16, interface: Ipv4Addr) -> anyhow::Result<std::net::UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(target_os = "macos")]
    socket.set_reuse_port(true)?;
    let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
    socket.bind(&addr.into())?;
    socket.join_multicast_v4(&mcast, &interface)?;
    socket.set_nonblocking(true)?;
    // Increase receive buffer for burst tests
    socket.set_recv_buffer_size(4 * 1024 * 1024)?;
    Ok(socket.into())
}

// ── Statistics ────────────────────────────────────────────────

#[derive(Default)]
struct LatencyStats {
    samples: Vec<f64>,
}

impl LatencyStats {
    fn add(&mut self, us: f64) {
        self.samples.push(us);
    }

    fn report(&mut self, label: &str) {
        if self.samples.is_empty() {
            println!("  {label}: no samples collected");
            return;
        }
        self.samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = self.samples.len();
        let min = self.samples[0];
        let max = self.samples[n - 1];
        let sum: f64 = self.samples.iter().sum();
        let mean = sum / n as f64;
        let p50 = self.samples[n * 50 / 100];
        let p95 = self.samples[n * 95 / 100];
        let p99 = self.samples[n * 99 / 100];
        let p999 = self.samples[(n as f64 * 0.999) as usize];

        // Jitter: standard deviation of inter-sample differences
        let jitter = if n > 1 {
            let diffs: Vec<f64> = self.samples.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
            let jitter_mean: f64 = diffs.iter().sum::<f64>() / diffs.len() as f64;
            jitter_mean
        } else {
            0.0
        };

        println!("  {label} ({n} samples):");
        println!("    min={min:.1}us  mean={mean:.1}us  max={max:.1}us");
        println!("    p50={p50:.1}us  p95={p95:.1}us  p99={p99:.1}us  p99.9={p999:.1}us");
        println!("    jitter(avg)={jitter:.1}us");
    }
}

// ── Test: Latency ────────────────────────────────────────────

async fn test_latency(count: u64, interface: Ipv4Addr) -> anyhow::Result<bool> {
    println!("\n=== LATENCY TEST ===");
    println!("  Sending {count} MIDI packets via loopback multicast...\n");

    let sender = UdpSocket::from_std(create_sender(interface)?)?;
    let receiver = UdpSocket::from_std(create_receiver(TEST_MCAST_GROUP, TEST_DATA_PORT, interface)?)?;
    let dest = SocketAddrV4::new(TEST_MCAST_GROUP, TEST_DATA_PORT);

    let mut stats = LatencyStats::default();
    let mut send_buf = Vec::with_capacity(128);
    let mut recv_buf = [0u8; 1024];

    // Warmup
    for _ in 0..100 {
        let pkt = MidiDataPacket {
            sequence: 0,
            timestamp_us: now_us(),
            host_id: 1,
            midi_data: vec![0x90, 60, 127],
            journal: None,
        };
        pkt.serialize(&mut send_buf);
        sender.send_to(&send_buf, dest).await?;
        let _ = tokio::time::timeout(Duration::from_millis(50), receiver.recv_from(&mut recv_buf)).await;
    }

    // Drain any remaining warmup packets
    tokio::time::sleep(Duration::from_millis(10)).await;
    while receiver.try_recv(&mut recv_buf).is_ok() {}

    // Measured run
    for seq in 0..count {
        let send_time = now_us();
        let pkt = MidiDataPacket {
            sequence: seq as u16,
            timestamp_us: send_time,
            host_id: 1,
            midi_data: vec![0x90, 60, 127],
            journal: None,
        };
        pkt.serialize(&mut send_buf);
        sender.send_to(&send_buf, dest).await?;

        match tokio::time::timeout(Duration::from_millis(100), receiver.recv_from(&mut recv_buf)).await {
            Ok(Ok((len, _))) => {
                let recv_time = now_us();
                if let Some(decoded) = MidiDataPacket::deserialize(&recv_buf[..len]) {
                    let latency = recv_time.saturating_sub(decoded.timestamp_us) as f64;
                    stats.add(latency);
                }
            }
            _ => {
                // Packet lost or timeout — counted as loss
            }
        }
    }

    let received = stats.samples.len() as u64;
    let loss_pct = ((count - received) as f64 / count as f64) * 100.0;

    stats.report("Loopback latency");
    println!("    received={received}/{count}  loss={loss_pct:.2}%");

    let pass = stats.samples.len() > 0
        && stats.samples[stats.samples.len() * 99 / 100] < 5000.0 // p99 < 5ms
        && loss_pct < 1.0;

    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: p99 < 5ms, loss < 1%");
    Ok(pass)
}

// ── Test: Throughput ─────────────────────────────────────────

async fn test_throughput(duration_secs: u64, interface: Ipv4Addr) -> anyhow::Result<bool> {
    println!("\n=== THROUGHPUT TEST ===");
    println!("  Saturating link for {duration_secs}s...\n");

    let sender = UdpSocket::from_std(create_sender(interface)?)?;
    let receiver = UdpSocket::from_std(create_receiver(TEST_MCAST_GROUP, TEST_DATA_PORT, interface)?)?;
    let dest = SocketAddrV4::new(TEST_MCAST_GROUP, TEST_DATA_PORT);

    let sent = Arc::new(AtomicU64::new(0));
    let received = Arc::new(AtomicU64::new(0));
    let bytes_sent = Arc::new(AtomicU64::new(0));
    let running = Arc::new(AtomicBool::new(true));

    // Receiver task
    let recv_count = Arc::clone(&received);
    let recv_running = Arc::clone(&running);
    let recv_handle = tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        while recv_running.load(Ordering::Relaxed) {
            match tokio::time::timeout(Duration::from_millis(10), receiver.recv_from(&mut buf)).await {
                Ok(Ok((len, _))) => {
                    if MidiDataPacket::deserialize(&buf[..len]).is_some() {
                        recv_count.fetch_add(1, Ordering::Relaxed);
                    }
                }
                _ => {}
            }
        }
    });

    // Sender: fire as fast as possible
    let mut send_buf = Vec::with_capacity(128);
    let mut seq: u16 = 0;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(duration_secs);

    // Typical MIDI messages: 3-byte Note On/Off, CC, with occasional journal
    let midi_messages: Vec<Vec<u8>> = vec![
        vec![0x90, 60, 127],           // Note On
        vec![0x80, 60, 0],             // Note Off
        vec![0xB0, 1, 64],             // CC Mod Wheel
        vec![0xB0, 7, 100],            // CC Volume
        vec![0xE0, 0, 64],             // Pitch Bend
        vec![0xC0, 5],                 // Program Change
    ];

    while Instant::now() < deadline {
        let midi = &midi_messages[seq as usize % midi_messages.len()];
        let pkt = MidiDataPacket {
            sequence: seq,
            timestamp_us: now_us(),
            host_id: 1,
            midi_data: midi.clone(),
            journal: None,
        };
        pkt.serialize(&mut send_buf);

        match sender.try_send_to(&send_buf, dest.into()) {
            Ok(n) => {
                sent.fetch_add(1, Ordering::Relaxed);
                bytes_sent.fetch_add(n as u64, Ordering::Relaxed);
            }
            Err(_) => {
                // Socket buffer full — yield and retry
                tokio::task::yield_now().await;
            }
        }

        seq = seq.wrapping_add(1);
    }

    // Let receiver catch up
    tokio::time::sleep(Duration::from_millis(100)).await;
    running.store(false, Ordering::Relaxed);
    recv_handle.await?;

    let elapsed = start.elapsed();
    let total_sent = sent.load(Ordering::Relaxed);
    let total_recv = received.load(Ordering::Relaxed);
    let total_bytes = bytes_sent.load(Ordering::Relaxed);
    let pps = total_sent as f64 / elapsed.as_secs_f64();
    let mbps = (total_bytes as f64 * 8.0) / (elapsed.as_secs_f64() * 1_000_000.0);
    let loss_pct = if total_sent > 0 && total_sent > total_recv {
        ((total_sent - total_recv) as f64 / total_sent as f64) * 100.0
    } else {
        0.0
    };

    println!("  Duration:    {:.1}s", elapsed.as_secs_f64());
    println!("  Sent:        {total_sent} packets");
    println!("  Received:    {total_recv} packets");
    println!("  Rate:        {pps:.0} pkt/s");
    println!("  Bandwidth:   {mbps:.2} Mbit/s");
    println!("  Loss:        {loss_pct:.2}%");

    // For context: a heavy MIDI session is ~1000 msg/s.
    // We should sustain >50,000 msg/s easily on loopback.
    let pass = pps > 10_000.0 && loss_pct < 5.0;
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: >10k pkt/s sustained, <5% loss");
    Ok(pass)
}

// ── Test: Burst ──────────────────────────────────────────────

async fn test_burst(interface: Ipv4Addr) -> anyhow::Result<bool> {
    println!("\n=== BURST TEST ===");
    println!("  Simulating real-world MIDI patterns...\n");

    let sender = UdpSocket::from_std(create_sender(interface)?)?;
    let receiver = UdpSocket::from_std(create_receiver(TEST_MCAST_GROUP, TEST_DATA_PORT, interface)?)?;
    let dest = SocketAddrV4::new(TEST_MCAST_GROUP, TEST_DATA_PORT);

    let mut send_buf = Vec::with_capacity(256);
    let mut recv_buf = [0u8; 1024];
    let mut total_sent: u64 = 0;
    let mut total_recv: u64 = 0;
    let mut latencies = LatencyStats::default();

    // Scenario 1: 16th-note drum pattern at 140 BPM
    // 140 BPM = 2.33 beats/sec, 16th notes = 9.33 notes/sec → ~107ms between notes
    println!("  [1/4] Drum pattern (140 BPM, 16th notes)...");
    for i in 0..64u16 {
        let note = match i % 4 {
            0 => 36, // Kick
            1 => 42, // HiHat
            2 => 38, // Snare
            _ => 42, // HiHat
        };
        let pkt = MidiDataPacket {
            sequence: i,
            timestamp_us: now_us(),
            host_id: 1,
            midi_data: vec![0x99, note, 100 + (i % 28) as u8], // Ch 10, varying velocity
            journal: None,
        };
        pkt.serialize(&mut send_buf);
        sender.send_to(&send_buf, dest).await?;
        total_sent += 1;
        tokio::time::sleep(Duration::from_millis(107)).await;
    }
    // Collect
    tokio::time::sleep(Duration::from_millis(50)).await;
    while let Ok((len, _)) = receiver.try_recv_from(&mut recv_buf) {
        if let Some(pkt) = MidiDataPacket::deserialize(&recv_buf[..len]) {
            latencies.add((now_us() - pkt.timestamp_us) as f64);
            total_recv += 1;
        }
    }

    // Scenario 2: Chord stab — 6 notes simultaneously
    println!("  [2/4] Chord stabs (6-note chords, 10 stabs)...");
    let chord = [60u8, 64, 67, 72, 76, 79]; // C major spread voicing
    for stab in 0..10u16 {
        for (j, &note) in chord.iter().enumerate() {
            let pkt = MidiDataPacket {
                sequence: 100 + stab * 12 + j as u16,
                timestamp_us: now_us(),
                host_id: 1,
                midi_data: vec![0x90, note, 110],
                journal: None,
            };
            pkt.serialize(&mut send_buf);
            sender.send_to(&send_buf, dest).await?;
            total_sent += 1;
        }
        // Note offs after 200ms
        tokio::time::sleep(Duration::from_millis(200)).await;
        for (j, &note) in chord.iter().enumerate() {
            let pkt = MidiDataPacket {
                sequence: 100 + stab * 12 + 6 + j as u16,
                timestamp_us: now_us(),
                host_id: 1,
                midi_data: vec![0x80, note, 0],
                journal: None,
            };
            pkt.serialize(&mut send_buf);
            sender.send_to(&send_buf, dest).await?;
            total_sent += 1;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    while let Ok((len, _)) = receiver.try_recv_from(&mut recv_buf) {
        if let Some(pkt) = MidiDataPacket::deserialize(&recv_buf[..len]) {
            latencies.add((now_us() - pkt.timestamp_us) as f64);
            total_recv += 1;
        }
    }

    // Scenario 3: CC sweep — fader moving smoothly (127 steps in 1 second)
    println!("  [3/4] CC fader sweep (127 steps in 1s)...");
    for val in 0..128u8 {
        let pkt = MidiDataPacket {
            sequence: 300 + val as u16,
            timestamp_us: now_us(),
            host_id: 1,
            midi_data: vec![0xB0, 7, val], // CC7 Volume
            journal: None,
        };
        pkt.serialize(&mut send_buf);
        sender.send_to(&send_buf, dest).await?;
        total_sent += 1;
        tokio::time::sleep(Duration::from_micros(7874)).await; // ~1s / 127 steps
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    while let Ok((len, _)) = receiver.try_recv_from(&mut recv_buf) {
        if let Some(pkt) = MidiDataPacket::deserialize(&recv_buf[..len]) {
            latencies.add((now_us() - pkt.timestamp_us) as f64);
            total_recv += 1;
        }
    }

    // Scenario 4: Machine-gun burst — 100 notes in <10ms (worst case stress)
    println!("  [4/4] Machine-gun burst (100 notes in <10ms)...");
    for i in 0..100u16 {
        let pkt = MidiDataPacket {
            sequence: 500 + i,
            timestamp_us: now_us(),
            host_id: 1,
            midi_data: vec![0x90, 36 + (i as u8 % 48), 127],
            journal: None,
        };
        pkt.serialize(&mut send_buf);
        sender.send_to(&send_buf, dest).await?;
        total_sent += 1;
        // No sleep — fire as fast as possible
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok((len, _)) = receiver.try_recv_from(&mut recv_buf) {
        if let Some(pkt) = MidiDataPacket::deserialize(&recv_buf[..len]) {
            latencies.add((now_us() - pkt.timestamp_us) as f64);
            total_recv += 1;
        }
    }

    let loss_pct = if total_sent > total_recv {
        ((total_sent - total_recv) as f64 / total_sent as f64) * 100.0
    } else {
        0.0
    };
    println!();
    latencies.report("All burst patterns");
    println!("    sent={total_sent}  received={total_recv}  loss={loss_pct:.2}%");

    let pass = loss_pct < 1.0;
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: <1% packet loss across all burst patterns");
    Ok(pass)
}

// ── Test: Heartbeat Timing ───────────────────────────────────

async fn test_heartbeat(count: u64, interface: Ipv4Addr) -> anyhow::Result<bool> {
    println!("\n=== HEARTBEAT TIMING TEST ===");
    println!("  Measuring {count} heartbeat intervals at 3ms target...\n");

    let sender = UdpSocket::from_std(create_sender(interface)?)?;
    let receiver = UdpSocket::from_std(create_receiver(TEST_MCAST_GROUP, TEST_HB_PORT, interface)?)?;
    let dest = SocketAddrV4::new(TEST_MCAST_GROUP, TEST_HB_PORT);

    let running = Arc::new(AtomicBool::new(true));
    let send_running = Arc::clone(&running);

    // Sender task: heartbeat at 3ms intervals (matching production config)
    let send_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(3));
        let mut seq: u16 = 0;
        let mut buf = [0u8; HeartbeatPacket::SIZE];
        while send_running.load(Ordering::Relaxed) {
            interval.tick().await;
            let pkt = HeartbeatPacket {
                host_id: 1,
                role: HostRole::Primary,
                sequence: seq,
                timestamp_us: now_us(),
            };
            pkt.serialize(&mut buf);
            let _ = sender.send_to(&buf, dest).await;
            seq = seq.wrapping_add(1);
        }
    });

    // Receiver: measure inter-arrival times
    let mut intervals = LatencyStats::default();
    let mut recv_buf = [0u8; 64];
    let mut received = 0u64;
    let mut missed_sequence = 0u64;
    let mut last_seq: Option<u16> = None;

    // Skip first packet (no interval to measure)
    let _ = tokio::time::timeout(Duration::from_millis(100), receiver.recv_from(&mut recv_buf)).await;
    received += 1;
    let mut last_recv = Instant::now();

    while received < count {
        match tokio::time::timeout(Duration::from_millis(50), receiver.recv_from(&mut recv_buf)).await {
            Ok(Ok((len, _))) => {
                let now = Instant::now();
                let interval = now.duration_since(last_recv);
                intervals.add(interval.as_micros() as f64);
                last_recv = now;
                received += 1;

                if let Some(pkt) = HeartbeatPacket::deserialize(&recv_buf[..len]) {
                    if let Some(prev) = last_seq {
                        let expected = prev.wrapping_add(1);
                        if pkt.sequence != expected {
                            missed_sequence += pkt.sequence.wrapping_sub(expected) as u64;
                        }
                    }
                    last_seq = Some(pkt.sequence);
                }
            }
            _ => break,
        }
    }

    running.store(false, Ordering::Relaxed);
    send_handle.await?;

    // Analyze: ideal is 3000us (3ms) between each heartbeat
    intervals.report("Inter-heartbeat interval");
    println!("    target=3000us");
    println!("    sequence_gaps={missed_sequence}");

    // Calculate percentage within tolerance (3ms +/- 1ms)
    let within_tolerance = intervals.samples.iter()
        .filter(|&&v| v >= 2000.0 && v <= 4000.0)
        .count();
    let tolerance_pct = (within_tolerance as f64 / intervals.samples.len() as f64) * 100.0;
    println!("    within_tolerance(2-4ms)={tolerance_pct:.1}%");

    let pass = tolerance_pct > 95.0 && missed_sequence < count / 100;
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: >95% within 2-4ms tolerance, <1% sequence gaps");
    Ok(pass)
}

// ── Test: Failover ───────────────────────────────────────────

async fn test_failover(interface: Ipv4Addr) -> anyhow::Result<bool> {
    println!("\n=== FAILOVER SIMULATION TEST ===");
    println!("  Simulating primary failure, measuring detection and switch time...\n");

    let primary_sender = UdpSocket::from_std(create_sender(interface)?)?;
    let standby_sender = UdpSocket::from_std(create_sender(interface)?)?;
    let receiver = UdpSocket::from_std(create_receiver(TEST_MCAST_GROUP, TEST_HB_PORT, interface)?)?;
    let dest = SocketAddrV4::new(TEST_MCAST_GROUP, TEST_HB_PORT);

    let primary_alive = Arc::new(AtomicBool::new(true));
    let test_running = Arc::new(AtomicBool::new(true));

    // Primary heartbeat sender
    let primary_flag = Arc::clone(&primary_alive);
    let running1 = Arc::clone(&test_running);
    let primary_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(3));
        let mut seq: u16 = 0;
        let mut buf = [0u8; HeartbeatPacket::SIZE];
        while running1.load(Ordering::Relaxed) {
            interval.tick().await;
            if primary_flag.load(Ordering::Relaxed) {
                let pkt = HeartbeatPacket {
                    host_id: 1,
                    role: HostRole::Primary,
                    sequence: seq,
                    timestamp_us: now_us(),
                };
                pkt.serialize(&mut buf);
                let _ = primary_sender.send_to(&buf, dest).await;
            }
            seq = seq.wrapping_add(1);
        }
    });

    // Standby heartbeat sender (always alive)
    let running2 = Arc::clone(&test_running);
    let standby_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(3));
        let mut seq: u16 = 0;
        let mut buf = [0u8; HeartbeatPacket::SIZE];
        while running2.load(Ordering::Relaxed) {
            interval.tick().await;
            let pkt = HeartbeatPacket {
                host_id: 2,
                role: HostRole::Standby,
                sequence: seq,
                timestamp_us: now_us(),
            };
            pkt.serialize(&mut buf);
            let _ = standby_sender.send_to(&buf, dest).await;
            seq = seq.wrapping_add(1);
        }
    });

    // Client-side failover detector
    let mut recv_buf = [0u8; 64];
    let mut primary_last_seen = Instant::now();
    let mut standby_last_seen = Instant::now();
    let mut active_host: u8 = 1; // Start on primary
    let miss_threshold = Duration::from_millis(9); // 3 missed HBs at 3ms

    // Let both hosts stabilize
    println!("  Phase 1: Both hosts healthy (2s warmup)...");
    let warmup_end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < warmup_end {
        if let Ok(Ok((len, _))) = tokio::time::timeout(
            Duration::from_millis(10), receiver.recv_from(&mut recv_buf)
        ).await {
            if let Some(pkt) = HeartbeatPacket::deserialize(&recv_buf[..len]) {
                match pkt.host_id {
                    1 => primary_last_seen = Instant::now(),
                    2 => standby_last_seen = Instant::now(),
                    _ => {}
                }
            }
        }
    }
    println!("    Primary: alive, Standby: alive, Active: host {active_host}");

    // Kill primary
    println!("  Phase 2: Killing primary host...");
    let kill_time = Instant::now();
    primary_alive.store(false, Ordering::Relaxed);

    // Monitor until failover is detected
    let mut failover_detected = false;
    let mut failover_time = Duration::ZERO;
    let detect_deadline = kill_time + Duration::from_millis(100); // 100ms max

    while Instant::now() < detect_deadline {
        match tokio::time::timeout(Duration::from_millis(1), receiver.recv_from(&mut recv_buf)).await {
            Ok(Ok((len, _))) => {
                if let Some(pkt) = HeartbeatPacket::deserialize(&recv_buf[..len]) {
                    match pkt.host_id {
                        1 => primary_last_seen = Instant::now(),
                        2 => standby_last_seen = Instant::now(),
                        _ => {}
                    }
                }
            }
            _ => {}
        }

        // Check if primary is missed
        if active_host == 1 && primary_last_seen.elapsed() > miss_threshold {
            // Check standby is healthy
            if standby_last_seen.elapsed() < miss_threshold {
                failover_time = kill_time.elapsed();
                active_host = 2;
                failover_detected = true;
                break;
            }
        }
    }

    if failover_detected {
        println!("    Failover detected in {:.2}ms", failover_time.as_secs_f64() * 1000.0);
    } else {
        println!("    FAILOVER NOT DETECTED within 100ms!");
    }

    // Phase 3: Verify standby is still streaming
    println!("  Phase 3: Verifying standby stream...");
    let mut standby_packets = 0u64;
    let verify_end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < verify_end {
        if let Ok(Ok((len, _))) = tokio::time::timeout(
            Duration::from_millis(10), receiver.recv_from(&mut recv_buf)
        ).await {
            if let Some(pkt) = HeartbeatPacket::deserialize(&recv_buf[..len]) {
                if pkt.host_id == 2 {
                    standby_packets += 1;
                }
            }
        }
    }
    println!("    Standby packets received: {standby_packets} (expected ~333/s)");

    // Phase 4: Bring primary back
    println!("  Phase 4: Primary recovery...");
    primary_alive.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut primary_recovered = false;
    let recovery_end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < recovery_end {
        if let Ok(Ok((len, _))) = tokio::time::timeout(
            Duration::from_millis(10), receiver.recv_from(&mut recv_buf)
        ).await {
            if let Some(pkt) = HeartbeatPacket::deserialize(&recv_buf[..len]) {
                if pkt.host_id == 1 {
                    primary_recovered = true;
                    break;
                }
            }
        }
    }
    println!("    Primary recovered: {primary_recovered}");
    println!("    Active host remains: {active_host} (manual switch-back policy)");

    test_running.store(false, Ordering::Relaxed);
    primary_handle.await?;
    standby_handle.await?;

    let failover_ms = failover_time.as_secs_f64() * 1000.0;
    let pass = failover_detected && failover_ms < 15.0 && standby_packets > 200 && primary_recovered;
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: failover < 15ms, standby streaming, primary recoverable");
    Ok(pass)
}

// ── Test: Soak ───────────────────────────────────────────────

async fn test_soak(duration_secs: u64, rate: u64, interface: Ipv4Addr) -> anyhow::Result<bool> {
    println!("\n=== SOAK TEST ===");
    println!("  Running for {duration_secs}s at {rate} msg/s...\n");

    let sender = UdpSocket::from_std(create_sender(interface)?)?;
    let receiver = UdpSocket::from_std(create_receiver(TEST_MCAST_GROUP, TEST_DATA_PORT, interface)?)?;
    let dest = SocketAddrV4::new(TEST_MCAST_GROUP, TEST_DATA_PORT);

    let running = Arc::new(AtomicBool::new(true));
    let total_recv = Arc::new(AtomicU64::new(0));
    let max_latency_us = Arc::new(AtomicU64::new(0));
    let sum_latency = Arc::new(AtomicU64::new(0));

    // Receiver task
    let recv_count = Arc::clone(&total_recv);
    let recv_max = Arc::clone(&max_latency_us);
    let recv_sum = Arc::clone(&sum_latency);
    let recv_running = Arc::clone(&running);
    let recv_handle = tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        while recv_running.load(Ordering::Relaxed) {
            match tokio::time::timeout(Duration::from_millis(10), receiver.recv_from(&mut buf)).await {
                Ok(Ok((len, _))) => {
                    let recv_time = now_us();
                    if let Some(pkt) = MidiDataPacket::deserialize(&buf[..len]) {
                        let lat = recv_time.saturating_sub(pkt.timestamp_us);
                        recv_count.fetch_add(1, Ordering::Relaxed);
                        recv_sum.fetch_add(lat, Ordering::Relaxed);
                        // Update max (CAS loop)
                        let mut current = recv_max.load(Ordering::Relaxed);
                        while lat > current {
                            match recv_max.compare_exchange_weak(current, lat, Ordering::Relaxed, Ordering::Relaxed) {
                                Ok(_) => break,
                                Err(actual) => current = actual,
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    });

    // Sender: regulated rate
    let interval_us = 1_000_000 / rate;
    let mut send_buf = Vec::with_capacity(128);
    let mut seq: u16 = 0;
    let mut total_sent: u64 = 0;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(duration_secs);

    // Progress reporting
    let mut last_report = Instant::now();
    let report_interval = Duration::from_secs(10);

    while Instant::now() < deadline {
        let midi = match seq % 6 {
            0 => vec![0x90, 60 + (seq as u8 % 48), 100],
            1 => vec![0x80, 60 + ((seq - 1) as u8 % 48), 0],
            2 => vec![0xB0, 1, (seq as u8) % 128],
            3 => vec![0xB0, 7, (seq as u8) % 128],
            4 => vec![0xE0, 0, (seq as u8) % 128],
            _ => vec![0xC0, (seq as u8) % 128],
        };

        let pkt = MidiDataPacket {
            sequence: seq,
            timestamp_us: now_us(),
            host_id: 1,
            midi_data: midi,
            journal: None,
        };
        pkt.serialize(&mut send_buf);
        let _ = sender.send_to(&send_buf, dest).await;
        total_sent += 1;
        seq = seq.wrapping_add(1);

        // Rate limiting
        tokio::time::sleep(Duration::from_micros(interval_us)).await;

        // Progress
        if last_report.elapsed() > report_interval {
            let recv = total_recv.load(Ordering::Relaxed);
            let loss = if total_sent > recv { ((total_sent - recv) as f64 / total_sent as f64) * 100.0 } else { 0.0 };
            let elapsed = start.elapsed().as_secs();
            println!("  [{elapsed}s] sent={total_sent} recv={recv} loss={loss:.2}%");
            last_report = Instant::now();
        }
    }

    tokio::time::sleep(Duration::from_millis(200)).await;
    running.store(false, Ordering::Relaxed);
    recv_handle.await?;

    let elapsed = start.elapsed();
    let recv = total_recv.load(Ordering::Relaxed);
    let max_lat = max_latency_us.load(Ordering::Relaxed);
    let avg_lat = if recv > 0 { sum_latency.load(Ordering::Relaxed) / recv } else { 0 };
    let actual_rate = total_sent as f64 / elapsed.as_secs_f64();
    let loss_pct = if total_sent > recv {
        ((total_sent - recv) as f64 / total_sent as f64) * 100.0
    } else {
        0.0
    };

    println!("\n  Summary:");
    println!("    Duration:     {:.1}s", elapsed.as_secs_f64());
    println!("    Sent:         {total_sent}");
    println!("    Received:     {recv}");
    println!("    Actual rate:  {actual_rate:.0} msg/s");
    println!("    Packet loss:  {loss_pct:.3}%");
    println!("    Avg latency:  {avg_lat}us");
    println!("    Max latency:  {max_lat}us ({:.2}ms)", max_lat as f64 / 1000.0);

    let pass = loss_pct < 0.1 && max_lat < 10_000; // <0.1% loss, max latency <10ms
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: <0.1% loss, max latency <10ms over {duration_secs}s soak");
    Ok(pass)
}

// ── Test: Pipeline Benchmark ─────────────────────────────────

async fn test_pipeline() -> anyhow::Result<bool> {
    println!("\n=== PIPELINE BENCHMARK ===");
    println!("  Measuring MIDI processing pipeline throughput...\n");

    let config = PipelineConfig::default();

    // Generate test messages
    let messages: Vec<Vec<u8>> = (0..1000).map(|i| {
        match i % 5 {
            0 => vec![0x90, 60 + (i as u8 % 48), 100],  // Note On
            1 => vec![0x80, 60 + (i as u8 % 48), 0],    // Note Off
            2 => vec![0xB0, (i as u8) % 120, 64],        // CC
            3 => vec![0xE0, 0, 64],                       // Pitch bend
            _ => vec![0xC0, (i as u8) % 128],             // Program change
        }
    }).collect();

    // Benchmark: pass-through (all enabled)
    let iterations = 1_000_000u64;
    let start = Instant::now();
    let mut processed = 0u64;
    for i in 0..iterations {
        let msg = &messages[i as usize % messages.len()];
        if config.process(msg).is_some() {
            processed += 1;
        }
    }
    let elapsed = start.elapsed();
    let rate = iterations as f64 / elapsed.as_secs_f64();
    println!("  Pass-through:  {rate:.0} msg/s ({:.0}ns/msg)", elapsed.as_nanos() as f64 / iterations as f64);
    println!("    processed:   {processed}/{iterations}");

    // Benchmark: with channel filter (disable channels 8-15)
    let mut filtered_config = PipelineConfig::default();
    for ch in 8..16 {
        filtered_config.channel_filter[ch] = false;
    }
    let start = Instant::now();
    let mut filtered = 0u64;
    for i in 0..iterations {
        let msg = &messages[i as usize % messages.len()];
        if filtered_config.process(msg).is_some() {
            filtered += 1;
        }
    }
    let elapsed = start.elapsed();
    let rate = iterations as f64 / elapsed.as_secs_f64();
    println!("  With filter:   {rate:.0} msg/s ({:.0}ns/msg)", elapsed.as_nanos() as f64 / iterations as f64);
    println!("    processed:   {filtered}/{iterations}");

    // Benchmark: with transpose + velocity curve
    let mut complex_config = PipelineConfig::default();
    complex_config.transpose = [2; 16]; // +2 semitones
    complex_config.velocity_curve = midi_protocol::pipeline::VelocityCurve::Logarithmic;
    let start = Instant::now();
    for i in 0..iterations {
        let msg = &messages[i as usize % messages.len()];
        let _ = complex_config.process(msg);
    }
    let elapsed = start.elapsed();
    let rate = iterations as f64 / elapsed.as_secs_f64();
    println!("  Full pipeline: {rate:.0} msg/s ({:.0}ns/msg)", elapsed.as_nanos() as f64 / iterations as f64);

    // At 3ms heartbeat, we need to process at least 333 packets/s.
    // Even heavy MIDI is ~5000 msg/s. The pipeline should handle millions/s.
    let pass = rate > 100_000.0;
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: pipeline >100k msg/s (production needs ~5k max)");
    Ok(pass)
}

// ── Test: Journal Benchmark ──────────────────────────────────

async fn test_journal() -> anyhow::Result<bool> {
    println!("\n=== JOURNAL BENCHMARK ===");
    println!("  Measuring state journal encode/decode and reconciliation...\n");

    // Build a realistic MIDI state
    let mut state = MidiState::new();

    // Simulate a live show: 20 active notes, 40 active CCs, some pitch bends
    for ch in 0..4 {
        for note in 0..5 {
            state.process_message(&[0x90 | ch, 36 + note * 12, 100]);
        }
        for cc in 0..10 {
            state.process_message(&[0xB0 | ch, cc, 64 + cc]);
        }
        state.process_message(&[0xE0 | ch, 0, 72]); // Pitch bend
        state.process_message(&[0xC0 | ch, ch * 4]); // Program change
    }

    println!("  State: {} active notes, 4 channels with CCs/pitch bend", state.active_note_count());

    // Benchmark encode
    let iterations = 100_000u64;
    let start = Instant::now();
    let mut last_encoded = Vec::new();
    for _ in 0..iterations {
        last_encoded = encode_journal(&state);
    }
    let elapsed = start.elapsed();
    let encode_rate = iterations as f64 / elapsed.as_secs_f64();
    let encode_ns = elapsed.as_nanos() as f64 / iterations as f64;
    println!("  Encode:        {encode_rate:.0}/s ({encode_ns:.0}ns/op), {len} bytes", len = last_encoded.len());

    // Benchmark decode
    let start = Instant::now();
    let mut decoded_state = MidiState::new();
    for _ in 0..iterations {
        decoded_state = decode_journal(&last_encoded).unwrap_or_default();
    }
    let elapsed = start.elapsed();
    let decode_rate = iterations as f64 / elapsed.as_secs_f64();
    let decode_ns = elapsed.as_nanos() as f64 / iterations as f64;
    println!("  Decode:        {decode_rate:.0}/s ({decode_ns:.0}ns/op)");

    // Verify roundtrip correctness
    let mut roundtrip_correct = true;
    for ch in 0..16 {
        if state.channels[ch].notes != decoded_state.channels[ch].notes {
            roundtrip_correct = false;
            println!("    ERROR: Channel {ch} notes mismatch!");
        }
        if state.channels[ch].cc != decoded_state.channels[ch].cc {
            roundtrip_correct = false;
            println!("    ERROR: Channel {ch} CCs mismatch!");
        }
    }
    println!("  Roundtrip:     {}", if roundtrip_correct { "correct" } else { "MISMATCH" });

    // Benchmark reconciliation (generate messages to restore state after failover)
    let start = Instant::now();
    let mut last_recon = Vec::new();
    for _ in 0..iterations {
        last_recon = state.generate_reconciliation();
    }
    let elapsed = start.elapsed();
    let recon_rate = iterations as f64 / elapsed.as_secs_f64();
    let recon_ns = elapsed.as_nanos() as f64 / iterations as f64;
    println!("  Reconcile:     {recon_rate:.0}/s ({recon_ns:.0}ns/op), {n} messages", n = last_recon.len());

    // The journal is appended every 100ms. Encode must be <1ms to not delay packets.
    let pass = encode_ns < 1_000_000.0 && decode_ns < 1_000_000.0 && roundtrip_correct;
    println!("\n  RESULT: {}", if pass { "PASS" } else { "FAIL" });
    println!("  Criteria: encode/decode <1ms, roundtrip correct");
    Ok(pass)
}

// ── Run All ──────────────────────────────────────────────────

async fn run_all(interface: Ipv4Addr) -> anyhow::Result<()> {
    println!("╔═══════════════════════════════════════════════════╗");
    println!("║  MIDInet Load Test Suite — Hakol Fine AV Services ║");
    println!("╚═══════════════════════════════════════════════════╝");

    let mut results: Vec<(&str, bool)> = Vec::new();

    results.push(("Pipeline Benchmark", test_pipeline().await?));
    results.push(("Journal Benchmark", test_journal().await?));
    results.push(("Latency (10k pkts)", test_latency(10_000, interface).await?));
    results.push(("Heartbeat Timing (3k)", test_heartbeat(3_000, interface).await?));
    results.push(("Burst Patterns", test_burst(interface).await?));
    results.push(("Throughput (10s)", test_throughput(10, interface).await?));
    results.push(("Failover Simulation", test_failover(interface).await?));
    results.push(("Soak Test (30s)", test_soak(30, 1000, interface).await?));

    println!("\n\n╔═══════════════════════════════════════════════════╗");
    println!("║                  FINAL REPORT                     ║");
    println!("╠═══════════════════════════════════════════════════╣");

    let mut all_pass = true;
    for (name, pass) in &results {
        let status = if *pass { "PASS" } else { "FAIL" };
        let indicator = if *pass { "  " } else { "!!" };
        println!("║ {indicator} {name:<40} {status:>4} ║");
        if !pass {
            all_pass = false;
        }
    }

    println!("╠═══════════════════════════════════════════════════╣");
    let overall = if all_pass { "ALL TESTS PASSED" } else { "SOME TESTS FAILED" };
    println!("║ {overall:^49} ║");
    println!("╚═══════════════════════════════════════════════════╝");

    if !all_pass {
        std::process::exit(1);
    }

    Ok(())
}

// ── Main ─────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let interface: Ipv4Addr = args.interface.parse()?;

    match args.command {
        Command::Latency { count } => { test_latency(count, interface).await?; }
        Command::Throughput { duration } => { test_throughput(duration, interface).await?; }
        Command::Burst => { test_burst(interface).await?; }
        Command::Heartbeat { count } => { test_heartbeat(count, interface).await?; }
        Command::Failover => { test_failover(interface).await?; }
        Command::Soak { duration, rate } => { test_soak(duration, rate, interface).await?; }
        Command::Pipeline => { test_pipeline().await?; }
        Command::Journal => { test_journal().await?; }
        Command::All => { run_all(interface).await?; }
    }

    Ok(())
}
