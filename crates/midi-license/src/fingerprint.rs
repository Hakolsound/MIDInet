/// Machine fingerprint generation.
///
/// Produces a stable SHA-256 hash that uniquely identifies a machine.
/// Used for seat management (activation locking) and trial tamper detection.

use sha2::{Digest, Sha256};
use tracing::debug;

/// A 32-byte machine fingerprint hash.
pub type MachineHash = [u8; 32];

/// Generate a fingerprint for the current machine.
pub fn generate() -> MachineHash {
    let raw = collect_raw_identifiers();
    let mut hasher = Sha256::new();
    Digest::update(&mut hasher, b"midinet-license-v1:"); // domain separator
    for (key, value) in &raw {
        Digest::update(&mut hasher, key.as_bytes());
        Digest::update(&mut hasher, b"=");
        Digest::update(&mut hasher, value.as_bytes());
        Digest::update(&mut hasher, b"\n");
    }
    let result: [u8; 32] = hasher.finalize().into();
    debug!(
        hash = hex::encode(&result[..8]),
        "Machine fingerprint generated"
    );
    result
}

/// Hex-encoded fingerprint for display and API calls.
pub fn hex_fingerprint() -> String {
    hex::encode(&generate())
}

/// Collect raw platform-specific machine identifiers.
fn collect_raw_identifiers() -> Vec<(String, String)> {
    let mut ids = Vec::new();

    // Hostname
    if let Ok(name) = crate::hostname::get() {
        let name_str: String = name.to_string_lossy().into_owned();
        ids.push(("hostname".into(), name_str));
    }

    // OS
    ids.push(("os".into(), std::env::consts::OS.into()));
    ids.push(("arch".into(), std::env::consts::ARCH.into()));

    // Platform-specific stable identifier
    if let Some(machine_id) = platform_machine_id() {
        ids.push(("machine_id".into(), machine_id));
    }

    ids
}

#[cfg(target_os = "linux")]
fn platform_machine_id() -> Option<String> {
    // /etc/machine-id is stable across reboots (set at install time)
    std::fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|s| s.trim().to_string())
        .or_else(|| {
            // Raspberry Pi serial number fallback
            std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|cpuinfo| {
                    cpuinfo
                        .lines()
                        .find(|line| line.starts_with("Serial"))
                        .and_then(|line| line.split(':').nth(1))
                        .map(|s| s.trim().to_string())
                })
        })
}

#[cfg(target_os = "macos")]
fn platform_machine_id() -> Option<String> {
    // IOPlatformUUID via ioreg
    std::process::Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()
        .and_then(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout
                .lines()
                .find(|line| line.contains("IOPlatformUUID"))
                .and_then(|line| line.split('\"').nth(3))
                .map(|s| s.to_string())
        })
}

#[cfg(target_os = "windows")]
fn platform_machine_id() -> Option<String> {
    // Windows MachineGuid from registry
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey("SOFTWARE\\Microsoft\\Cryptography")
        .ok()?;
    let guid: String = key.get_value("MachineGuid").ok()?;
    Some(guid)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_machine_id() -> Option<String> {
    None
}

/// Simple hex encoding (avoid pulling in the `hex` crate just for this).
mod hex {
    pub fn encode(data: &[u8]) -> String {
        data.iter().map(|b| format!("{b:02x}")).collect()
    }
}
