use serde::{Deserialize, Serialize};

// -- Magic bytes for packet identification --

pub const MAGIC_MIDI: [u8; 4] = *b"MDMI";
pub const MAGIC_HEARTBEAT: [u8; 4] = *b"MDHB";
pub const MAGIC_IDENTITY: [u8; 4] = *b"MDID";
pub const MAGIC_FOCUS: [u8; 4] = *b"MDFC";
pub const MAGIC_DISCOVER_REQ: [u8; 4] = *b"MDDS";
pub const MAGIC_DISCOVER_RESP: [u8; 4] = *b"MDDR";

/// Flag bit in MidiDataPacket indicating protocol v2 (device_id present).
/// V1 only used bit 0 (journal), so bit 7 is safe as a version marker.
const FLAG_V2: u8 = 0x80;
/// Flag bit indicating journal data is appended.
const FLAG_JOURNAL: u8 = 0x01;

// -- Host roles --

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum HostRole {
    Primary = 0x01,
    Standby = 0x02,
}

impl HostRole {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Primary),
            0x02 => Some(Self::Standby),
            _ => None,
        }
    }
}

// -- MIDI Data Packet --
// Lightweight RTP-inspired framing for MIDI over UDP.
//
// V1 layout (18-byte header):
//   magic(4) + seq(2) + ts(8) + host_id(1) + flags(1) + midi_len(2)
//
// V2 layout (19-byte header) — flags has bit 7 set:
//   magic(4) + seq(2) + ts(8) + host_id(1) + flags(1) + device_id(1) + midi_len(2)
//
// Backward compatibility:
//   - V2 code reads V1 packets: flags bit 7 clear → device_id=0, header=18
//   - V1 code reads V2 packets: will misparse (midi_len off by 1) — upgrade required
//     for multi-device mode. Single/Redundant hosts can stay on v1 temporarily.

#[derive(Debug, Clone)]
pub struct MidiDataPacket {
    pub sequence: u16,
    pub timestamp_us: u64,
    pub host_id: u8,
    /// Device slot on the host (0 in Single/Redundant modes, 0..15 in MultiDevice)
    pub device_id: u8,
    pub midi_data: Vec<u8>,
    /// Journal is appended periodically for state recovery
    pub journal: Option<Vec<u8>>,
}

impl MidiDataPacket {
    /// V1 header size (without device_id)
    pub const HEADER_SIZE_V1: usize = 18;
    /// V2 header size (with device_id)
    pub const HEADER_SIZE: usize = 19;

    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.clear();
        buf.extend_from_slice(&MAGIC_MIDI);
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        buf.extend_from_slice(&self.timestamp_us.to_be_bytes());
        buf.push(self.host_id);

        // V2: set FLAG_V2 to indicate device_id follows
        let mut flags: u8 = FLAG_V2;
        if self.journal.is_some() {
            flags |= FLAG_JOURNAL;
        }
        buf.push(flags);
        buf.push(self.device_id);

        let midi_len = self.midi_data.len() as u16;
        buf.extend_from_slice(&midi_len.to_be_bytes());
        buf.extend_from_slice(&self.midi_data);

        if let Some(ref journal) = self.journal {
            let journal_len = journal.len() as u16;
            buf.extend_from_slice(&journal_len.to_be_bytes());
            buf.extend_from_slice(journal);
        }
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < Self::HEADER_SIZE_V1 {
            return None;
        }
        if &data[0..4] != &MAGIC_MIDI {
            return None;
        }

        let sequence = u16::from_be_bytes([data[4], data[5]]);
        let timestamp_us = u64::from_be_bytes([
            data[6], data[7], data[8], data[9], data[10], data[11], data[12], data[13],
        ]);
        let host_id = data[14];
        let flags = data[15];

        // Detect V2 by checking FLAG_V2 in flags byte
        let (device_id, header_size) = if flags & FLAG_V2 != 0 {
            // V2: device_id at byte 16, midi_len at bytes 17-18
            if data.len() < Self::HEADER_SIZE {
                return None;
            }
            (data[16], Self::HEADER_SIZE)
        } else {
            // V1: no device_id, midi_len at bytes 16-17
            (0u8, Self::HEADER_SIZE_V1)
        };

