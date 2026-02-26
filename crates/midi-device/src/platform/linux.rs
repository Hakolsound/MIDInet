/// Linux ALSA sequencer virtual MIDI device implementation.
/// Creates a virtual MIDI port pair that appears to apps (Resolume, Ardour, etc.)
/// as if the physical controller is connected.
///
/// Architecture:
///   - Output port (READ | SUBS_READ): we push MIDI events here → apps receive them
///   - Input port (WRITE | SUBS_WRITE): apps send feedback here → we receive it
///
/// The ALSA sequencer API operates on structured events (EvNote, EvCtrl, etc.),
/// not raw MIDI bytes. We convert between raw MIDI wire format and ALSA events
/// at the boundary.

use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use alsa::seq::{
    self, Addr, EvCtrl, EvNote, Event, EventType, PortCap, PortInfo, PortType, Seq,
};
use tracing::{debug, error, info, warn};

use crate::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;

pub struct AlsaVirtualDevice {
    name: String,
    seq_handle: Option<Seq>,
    output_port: i32,
    input_port: i32,
    client_id: i32,
    /// Buffer for received MIDI feedback from apps (input port → raw bytes)
    feedback_buffer: Arc<Mutex<Vec<Vec<u8>>>>,
    /// Flag to signal the polling thread to stop
    running: Arc<AtomicBool>,
    /// Handle to the polling thread
    poll_thread: Option<std::thread::JoinHandle<()>>,
}

impl AlsaVirtualDevice {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            seq_handle: None,
            output_port: -1,
            input_port: -1,
            client_id: -1,
            feedback_buffer: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            poll_thread: None,
        }
    }
}

impl VirtualMidiDevice for AlsaVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        // Open ALSA sequencer in duplex mode, non-blocking for the main handle
        let seq_handle = Seq::open(None, None, true).map_err(|e| {
            anyhow::anyhow!("Failed to open ALSA sequencer: {}", e)
        })?;

        // Set client name to match the physical controller
        let client_name = CString::new(self.name.as_str()).map_err(|e| {
            anyhow::anyhow!("Invalid device name '{}': {}", self.name, e)
        })?;
        seq_handle.set_client_name(&client_name).map_err(|e| {
            anyhow::anyhow!("Failed to set client name: {}", e)
        })?;

        self.client_id = seq_handle.client_id().map_err(|e| {
            anyhow::anyhow!("Failed to get client ID: {}", e)
        })?;

        // Create output port — apps subscribe to read MIDI from this port.
        // Name matches the physical device so apps identify it correctly.
        let mut out_info = PortInfo::empty().map_err(|e| {
            anyhow::anyhow!("Failed to create port info: {}", e)
        })?;
        let out_name = CString::new(format!("{} MIDI 1", &self.name)).map_err(|e| {
            anyhow::anyhow!("Invalid port name: {}", e)
        })?;
        out_info.set_name(&out_name);
        out_info.set_capability(PortCap::READ | PortCap::SUBS_READ);
        out_info.set_type(PortType::MIDI_GENERIC | PortType::APPLICATION);
        out_info.set_midi_channels(16);
        seq_handle.create_port(&out_info).map_err(|e| {
            anyhow::anyhow!("Failed to create output port: {}", e)
        })?;
        self.output_port = out_info.get_port();

        // Create input port — apps subscribe to write feedback (LEDs, faders) here.
        let mut in_info = PortInfo::empty().map_err(|e| {
            anyhow::anyhow!("Failed to create port info: {}", e)
        })?;
        let in_name = CString::new(format!("{} MIDI 1", &self.name)).map_err(|e| {
            anyhow::anyhow!("Invalid port name: {}", e)
        })?;
        in_info.set_name(&in_name);
        in_info.set_capability(PortCap::WRITE | PortCap::SUBS_WRITE);
        in_info.set_type(PortType::MIDI_GENERIC | PortType::APPLICATION);
        in_info.set_midi_channels(16);
        seq_handle.create_port(&in_info).map_err(|e| {
            anyhow::anyhow!("Failed to create input port: {}", e)
        })?;
        self.input_port = in_info.get_port();

        info!(
            name = %self.name,
            client_id = self.client_id,
            output_port = self.output_port,
            input_port = self.input_port,
            "ALSA virtual MIDI device created"
        );

        self.seq_handle = Some(seq_handle);

        // Spawn a dedicated polling thread for receiving feedback from apps.
        // We open a *separate* blocking sequencer handle for the receive thread,
        // because ALSA seq Input borrows the Seq mutably.
        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let feedback_buf = Arc::clone(&self.feedback_buffer);
        let device_name = self.name.clone();
        let client_id = self.client_id;
        let input_port = self.input_port;

        let poll_thread = std::thread::Builder::new()
            .name(format!("midinet-alsa-rx-{}", &self.name))
            .spawn(move || {
                if let Err(e) = run_feedback_receiver(
                    &device_name,
                    client_id,
                    input_port,
                    running,
                    feedback_buf,
                ) {
                    error!(name = %device_name, "Feedback receiver error: {}", e);
                }
            })
            .map_err(|e| anyhow::anyhow!("Failed to spawn feedback thread: {}", e))?;

        self.poll_thread = Some(poll_thread);

        Ok(())
    }

    fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        let seq_handle = self.seq_handle.as_ref().ok_or_else(|| {
            anyhow::anyhow!("ALSA sequencer not initialized")
        })?;

        if data.is_empty() {
            return Ok(());
        }

        // Parse raw MIDI bytes and send as ALSA sequencer events
        let mut offset = 0;
        while offset < data.len() {
            let remaining = &data[offset..];
            let (event_opt, consumed) = raw_midi_to_alsa_event(remaining, self.output_port);

            if consumed == 0 {
                // Can't parse — skip one byte (running status recovery)
                offset += 1;
                continue;
            }

            if let Some(mut ev) = event_opt {
                ev.set_source(self.output_port);
                ev.set_subs();
                ev.set_direct();

                if let Err(e) = seq_handle.event_output(&mut ev) {
                    warn!("Failed to output ALSA event: {}", e);
                }
            }

            offset += consumed;
        }

        // Flush all buffered events
        if let Err(e) = seq_handle.drain_output() {
            warn!("Failed to drain ALSA output: {}", e);
        }

        debug!(bytes = data.len(), "Sent MIDI to ALSA virtual port");
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        if let Ok(mut buf) = self.feedback_buffer.lock() {
            if buf.is_empty() {
                Ok(None)
            } else {
                // Return oldest message (FIFO)
                Ok(Some(buf.remove(0)))
            }
        } else {
            Ok(None)
        }
    }

    fn close(&mut self) -> anyhow::Result<()> {
        info!(name = %self.name, "Closing ALSA virtual MIDI device");

        // Signal the polling thread to stop
        self.running.store(false, Ordering::SeqCst);

        // Wait for the polling thread
        if let Some(thread) = self.poll_thread.take() {
            let _ = thread.join();
        }

        // Drop the sequencer handle — this unregisters ports and client
        self.seq_handle = None;

        Ok(())
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}

