use serde::{Deserialize, Serialize};

/// Captured identity of a physical MIDI controller.
/// This is what gets cloned on each client to make the virtual device
/// appear identical to the original controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceIdentity {
    /// Display name (e.g., "Akai APC40")
    pub name: String,
    /// Manufacturer name (e.g., "Akai Professional")
    pub manufacturer: String,
    /// USB Vendor ID
    pub vendor_id: u16,
    /// USB Product ID
    pub product_id: u16,
    /// SysEx Identity Reply (response to Universal Device Inquiry F0 7E 7F 06 01 F7)
    /// Format: [manufacturer_id(1-3), family(2), model(2), version(4)]
    /// Padded to 15 bytes (max SysEx identity payload)
    pub sysex_identity: [u8; 15],
    /// Number of MIDI input ports
    pub port_count_in: u8,
    /// Number of MIDI output ports
    pub port_count_out: u8,
}

impl Default for DeviceIdentity {
    fn default() -> Self {
        Self {
            name: "Unknown MIDI Device".to_string(),
            manufacturer: "Unknown".to_string(),
            vendor_id: 0,
            product_id: 0,
            sysex_identity: [0; 15],
            port_count_in: 1,
            port_count_out: 1,
        }
    }
}

impl DeviceIdentity {
    /// Generate a SysEx Identity Reply message that this device would send.
    /// Universal Device Inquiry response:
    /// F0 7E <device_id> 06 02 <mfr_id> <family_lsb> <family_msb> <model_lsb> <model_msb> <ver1> <ver2> <ver3> <ver4> F7
    pub fn sysex_identity_reply(&self) -> Vec<u8> {
        let mut msg = vec![0xF0, 0x7E, 0x7F, 0x06, 0x02];
        // Append identity bytes (manufacturer + family + model + version)
        for &b in &self.sysex_identity {
            if b == 0xF7 {
                break; // stop before end-of-sysex if embedded
            }
            msg.push(b);
        }
        msg.push(0xF7);
        msg
    }

    /// Check if this is a valid (non-default) identity
    pub fn is_valid(&self) -> bool {
        !self.name.is_empty() && self.name != "Unknown MIDI Device"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_identity() {
        let id = DeviceIdentity::default();
        assert!(!id.is_valid());
    }

    #[test]
    fn test_sysex_reply() {
        let id = DeviceIdentity {
            name: "APC40".to_string(),
            manufacturer: "Akai".to_string(),
            vendor_id: 0x09E8,
            product_id: 0x0028,
            sysex_identity: [
                0x47, // Akai manufacturer ID
                0x73, 0x00, // Family
                0x19, 0x00, // Model
                0x01, 0x00, 0x00, 0x00, // Version
                0, 0, 0, 0, 0, 0,
            ],
            port_count_in: 1,
            port_count_out: 1,
        };

        let reply = id.sysex_identity_reply();
        assert_eq!(reply[0], 0xF0); // SysEx start
        assert_eq!(reply[1], 0x7E); // Universal Non-Realtime
        assert_eq!(reply[3], 0x06); // General Information
        assert_eq!(reply[4], 0x02); // Identity Reply
        assert_eq!(*reply.last().unwrap(), 0xF7); // SysEx end
        assert!(id.is_valid());
    }
}
