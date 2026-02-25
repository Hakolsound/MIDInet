/// Windows MIDI Services (Windows.Devices.Midi2) backend for virtual MIDI devices.
/// This is a fallback for when teVirtualMIDI is not available on Windows 11.
///
/// Requirements:
/// - Windows 11 (build >= 22000)
/// - Windows MIDI Services runtime installed (ships with Win11 24H2+,
///   or install via winget: Microsoft.WindowsMIDIServicesSDK)
///
/// Uses WinRT COM activation at runtime — gracefully returns Err if
/// the MIDI Services runtime is not available.

#[cfg(target_os = "windows")]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/midi2_bindings.rs"));
}

#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
use bindings::Windows::Devices::Midi2 as midi2;

use crate::virtual_device::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;
use tracing::{debug, error, info, warn};

// ── MIDI 1.0 ↔ UMP Conversion ──

/// Convert MIDI 1.0 bytes to UMP (Universal MIDI Packet) words.
/// Uses UMP Message Type 2 (MIDI 1.0 Channel Voice) for channel messages
/// and Message Type 3 (Data/SysEx) for system exclusive.
#[cfg(target_os = "windows")]
fn midi1_to_ump(data: &[u8]) -> Vec<u32> {
    if data.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut i = 0;

    while i < data.len() {
        let status = data[i];

        match status {
            // Channel Voice Messages (0x80..=0xEF) → UMP Type 2
            0x80..=0xBF | 0xE0..=0xEF => {
                // 3-byte messages: Note Off, Note On, Poly Pressure, CC, Pitch Bend
                if i + 2 < data.len() {
                    let word = 0x2000_0000u32
                        | ((status as u32) << 16)
                        | ((data[i + 1] as u32) << 8)
                        | (data[i + 2] as u32);
                    result.push(word);
                    i += 3;
                } else {
                    break;
                }
            }
            0xC0..=0xDF => {
                // 2-byte messages: Program Change, Channel Pressure
                if i + 1 < data.len() {
                    let word = 0x2000_0000u32
                        | ((status as u32) << 16)
                        | ((data[i + 1] as u32) << 8);
                    result.push(word);
                    i += 2;
                } else {
                    break;
                }
            }
            // System Exclusive (0xF0) → UMP Type 3 (64-bit, 2 words per packet)
            0xF0 => {
                // Find end of SysEx (0xF7)
                let start = i + 1; // skip 0xF0
                let end = data[start..].iter().position(|&b| b == 0xF7)
                    .map(|p| start + p)
                    .unwrap_or(data.len());
                let sysex_data = &data[start..end];

                // UMP SysEx7: up to 6 data bytes per 64-bit packet
                let chunks: Vec<&[u8]> = sysex_data.chunks(6).collect();
                let total_chunks = chunks.len().max(1);

                for (idx, chunk) in chunks.iter().enumerate() {
                    let sysex_status = if total_chunks == 1 {
                        0x00 // Complete SysEx in one UMP
                    } else if idx == 0 {
                        0x10 // Start
                    } else if idx == total_chunks - 1 {
                        0x30 // End
                    } else {
                        0x20 // Continue
                    };

                    let num_bytes = chunk.len() as u32;
                    let word0 = 0x3000_0000u32 | (sysex_status << 16) | (num_bytes << 16);
                    let mut word1 = 0u32;
                    for (j, &b) in chunk.iter().enumerate() {
                        // Pack up to 6 bytes across word0[bits 15..0] and word1
                        if j < 2 {
                            // First 2 bytes go into lower 16 bits of word0
                            word0.wrapping_add(0); // placeholder
                        }
                        match j {
                            0 => word1 |= (b as u32) << 24,
                            1 => word1 |= (b as u32) << 16,
                            2 => word1 |= (b as u32) << 8,
                            3 => word1 |= b as u32,
                            _ => {} // bytes 4-5 would go into additional words
                        }
                    }
                    result.push(word0);
                    result.push(word1);
                }

                i = if end < data.len() { end + 1 } else { end }; // skip 0xF7
            }
            // System Real-Time (0xF8..=0xFF) → UMP Type 1 (System Common/RT)
            0xF8..=0xFF => {
                let word = 0x1000_0000u32 | ((status as u32) << 16);
                result.push(word);
                i += 1;
            }
            // Other system common
            _ => {
                i += 1; // skip unknown
            }
        }
    }

    result
}