/// Feedback receiver thread: opens a separate blocking ALSA sequencer connection,
/// subscribes to the input port, and polls for incoming events.
fn run_feedback_receiver(
    device_name: &str,
    client_id: i32,
    input_port: i32,
    running: Arc<AtomicBool>,
    feedback_buf: Arc<Mutex<Vec<Vec<u8>>>>,
) -> anyhow::Result<()> {
    // Open a second sequencer handle in blocking mode for efficient polling
    let seq_rx = Seq::open(None, Some(alsa::Direction::Capture), false).map_err(|e| {
        anyhow::anyhow!("Failed to open ALSA sequencer for feedback: {}", e)
    })?;

    let rx_name = CString::new(format!("MIDInet-rx-{}", device_name))?;
    seq_rx.set_client_name(&rx_name)?;

    // Create a port to receive events, then connect it to our input port
    let rx_port = seq_rx.create_simple_port(
        &CString::new("feedback-rx")?,
        PortCap::WRITE | PortCap::SUBS_WRITE,
        PortType::MIDI_GENERIC | PortType::APPLICATION,
    )?;

    // Subscribe: any events arriving at our virtual input port get forwarded here
    let sub = seq::PortSubscribe::empty()?;
    sub.set_sender(Addr {
        client: client_id,
        port: input_port,
    });
    sub.set_dest(Addr {
        client: seq_rx.client_id()?,
        port: rx_port,
    });
    seq_rx.subscribe_port(&sub)?;

    debug!(name = %device_name, "Feedback receiver subscribed to input port");

    // Use poll-based waiting with a timeout so we can check the running flag
    use alsa::PollDescriptors;
    let mut fds: Vec<libc::pollfd> = (&seq_rx, Some(alsa::Direction::Capture))
        .get()
        .map_err(|e| anyhow::anyhow!("Failed to get poll descriptors: {}", e))?;

    while running.load(Ordering::SeqCst) {
        // Poll with 100ms timeout
        let ret = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 100) };

        if ret < 0 {
            let errno = std::io::Error::last_os_error();
            if errno.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(anyhow::anyhow!("poll() failed: {}", errno));
        }

        if ret == 0 {
            // Timeout, check running flag and loop
            continue;
        }

        // Events available — drain them
        let mut input = seq_rx.input();
        while input.event_input_pending(true)? > 0 {
            match input.event_input() {
                Ok(event) => {
                    if let Some(raw_bytes) = alsa_event_to_raw_midi(&event) {
                        if let Ok(mut buf) = feedback_buf.lock() {
                            if buf.len() < 4096 {
                                buf.push(raw_bytes);
                            } else {
                                buf.remove(0);
                                buf.push(alsa_event_to_raw_midi(&event).unwrap_or_default());
                            }
                        }
                    }
                }
                Err(e) => {
                    if running.load(Ordering::SeqCst) {
                        warn!(name = %device_name, "Feedback event_input error: {}", e);
                    }
                    break;
                }
            }
        }
    }

    debug!(name = %device_name, "Feedback receiver stopped");
    Ok(())
}

