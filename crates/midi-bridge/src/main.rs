/// midi-bridge: Sidecar process that owns the virtual MIDI device.
///
/// The bridge creates and maintains the virtual MIDI device independently
/// of the midi-client process. When the client connects via IPC, MIDI data
/// flows bidirectionally through the bridge. When the client disconnects
/// (restart, crash, update), the bridge silences the device but keeps it
/// alive - apps like Resolume Arena never see the device disappear.
///
/// Architecture:
///   1. Bridge starts, creates IPC listener (Unix domain socket / Windows named pipe)
///   2. Client connects, sends DeviceIdentity -> bridge creates virtual device
///   3. Client sends MIDI -> bridge forwards to virtual device -> apps receive it
///   4. Apps send feedback MIDI -> bridge receives it -> forwards to client
///   5. Client disconnects -> bridge sends All Notes Off, waits for reconnect
///   6. Client reconnects -> bridge resumes forwarding (no device recreation)

use std::io::{Read as IoRead, Write as IoWrite};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use midi_device::VirtualMidiDevice;
use midi_protocol::bridge_ipc::{self, AckPayload, FrameType, BRIDGE_SOCKET_PATH, HEADER_SIZE};
use midi_protocol::identity::DeviceIdentity;
use tracing::{debug, error, info, warn};

/// Bridge shared state.
struct BridgeState {
    /// The virtual MIDI device (created on first client connection).
    device: Mutex<Option<Box<dyn VirtualMidiDevice>>>,
    /// Device identity (set by first client).
    identity: Mutex<Option<DeviceIdentity>>,
    /// Whether a client is currently connected.
    client_connected: AtomicBool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(socket = BRIDGE_SOCKET_PATH, "MIDInet bridge starting");

    let state = Arc::new(BridgeState {
        device: Mutex::new(None),
        identity: Mutex::new(None),
        client_connected: AtomicBool::new(false),
    });

    #[cfg(unix)]
    run_unix(&state)?;

    #[cfg(windows)]
    run_windows(&state)?;

    // Cleanup
    info!("Bridge shutting down, closing device...");
    if let Ok(mut dev) = state.device.lock() {
        if let Some(ref mut d) = *dev {
            let _ = d.silence_and_detach();
        }
    }

    info!("Bridge stopped");
    Ok(())
}

// ── Unix: Unix domain socket ──

#[cfg(unix)]
fn run_unix(state: &Arc<BridgeState>) -> anyhow::Result<()> {
    let _ = std::fs::remove_file(BRIDGE_SOCKET_PATH);
    let listener = std::os::unix::net::UnixListener::bind(BRIDGE_SOCKET_PATH)?;
    info!(path = BRIDGE_SOCKET_PATH, "Bridge listening");
    listener.set_nonblocking(false)?;

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                info!("Client connected");
                handle_client(state, stream);
                info!("Waiting for next client connection...");
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                info!("Interrupted, shutting down");
                break;
            }
            Err(e) => {
                error!(error = %e, "Accept error");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    let _ = std::fs::remove_file(BRIDGE_SOCKET_PATH);
    Ok(())
}

// ── Windows: Named pipe ──

#[cfg(windows)]
fn run_windows(state: &Arc<BridgeState>) -> anyhow::Result<()> {
    use windows::core::HSTRING;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_ACCESS_DUPLEX,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    let pipe_name = HSTRING::from(BRIDGE_SOCKET_PATH);

    info!(path = BRIDGE_SOCKET_PATH, "Bridge listening on named pipe");

    loop {
        // Create a new named pipe instance for each client
        let pipe_handle = unsafe {
            CreateNamedPipeW(
                &pipe_name,
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES.0,
                4096,
                4096,
                0,
                None,
            )
        };

        let pipe_handle = match pipe_handle {
            Ok(h) => h,
            Err(e) => {
                error!(error = %e, "Failed to create named pipe");
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        // Wait for client to connect (blocking)
        let connect_result = unsafe { ConnectNamedPipe(pipe_handle, None) };
        if let Err(e) = connect_result {
            // ERROR_PIPE_CONNECTED (535) means client connected between Create and Connect
            let win_err = e.code().0 as u32;
            if win_err != 0x80070217 {
                // Not ERROR_PIPE_CONNECTED
                error!(error = %e, "ConnectNamedPipe failed");
                unsafe {
                    let _ = windows::Win32::Foundation::CloseHandle(pipe_handle);
                };
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        }

        info!("Client connected via named pipe");

        let stream = PipeStream {
            handle: pipe_handle,
        };
        handle_client(state, stream);

        unsafe {
            let _ = DisconnectNamedPipe(pipe_handle);
            let _ = windows::Win32::Foundation::CloseHandle(pipe_handle);
        };

        info!("Waiting for next client connection...");
    }
}

/// Wrapper around a Windows named pipe handle that implements Read + Write.
#[cfg(windows)]
struct PipeStream {
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
unsafe impl Send for PipeStream {}

#[cfg(windows)]
impl IoRead for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut bytes_read: u32 = 0;
        unsafe {
            windows::Win32::Storage::FileSystem::ReadFile(
                self.handle,
                Some(buf),
                Some(&mut bytes_read),
                None,
            )
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 as i32))?;
        }
        Ok(bytes_read as usize)
    }
}