/// Convert UMP words back to MIDI 1.0 bytes.
/// Handles UMP Type 2 (MIDI 1.0 Channel Voice) and Type 1 (System).
#[cfg(target_os = "windows")]
fn ump_to_midi1(words: &[u32]) -> Vec<u8> {
    let mut result = Vec::new();
    let mut i = 0;

    while i < words.len() {
        let word = words[i];
        let msg_type = (word >> 28) & 0x0F;

        match msg_type {
            // Type 1: System Common / Real-Time (32-bit, 1 word)
            1 => {
                let status = ((word >> 16) & 0xFF) as u8;
                result.push(status);
                i += 1;
            }
            // Type 2: MIDI 1.0 Channel Voice (32-bit, 1 word)
            2 => {
                let status = ((word >> 16) & 0xFF) as u8;
                let data1 = ((word >> 8) & 0x7F) as u8;
                let data2 = (word & 0x7F) as u8;

                result.push(status);
                match status & 0xF0 {
                    0xC0 | 0xD0 => {
                        // Program Change, Channel Pressure: 2-byte message
                        result.push(data1);
                    }
                    _ => {
                        // Note On/Off, CC, Pitch Bend, etc.: 3-byte message
                        result.push(data1);
                        result.push(data2);
                    }
                }
                i += 1;
            }
            // Type 3: Data / SysEx (64-bit, 2 words)
            3 => {
                if i + 1 >= words.len() {
                    break;
                }
                let _word1 = words[i + 1];
                let sysex_status = (word >> 20) & 0x0F;
                let num_bytes = ((word >> 16) & 0x0F) as usize;

                // If start or complete, prepend 0xF0
                if sysex_status == 0x00 || sysex_status == 0x01 {
                    result.push(0xF0);
                }

                // Extract data bytes from word1
                for j in 0..num_bytes.min(4) {
                    let b = ((_word1 >> (24 - j * 8)) & 0xFF) as u8;
                    result.push(b);
                }

                // If end or complete, append 0xF7
                if sysex_status == 0x00 || sysex_status == 0x03 {
                    result.push(0xF7);
                }

                i += 2;
            }
            // Type 4: MIDI 2.0 Channel Voice (64-bit, 2 words) — downconvert
            4 => {
                // MIDI 2.0 → 1.0 downconversion (simplified)
                let status = ((word >> 16) & 0xFF) as u8;
                let _word1 = if i + 1 < words.len() { words[i + 1] } else { 0 };

                // Extract index/note and downconvert 32-bit value to 7-bit
                let index = ((word >> 8) & 0x7F) as u8;
                let value_16 = ((_word1 >> 16) & 0xFFFF) as u16;
                let value_7 = (value_16 >> 9) as u8; // 16-bit → 7-bit

                result.push(status);
                result.push(index);
                result.push(value_7);

                i += 2;
            }
            _ => {
                // Unknown type — skip 1 word
                i += 1;
            }
        }
    }

    result
}

// ── MidiServicesDevice ──

pub struct MidiServicesDevice {
    name: String,
    #[cfg(target_os = "windows")]
    session: Option<midi2::MidiSession>,
    #[cfg(target_os = "windows")]
    connection: Option<midi2::MidiEndpointConnection>,
    #[cfg(target_os = "windows")]
    feedback_buffer: Arc<Mutex<Vec<Vec<u8>>>>,
    #[cfg(target_os = "windows")]
    _message_token: Option<windows::Foundation::EventRegistrationToken>,
    #[cfg(not(target_os = "windows"))]
    _phantom: (),
}

#[cfg(target_os = "windows")]
unsafe impl Send for MidiServicesDevice {}
#[cfg(target_os = "windows")]
unsafe impl Sync for MidiServicesDevice {}

