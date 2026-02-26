/// Bridge virtual MIDI device — IPC adapter for communicating with midi-bridge.
///
/// Instead of creating a platform-native virtual MIDI device directly,
/// this adapter connects to the midi-bridge sidecar process over a Unix domain
/// socket (macOS/Linux) or named pipe (Windows). The bridge owns the actual
/// virtual device, so it survives client restarts.
///
/// Protocol: length-prefixed frames (see `midi_protocol::bridge_ipc`).

use std::io::{Read as IoRead, Write as IoWrite};
use std::sync::Mutex;
use std::time::Instant;

use midi_device::VirtualMidiDevice;
use midi_protocol::bridge_ipc::{
    self, AckPayload, FrameType, HEADER_SIZE,
};
use midi_protocol::identity::DeviceIdentity;
use tracing::{debug, error, info, warn};

/// Heartbeat interval — send if no data has been sent for this long.
const HEARTBEAT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Client-side bridge adapter implementing VirtualMidiDevice.
pub struct BridgeVirtualDevice {
    socket_path: String,
    name: String,
    /// The IPC connection to the bridge (None if not connected)
    conn: Mutex<Option<BridgeConnection>>,
    /// Last time we sent anything to the bridge (for heartbeat timing)
    last_send: Mutex<Instant>,
}

/// Wrapper for the platform-specific IPC connection.
struct BridgeConnection {
    #[cfg(unix)]
    stream: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    stream: std::fs::File,
}

impl BridgeConnection {
    #[cfg(unix)]
    fn connect(path: &str) -> anyhow::Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(path)?;
        // Set non-blocking for receive() to return immediately when no data
        stream.set_nonblocking(true)?;
        // But we need blocking for the initial handshake
        Ok(Self {
            stream,
        })
    }

    #[cfg(windows)]
    fn connect(path: &str) -> anyhow::Result<Self> {
        use std::fs::OpenOptions;
        let stream = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self {
            stream,
        })
    }

    fn send_frame(&mut self, frame_type: FrameType, payload: &[u8]) -> anyhow::Result<()> {
        let mut buf = vec![0u8; HEADER_SIZE + payload.len()];
        let len = bridge_ipc::encode_frame(&mut buf, frame_type, payload);
        self.write_all(&buf[..len])?;
        Ok(())
    }

    fn write_all(&mut self, data: &[u8]) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            // Temporarily set blocking for write
            self.stream.set_nonblocking(false)?;
            self.stream.write_all(data)?;
            self.stream.set_nonblocking(true)?;
        }
        #[cfg(windows)]
        {
            self.stream.write_all(data)?;
        }
        Ok(())
    }

    /// Read exactly `n` bytes (blocking).
    fn read_exact_blocking(&mut self, buf: &mut [u8]) -> anyhow::Result<()> {
        #[cfg(unix)]
        {
            self.stream.set_nonblocking(false)?;
            self.stream.read_exact(buf)?;
            self.stream.set_nonblocking(true)?;
        }
        #[cfg(windows)]
        {
            self.stream.read_exact(buf)?;
        }
        Ok(())
    }

    /// Try to read a frame non-blocking. Returns None if no data available.
    fn try_read_frame(&mut self) -> anyhow::Result<Option<(FrameType, Vec<u8>)>> {
        // Try to read a header
        let mut header = [0u8; HEADER_SIZE];
        match self.try_read(&mut header) {
            Ok(HEADER_SIZE) => {}
            Ok(0) => return Ok(None), // No data
            Ok(n) => {
                // Partial header — read the rest blocking
                #[cfg(unix)]
                {
                    self.stream.set_nonblocking(false)?;
                    self.stream.read_exact(&mut header[n..])?;
                    self.stream.set_nonblocking(true)?;
                }
                #[cfg(windows)]
                {
                    self.stream.read_exact(&mut header[n..])?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e.into()),
        }

        let (frame_type, payload_len) = bridge_ipc::decode_header(&header)
            .ok_or_else(|| anyhow::anyhow!("Invalid frame header"))?;

        if payload_len == 0 {
            return Ok(Some((frame_type, Vec::new())));
        }

        let mut payload = vec![0u8; payload_len];
        self.read_exact_blocking(&mut payload)?;

        Ok(Some((frame_type, payload)))
    }

    fn try_read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            self.stream.read(buf)
        }
        #[cfg(windows)]
        {
            self.stream.read(buf)
        }
    }
}

