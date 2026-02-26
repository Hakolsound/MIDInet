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
#[allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    clippy::all
)]
mod bindings {
    include!(concat!(env!("OUT_DIR"), "/midi2_bindings.rs"));
}

#[cfg(target_os = "windows")]
use std::sync::{Arc, Mutex};

#[cfg(target_os = "windows")]
use bindings::Windows::Devices::Midi2 as midi2;

use crate::virtual_device::VirtualMidiDevice;
use midi_protocol::identity::DeviceIdentity;
use tracing::{debug, info, warn};

// ── MIDI 1.0 → UMP Conversion ──

/// Convert MIDI 1.0 bytes to UMP word(s).
/// Returns a Vec of (word_count, word0, word1) tuples.
/// Type 2 (Channel Voice) = 1 word. Type 3 (SysEx) = 2 words.
#[cfg(target_os = "windows")]
fn midi1_to_ump_messages(data: &[u8]) -> Vec<(u8, u32, u32)> {
    if data.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut i = 0;

    while i < data.len() {
        let status = data[i];

        match status {
            // Channel Voice Messages (0x80..=0xEF) → UMP Type 2, 1 word
            0x80..=0xBF | 0xE0..=0xEF => {
                if i + 2 < data.len() {
                    let word0 = 0x2000_0000u32
                        | ((status as u32) << 16)
                        | ((data[i + 1] as u32) << 8)
                        | (data[i + 2] as u32);
                    result.push((1, word0, 0));
                    i += 3;
                } else {
                    break;
                }
            }
            0xC0..=0xDF => {
                if i + 1 < data.len() {
                    let word0 = 0x2000_0000u32
                        | ((status as u32) << 16)
                        | ((data[i + 1] as u32) << 8);
                    result.push((1, word0, 0));
                    i += 2;
                } else {
                    break;
                }
            }
            // System Exclusive (0xF0) → UMP Type 3, 2 words per packet
            0xF0 => {
                let start = i + 1;
                let end = data[start..].iter().position(|&b| b == 0xF7)
                    .map(|p| start + p)
                    .unwrap_or(data.len());
                let sysex_data = &data[start..end];

                let chunks: Vec<&[u8]> = sysex_data.chunks(6).collect();
                let total_chunks = chunks.len().max(1);

                for (idx, chunk) in chunks.iter().enumerate() {
                    let sysex_status: u32 = if total_chunks == 1 {
                        0x00 // Complete
                    } else if idx == 0 {
                        0x01 // Start
                    } else if idx == total_chunks - 1 {
                        0x03 // End
                    } else {
                        0x02 // Continue
                    };

                    let num_bytes = chunk.len() as u32;
                    // Type 3, group 0, status | num_bytes in upper nibbles
                    let word0 = 0x3000_0000u32
                        | (sysex_status << 20)
                        | (num_bytes << 16);
                    let mut word1 = 0u32;
                    for (j, &b) in chunk.iter().enumerate() {
                        match j {
                            0 => word1 |= (b as u32) << 24,
                            1 => word1 |= (b as u32) << 16,
                            2 => word1 |= (b as u32) << 8,
                            3 => word1 |= b as u32,
                            // bytes 4-5 go into lower bits of word0
                            4 => { /* word0 lower bits - simplified */ }
                            5 => { /* word0 lower bits - simplified */ }
                            _ => {}
                        }
                    }
                    result.push((2, word0, word1));
                }

                i = if end < data.len() { end + 1 } else { end };
            }
            // System Real-Time (0xF8..=0xFF) → UMP Type 1, 1 word
            0xF8..=0xFF => {
                let word0 = 0x1000_0000u32 | ((status as u32) << 16);
                result.push((1, word0, 0));
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    result
}

/// Convert UMP words (from FillWords) back to MIDI 1.0 bytes.
#[cfg(target_os = "windows")]
fn ump_to_midi1(word_count: u8, w0: u32, w1: u32) -> Vec<u8> {
    let mut result = Vec::new();
    let msg_type = (w0 >> 28) & 0x0F;

    match msg_type {
        // Type 1: System Common / Real-Time
        1 => {
            let status = ((w0 >> 16) & 0xFF) as u8;
            result.push(status);
        }
        // Type 2: MIDI 1.0 Channel Voice
        2 => {
            let status = ((w0 >> 16) & 0xFF) as u8;
            let data1 = ((w0 >> 8) & 0x7F) as u8;
            let data2 = (w0 & 0x7F) as u8;

            result.push(status);
            match status & 0xF0 {
                0xC0 | 0xD0 => result.push(data1),
                _ => {
                    result.push(data1);
                    result.push(data2);
                }
            }
        }
        // Type 3: SysEx
        3 if word_count >= 2 => {
            let sysex_status = (w0 >> 20) & 0x0F;
            let num_bytes = ((w0 >> 16) & 0x0F) as usize;

            if sysex_status == 0x00 || sysex_status == 0x01 {
                result.push(0xF0);
            }
            for j in 0..num_bytes.min(4) {
                result.push(((w1 >> (24 - j * 8)) & 0xFF) as u8);
            }
            if sysex_status == 0x00 || sysex_status == 0x03 {
                result.push(0xF7);
            }
        }
        // Type 4: MIDI 2.0 Channel Voice — downconvert
        4 if word_count >= 2 => {
            let status = ((w0 >> 16) & 0xFF) as u8;
            let index = ((w0 >> 8) & 0x7F) as u8;
            let value_16 = ((w1 >> 16) & 0xFFFF) as u16;
            let value_7 = (value_16 >> 9) as u8;
            result.push(status);
            result.push(index);
            result.push(value_7);
        }
        _ => {}
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
    _virtual_device: Option<midi2::Endpoints::Virtual::MidiVirtualDevice>,
    #[cfg(target_os = "windows")]
    feedback_buffer: Arc<Mutex<Vec<Vec<u8>>>>,
    #[cfg(target_os = "windows")]
    _message_token: Option<windows::Foundation::EventRegistrationToken>,
    /// COM bootstrapper that installs Detours hooks for WinRT class activation.
    /// Must be kept alive for the entire lifetime of the MIDI session.
    #[cfg(target_os = "windows")]
    _initializer: Option<windows::core::IUnknown>,
    /// When true, Drop skips teardown — the OS cleans up handles on process exit.
    /// This avoids a bug in Midi2.VirtualMidiTransport.dll that causes midisrv.exe
    /// to crash (access violation) when a virtual device session is explicitly closed
    /// while other apps (e.g. Resolume) hold open handles to the endpoint.
    #[cfg(target_os = "windows")]
    detached: bool,
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
            _virtual_device: None,
            #[cfg(target_os = "windows")]
            feedback_buffer: Arc::new(Mutex::new(Vec::new())),
            #[cfg(target_os = "windows")]
            _message_token: None,
            #[cfg(target_os = "windows")]
            _initializer: None,
            #[cfg(target_os = "windows")]
            detached: false,
            #[cfg(not(target_os = "windows"))]
            _phantom: (),
        }
    }

    /// Check if Windows MIDI Services runtime is available.
    #[cfg(target_os = "windows")]
    pub fn is_available() -> bool {
        match midi2::MidiSession::Create(&windows_core::HSTRING::from("MIDInet-probe")) {
            Ok(session) => {
                let _ = session.Close();
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
            use windows_core::HSTRING;

            // ── Step 1: Initialize WinRT runtime (MTA) ──
            // Required before any WinRT class activation. MTA is compatible with tokio's
            // multi-threaded runtime. Returns S_FALSE if already initialized — that's fine.
            info!(name = %self.name, "Initializing WinRT runtime for MIDI Services...");
            unsafe {
                windows::Win32::System::WinRT::RoInitialize(
                    windows::Win32::System::WinRT::RO_INIT_MULTITHREADED,
                ).map_err(|e| anyhow::anyhow!(
                    "Failed to initialize WinRT: {} (0x{:08X})",
                    e, e.code().0 as u32
                ))?;
            }

            // ── Step 2: Bootstrap the MIDI Services SDK ──
            // The SDK uses side-loaded WinRT components that aren't in the standard
            // Windows Runtime activation catalog. The bootstrapper COM object
            // (WindowsMidiServicesClientInitialization.dll) installs Microsoft Detours
            // hooks that intercept RoActivateInstance/RoGetActivationFactory for all
            // Microsoft.Windows.Devices.Midi2.* types and redirect them to the SDK's
            // implementation DLL. Without this, MidiSession::Create() fails with
            // "Class not registered" (0x80040154).
            const CLSID_MIDI_CLIENT_INITIALIZER: windows::core::GUID =
                windows::core::GUID::from_u128(0xc3263827_c3b0_bdbd_2500_ce63a3f3f2c3);

            info!(name = %self.name, "Creating MIDI SDK bootstrapper (Detours activation)...");
            let initializer: windows::core::IUnknown = unsafe {
                windows::Win32::System::Com::CoCreateInstance(
                    &CLSID_MIDI_CLIENT_INITIALIZER,
                    None,
                    windows::Win32::System::Com::CLSCTX_INPROC_SERVER,
                )
            }.map_err(|e| anyhow::anyhow!(
                "Failed to create MIDI SDK bootstrapper: {} (0x{:08X}). \
                 Install Windows MIDI Services SDK via: winget install Microsoft.WindowsMIDIServicesSDK",
                e, e.code().0 as u32
            ))?;
            info!(name = %self.name, "MIDI SDK bootstrapper active — WinRT activation hooks installed");

            // ── Step 3: Create MIDI session ──
            let session_name = HSTRING::from(format!("MIDInet-{}", &self.name));
            let session = midi2::MidiSession::Create(&session_name)
                .map_err(|e| anyhow::anyhow!(
                    "Failed to create MIDI Services session: {} (0x{:08X})",
                    e, e.code().0 as u32
                ))?;

            info!(name = %self.name, "Windows MIDI Services session created");

            // Build endpoint info struct
            let instance_id = format!("MIDINET_{}", self.name.replace(' ', "_").to_uppercase());
            let endpoint_info = midi2::MidiDeclaredEndpointInfo {
                Name: HSTRING::from(&self.name),
                ProductInstanceId: HSTRING::from(&instance_id),
                SupportsMidi10Protocol: true,
                SupportsMidi20Protocol: false,
                SupportsReceivingJitterReductionTimestamps: false,
                SupportsSendingJitterReductionTimestamps: false,
                HasStaticFunctionBlocks: true,
                DeclaredFunctionBlockCount: 1,
                SpecificationVersionMajor: 1,
                SpecificationVersionMinor: 0,
            };

            // Create virtual device config
            let config = midi2::Endpoints::Virtual::MidiVirtualDeviceCreationConfig::CreateInstance(
                &HSTRING::from(&self.name),
                &HSTRING::from(format!("MIDInet virtual device: {}", &self.name)),
                &HSTRING::from(&identity.manufacturer),
                &endpoint_info,
            ).map_err(|e| anyhow::anyhow!("Failed to create virtual device config: {}", e))?;

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
            block.SetRepresentsMidi10Connection(
                midi2::MidiFunctionBlockRepresentsMidi10Connection::YesBandwidthUnrestricted,
            ).map_err(|e| anyhow::anyhow!("Failed to set MIDI 1.0 mode: {}", e))?;
            // FirstGroup and GroupCount are required — without them CreateVirtualDevice returns null
            let group0 = midi2::MidiGroup::CreateInstance(0)
                .map_err(|e| anyhow::anyhow!("Failed to create MidiGroup(0): {}", e))?;
            block.SetFirstGroup(&group0)
                .map_err(|e| anyhow::anyhow!("Failed to set first group: {}", e))?;
            block.SetGroupCount(1)
                .map_err(|e| anyhow::anyhow!("Failed to set group count: {}", e))?;

            config.FunctionBlocks()
                .map_err(|e| anyhow::anyhow!("Failed to get function blocks: {}", e))?
                .Append(&block)
                .map_err(|e| anyhow::anyhow!("Failed to add function block: {}", e))?;

            info!(name = %self.name, "Virtual device config ready, creating device...");

            // Create the virtual device via the manager
            let virtual_device = midi2::Endpoints::Virtual::MidiVirtualDeviceManager::CreateVirtualDevice(&config)
                .map_err(|e| anyhow::anyhow!(
                    "Failed to create virtual MIDI device: {} (0x{:08X}). \
                     Windows MIDI Services may not support virtual devices on this version.",
                    e, e.code().0 as u32
                ))?;

            // Connect to the DEVICE-side endpoint (not the client-side).
            // The device-side is what we send/receive through; the client-side
            // is what other apps (DAWs, Resolume) see and connect to.
            let device_endpoint_id = virtual_device.DeviceEndpointDeviceId()
                .map_err(|e| anyhow::anyhow!("Failed to get device endpoint ID: {}", e))?;

            info!(
                name = %self.name,
                device_endpoint = %device_endpoint_id,
                "Virtual device created, connecting to device-side endpoint..."
            );

            // Create endpoint connection via session
            let connection = session.CreateEndpointConnection(&device_endpoint_id)
                .map_err(|e| anyhow::anyhow!("Failed to create endpoint connection: {}", e))?;

            // Register the virtual device as a message processing plugin.
            // This is MANDATORY — without it the virtual device won't handle
            // protocol negotiation or endpoint discovery messages.
            connection.AddMessageProcessingPlugin(&virtual_device)
                .map_err(|e| anyhow::anyhow!("Failed to add virtual device as message plugin: {}", e))?;

            // Register callback for incoming MIDI messages (feedback from apps)
            let feedback_buf = Arc::clone(&self.feedback_buffer);
            let token = connection.MessageReceived(
                &windows::Foundation::TypedEventHandler::new(
                    move |_sender, args: &Option<midi2::MidiMessageReceivedEventArgs>| {
                        if let Some(args) = args {
                            let mut w0 = 0u32;
                            let mut w1 = 0u32;
                            let mut w2 = 0u32;
                            let mut w3 = 0u32;
                            if let Ok(word_count) = args.FillWords(&mut w0, &mut w1, &mut w2, &mut w3) {
                                let midi_bytes = ump_to_midi1(word_count, w0, w1);
                                if !midi_bytes.is_empty() {
                                    if let Ok(mut buf) = feedback_buf.lock() {
                                        if buf.len() < 4096 {
                                            buf.push(midi_bytes);
                                        } else {
                                            buf.remove(0);
                                            buf.push(ump_to_midi1(word_count, w0, w1));
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
            let opened = connection.Open()
                .map_err(|e| anyhow::anyhow!("Failed to open MIDI endpoint connection: {}", e))?;

            if !opened {
                warn!(name = %self.name, "Connection.Open() returned false — endpoint may not be fully ready");
            }

            info!(
                name = %self.name,
                instance_id = %instance_id,
                "Windows MIDI Services virtual endpoint created and connected"
            );

            self.session = Some(session);
            self.connection = Some(connection);
            self._virtual_device = Some(virtual_device);
            self._message_token = Some(token);
            self._initializer = Some(initializer);
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

            let messages = midi1_to_ump_messages(data);
            for &(word_count, word0, word1) in &messages {
                let _result = match word_count {
                    1 => connection.SendSingleMessageWords(0, word0),
                    2 => connection.SendSingleMessageWords2(0, word0, word1),
                    _ => continue,
                }.map_err(|e| anyhow::anyhow!("Failed to send MIDI via MIDI Services: {}", e))?;
            }

            debug!(bytes = data.len(), messages = messages.len(), "Sent MIDI via MIDI Services");
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
            if self.detached {
                info!(name = %self.name, "MIDI Services device detached — skipping explicit close");
                return Ok(());
            }

            if let (Some(conn), Some(token)) = (self.connection.as_ref(), self._message_token.take()) {
                let _ = conn.RemoveMessageReceived(token);
            }

            self.connection = None;
            self._virtual_device = None;
            if let Some(session) = self.session.take() {
                let _ = session.Close();
            }
            // Release the bootstrapper LAST — dropping it removes the Detours hooks
            self._initializer = None;

            info!(name = %self.name, "Closed Windows MIDI Services virtual endpoint");
        }
        Ok(())
    }

    fn silence_and_detach(&mut self) -> anyhow::Result<()> {
        self.send_all_off()?;

        #[cfg(target_os = "windows")]
        {
            self.detached = true;
            info!(
                name = %self.name,
                "MIDI Services device silenced and detached — port stays alive until process exit"
            );
        }

        #[cfg(not(target_os = "windows"))]
        {
            self.close()?;
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
        if self.detached {
            // Detached mode: let the OS clean up COM handles on process exit.
            // Explicit teardown triggers a bug in Midi2.VirtualMidiTransport.dll
            // (access violation in midisrv.exe) when apps hold open handles.
            return;
        }
        if let (Some(conn), Some(token)) = (self.connection.as_ref(), self._message_token.take()) {
            let _ = conn.RemoveMessageReceived(token);
        }
        self.connection = None;
        self._virtual_device = None;
        if let Some(session) = self.session.take() {
            let _ = session.Close();
        }
        // Release bootstrapper last — removes Detours hooks
        self._initializer = None;
    }
}
