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
///
/// On Windows, supports running as a native Windows service (pass `--service`).
/// The Windows Service Control Manager (SCM) protocol is handled automatically.

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

/// Global shutdown flag for Windows service mode.
#[cfg(windows)]
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // On Windows, check for --service flag to enter SCM mode
    #[cfg(windows)]
    {
        if args.iter().any(|a| a == "--service") {
            return windows_service_mode::run()
                .map_err(|e| anyhow::anyhow!("Windows service error: {}", e));
        }
    }

    // Check for --log-file <path> (used by Task Scheduler mode where no console exists)
    let log_file = args
        .iter()
        .position(|a| a == "--log-file")
        .and_then(|pos| args.get(pos + 1))
        .map(std::path::PathBuf::from);

    init_logging(log_file.as_deref());
    run_bridge()
}

/// Initialize tracing subscriber.
/// When `log_file` is `Some`, logs to a file (for Windows service mode where no console exists).
fn init_logging(log_file: Option<&std::path::Path>) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    if let Some(path) = log_file {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("Failed to open log file");
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_writer(std::sync::Mutex::new(file))
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .init();
    }
}

/// Run the bridge main loop (platform-dispatched).
fn run_bridge() -> anyhow::Result<()> {
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
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };

    // PIPE_ACCESS_DUPLEX = 0x3 (not exported by the windows crate)
    const PIPE_ACCESS_DUPLEX: FILE_FLAGS_AND_ATTRIBUTES = FILE_FLAGS_AND_ATTRIBUTES(0x3);

    let pipe_name = HSTRING::from(BRIDGE_SOCKET_PATH);

    info!(path = BRIDGE_SOCKET_PATH, "Bridge listening on named pipe");

    loop {
        // Check shutdown before creating a new pipe instance
        if SHUTDOWN.load(Ordering::SeqCst) {
            info!("Shutdown flag set, exiting pipe listener");
            break;
        }

        // Create a new named pipe instance for each client
        let pipe_handle = unsafe {
            CreateNamedPipeW(
                &pipe_name,
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                4096,
                4096,
                0,
                None,
            )
        };

        if pipe_handle.is_invalid() {
            let e = std::io::Error::last_os_error();
            error!(error = %e, "Failed to create named pipe");
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        // Wait for client to connect (blocking)
        let connect_result = unsafe { ConnectNamedPipe(pipe_handle, None) };

        // Check if we were unblocked by a shutdown dummy connection
        if SHUTDOWN.load(Ordering::SeqCst) {
            info!("Shutdown signal received during pipe accept");
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(pipe_handle);
            };
            break;
        }

        if let Err(e) = connect_result {
            // ERROR_PIPE_CONNECTED (535) means client raced between Create and Connect - benign
            if e.code() != windows::core::HRESULT::from_win32(535) {
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

    Ok(())
}

/// Signal the bridge to shut down by setting the flag and unblocking ConnectNamedPipe.
#[cfg(windows)]
fn trigger_shutdown() {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_MODE, OPEN_EXISTING,
    };

    SHUTDOWN.store(true, Ordering::SeqCst);

    // Make a dummy connection to unblock the blocking ConnectNamedPipe call
    let pipe_name = HSTRING::from(BRIDGE_SOCKET_PATH);
    let result = unsafe {
        CreateFileW(
            &pipe_name,
            0x80000000, // GENERIC_READ
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    };
    if let Ok(handle) = result {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
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
fn win_err(e: windows::core::Error) -> std::io::Error {
    std::io::Error::from_raw_os_error(e.code().0 as i32)
}

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
            .map_err(win_err)?;
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
            .map_err(win_err)?;
        }
        Ok(bytes_written as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        unsafe {
            windows::Win32::Storage::FileSystem::FlushFileBuffers(self.handle)
                .map_err(win_err)?;
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
            .map_err(win_err)?;
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

// ── Windows service mode ──
//
// When launched with `--service`, the bridge registers with the Windows
// Service Control Manager (SCM) and runs as a native Windows service.
// This allows `sc.exe` to manage the service without needing NSSM.

#[cfg(windows)]
mod windows_service_mode {
    use std::ffi::OsString;
    use std::sync::OnceLock;
    use std::time::Duration;

    use tracing::{error, info};
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{
        self, ServiceControlHandlerResult, ServiceStatusHandle,
    };
    use windows_service::{define_windows_service, service_dispatcher};

    const SERVICE_NAME: &str = "MIDInetBridge";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

    /// Shared status handle so the control handler can report StopPending.
    static STATUS_HANDLE: OnceLock<ServiceStatusHandle> = OnceLock::new();

    /// Entry point: register with the SCM dispatcher.
    /// This call blocks until the service is stopped.
    pub fn run() -> windows_service::Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    // Macro-generated low-level entry point that SCM calls.
    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            eprintln!("MIDInet bridge service error: {e}");
        }
    }

    fn run_service() -> anyhow::Result<()> {
        // Register control handler (receives Stop/Shutdown from SCM)
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    // Report StopPending so SCM knows we're shutting down
                    if let Some(h) = STATUS_HANDLE.get() {
                        let _ = h.set_service_status(ServiceStatus {
                            service_type: SERVICE_TYPE,
                            current_state: ServiceState::StopPending,
                            controls_accepted: ServiceControlAccept::empty(),
                            exit_code: ServiceExitCode::Win32(0),
                            checkpoint: 0,
                            wait_hint: Duration::from_secs(15),
                            process_id: None,
                        });
                    }
                    super::trigger_shutdown();
                    ServiceControlHandlerResult::NoError
                }
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
            .map_err(|e| anyhow::anyhow!("Failed to register service control handler: {}", e))?;
        let _ = STATUS_HANDLE.set(status_handle);

        // Report StartPending
        status_handle
            .set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::StartPending,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(5),
                process_id: None,
            })
            .map_err(|e| anyhow::anyhow!("Failed to report StartPending: {}", e))?;

        // Initialize logging to file (no console in service mode)
        let log_path = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("bridge.log")))
            .unwrap_or_else(|| std::path::PathBuf::from(r"C:\MIDInet\bridge.log"));
        super::init_logging(Some(&log_path));

        // Report Running
        status_handle
            .set_service_status(ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: ServiceState::Running,
                controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })
            .map_err(|e| anyhow::anyhow!("Failed to report Running: {}", e))?;

        info!("MIDInet bridge service running");

        // Run bridge (blocks until shutdown signal)
        let result = super::run_bridge();

        if let Err(ref e) = result {
            error!(error = %e, "Bridge exited with error");
        }

        // Report Stopped
        let exit_code = if result.is_ok() {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(1)
        };

        let _ = status_handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });

        info!("MIDInet bridge service stopped");
        Ok(())
    }
}
