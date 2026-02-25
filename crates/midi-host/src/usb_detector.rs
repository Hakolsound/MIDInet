/// USB MIDI device detection and enumeration.
/// Discovers connected MIDI controllers and reads their identity.

use midi_protocol::identity::DeviceIdentity;
#[cfg(target_os = "linux")]
use tracing::info;

/// List all available MIDI devices on the system.
/// Returns a vector of (device_path, device_name) pairs.
#[allow(dead_code)]
#[cfg(target_os = "linux")]
pub fn list_midi_devices() -> Vec<(String, String)> {
    // Read from /proc/asound/cards or use ALSA APIs
    // For now, return a placeholder
    let mut devices = Vec::new();

    // Try to enumerate ALSA rawmidi devices
    if let Ok(entries) = std::fs::read_dir("/dev/snd") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("midi") {
                let path = entry.path().to_string_lossy().to_string();
                devices.push((path, name));
            }
        }
    }

    devices
}

#[allow(dead_code)]
#[cfg(not(target_os = "linux"))]
pub fn list_midi_devices() -> Vec<(String, String)> {
    Vec::new()
}

/// Read device identity from an ALSA device.
/// Extracts name, manufacturer, VID/PID from the ALSA card info.
#[cfg(target_os = "linux")]
pub fn read_device_identity(device: &str) -> DeviceIdentity {
    info!(device = %device, "Reading device identity");

    // Parse card number from device string (e.g., "hw:3,0,0" → 3)
    let card_num = device
        .strip_prefix("hw:")
        .and_then(|s| s.split(',').next())
        .and_then(|n| n.parse::<u32>().ok());

    let mut identity = DeviceIdentity::default();

    if let Some(card) = card_num {
        // Parse full device name from /proc/asound/cards.
        // Format per card is two lines:
        //   " 3 [mkII           ]: USB-Audio - APC40 mkII"
        //   "                      Akai APC40 mkII at usb-0000:01:00.0-1.1, full speed"
        // The second line has the full name (strip " at usb-..." suffix).
        if let Ok(cards_content) = std::fs::read_to_string("/proc/asound/cards") {
            // Match lines like " 3 [mkII    ]: ..." — card number after trimming whitespace
            let card_str = card.to_string();
            let mut found_card = false;
            for line in cards_content.lines() {
                if found_card {
                    // This is the description line — trim and strip USB path suffix
                    let desc = line.trim();
                    let clean = if let Some(pos) = desc.find(" at ") {
                        &desc[..pos]
                    } else {
                        desc
                    };
                    if !clean.is_empty() {
                        identity.name = clean.to_string();
                        info!(name = %identity.name, "Identified MIDI device from ALSA");
                    }
                    break;
                }
                // Check if this line starts with the card number (after trimming)
                let trimmed = line.trim_start();
                if trimmed.starts_with(&card_str) && trimmed[card_str.len()..].starts_with(" [") {
                    found_card = true;
                }
            }
        }

        // Fallback: read short card id if we still have default name
        if identity.name == "Unknown MIDI Device" {
            let id_path = format!("/proc/asound/card{}/id", card);
            if let Ok(id) = std::fs::read_to_string(&id_path) {
                let id = id.trim();
                if !id.is_empty() {
                    identity.name = id.to_string();
                }
            }
        }

        // Read USB VID:PID from /proc/asound/cardN/usbid (e.g., "09e8:0028")
        let usbid_path = format!("/proc/asound/card{}/usbid", card);
        if let Ok(usbid) = std::fs::read_to_string(&usbid_path) {
            let usbid = usbid.trim();
            let parts: Vec<&str> = usbid.split(':').collect();
            if parts.len() == 2 {
                if let Ok(vid) = u16::from_str_radix(parts[0], 16) {
                    identity.vendor_id = vid;
                }
                if let Ok(pid) = u16::from_str_radix(parts[1], 16) {
                    identity.product_id = pid;
                }
                info!(vid = format!("{:04x}", identity.vendor_id), pid = format!("{:04x}", identity.product_id), "Read USB IDs");
            }
        }
    }

    // If we still have the default name, use the device path as fallback
    if identity.name == "Unknown MIDI Device" {
        identity.name = device.to_string();
    }

    identity
}

#[cfg(not(target_os = "linux"))]
pub fn read_device_identity(device: &str) -> DeviceIdentity {
    DeviceIdentity {
        name: device.to_string(),
        ..DeviceIdentity::default()
    }
}
