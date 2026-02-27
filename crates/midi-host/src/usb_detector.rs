/// USB MIDI device detection and enumeration.
/// Discovers connected MIDI controllers and reads their identity.

use midi_protocol::identity::DeviceIdentity;
#[cfg(target_os = "linux")]
use tracing::{info, warn};

/// A discovered MIDI card from /proc/asound/cards.
#[cfg(target_os = "linux")]
struct MidiCard {
    card_num: u32,
    /// Short name from the first line (e.g., "APC40 mkII")
    name: String,
    /// Whether /proc/asound/cardN/usbid exists (USB device vs built-in)
    is_usb: bool,
}

/// Scan /proc/asound/cards for cards that have MIDI ports.
/// Returns only cards where /proc/asound/cardN/midi0 exists.
#[cfg(target_os = "linux")]
fn scan_midi_cards() -> Vec<MidiCard> {
    let cards_content = match std::fs::read_to_string("/proc/asound/cards") {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut result = Vec::new();

    for line in cards_content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.as_bytes()[0].is_ascii_digit() {
            continue;
        }

        let card_num: u32 = match trimmed.split_whitespace().next().and_then(|s| s.parse().ok()) {
            Some(n) => n,
            None => continue,
        };

        // Only include cards that have actual MIDI ports
        let midi_path = format!("/proc/asound/card{}/midi0", card_num);
        if !std::path::Path::new(&midi_path).exists() {
            continue;
        }

        // Extract name from "USB-Audio - APC40 mkII"
        let name = if let Some(pos) = trimmed.find(" - ") {
            trimmed[pos + 3..].trim().to_string()
        } else {
            format!("Card {}", card_num)
        };

        let is_usb = std::path::Path::new(&format!("/proc/asound/card{}/usbid", card_num)).exists();

        result.push(MidiCard { card_num, name, is_usb });
    }

    result
}

/// Resolve a device config string to a concrete ALSA hw: device path.
///
/// Supported formats:
///   - `"hw:3,0,0"` — passed through as-is
///   - `"auto"`      — first USB MIDI device found (skips HDMI/built-in)
///   - `"auto:APC40"` — first USB MIDI device whose name contains "APC40" (case-insensitive)
///
/// Returns the resolved device string (e.g., `"hw:3,0,0"`), or the original
/// string unchanged if it's already a concrete path or resolution fails.
#[cfg(target_os = "linux")]
pub fn resolve_device(device: &str) -> String {
    // Already a concrete device path — pass through
    if device.starts_with("hw:") || device.starts_with("/dev/") {
        return device.to_string();
    }

    // Parse "auto" or "auto:PATTERN"
    if !device.starts_with("auto") {
        return device.to_string();
    }

    let pattern = device.strip_prefix("auto:").or_else(|| device.strip_prefix("auto"));
    let pattern = pattern.unwrap_or("").trim();
    let has_pattern = !pattern.is_empty() && pattern != "auto";

    let cards = scan_midi_cards();
    if cards.is_empty() {
        warn!("No MIDI devices found in /proc/asound/cards");
        return device.to_string();
    }

    // If a name pattern is given, match against it (case-insensitive)
    if has_pattern {
        let pat_lower = pattern.to_lowercase();
        if let Some(card) = cards.iter().find(|c| c.name.to_lowercase().contains(&pat_lower)) {
            let resolved = format!("hw:{},0,0", card.card_num);
            info!(
                pattern = %pattern,
                resolved = %resolved,
                name = %card.name,
                "Auto-detected MIDI device by name"
            );
            return resolved;
        }
        warn!(
            pattern = %pattern,
            available = ?cards.iter().map(|c| &c.name).collect::<Vec<_>>(),
            "No MIDI device matched pattern — falling back to first USB device"
        );
    }

    // No pattern or pattern didn't match — prefer first USB MIDI device
    if let Some(card) = cards.iter().find(|c| c.is_usb) {
        let resolved = format!("hw:{},0,0", card.card_num);
        info!(
            resolved = %resolved,
            name = %card.name,
            "Auto-detected USB MIDI device"
        );
        return resolved;
    }

    // Last resort: first MIDI device of any kind
    let card = &cards[0];
    let resolved = format!("hw:{},0,0", card.card_num);
    warn!(
        resolved = %resolved,
        name = %card.name,
        "No USB MIDI device found — using first available MIDI device"
    );
    resolved
}

#[cfg(not(target_os = "linux"))]
pub fn resolve_device(device: &str) -> String {
    device.to_string()
}

/// Read device identity from an ALSA device.
/// Extracts name, manufacturer, VID/PID from the ALSA card info.
///
/// The device string should already be resolved (e.g., "hw:3,0,0").
/// Call `resolve_device()` first if the input may be "auto".
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