        let midi_len_offset = header_size - 2;
        let midi_len =
            u16::from_be_bytes([data[midi_len_offset], data[midi_len_offset + 1]]) as usize;

        if data.len() < header_size + midi_len {
            return None;
        }

        let midi_data = data[header_size..header_size + midi_len].to_vec();

        let has_journal = flags & FLAG_JOURNAL != 0;
        let journal = if has_journal {
            let journal_offset = header_size + midi_len;
            if data.len() < journal_offset + 2 {
                return None;
            }
            let journal_len =
                u16::from_be_bytes([data[journal_offset], data[journal_offset + 1]]) as usize;
            if data.len() < journal_offset + 2 + journal_len {
                return None;
            }
            Some(data[journal_offset + 2..journal_offset + 2 + journal_len].to_vec())
        } else {
            None
        };

        Some(Self {
            sequence,
            timestamp_us,
            host_id,
            device_id,
            midi_data,
            journal,
        })
    }
}

// -- Heartbeat Packet --
//
// V1: 16 bytes (no device_mask)
// V2: 18 bytes (appends device_mask: u16 bitmask of active devices)
//
// V2 code reading V1: if packet is 16 bytes, device_mask defaults to 0x0001.
// V1 code reading V2: reads first 16 bytes correctly, ignores trailing 2 bytes.

#[derive(Debug, Clone, Copy)]
pub struct HeartbeatPacket {
    pub host_id: u8,
    pub role: HostRole,
    pub sequence: u16,
    pub timestamp_us: u64,
    /// Bitmask of active device_ids on this host (bit N = device N alive).
    /// Defaults to 0x0001 for V1 packets or Single/Redundant mode.
    pub device_mask: u16,
}

impl HeartbeatPacket {
    /// V2 packet size (with device_mask)
    pub const SIZE: usize = 18;
    /// V1 packet size (without device_mask)
    pub const SIZE_V1: usize = 16;

    pub fn serialize(&self, buf: &mut [u8; Self::SIZE]) {
        buf[0..4].copy_from_slice(&MAGIC_HEARTBEAT);
        buf[4] = self.host_id;
        buf[5] = self.role as u8;
        buf[6..8].copy_from_slice(&self.sequence.to_be_bytes());
        buf[8..16].copy_from_slice(&self.timestamp_us.to_be_bytes());
        buf[16..18].copy_from_slice(&self.device_mask.to_be_bytes());
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        // Accept both V1 (16 bytes) and V2 (18 bytes)
        if data.len() < Self::SIZE_V1 {
            return None;
        }
        if &data[0..4] != &MAGIC_HEARTBEAT {
            return None;
        }

        let device_mask = if data.len() >= Self::SIZE {
            u16::from_be_bytes([data[16], data[17]])
        } else {
            // V1 packet — single device implied
            0x0001
        };

        Some(Self {
            host_id: data[4],
            role: HostRole::from_u8(data[5])?,
            sequence: u16::from_be_bytes([data[6], data[7]]),
            timestamp_us: u64::from_be_bytes([
                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
            ]),
            device_mask,
        })
    }
}

// -- Identity Announcement Packet --
//
// V2 adds device_id after host_id (byte 5). Since this is a variable-length packet,
// we use a version byte to distinguish formats cleanly.
//
// V1: magic(4) + host_id(1) + name_len(1) + name + mfr_len(1) + mfr + VID(2) + PID(2) + sysex(15) + ports(2)
// V2: magic(4) + host_id(1) + version(1=0x02) + device_id(1) + name_len(1) + name + ...
//
// Detection: After host_id, peek at next byte. In V1 this is name_len (typically 5-30).
// In V2 this is the version marker 0x02. Since device names are never 2 chars, this is safe.
// (If somehow ambiguous, we fall back to V1 parsing.)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityPacket {
    pub host_id: u8,
    /// Device slot on the host (0 in Single/Redundant, 0..15 in MultiDevice)
    pub device_id: u8,
    pub device_name: String,
    pub manufacturer: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub sysex_identity: [u8; 15],
    pub port_count_in: u8,
    pub port_count_out: u8,
}