// ── Raw MIDI ↔ ALSA event conversion ──────────────────────────────────────

/// Convert raw MIDI bytes at the given offset to an ALSA sequencer event.
/// Returns (Some(event), bytes_consumed) or (None, bytes_consumed) if unhandled.
fn raw_midi_to_alsa_event(data: &[u8], _port: i32) -> (Option<Event>, usize) {
    if data.is_empty() {
        return (None, 0);
    }

    let status = data[0];

    // SysEx: variable length, terminated by 0xF7
    if status == 0xF0 {
        // Find end of SysEx
        let end = data.iter().position(|&b| b == 0xF7);
        let sysex_len = match end {
            Some(pos) => pos + 1,
            None => data.len(), // Unterminated SysEx — take all remaining bytes
        };
        let sysex_data = &data[..sysex_len];
        let ev = Event::new_ext(EventType::Sysex, sysex_data);
        return (Some(ev), sysex_len);
    }

    // System realtime (single byte: 0xF8-0xFF)
    if status >= 0xF8 {
        let ev_type = match status {
            0xF8 => Some(EventType::Clock),
            0xFA => Some(EventType::Start),
            0xFB => Some(EventType::Continue),
            0xFC => Some(EventType::Stop),
            0xFE => Some(EventType::Sensing),
            0xFF => Some(EventType::Reset),
            _ => None,
        };
        if let Some(t) = ev_type {
            let ctrl = EvCtrl { channel: 0, param: 0, value: 0 };
            return (Some(Event::new(t, &ctrl)), 1);
        }
        return (None, 1);
    }

    // System common
    if status >= 0xF0 {
        match status {
            0xF1 => {
                // MTC Quarter Frame
                if data.len() < 2 { return (None, 1); }
                return (None, 2); // Skip — not commonly needed
            }
            0xF2 => {
                // Song Position Pointer (3 bytes)
                if data.len() < 3 { return (None, 1); }
                return (None, 3);
            }
            0xF3 => {
                // Song Select (2 bytes)
                if data.len() < 2 { return (None, 1); }
                return (None, 2);
            }
            0xF6 => {
                // Tune Request (1 byte)
                let ctrl = EvCtrl { channel: 0, param: 0, value: 0 };
                return (Some(Event::new(EventType::TuneRequest, &ctrl)), 1);
            }
            _ => return (None, 1),
        }
    }

    // Channel voice messages (status 0x80-0xEF)
    let msg_type = status & 0xF0;
    let channel = status & 0x0F;

    match msg_type {
        // Note Off (0x80): 3 bytes
        0x80 => {
            if data.len() < 3 { return (None, 1); }
            let note = EvNote {
                channel,
                note: data[1] & 0x7F,
                velocity: data[2] & 0x7F,
                off_velocity: 0,
                duration: 0,
            };
            (Some(Event::new(EventType::Noteoff, &note)), 3)
        }
        // Note On (0x90): 3 bytes
        0x90 => {
            if data.len() < 3 { return (None, 1); }
            let vel = data[2] & 0x7F;
            // Velocity 0 = Note Off per MIDI spec
            if vel == 0 {
                let note = EvNote {
                    channel,
                    note: data[1] & 0x7F,
                    velocity: 0,
                    off_velocity: 0,
                    duration: 0,
                };
                (Some(Event::new(EventType::Noteoff, &note)), 3)
            } else {
                let note = EvNote {
                    channel,
                    note: data[1] & 0x7F,
                    velocity: vel,
                    off_velocity: 0,
                    duration: 0,
                };
                (Some(Event::new(EventType::Noteon, &note)), 3)
            }
        }
        // Polyphonic Aftertouch (0xA0): 3 bytes
        0xA0 => {
            if data.len() < 3 { return (None, 1); }
            let note = EvNote {
                channel,
                note: data[1] & 0x7F,
                velocity: data[2] & 0x7F,
                off_velocity: 0,
                duration: 0,
            };
            (Some(Event::new(EventType::Keypress, &note)), 3)
        }
        // Control Change (0xB0): 3 bytes
        0xB0 => {
            if data.len() < 3 { return (None, 1); }
            let ctrl = EvCtrl {
                channel,
                param: (data[1] & 0x7F) as u32,
                value: (data[2] & 0x7F) as i32,
            };
            (Some(Event::new(EventType::Controller, &ctrl)), 3)
        }
        // Program Change (0xC0): 2 bytes
        0xC0 => {
            if data.len() < 2 { return (None, 1); }
            let ctrl = EvCtrl {
                channel,
                param: 0,
                value: (data[1] & 0x7F) as i32,
            };
            (Some(Event::new(EventType::Pgmchange, &ctrl)), 2)
        }
        // Channel Pressure (0xD0): 2 bytes
        0xD0 => {
            if data.len() < 2 { return (None, 1); }
            let ctrl = EvCtrl {
                channel,
                param: 0,
                value: (data[1] & 0x7F) as i32,
            };
            (Some(Event::new(EventType::Chanpress, &ctrl)), 2)
        }
        // Pitch Bend (0xE0): 3 bytes, 14-bit value centered at 8192
        0xE0 => {
            if data.len() < 3 { return (None, 1); }
            let lsb = (data[1] & 0x7F) as i32;
            let msb = (data[2] & 0x7F) as i32;
            // ALSA expects pitch bend as -8192..+8191
            let value = ((msb << 7) | lsb) - 8192;
            let ctrl = EvCtrl {
                channel,
                param: 0,
                value,
            };
            (Some(Event::new(EventType::Pitchbend, &ctrl)), 3)
        }
        _ => (None, 1),
    }
}

