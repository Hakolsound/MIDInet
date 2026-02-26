/// IPC protocol for communication between midi-client and midi-bridge.
///
/// Simple length-prefixed framing over Unix domain socket (macOS/Linux)
/// or named pipe (Windows).
///
/// Frame format: [1 byte type][2 bytes payload length (LE)][payload]
///
/// The protocol is bidirectional:
///   Client → Bridge: Identity, SendMidi, Heartbeat
///   Bridge → Client: Ack, FeedbackMidi, Status

use serde::{Deserialize, Serialize};

/// Frame type identifiers.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Client → Bridge: device identity (JSON), requests device creation
    Identity = 0x01,
    /// Client → Bridge: raw MIDI bytes to send through virtual device
    SendMidi = 0x02,
    /// Bridge → Client: raw MIDI bytes received from app (feedback)
    FeedbackMidi = 0x03,
    /// Client → Bridge: keepalive ping
    Heartbeat = 0x04,
    /// Bridge → Client: identity accepted, device ready
    Ack = 0x05,
    /// Bridge → Client: device status update
    Status = 0x06,
}

impl FrameType {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Identity),
            0x02 => Some(Self::SendMidi),
            0x03 => Some(Self::FeedbackMidi),
            0x04 => Some(Self::Heartbeat),
            0x05 => Some(Self::Ack),
            0x06 => Some(Self::Status),
            _ => None,
        }
    }
}

/// Maximum frame payload size (64 KiB — more than enough for any MIDI message).
pub const MAX_PAYLOAD_SIZE: usize = 65535;

/// Frame header size: 1 (type) + 2 (length).
pub const HEADER_SIZE: usize = 3;

/// Encode a frame into a buffer.
///
/// Returns the total frame size (header + payload).
/// The caller must ensure `buf` has at least `HEADER_SIZE + payload.len()` bytes.
pub fn encode_frame(buf: &mut [u8], frame_type: FrameType, payload: &[u8]) -> usize {
    let len = payload.len().min(MAX_PAYLOAD_SIZE);
    buf[0] = frame_type as u8;
    buf[1] = (len & 0xFF) as u8;
    buf[2] = ((len >> 8) & 0xFF) as u8;
    buf[HEADER_SIZE..HEADER_SIZE + len].copy_from_slice(&payload[..len]);
    HEADER_SIZE + len
}

/// Decode a frame header from a 3-byte buffer.
///
/// Returns (frame_type, payload_length) or None if invalid.
pub fn decode_header(header: &[u8; HEADER_SIZE]) -> Option<(FrameType, usize)> {
    let frame_type = FrameType::from_byte(header[0])?;
    let len = header[1] as usize | ((header[2] as usize) << 8);
    if len > MAX_PAYLOAD_SIZE {
        return None;
    }
    Some((frame_type, len))
}

/// Ack payload — sent by bridge after creating the device.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckPayload {
    /// Whether the device was newly created or already existed
    pub created: bool,
    /// Device name as registered with the OS
    pub device_name: String,
}

/// Status payload — periodic bridge status update.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusPayload {
    /// Whether the virtual device is alive
    pub device_alive: bool,
    /// Number of apps connected to the virtual device (if detectable)
    pub connected_apps: u32,
}

/// Well-known bridge socket paths per platform.
#[cfg(unix)]
pub const BRIDGE_SOCKET_PATH: &str = "/tmp/midinet-bridge.sock";
#[cfg(windows)]
pub const BRIDGE_SOCKET_PATH: &str = r"\\.\pipe\midinet-bridge";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_frame() {
        let payload = b"hello";
        let mut buf = [0u8; 256];
        let len = encode_frame(&mut buf, FrameType::SendMidi, payload);
        assert_eq!(len, HEADER_SIZE + 5);

        let mut header = [0u8; HEADER_SIZE];
        header.copy_from_slice(&buf[..HEADER_SIZE]);
        let (ft, plen) = decode_header(&header).unwrap();
        assert_eq!(ft, FrameType::SendMidi);
        assert_eq!(plen, 5);
        assert_eq!(&buf[HEADER_SIZE..HEADER_SIZE + plen], payload);
    }

    #[test]
    fn empty_payload() {
        let mut buf = [0u8; 8];
        let len = encode_frame(&mut buf, FrameType::Heartbeat, &[]);
        assert_eq!(len, HEADER_SIZE);

        let mut header = [0u8; HEADER_SIZE];
        header.copy_from_slice(&buf[..HEADER_SIZE]);
        let (ft, plen) = decode_header(&header).unwrap();
        assert_eq!(ft, FrameType::Heartbeat);
        assert_eq!(plen, 0);
    }

    #[test]
    fn invalid_type() {
        let header = [0xFF, 0x00, 0x00];
        assert!(decode_header(&header).is_none());
    }
}