#[cfg(windows)]
impl IoWrite for PipeStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut bytes_written: u32 = 0;
        unsafe {
            windows::Win32::Storage::FileSystem::WriteFile(
                self.handle,
                Some(buf),
                Some(&mut bytes_written),
                None,
            )
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 as i32))?;
        }
        Ok(bytes_written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        unsafe {
            windows::Win32::Storage::FileSystem::FlushFileBuffers(self.handle)
                .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 as i32))?;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl PipeStream {
    fn try_clone(&self) -> std::io::Result<Self> {
        let process =
            unsafe { windows::Win32::System::Threading::GetCurrentProcess() };
        let mut new_handle = windows::Win32::Foundation::HANDLE::default();
        unsafe {
            windows::Win32::Foundation::DuplicateHandle(
                process,
                self.handle,
                process,
                &mut new_handle,
                0,
                false,
                windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
            .map_err(|e| std::io::Error::from_raw_os_error(e.code().0 as i32))?;
        }
        Ok(PipeStream {
            handle: new_handle,
        })
    }
}

// ── Client handler (platform-agnostic) ──

/// Handle a connected client session.
///
/// Runs synchronously (only one client at a time).
/// Spawns a feedback thread for device->client direction.
fn handle_client<S: IoRead + IoWrite + Send + 'static>(
    state: &Arc<BridgeState>,
    mut stream: S,
) where
    S: TryClone,
{
    state.client_connected.store(true, Ordering::SeqCst);

    // Read the Identity frame
    let identity = match read_identity(&mut stream) {
        Ok(id) => id,
        Err(e) => {
            error!(error = %e, "Failed to read identity from client");
            state.client_connected.store(false, Ordering::SeqCst);
            return;
        }
    };

    // Create or reuse the virtual device
    let created = ensure_device(state, &identity);

    // Send Ack
    let ack = AckPayload {
        created,
        device_name: identity.name.clone(),
    };
    let ack_json = serde_json::to_vec(&ack).unwrap_or_default();
    let mut ack_buf = vec![0u8; HEADER_SIZE + ack_json.len()];
    let ack_len = bridge_ipc::encode_frame(&mut ack_buf, FrameType::Ack, &ack_json);
    if let Err(e) = stream.write_all(&ack_buf[..ack_len]) {
        error!(error = %e, "Failed to send ack to client");
        state.client_connected.store(false, Ordering::SeqCst);
        return;
    }

    info!(name = %identity.name, created, "Client handshake complete");

    // Spawn feedback thread: reads from virtual device -> writes to client
    let feedback_running = Arc::new(AtomicBool::new(true));
    let feedback_flag = Arc::clone(&feedback_running);
    let feedback_state = Arc::clone(state);
    let mut feedback_stream = match stream.try_clone_stream() {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "Failed to clone stream for feedback");
            state.client_connected.store(false, Ordering::SeqCst);
            return;
        }
    };

    let feedback_thread = std::thread::Builder::new()
        .name("bridge-feedback".into())
        .spawn(move || {
            let mut frame_buf = vec![0u8; HEADER_SIZE + 256];
            while feedback_flag.load(Ordering::SeqCst) {
                let data = {
                    let dev = feedback_state.device.lock().unwrap();
                    dev.as_ref().and_then(|d| d.receive().ok().flatten())
                };

                if let Some(midi_data) = data {
                    let len =
                        bridge_ipc::encode_frame(&mut frame_buf, FrameType::FeedbackMidi, &midi_data);
                    if let Err(e) = feedback_stream.write_all(&frame_buf[..len]) {
                        debug!(error = %e, "Feedback write failed (client disconnected?)");
                        break;
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        })
        .ok();

    // Main loop: read frames from client -> forward to virtual device
    let mut header = [0u8; HEADER_SIZE];
    let mut payload_buf = vec![0u8; 4096];
    let mut last_heartbeat = Instant::now();

    loop {
        match stream.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut
                || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                if last_heartbeat.elapsed() > Duration::from_secs(10) {
                    warn!("Client heartbeat timeout, disconnecting");
                    break;
                }
                continue;
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::UnexpectedEof
                    || e.kind() == std::io::ErrorKind::BrokenPipe
                    || e.kind() == std::io::ErrorKind::ConnectionReset
                {
                    info!("Client disconnected");
                } else {
                    warn!(error = %e, "Client read error");
                }
                break;
            }
        }

        let (frame_type, payload_len) = match bridge_ipc::decode_header(&header) {
            Some(h) => h,
            None => {
                warn!("Invalid frame header from client");
                break;
            }
        };

        if payload_len > 0 {
            if payload_len > payload_buf.len() {
                payload_buf.resize(payload_len, 0);
            }
            if let Err(e) = stream.read_exact(&mut payload_buf[..payload_len]) {
                warn!(error = %e, "Failed to read payload");
                break;
            }
        }

        match frame_type {
            FrameType::SendMidi => {
                let data = &payload_buf[..payload_len];
                let dev = state.device.lock().unwrap();
                if let Some(ref d) = *dev {
                    if let Err(e) = d.send(data) {
                        debug!(error = %e, "Device send error");
                    }
                }
            }
            FrameType::Heartbeat => {
                last_heartbeat = Instant::now();
            }
            FrameType::Identity => {
                debug!("Received updated identity from client");
                if let Ok(new_identity) =
                    serde_json::from_slice::<DeviceIdentity>(&payload_buf[..payload_len])
                {
                    *state.identity.lock().unwrap() = Some(new_identity);
                }
            }
            _ => {
                debug!(?frame_type, "Unexpected frame type from client");
            }
        }
    }

    // Client disconnected - silence the device but keep it alive
    info!("Client session ended, silencing device (keeping alive for reconnect)");
    feedback_running.store(false, Ordering::SeqCst);
    if let Some(t) = feedback_thread {
        let _ = t.join();
    }

    {
        let dev = state.device.lock().unwrap();
        if let Some(ref d) = *dev {
            if let Err(e) = d.send_all_off() {
                warn!(error = %e, "Failed to silence device on client disconnect");
            } else {
                info!("Device silenced (All Notes Off + All Sound Off)");
            }
        }
    }

    state.client_connected.store(false, Ordering::SeqCst);
}