/// Convert an ALSA sequencer event back to raw MIDI bytes.
/// Used for the feedback path (apps → controller).
fn alsa_event_to_raw_midi(event: &Event) -> Option<Vec<u8>> {
    match event.get_type() {
        EventType::Noteon => {
            let d: EvNote = event.get_data()?;
            Some(vec![0x90 | (d.channel & 0x0F), d.note & 0x7F, d.velocity & 0x7F])
        }
        EventType::Noteoff => {
            let d: EvNote = event.get_data()?;
            Some(vec![0x80 | (d.channel & 0x0F), d.note & 0x7F, d.velocity & 0x7F])
        }
        EventType::Keypress => {
            let d: EvNote = event.get_data()?;
            Some(vec![0xA0 | (d.channel & 0x0F), d.note & 0x7F, d.velocity & 0x7F])
        }
        EventType::Controller => {
            let d: EvCtrl = event.get_data()?;
            Some(vec![0xB0 | (d.channel & 0x0F), (d.param & 0x7F) as u8, (d.value & 0x7F) as u8])
        }
        EventType::Pgmchange => {
            let d: EvCtrl = event.get_data()?;
            Some(vec![0xC0 | (d.channel & 0x0F), (d.value & 0x7F) as u8])
        }
        EventType::Chanpress => {
            let d: EvCtrl = event.get_data()?;
            Some(vec![0xD0 | (d.channel & 0x0F), (d.value & 0x7F) as u8])
        }
        EventType::Pitchbend => {
            let d: EvCtrl = event.get_data()?;
            // Convert from -8192..+8191 back to 14-bit unsigned
            let unsigned = (d.value + 8192).clamp(0, 16383) as u16;
            let lsb = (unsigned & 0x7F) as u8;
            let msb = ((unsigned >> 7) & 0x7F) as u8;
            Some(vec![0xE0 | (d.channel & 0x0F), lsb, msb])
        }
        EventType::Sysex => {
            event.get_ext().map(|data| data.to_vec())
        }
        EventType::Clock => Some(vec![0xF8]),
        EventType::Start => Some(vec![0xFA]),
        EventType::Continue => Some(vec![0xFB]),
        EventType::Stop => Some(vec![0xFC]),
        EventType::Sensing => Some(vec![0xFE]),
        EventType::Reset => Some(vec![0xFF]),
        EventType::TuneRequest => Some(vec![0xF6]),
        _ => None,
    }
}