/// Version marker byte for V2 identity packets (placed after host_id).
const IDENTITY_V2_MARKER: u8 = 0x02;

impl IdentityPacket {
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.clear();
        buf.extend_from_slice(&MAGIC_IDENTITY);
        buf.push(self.host_id);
        // V2: version marker + device_id before the name
        buf.push(IDENTITY_V2_MARKER);
        buf.push(self.device_id);

        let name_bytes = self.device_name.as_bytes();
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);

        let mfr_bytes = self.manufacturer.as_bytes();
        buf.push(mfr_bytes.len() as u8);
        buf.extend_from_slice(mfr_bytes);

        buf.extend_from_slice(&self.vendor_id.to_be_bytes());
        buf.extend_from_slice(&self.product_id.to_be_bytes());
        buf.extend_from_slice(&self.sysex_identity);
        buf.push(self.port_count_in);
        buf.push(self.port_count_out);
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 5 {
            return None;
        }
        if &data[0..4] != &MAGIC_IDENTITY {
            return None;
        }

        let host_id = data[4];

        // Detect V2: byte 5 is the version marker (0x02).
        // In V1, byte 5 is name_len which is typically 5-30+ (never 0x02 for real device names).
        let (device_id, mut offset) = if data.len() > 5 && data[5] == IDENTITY_V2_MARKER {
            // V2: version(1) + device_id(1) before name
            if data.len() < 7 {
                return None;
            }
            (data[6], 7)
        } else {
            // V1: no version marker, name_len starts at byte 5
            (0u8, 5)
        };

        // Device name
        if offset >= data.len() {
            return None;
        }
        let name_len = data[offset] as usize;
        offset += 1;
        if offset + name_len > data.len() {
            return None;
        }
        let device_name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
        offset += name_len;

        // Manufacturer
        if offset >= data.len() {
            return None;
        }
        let mfr_len = data[offset] as usize;
        offset += 1;
        if offset + mfr_len > data.len() {
            return None;
        }
        let manufacturer = String::from_utf8_lossy(&data[offset..offset + mfr_len]).to_string();
        offset += mfr_len;

        // VID/PID + SysEx + ports = 2+2+15+1+1 = 21 bytes
        if offset + 21 > data.len() {
            return None;
        }
        let vendor_id = u16::from_be_bytes([data[offset], data[offset + 1]]);
        offset += 2;
        let product_id = u16::from_be_bytes([data[offset], data[offset + 1]]);
        offset += 2;

        let mut sysex_identity = [0u8; 15];
        sysex_identity.copy_from_slice(&data[offset..offset + 15]);
        offset += 15;

        let port_count_in = data[offset];
        let port_count_out = data[offset + 1];

        Some(Self {
            host_id,
            device_id,
            device_name,
            manufacturer,
            vendor_id,
            product_id,
            sysex_identity,
            port_count_in,
            port_count_out,
        })
    }
}

// -- Focus Packets --
//
// V1 (legacy):  19 bytes — no mode byte, no device_id
// V3.1:         20 bytes — adds mode byte
// V2 (current): 21 bytes — adds device_id
//
// Focus is global (one client holds focus for all devices).
// device_id in feedback MidiDataPackets routes to the correct physical controller.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FocusAction {
    Claim = 0x01,
    Release = 0x02,
    Ack = 0x03,
}

impl FocusAction {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0x01 => Some(Self::Claim),
            0x02 => Some(Self::Release),
            0x03 => Some(Self::Ack),
            _ => None,
        }
    }
}

/// Distinguishes operator-initiated focus from automatic focus.
/// Auto claims only succeed when no client holds focus (disaster recovery).
/// Manual claims always override (operator intent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FocusClaimMode {
    /// Automatic claim — only granted when no one holds focus (disaster recovery)
    Auto = 0x00,
    /// Manual claim — operator override, always granted
    Manual = 0x01,
}

impl FocusClaimMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x01 => Self::Manual,
            _ => Self::Auto,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FocusPacket {
    pub action: FocusAction,
    pub client_id: u32,
    pub sequence: u16,
    pub timestamp_us: u64,
    /// Claim mode: Auto (disaster recovery only) vs Manual (operator override)
    pub mode: FocusClaimMode,
    /// Device this focus/feedback pertains to (0 in Single/Redundant modes)
    pub device_id: u8,
}