/// Trait for cloning stream handles (UnixStream::try_clone / PipeStream::try_clone).
trait TryClone: Sized {
    fn try_clone_stream(&self) -> std::io::Result<Self>;
}

#[cfg(unix)]
impl TryClone for std::os::unix::net::UnixStream {
    fn try_clone_stream(&self) -> std::io::Result<Self> {
        self.try_clone()
    }
}

#[cfg(windows)]
impl TryClone for PipeStream {
    fn try_clone_stream(&self) -> std::io::Result<Self> {
        self.try_clone()
    }
}

/// Read and parse the Identity frame from the client.
fn read_identity(stream: &mut impl IoRead) -> anyhow::Result<DeviceIdentity> {
    let mut header = [0u8; HEADER_SIZE];
    stream.read_exact(&mut header)?;

    let (frame_type, payload_len) = bridge_ipc::decode_header(&header)
        .ok_or_else(|| anyhow::anyhow!("Invalid identity frame header"))?;

    if frame_type != FrameType::Identity {
        return Err(anyhow::anyhow!(
            "Expected Identity frame, got {:?}",
            frame_type
        ));
    }

    let mut payload = vec![0u8; payload_len];
    stream.read_exact(&mut payload)?;

    let identity: DeviceIdentity = serde_json::from_slice(&payload)?;
    info!(name = %identity.name, "Received device identity from client");
    Ok(identity)
}

/// Ensure the virtual device exists, creating it if needed.
/// Returns true if the device was newly created.
fn ensure_device(state: &BridgeState, identity: &DeviceIdentity) -> bool {
    let mut dev = state.device.lock().unwrap();

    if let Some(ref existing) = *dev {
        if existing.device_name() == identity.name {
            info!(name = %identity.name, "Reusing existing virtual device");
            *state.identity.lock().unwrap() = Some(identity.clone());
            return false;
        }
        warn!(
            old = existing.device_name(),
            new = %identity.name,
            "Identity changed, recreating device"
        );
    }

    info!(name = %identity.name, "Creating virtual MIDI device via bridge");
    let mut device = midi_device::create_virtual_device();
    match device.create(identity) {
        Ok(()) => {
            info!(name = %identity.name, "Virtual device created by bridge");
            *dev = Some(device);
            *state.identity.lock().unwrap() = Some(identity.clone());
            true
        }
        Err(e) => {
            error!(error = %e, "Failed to create virtual device in bridge");
            false
        }
    }
}
