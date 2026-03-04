/// Trial timer with tamper detection.
///
/// Tracks cumulative runtime in an HMAC-protected file.
/// Secondary markers prevent delete-to-reset attacks.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

use crate::fingerprint;
use crate::key::TRIAL_BUDGET_SECS;

type HmacSha256 = Hmac<Sha256>;

/// On-disk trial state (serialized as JSON, protected by HMAC).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TrialState {
    /// When the trial first started (unix secs)
    pub first_run_epoch: u64,
    /// Total seconds of active use
    pub cumulative_secs: u64,
    /// When the last session ended (unix secs)
    pub last_session_epoch: u64,
    /// HMAC-SHA256 of the above fields (hex-encoded)
    pub checksum: String,
}

/// Salt mixed with machine fingerprint for HMAC key derivation.
const HMAC_SALT: &[u8] = b"midinet-trial-v1-salt-9f3a2b";

impl TrialState {
    /// Create a new trial (first run).
    pub fn new() -> Self {
        let now = now_secs();
        let mut state = Self {
            first_run_epoch: now,
            cumulative_secs: 0,
            last_session_epoch: now,
            checksum: String::new(),
        };
        state.update_checksum();
        state
    }

    /// Remaining seconds of trial.
    pub fn remaining_secs(&self) -> u64 {
        TRIAL_BUDGET_SECS.saturating_sub(self.cumulative_secs)
    }

    /// Whether the trial has expired.
    pub fn is_expired(&self) -> bool {
        self.cumulative_secs >= TRIAL_BUDGET_SECS
    }

    /// Tick the trial: add elapsed seconds.
    pub fn tick(&mut self, elapsed_secs: u64) {
        self.cumulative_secs = self.cumulative_secs.saturating_add(elapsed_secs);
        self.last_session_epoch = now_secs();
        self.update_checksum();
    }

    /// Verify the HMAC checksum.
    pub fn verify_checksum(&self) -> bool {
        let expected = self.compute_checksum();
        // Constant-time comparison
        if expected.len() != self.checksum.len() {
            return false;
        }
        let mut diff = 0u8;
        for (a, b) in expected.bytes().zip(self.checksum.bytes()) {
            diff |= a ^ b;
        }
        diff == 0
    }

    /// Check for clock rollback (system time is before last session).
    pub fn detect_clock_rollback(&self) -> bool {
        let now = now_secs();
        // Allow 60 seconds of tolerance for minor clock drift
        now + 60 < self.last_session_epoch
    }

    fn update_checksum(&mut self) {
        self.checksum = self.compute_checksum();
    }

    fn compute_checksum(&self) -> String {
        let key = derive_hmac_key();
        let mut mac = HmacSha256::new_from_slice(&key).expect("HMAC key size is valid");
        mac.update(&self.first_run_epoch.to_le_bytes());
        mac.update(&self.cumulative_secs.to_le_bytes());
        mac.update(&self.last_session_epoch.to_le_bytes());
        let result = mac.finalize().into_bytes();
        result.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Derive the HMAC key from the machine fingerprint + salt.
fn derive_hmac_key() -> [u8; 32] {
    let fp = fingerprint::generate();
    let mut hasher = sha2::Sha256::new();
    sha2::Digest::update(&mut hasher, &fp);
    sha2::Digest::update(&mut hasher, HMAC_SALT);
    hasher.finalize().into()
}

/// Read trial state from disk.
pub fn read_trial(data_dir: &Path) -> Option<TrialState> {
    let path = trial_path(data_dir);
    let contents = std::fs::read_to_string(&path).ok()?;
    let state: TrialState = serde_json::from_str(&contents).ok()?;
    Some(state)
}

/// Write trial state to disk.
pub fn write_trial(data_dir: &Path, state: &TrialState) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = trial_path(data_dir);
    let contents = serde_json::to_string(state)?;
    std::fs::write(&path, contents)?;
    debug!(
        remaining = state.remaining_secs(),
        "Trial state saved"
    );
    Ok(())
}

fn trial_path(data_dir: &Path) -> PathBuf {
    data_dir.join("trial.dat")
}

// ── Secondary trial marker ──────────────────────────────────────────────

/// Check if the secondary trial marker exists (indicates trial was started before).
pub fn secondary_marker_exists() -> bool {
    #[cfg(target_os = "windows")]
    {
        windows_marker::exists()
    }
    #[cfg(target_os = "macos")]
    {
        macos_marker::exists()
    }
    #[cfg(target_os = "linux")]
    {
        linux_marker::exists()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Set the secondary trial marker.
pub fn set_secondary_marker() {
    #[cfg(target_os = "windows")]
    windows_marker::set();
    #[cfg(target_os = "macos")]
    macos_marker::set();
    #[cfg(target_os = "linux")]
    linux_marker::set();
}

// ── Platform-specific marker implementations ────────────────────────────

#[cfg(target_os = "windows")]
mod windows_marker {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    const SUBKEY: &str = "Software\\MIDInet";
    const VALUE_NAME: &str = "trial_marker";

    pub fn exists() -> bool {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        hkcu.open_subkey(SUBKEY)
            .and_then(|key| key.get_value::<String, _>(VALUE_NAME))
            .is_ok()
    }

    pub fn set() {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok((key, _)) = hkcu.create_subkey(SUBKEY) {
            let _ = key.set_value(VALUE_NAME, &"1");
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_marker {
    use std::process::Command;

    fn marker_path() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.midinet")
    }

    pub fn exists() -> bool {
        Command::new("xattr")
            .args(["-p", "com.midinet.trial", &marker_path()])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn set() {
        // Ensure the directory exists
        let path = marker_path();
        let _ = std::fs::create_dir_all(&path);
        let _ = Command::new("xattr")
            .args(["-w", "com.midinet.trial", "1", &path])
            .output();
    }
}

#[cfg(target_os = "linux")]
mod linux_marker {
    use std::path::PathBuf;

    fn marker_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".midinet").join(".trial_marker")
    }

    pub fn exists() -> bool {
        marker_path().exists()
    }

    pub fn set() {
        let path = marker_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, "1");
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trial_has_full_budget() {
        let state = TrialState::new();
        assert_eq!(state.remaining_secs(), TRIAL_BUDGET_SECS);
        assert!(!state.is_expired());
    }

    #[test]
    fn tick_reduces_remaining() {
        let mut state = TrialState::new();
        state.tick(60);
        assert_eq!(state.remaining_secs(), TRIAL_BUDGET_SECS - 60);
        assert!(state.verify_checksum());
    }

    #[test]
    fn trial_expires_at_budget() {
        let mut state = TrialState::new();
        state.tick(TRIAL_BUDGET_SECS);
        assert!(state.is_expired());
        assert_eq!(state.remaining_secs(), 0);
    }

    #[test]
    fn tampered_checksum_detected() {
        let mut state = TrialState::new();
        state.cumulative_secs = 100; // tamper without updating checksum
        assert!(!state.verify_checksum());
    }

    #[test]
    fn clock_rollback_detected() {
        let mut state = TrialState::new();
        state.last_session_epoch = now_secs() + 3600; // 1 hour in future
        assert!(state.detect_clock_rollback());
    }
}