impl FocusPacket {
    /// V2 size: magic(4) + action(1) + client_id(4) + seq(2) + timestamp(8) + mode(1) + device_id(1) = 21
    pub const SIZE: usize = 21;
    /// V3.1 size (with mode, no device_id)
    const SIZE_V31: usize = 20;
    /// Legacy size (no mode, no device_id)
    const LEGACY_SIZE: usize = 19;

    pub fn serialize(&self, buf: &mut [u8; Self::SIZE]) {
        buf[0..4].copy_from_slice(&MAGIC_FOCUS);
        buf[4] = self.action as u8;
        buf[5..9].copy_from_slice(&self.client_id.to_be_bytes());
        buf[9..11].copy_from_slice(&self.sequence.to_be_bytes());
        buf[11..19].copy_from_slice(&self.timestamp_us.to_be_bytes());
        buf[19] = self.mode as u8;
        buf[20] = self.device_id;
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        // Accept 19-byte (legacy), 20-byte (v3.1), and 21-byte (v2) packets
        if data.len() < Self::LEGACY_SIZE {
            return None;
        }
        if &data[0..4] != &MAGIC_FOCUS {
            return None;
        }

        let mode = if data.len() >= Self::SIZE_V31 {
            FocusClaimMode::from_u8(data[19])
        } else {
            // Legacy packet without mode byte — treat as Auto
            FocusClaimMode::Auto
        };

        let device_id = if data.len() >= Self::SIZE {
            data[20]
        } else {
            0
        };

        Some(Self {
            action: FocusAction::from_u8(data[4])?,
            client_id: u32::from_be_bytes([data[5], data[6], data[7], data[8]]),
            sequence: u16::from_be_bytes([data[9], data[10]]),
            timestamp_us: u64::from_be_bytes([
                data[11], data[12], data[13], data[14], data[15], data[16], data[17], data[18],
            ]),
            mode,
            device_id,
        })
    }
}

// -- Discovery Packets (UDP broadcast) --

/// Sent by clients as a broadcast to find hosts on the LAN.
#[derive(Debug, Clone)]
pub struct DiscoverRequest {
    pub client_id: u32,
    pub protocol_version: u8,
}

impl DiscoverRequest {
    pub const SIZE: usize = 9; // magic(4) + client_id(4) + version(1)

    pub fn serialize(&self, buf: &mut [u8; Self::SIZE]) {
        buf[0..4].copy_from_slice(&MAGIC_DISCOVER_REQ);
        buf[4..8].copy_from_slice(&self.client_id.to_be_bytes());
        buf[8] = self.protocol_version;
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        if &data[0..4] != &MAGIC_DISCOVER_REQ {
            return None;
        }

        Some(Self {
            client_id: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
            protocol_version: data[8],
        })
    }
}

/// Sent by hosts as a unicast reply to a discovery broadcast.
///
/// V2 appends device_count + additional device names after the primary device_name.
/// V1 code ignores trailing bytes (reads only up to device_name).
#[derive(Debug, Clone)]
pub struct DiscoverResponse {
    pub host_id: u8,
    pub role: HostRole,
    pub protocol_version: u8,
    pub data_port: u16,
    pub heartbeat_port: u16,
    pub admin_port: u16,
    pub multicast_group: [u8; 4], // IPv4 octets
    pub device_name: String,
    /// Additional device names (MultiDevice mode). Empty in Single/Redundant.
    pub extra_device_names: Vec<String>,
}

impl DiscoverResponse {
    /// Minimum size: magic(4) + host_id(1) + role(1) + ver(1) + data_port(2) +
    /// hb_port(2) + admin_port(2) + mcast(4) + name_len(1) = 18
    pub const HEADER_SIZE: usize = 18;

    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.clear();
        buf.extend_from_slice(&MAGIC_DISCOVER_RESP);
        buf.push(self.host_id);
        buf.push(self.role as u8);
        buf.push(self.protocol_version);
        buf.extend_from_slice(&self.data_port.to_be_bytes());
        buf.extend_from_slice(&self.heartbeat_port.to_be_bytes());
        buf.extend_from_slice(&self.admin_port.to_be_bytes());
        buf.extend_from_slice(&self.multicast_group);
        let name_bytes = self.device_name.as_bytes();
        buf.push(name_bytes.len() as u8);
        buf.extend_from_slice(name_bytes);