impl BridgeVirtualDevice {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
            name: String::new(),
            conn: Mutex::new(None),
            last_send: Mutex::new(Instant::now()),
        }
    }
}

impl VirtualMidiDevice for BridgeVirtualDevice {
    fn create(&mut self, identity: &DeviceIdentity) -> anyhow::Result<()> {
        self.name = identity.name.clone();

        info!(name = %self.name, path = %self.socket_path, "Connecting to MIDI bridge...");

        // Connect to bridge
        let mut conn = BridgeConnection::connect(&self.socket_path)?;

        // Send identity frame
        let identity_json = serde_json::to_vec(identity)?;
        conn.send_frame(FrameType::Identity, &identity_json)?;
        info!(name = %self.name, "Sent device identity to bridge");

        // Wait for Ack (blocking read with timeout)
        let mut header = [0u8; HEADER_SIZE];
        conn.read_exact_blocking(&mut header)?;

        let (frame_type, payload_len) = bridge_ipc::decode_header(&header)
            .ok_or_else(|| anyhow::anyhow!("Invalid ack header from bridge"))?;

        if frame_type != FrameType::Ack {
            return Err(anyhow::anyhow!(
                "Expected Ack from bridge, got {:?}",
                frame_type
            ));
        }

        let mut payload = vec![0u8; payload_len];
        if payload_len > 0 {
            conn.read_exact_blocking(&mut payload)?;
        }

        let ack: AckPayload = serde_json::from_slice(&payload)?;
        info!(
            name = %ack.device_name,
            created = ack.created,
            "Bridge acknowledged device"
        );

        *self.conn.lock().unwrap() = Some(conn);
        Ok(())
    }

    fn send(&self, data: &[u8]) -> anyhow::Result<()> {
        let mut guard = self.conn.lock().unwrap();
        let conn = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Not connected to bridge"))?;
        conn.send_frame(FrameType::SendMidi, data)?;
        *self.last_send.lock().unwrap() = Instant::now();
        Ok(())
    }

    fn receive(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let mut guard = self.conn.lock().unwrap();
        let conn = match guard.as_mut() {
            Some(c) => c,
            None => return Ok(None),
        };

        // Send heartbeat if idle (no send() calls recently)
        if self.last_send.lock().unwrap().elapsed() >= HEARTBEAT_INTERVAL {
            let _ = conn.send_frame(FrameType::Heartbeat, &[]);
            *self.last_send.lock().unwrap() = Instant::now();
        }

        // Non-blocking: try to read any feedback frames
        match conn.try_read_frame() {
            Ok(Some((FrameType::FeedbackMidi, payload))) => Ok(Some(payload)),
            Ok(Some((FrameType::Status, _payload))) => {
                // Bridge status update — ignore for now
                debug!("Received bridge status update");
                Ok(None)
            }
            Ok(Some((ft, _))) => {
                warn!(?ft, "Unexpected frame type from bridge");
                Ok(None)
            }
            Ok(None) => Ok(None),
            Err(e) => {
                error!(error = %e, "Bridge receive error");
                // Connection may be broken — clear it
                *guard = None;
                Ok(None)
            }
        }
    }

    fn close(&mut self) -> anyhow::Result<()> {
        info!(name = %self.name, "Disconnecting from MIDI bridge (device stays alive)");
        // Drop the connection — bridge keeps the device alive
        *self.conn.lock().unwrap() = None;
        Ok(())
    }

    fn silence_and_detach(&mut self) -> anyhow::Result<()> {
        // Just disconnect — bridge handles silence on client disconnect
        self.close()
    }

    fn device_name(&self) -> &str {
        &self.name
    }
}
