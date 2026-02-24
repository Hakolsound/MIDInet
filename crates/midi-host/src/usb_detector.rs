/// USB MIDI device detection and enumeration.
/// Discovers connected MIDI controllers and reads their identity.

use midi_protocol::identity::DeviceIdentity;

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
#[allow(dead_code)]
#[cfg(target_os = "linux")]
pub fn read_device_identity(device: &str) -> DeviceIdentity {
    // In a full implementation, this would:
    // 1. Parse the ALSA card/device number from the device string
    // 2. Read /proc/asound/cardN/usbid for VID:PID
    // 3. Read /proc/asound/cardN/id for card name
    // 4. Send SysEx Device Inquiry to get identity reply
    //
    // For now, return a default with the device path as name
    info!(device = %device, "Reading device identity");

    DeviceIdentity {
        name: device.to_string(),
        ..DeviceIdentity::default()
    }
}

#[allow(dead_code)]
#[cfg(not(target_os = "linux"))]
pub fn read_device_identity(device: &str) -> DeviceIdentity {
    DeviceIdentity {
        name: device.to_string(),
        ..DeviceIdentity::default()
    }
}