        // V2: append device_count + extra device names
        let total_devices = 1 + self.extra_device_names.len();
        buf.push(total_devices as u8);
        for name in &self.extra_device_names {
            let nb = name.as_bytes();
            buf.push(nb.len() as u8);
            buf.extend_from_slice(nb);
        }
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < Self::HEADER_SIZE {
            return None;
        }
        if &data[0..4] != &MAGIC_DISCOVER_RESP {
            return None;
        }

        let host_id = data[4];
        let role = HostRole::from_u8(data[5])?;
        let protocol_version = data[6];
        let data_port = u16::from_be_bytes([data[7], data[8]]);
        let heartbeat_port = u16::from_be_bytes([data[9], data[10]]);
        let admin_port = u16::from_be_bytes([data[11], data[12]]);
        let multicast_group = [data[13], data[14], data[15], data[16]];
        let name_len = data[17] as usize;

        if data.len() < Self::HEADER_SIZE + name_len {
            return None;
        }
        let device_name =
            String::from_utf8_lossy(&data[Self::HEADER_SIZE..Self::HEADER_SIZE + name_len])
                .to_string();

        // V2: read extra device names if present
        let mut extra_device_names = Vec::new();
        let mut offset = Self::HEADER_SIZE + name_len;
        if offset < data.len() {
            let device_count = data[offset] as usize;
            offset += 1;
            // device_count includes the primary, so extra = device_count - 1
            let extra_count = device_count.saturating_sub(1);
            for _ in 0..extra_count {
                if offset >= data.len() {
                    break;
                }
                let elen = data[offset] as usize;
                offset += 1;
                if offset + elen > data.len() {
                    break;
                }
                extra_device_names
                    .push(String::from_utf8_lossy(&data[offset..offset + elen]).to_string());
                offset += elen;
            }
        }

        Some(Self {
            host_id,
            role,
            protocol_version,
            data_port,
            heartbeat_port,
            admin_port,
            multicast_group,
            device_name,
            extra_device_names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- MidiDataPacket tests --

    #[test]
    fn test_midi_data_roundtrip() {
        let packet = MidiDataPacket {
            sequence: 42,
            timestamp_us: 1234567890,
            host_id: 1,
            device_id: 0,
            midi_data: vec![0x90, 0x3C, 0x7F],
            journal: None,
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = MidiDataPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.timestamp_us, 1234567890);
        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.device_id, 0);
        assert_eq!(decoded.midi_data, vec![0x90, 0x3C, 0x7F]);
        assert!(decoded.journal.is_none());
    }

    #[test]
    fn test_midi_data_with_device_id() {
        let packet = MidiDataPacket {
            sequence: 1,
            timestamp_us: 100,
            host_id: 1,
            device_id: 5,
            midi_data: vec![0xB0, 0x01, 0x40],
            journal: None,
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = MidiDataPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_id, 5);
        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.midi_data, vec![0xB0, 0x01, 0x40]);
    }

    #[test]
    fn test_midi_data_with_journal() {
        let packet = MidiDataPacket {
            sequence: 100,
            timestamp_us: 9999,
            host_id: 2,
            device_id: 3,
            midi_data: vec![0xB0, 0x01, 0x40],
            journal: Some(vec![0x01, 0x02, 0x03, 0x04]),
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = MidiDataPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_id, 3);
        assert_eq!(decoded.midi_data, vec![0xB0, 0x01, 0x40]);
        assert_eq!(decoded.journal, Some(vec![0x01, 0x02, 0x03, 0x04]));
    }