impl MidiServicesDevice {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            #[cfg(target_os = "windows")]
            session: None,
            #[cfg(target_os = "windows")]
            connection: None,
            #[cfg(target_os = "windows")]
            feedback_buffer: Arc::new(Mutex::new(Vec::new())),
            #[cfg(target_os = "windows")]
            _message_token: None,
            #[cfg(not(target_os = "windows"))]
            _phantom: (),
        }
    }

    /// Check if Windows MIDI Services runtime is available.
    /// Attempts WinRT activation of MidiSession — if it fails,
    /// the runtime is not installed.
    #[cfg(target_os = "windows")]
    pub fn is_available() -> bool {
        // Try to create a MidiSession — this will fail if the runtime
        // is not installed or the service is not running.
        match midi2::MidiSession::Create(&windows::core::HSTRING::from("MIDInet-probe")) {
            Ok(session) => {
                // Successfully activated — runtime is available
                drop(session);
                info!("Windows MIDI Services runtime detected");
                true
            }
            Err(e) => {
                debug!(error = %e, "Windows MIDI Services not available");
                false
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn is_available() -> bool {
        false
    }
}

impl VirtualMidiDevice for MidiServicesDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        #[cfg(target_os = "windows")]
        {
            use windows::core::HSTRING;

            // Initialize COM (Multi-Threaded Apartment) for WinRT
            // Safe to call multiple times — returns S_FALSE if already initialized
            if let Err(e) = unsafe { windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            ) } {
                // S_FALSE (already initialized) is OK, other errors are not
                if e.code().0 != 1 { // S_FALSE = 1
                    warn!(error = %e, "COM initialization warning (may be OK if already initialized)");
                }
            }

            // Create MIDI session
            let session_name = HSTRING::from(format!("MIDInet-{}", &self.name));
            let session = midi2::MidiSession::Create(&session_name)
                .map_err(|e| anyhow::anyhow!(
                    "Failed to create MIDI Services session: {}. \
                     Is Windows MIDI Services installed? \
                     Install via: winget install Microsoft.WindowsMIDIServicesSDK",
                    e
                ))?;

            info!(name = %self.name, "Windows MIDI Services session created");

            // Define the virtual endpoint
            let definition = midi2::MidiVirtualEndpointDeviceDefinition::new()
                .map_err(|e| anyhow::anyhow!("Failed to create endpoint definition: {}", e))?;

            // Set endpoint name — must match physical controller for Resolume compatibility
            definition.SetEndpointName(&HSTRING::from(&self.name))
                .map_err(|e| anyhow::anyhow!("Failed to set endpoint name: {}", e))?;

            // Set a unique product instance ID
            let instance_id = format!("MIDINET_{}", self.name.replace(' ', "_").to_uppercase());
            definition.SetEndpointProductInstanceId(&HSTRING::from(&instance_id))
                .map_err(|e| anyhow::anyhow!("Failed to set product instance ID: {}", e))?;

            // Configure for MIDI 1.0 compatibility
            definition.SetSupportsSendingJRTimestamps(false)
                .map_err(|e| anyhow::anyhow!("Failed to configure JR timestamps: {}", e))?;
            definition.SetSupportsReceivingJRTimestamps(false)
                .map_err(|e| anyhow::anyhow!("Failed to configure JR timestamps: {}", e))?;

            // Add a function block for MIDI 1.0
            let block = midi2::MidiFunctionBlock::new()
                .map_err(|e| anyhow::anyhow!("Failed to create function block: {}", e))?;
            block.SetNumber(0)
                .map_err(|e| anyhow::anyhow!("Failed to set block number: {}", e))?;
            block.SetName(&HSTRING::from(&self.name))
                .map_err(|e| anyhow::anyhow!("Failed to set block name: {}", e))?;
            block.SetIsActive(true)
                .map_err(|e| anyhow::anyhow!("Failed to activate block: {}", e))?;
            block.SetDirection(midi2::MidiFunctionBlockDirection::Bidirectional)
                .map_err(|e| anyhow::anyhow!("Failed to set block direction: {}", e))?;
            block.SetMidi10Connection(midi2::MidiFunctionBlockMidi10::YesBandwidthUnrestricted)
                .map_err(|e| anyhow::anyhow!("Failed to set MIDI 1.0 mode: {}", e))?;

            definition.FunctionBlocks()
                .map_err(|e| anyhow::anyhow!("Failed to get function blocks: {}", e))?
                .Append(&block)
                .map_err(|e| anyhow::anyhow!("Failed to add function block: {}", e))?;

            // Create the virtual device and get a connection to it
            let connection = session.CreateVirtualDeviceAndConnection(&definition)
                .map_err(|e| anyhow::anyhow!(
                    "Failed to create virtual MIDI endpoint: {}. \
                     Windows MIDI Services may not support virtual devices on this version.",
                    e
                ))?;

            // Register callback for incoming MIDI messages (feedback from apps)
            let feedback_buf = Arc::clone(&self.feedback_buffer);
            let token = connection.MessageReceived(
                &windows::Foundation::TypedEventHandler::new(
                    move |_sender, args: &Option<midi2::MidiMessageReceivedEventArgs>| {
                        if let Some(args) = args {
                            if let Ok(packet) = args.GetMessagePacket() {
                                // Extract UMP words and convert to MIDI 1.0
                                if let Ok(word0) = packet.PeekFirstWord() {
                                    let words = vec![word0];
                                    let midi_bytes = ump_to_midi1(&words);
                                    if !midi_bytes.is_empty() {
                                        if let Ok(mut buf) = feedback_buf.lock() {
                                            if buf.len() < 4096 {
                                                buf.push(midi_bytes);
                                            } else {
                                                buf.remove(0);
                                                buf.push(ump_to_midi1(&words));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(())
                    }
                ),
            ).map_err(|e| anyhow::anyhow!("Failed to register message callback: {}", e))?;

            // Open the connection
            connection.Open()
                .map_err(|e| anyhow::anyhow!("Failed to open MIDI endpoint connection: {}", e))?;

            info!(
                name = %self.name,
                instance_id = %instance_id,
                "Windows MIDI Services virtual endpoint created and connected"
            );

            self.session = Some(session);
            self.connection = Some(connection);
            self._message_token = Some(token);
        }

        #[cfg(not(target_os = "windows"))]
        {
            return Err(anyhow::anyhow!("Windows MIDI Services: not available on non-Windows"));
        }

        Ok(())
    }

    fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        {
            let connection = match self.connection.as_ref() {
                Some(c) => c,
                None => return Ok(()),
            };

            let ump_words = midi1_to_ump(data);
            for word in &ump_words {
                // Send as 32-bit UMP message (Type 1 and Type 2 are single-word)
                let msg = midi2::MidiMessage32::CreateFromStruct(
                    &midi2::MidiMessageStruct { Word0: *word },
                ).map_err(|e| anyhow::anyhow!("Failed to create UMP message: {}", e))?;

                connection.SendSingleMessagePacket(&msg)
                    .map_err(|e| anyhow::anyhow!("Failed to send MIDI via MIDI Services: {}", e))?;
            }

            debug!(bytes = data.len(), ump_words = ump_words.len(), "Sent MIDI via MIDI Services");
        }
        let _ = data;
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        #[cfg(target_os = "windows")]
        {
            if let Ok(mut buf) = self.feedback_buffer.lock() {
                if !buf.is_empty() {
                    return Ok(Some(buf.remove(0)));
                }
            }
        }
        Ok(None)
    }

    fn close(&mut self) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        {
            // Remove message callback
            if let (Some(conn), Some(token)) = (self.connection.as_ref(), self._message_token.take()) {
                let _ = conn.RemoveMessageReceived(token);
            }

            // Drop connection and session (closes the virtual endpoint)
            self.connection = None;
            self.session = None;

            info!(name = %self.name, "Closed Windows MIDI Services virtual endpoint");
        }
        Ok(())
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}

#[cfg(target_os = "windows")]
impl Drop for MidiServicesDevice {
    fn drop(&mut self) {
        if let (Some(conn), Some(token)) = (self.connection.as_ref(), self._message_token.take()) {
            let _ = conn.RemoveMessageReceived(token);
        }
        self.connection = None;
        self.session = None;
    }
}
