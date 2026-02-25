use serde::{Deserialize, Serialize};

// -- Magic bytes for packet identification --

pub const MAGIC_MIDI: [u8; 4] = *b"MDMI";
pub const MAGIC_HEARTBEAT: [u8; 4] = *b"MDHB";
pub const MAGIC_IDENTITY: [u8; 4] = *b"MDID";
pub const MAGIC_FOCUS: [u8; 4] = *b"MDFC";
pub const MAGIC_DISCOVER_REQ: [u8; 4] = *b"MDDS";
pub const MAGIC_DISCOVER_RESP: [u8; 4] = *b"MDDR";

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

#[derive(Debug, Clone)]
pub struct MidiDataPacket {
    pub sequence: u16,
    pub timestamp_us: u64,
    pub host_id: u8,
    pub midi_data: Vec<u8>,
    // Journal is appended periodically for state recovery
    pub journal: Option<Vec<u8>>,
}

impl MidiDataPacket {
    /// Minimum packet size: magic(4) + seq(2) + timestamp(8) + host_id(1) + flags(1) + midi_len(2) = 18
    pub const HEADER_SIZE: usize = 18;

    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.clear();
        buf.extend_from_slice(&MAGIC_MIDI);
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        buf.extend_from_slice(&self.timestamp_us.to_be_bytes());
        buf.push(self.host_id);

        let flags: u8 = if self.journal.is_some() { 0x01 } else { 0x00 };
        buf.push(flags);

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
        if data.len() < Self::HEADER_SIZE {
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
        let midi_len = u16::from_be_bytes([data[16], data[17]]) as usize;

        if data.len() < Self::HEADER_SIZE + midi_len {
            return None;
        }

        let midi_data = data[Self::HEADER_SIZE..Self::HEADER_SIZE + midi_len].to_vec();

        let journal = if flags & 0x01 != 0 {
            let journal_offset = Self::HEADER_SIZE + midi_len;
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
            midi_data,
            journal,
        })
    }
}

// -- Heartbeat Packet (16 bytes) --

#[derive(Debug, Clone, Copy)]
pub struct HeartbeatPacket {
    pub host_id: u8,
    pub role: HostRole,
    pub sequence: u16,
    pub timestamp_us: u64,
}

impl HeartbeatPacket {
    pub const SIZE: usize = 16;

    pub fn serialize(&self, buf: &mut [u8; Self::SIZE]) {
        buf[0..4].copy_from_slice(&MAGIC_HEARTBEAT);
        buf[4] = self.host_id;
        buf[5] = self.role as u8;
        buf[6..8].copy_from_slice(&self.sequence.to_be_bytes());
        buf[8..16].copy_from_slice(&self.timestamp_us.to_be_bytes());
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        if &data[0..4] != &MAGIC_HEARTBEAT {
            return None;
        }

        Some(Self {
            host_id: data[4],
            role: HostRole::from_u8(data[5])?,
            sequence: u16::from_be_bytes([data[6], data[7]]),
            timestamp_us: u64::from_be_bytes([
                data[8], data[9], data[10], data[11], data[12], data[13], data[14], data[15],
            ]),
        })
    }
}

// -- Identity Announcement Packet --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityPacket {
    pub host_id: u8,
    pub device_name: String,
    pub manufacturer: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub sysex_identity: [u8; 15],
    pub port_count_in: u8,
    pub port_count_out: u8,
}

impl IdentityPacket {
    pub fn serialize(&self, buf: &mut Vec<u8>) {
        buf.clear();
        buf.extend_from_slice(&MAGIC_IDENTITY);
        buf.push(self.host_id);

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
        let mut offset = 5;

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

#[derive(Debug, Clone)]
pub struct FocusPacket {
    pub action: FocusAction,
    pub client_id: u32,
    pub sequence: u16,
    pub timestamp_us: u64,
}

impl FocusPacket {
    pub const SIZE: usize = 19; // magic(4) + action(1) + client_id(4) + seq(2) + timestamp(8)

    pub fn serialize(&self, buf: &mut [u8; Self::SIZE]) {
        buf[0..4].copy_from_slice(&MAGIC_FOCUS);
        buf[4] = self.action as u8;
        buf[5..9].copy_from_slice(&self.client_id.to_be_bytes());
        buf[9..11].copy_from_slice(&self.sequence.to_be_bytes());
        buf[11..19].copy_from_slice(&self.timestamp_us.to_be_bytes());
    }

    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        if &data[0..4] != &MAGIC_FOCUS {
            return None;
        }

        Some(Self {
            action: FocusAction::from_u8(data[4])?,
            client_id: u32::from_be_bytes([data[5], data[6], data[7], data[8]]),
            sequence: u16::from_be_bytes([data[9], data[10]]),
            timestamp_us: u64::from_be_bytes([
                data[11], data[12], data[13], data[14], data[15], data[16], data[17], data[18],
            ]),
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

        Some(Self {
            host_id,
            role,
            protocol_version,
            data_port,
            heartbeat_port,
            admin_port,
            multicast_group,
            device_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_midi_data_roundtrip() {
        let packet = MidiDataPacket {
            sequence: 42,
            timestamp_us: 1234567890,
            host_id: 1,
            midi_data: vec![0x90, 0x3C, 0x7F], // Note On C4 velocity 127
            journal: None,
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = MidiDataPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.sequence, 42);
        assert_eq!(decoded.timestamp_us, 1234567890);
        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.midi_data, vec![0x90, 0x3C, 0x7F]);
        assert!(decoded.journal.is_none());
    }

    #[test]
    fn test_midi_data_with_journal() {
        let packet = MidiDataPacket {
            sequence: 100,
            timestamp_us: 9999,
            host_id: 2,
            midi_data: vec![0xB0, 0x01, 0x40], // CC1 value 64
            journal: Some(vec![0x01, 0x02, 0x03, 0x04]),
        };

        let mut buf = Vec::new();
        packet.serialize(&mut buf);
        let decoded = MidiDataPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.midi_data, vec![0xB0, 0x01, 0x40]);
        assert_eq!(decoded.journal, Some(vec![0x01, 0x02, 0x03, 0x04]));
    }

    #[test]
    fn test_heartbeat_roundtrip() {
        let packet = HeartbeatPacket {
            host_id: 1,
            role: HostRole::Primary,
            sequence: 1000,
            timestamp_us: 5555555,
        };

        let mut buf = [0u8; HeartbeatPacket::SIZE];
        packet.serialize(&mut buf);
        let decoded = HeartbeatPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.host_id, 1);
        assert_eq!(decoded.role, HostRole::Primary);
        assert_eq!(decoded.sequence, 1000);
        assert_eq!(decoded.timestamp_us, 5555555);
    }

    #[test]
    fn test_identity_roundtrip() {
        let packet = IdentityPacket {
            host_id: 1,
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
        assert_eq!(decoded.port_count_in, 1);
    }

    #[test]
    fn test_focus_roundtrip() {
        let packet = FocusPacket {
            action: FocusAction::Claim,
            client_id: 12345,
            sequence: 7,
            timestamp_us: 999999,
        };

        let mut buf = [0u8; FocusPacket::SIZE];
        packet.serialize(&mut buf);
        let decoded = FocusPacket::deserialize(&buf).unwrap();

        assert_eq!(decoded.action, FocusAction::Claim);
        assert_eq!(decoded.client_id, 12345);
        assert_eq!(decoded.sequence, 7);
    }

    #[test]
    fn test_reject_invalid_magic() {
        let bad_data = [0xFF; 20];
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