    #[test]
    fn test_midi_data_v1_compat() {
        // Manually build a V1 packet (no FLAG_V2, no device_id)
        let mut v1_buf = Vec::new();
        v1_buf.extend_from_slice(&MAGIC_MIDI);
        v1_buf.extend_from_slice(&42u16.to_be_bytes());
        v1_buf.extend_from_slice(&1000u64.to_be_bytes());
        v1_buf.push(1); // host_id
        v1_buf.push(0x00); // flags: no journal, no FLAG_V2
        v1_buf.extend_from_slice(&3u16.to_be_bytes()); // midi_len=3
        v1_buf.extend_from_slice(&[0x90, 0x3C, 0x7F]); // midi data

        let decoded = MidiDataPacket::deserialize(&v1_buf).unwrap();
        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.device_id, 0); // defaults to 0 for V1
        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.midi_data, vec![0x90, 0x3C, 0x7F]);
        assert!(decoded.journal.is_none());
    }

    #[test]
    fn test_midi_data_v1_with_journal_compat() {
        // V1 packet with journal (flag 0x01 set, no FLAG_V2)
        let mut v1_buf = Vec::new();
        v1_buf.extend_from_slice(&MAGIC_MIDI);
        v1_buf.extend_from_slice(&1u16.to_be_bytes());
        v1_buf.extend_from_slice(&500u64.to_be_bytes());
        v1_buf.push(2); // host_id
        v1_buf.push(0x01); // flags: journal, no FLAG_V2
        v1_buf.extend_from_slice(&2u16.to_be_bytes()); // midi_len=2
        v1_buf.extend_from_slice(&[0x90, 0x3C]); // midi data
        v1_buf.extend_from_slice(&4u16.to_be_bytes()); // journal_len=4
        v1_buf.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]); // journal

        let decoded = MidiDataPacket::deserialize(&v1_buf).unwrap();
        assert_eq!(decoded.device_id, 0);
        assert_eq!(decoded.host_id, 2);
        assert_eq!(decoded.midi_data, vec![0x90, 0x3C]);
        assert_eq!(decoded.journal, Some(vec![0xAA, 0xBB, 0xCC, 0xDD]));
    }

    // -- HeartbeatPacket tests --

    #[test]
    fn test_heartbeat_roundtrip() {
        let packet = HeartbeatPacket {
            host_id: 1,
            role: HostRole::Primary,
            sequence: 1000,
            timestamp_us: 5555555,
            device_mask: 0x0001,
        };

        let mut buf = [0u8; HeartbeatPacket::SIZE];
        packet.serialize(&mut buf);
        let decoded = HeartbeatPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.role, HostRole::Primary);
        assert_eq!(decoded.sequence, 1000);
        assert_eq!(decoded.timestamp_us, 5555555);
        assert_eq!(decoded.device_mask, 0x0001);
    }

    #[test]
    fn test_heartbeat_multi_device_mask() {
        let packet = HeartbeatPacket {
            host_id: 1,
            role: HostRole::Primary,
            sequence: 42,
            timestamp_us: 100,
            device_mask: 0b0000_0000_0010_0101, // devices 0, 2, 5 active
        };

        let mut buf = [0u8; HeartbeatPacket::SIZE];
        packet.serialize(&mut buf);
        let decoded = HeartbeatPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_mask, 0b0000_0000_0010_0101);
        assert!(decoded.device_mask & (1 << 0) != 0); // device 0
        assert!(decoded.device_mask & (1 << 1) == 0); // device 1 not active
        assert!(decoded.device_mask & (1 << 2) != 0); // device 2
        assert!(decoded.device_mask & (1 << 5) != 0); // device 5
    }

    #[test]
    fn test_heartbeat_v1_compat() {
        // V1 heartbeat is only 16 bytes (no device_mask)
        let mut v1_buf = [0u8; 16];
        v1_buf[0..4].copy_from_slice(&MAGIC_HEARTBEAT);
        v1_buf[4] = 1; // host_id
        v1_buf[5] = 0x01; // Primary
        v1_buf[6..8].copy_from_slice(&100u16.to_be_bytes());
        v1_buf[8..16].copy_from_slice(&999u64.to_be_bytes());

        let decoded = HeartbeatPacket::deserialize(&v1_buf).unwrap();
        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.device_mask, 0x0001); // defaults to single device
    }

    // -- IdentityPacket tests --

    #[test]
    fn test_identity_roundtrip() {
        let packet = IdentityPacket {
            host_id: 1,
            device_id: 0,
            device_name: "Akai APC40".to_string(),
            manufacturer: "Akai".to_string(),
            vendor_id: 0x09E8,
            product_id: 0x0028,
            sysex_identity: [0x47, 0x73, 0x00, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            port_count_in: 1,
            port_count_out: 1,
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = IdentityPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_name, "Akai APC40");
        assert_eq!(decoded.manufacturer, "Akai");
        assert_eq!(decoded.vendor_id, 0x09E8);
        assert_eq!(decoded.product_id, 0x0028);
        assert_eq!(decoded.device_id, 0);
        assert_eq!(decoded.port_count_in, 1);
    }

    #[test]
    fn test_identity_with_device_id() {
        let packet = IdentityPacket {
            host_id: 1,
            device_id: 3,
            device_name: "nanoKONTROL2".to_string(),
            manufacturer: "Korg".to_string(),
            vendor_id: 0x0944,
            product_id: 0x0117,
            sysex_identity: [0x42, 0x13, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            port_count_in: 1,
            port_count_out: 1,
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = IdentityPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_id, 3);
        assert_eq!(decoded.device_name, "nanoKONTROL2");
        assert_eq!(decoded.manufacturer, "Korg");
    }

    #[test]
    fn test_identity_v1_compat() {
        // Manually build a V1 identity packet (no version marker, no device_id)
        let mut v1_buf = Vec::new();
        v1_buf.extend_from_slice(&MAGIC_IDENTITY);
        v1_buf.push(1); // host_id
        // V1: name_len directly at byte 5
        let name = b"Akai APC40";
        v1_buf.push(name.len() as u8);
        v1_buf.extend_from_slice(name);
        let mfr = b"Akai";
        v1_buf.push(mfr.len() as u8);
        v1_buf.extend_from_slice(mfr);
        v1_buf.extend_from_slice(&0x09E8u16.to_be_bytes());
        v1_buf.extend_from_slice(&0x0028u16.to_be_bytes());
        v1_buf.extend_from_slice(&[0x47, 0x73, 0x00, 0x19, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
        v1_buf.push(1); // port_count_in
        v1_buf.push(1); // port_count_out

        let decoded = IdentityPacket::deserialize(&v1_buf).unwrap();
        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.device_id, 0); // V1 default
        assert_eq!(decoded.device_name, "Akai APC40");
        assert_eq!(decoded.manufacturer, "Akai");
    }

    // -- FocusPacket tests --

    #[test]
    fn test_focus_roundtrip() {
        let packet = FocusPacket {
            action: FocusAction::Claim,
            client_id: 12345,
            sequence: 7,
            timestamp_us: 999999,
            mode: FocusClaimMode::Manual,
            device_id: 0,
        };

        let mut buf = [0u8; FocusPacket::SIZE];
        packet.serialize(&mut buf);
        let decoded = FocusPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.action, FocusAction::Claim);
        assert_eq!(decoded.client_id, 12345);
        assert_eq!(decoded.sequence, 7);
        assert_eq!(decoded.mode, FocusClaimMode::Manual);
        assert_eq!(decoded.device_id, 0);
    }

    #[test]
    fn test_focus_with_device_id() {
        let packet = FocusPacket {
            action: FocusAction::Ack,
            client_id: 42,
            sequence: 1,
            timestamp_us: 500,
            mode: FocusClaimMode::Auto,
            device_id: 7,
        };

        let mut buf = [0u8; FocusPacket::SIZE];
        packet.serialize(&mut buf);
        let decoded = FocusPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_id, 7);
        assert_eq!(decoded.action, FocusAction::Ack);
    }

    #[test]
    fn test_focus_legacy_deserialize() {
        // 19-byte legacy packet (no mode byte, no device_id) should deserialize as Auto, device_id=0
        let packet = FocusPacket {
            action: FocusAction::Claim,
            client_id: 42,
            sequence: 3,
            timestamp_us: 12345,
            mode: FocusClaimMode::Auto,
            device_id: 0,
        };
        let mut buf = [0u8; FocusPacket::SIZE];
        packet.serialize(&mut buf);
        // Pass only 19 bytes (strip mode + device_id bytes)
        let decoded = FocusPacket::deserialize(&buf[..19]).unwrap();
        assert_eq!(decoded.mode, FocusClaimMode::Auto);
        assert_eq!(decoded.device_id, 0);
        assert_eq!(decoded.client_id, 42);
    }

    #[test]
    fn test_focus_v31_deserialize() {
        // 20-byte v3.1 packet (has mode, no device_id)
        let packet = FocusPacket {
            action: FocusAction::Release,
            client_id: 99,
            sequence: 5,
            timestamp_us: 777,
            mode: FocusClaimMode::Manual,
            device_id: 0,
        };
        let mut buf = [0u8; FocusPacket::SIZE];
        packet.serialize(&mut buf);
        // Pass only 20 bytes (strip device_id byte)
        let decoded = FocusPacket::deserialize(&buf[..20]).unwrap();
        assert_eq!(decoded.mode, FocusClaimMode::Manual);
        assert_eq!(decoded.device_id, 0); // defaults to 0
        assert_eq!(decoded.client_id, 99);
    }

    // -- DiscoverResponse tests --

    #[test]
    fn test_discover_response_roundtrip() {
        let packet = DiscoverResponse {
            host_id: 1,
            role: HostRole::Primary,
            protocol_version: 2,
            data_port: 5004,
            heartbeat_port: 5005,
            admin_port: 8080,
            multicast_group: [239, 69, 83, 1],
            device_name: "APC40".to_string(),
            extra_device_names: vec![],
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = DiscoverResponse::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_name, "APC40");
        assert!(decoded.extra_device_names.is_empty());
    }

    #[test]
    fn test_discover_response_multi_device() {
        let packet = DiscoverResponse {
            host_id: 1,
            role: HostRole::Primary,
            protocol_version: 2,
            data_port: 5004,
            heartbeat_port: 5005,
            admin_port: 8080,
            multicast_group: [239, 69, 83, 1],
            device_name: "APC40".to_string(),
            extra_device_names: vec!["nanoKONTROL2".to_string(), "Launchpad".to_string()],
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = DiscoverResponse::deserialize(&buf).unwrap();

        assert_eq!(decoded.device_name, "APC40");
        assert_eq!(decoded.extra_device_names.len(), 2);
        assert_eq!(decoded.extra_device_names[0], "nanoKONTROL2");
        assert_eq!(decoded.extra_device_names[1], "Launchpad");
    }

    #[test]
    fn test_discover_response_v1_compat() {
        // V1 packet has no device_count or extra names
        let mut v1_buf = Vec::new();
        v1_buf.extend_from_slice(&MAGIC_DISCOVER_RESP);
        v1_buf.push(1); // host_id
        v1_buf.push(0x01); // Primary
        v1_buf.push(1); // protocol_version
        v1_buf.extend_from_slice(&5004u16.to_be_bytes());
        v1_buf.extend_from_slice(&5005u16.to_be_bytes());
        v1_buf.extend_from_slice(&8080u16.to_be_bytes());
        v1_buf.extend_from_slice(&[239, 69, 83, 1]);
        let name = b"APC40";
        v1_buf.push(name.len() as u8);
        v1_buf.extend_from_slice(name);

        let decoded = DiscoverResponse::deserialize(&v1_buf).unwrap();
        assert_eq!(decoded.device_name, "APC40");
        assert!(decoded.extra_device_names.is_empty());
    }

    // -- Rejection tests --

    #[test]
    fn test_reject_invalid_magic() {
        let bad_data = [0xFF; 21];
        assert!(MidiDataPacket::deserialize(&bad_data).is_none());
        assert!(HeartbeatPacket::deserialize(&bad_data).is_none());
        assert!(IdentityPacket::deserialize(&bad_data).is_none());
        assert!(FocusPacket::deserialize(&bad_data).is_none());
    }

    #[test]
    fn test_reject_truncated_packets() {
        assert!(MidiDataPacket::deserialize(&[0u8; 5]).is_none());
        assert!(HeartbeatPacket::deserialize(&[0u8; 5]).is_none());
        assert!(FocusPacket::deserialize(&[0u8; 5]).is_none());
    }
}
