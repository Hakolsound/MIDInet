/// License file storage — read/write license data to the user's data directory.
///
/// Directory structure:
///   ~/.midinet/license/
///     license.key         — raw license key string
///     activation.token    — JSON activation token from server
///     trial.dat           — encrypted trial state

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Get the default license data directory.
pub fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        // %LOCALAPPDATA%\MIDInet\license
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local).join("MIDInet").join("license");
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // ~/.midinet/license
        if let Some(home) = dirs_home() {
            return home.join(".midinet").join("license");
        }
    }

    // Fallback
    PathBuf::from(".midinet").join("license")
}

/// Read the license key string from disk.
pub fn read_license_key(data_dir: &Path) -> Option<String> {
    let path = data_dir.join("license.key");
    match std::fs::read_to_string(&path) {
        Ok(key) => {
            let key = key.trim().to_string();
            if key.is_empty() {
                None
            } else {
                debug!("Loaded license key from {}", path.display());
                Some(key)
            }
        }
        Err(_) => None,
    }
}

/// Write the license key string to disk.
pub fn write_license_key(data_dir: &Path, key: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("license.key");
    std::fs::write(&path, key.trim())?;
    debug!("Saved license key to {}", path.display());
    Ok(())
}

/// Read the activation token from disk.
pub fn read_activation_token(data_dir: &Path) -> Option<ActivationToken> {
    let path = data_dir.join("activation.token");
    let contents = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str(&contents) {
        Ok(token) => {
            debug!("Loaded activation token from {}", path.display());
            Some(token)
        }
        Err(e) => {
            warn!("Corrupt activation token at {}: {e}", path.display());
            None
        }
    }
}

/// Write the activation token to disk.
pub fn write_activation_token(data_dir: &Path, token: &ActivationToken) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("activation.token");
    let contents = serde_json::to_string_pretty(token)?;
    std::fs::write(&path, contents)?;
    debug!("Saved activation token to {}", path.display());
    Ok(())
}

/// Delete the activation token (on deactivation).
pub fn delete_activation_token(data_dir: &Path) {
    let path = data_dir.join("activation.token");
    let _ = std::fs::remove_file(&path);
}

/// Delete the license key (on deactivation).
pub fn delete_license_key(data_dir: &Path) {
    let path = data_dir.join("license.key");
    let _ = std::fs::remove_file(&path);
}

/// Activation token returned by the licensing server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActivationToken {
    /// License ID (hex)
    pub license_id: String,
    /// Machine hash this activation is bound to
    pub machine_hash: String,
    /// Tier string
    pub tier: String,
    /// Max hosts allowed
    pub max_hosts: u8,
    /// Max clients allowed
    pub max_clients: u8,
    /// Feature flags bitmask
    pub features: u16,
    /// When the activation was created (unix secs)
    pub activated_at: u64,
    /// When the server last validated this activation (unix secs)
    pub last_validated_at: u64,
    /// When update entitlement expires (0 = never)
    pub update_expires_at: u64,
    /// Server-signed token for offline verification
    pub server_signature: String,
}

impl ActivationToken {
    /// How many seconds since the server last validated.
    pub fn seconds_since_validation(&self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.last_validated_at)
    }

    /// Offline grace period: 30 days.
    pub const OFFLINE_GRACE_SECS: u64 = 30 * 24 * 3600;
}

#[cfg(not(target_os = "windows"))]
fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}
