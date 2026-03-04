/// MIDInet software licensing.
///
/// Provides license key verification, trial management, machine fingerprinting,
/// and online activation. Used by midi-client, midi-host, midi-tray, and midi-cli.

pub mod activation;
pub mod api_client;
pub mod fingerprint;
pub mod key;
pub mod state;
pub mod storage;
pub mod trial;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;

use tracing::{debug, info, warn};

use crate::activation::validate_cached_token;
use crate::key::LicensePayload;
use crate::state::{DegradedReason, LicenseState};
use crate::trial::TrialState;

// ── Global state ────────────────────────────────────────────────────────

/// Global license state, initialized once at startup.
static LICENSE_STATE: OnceLock<std::sync::RwLock<LicenseState>> = OnceLock::new();

/// Atomic flag for the receiver hot path: true = currently in a MIDI blackout.
static IN_BLACKOUT: AtomicBool = AtomicBool::new(false);

/// Atomic flag: true = inject random MIDI noise.
static INJECT_NOISE: AtomicBool = AtomicBool::new(false);

/// Seconds spent in degraded mode this session (for escalation).
static DEGRADED_SESSION_SECS: AtomicU64 = AtomicU64::new(0);

/// Data directory path, set during init.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Cached license key string (for re-validation).
static LICENSE_KEY: OnceLock<String> = OnceLock::new();

// ── Public API ──────────────────────────────────────────────────────────

/// Initialize the license system. Call once at startup.
///
/// Checks for an existing license, activation token, or trial state.
/// Returns the initial license state.
pub async fn init(data_dir: &Path) -> anyhow::Result<LicenseState> {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    std::fs::create_dir_all(data_dir)?;

    let machine_hash = fingerprint::hex_fingerprint();
    let state = determine_initial_state(data_dir, &machine_hash).await;

    info!(state = state.label(), "License system initialized");

    let _ = LICENSE_STATE.set(std::sync::RwLock::new(state.clone()));
    Ok(state)
}

/// Get the current license state (fast, lock-free read for non-hot paths).
pub fn current_state() -> LicenseState {
    LICENSE_STATE
        .get()
        .and_then(|s| s.read().ok())
        .map(|s| s.clone())
        .unwrap_or(LicenseState::Unlicensed)
}

/// Update the global license state (called by the enforcer task).
pub fn set_state(new_state: LicenseState) {
    if let Some(lock) = LICENSE_STATE.get() {
        if let Ok(mut s) = lock.write() {
            *s = new_state;
        }
    }
}

/// Check the blackout flag (called per-packet in the receiver hot path).
/// This is a single atomic load — ~1ns overhead.
#[inline]
pub fn is_in_blackout() -> bool {
    IN_BLACKOUT.load(Ordering::Relaxed)
}

/// Check the noise injection flag (called per-packet in the receiver).
#[inline]
pub fn should_inject_noise() -> bool {
    INJECT_NOISE.load(Ordering::Relaxed)
}

/// Set the blackout flag (called by the enforcer task).
pub fn set_blackout(active: bool) {
    IN_BLACKOUT.store(active, Ordering::Relaxed);
}

/// Set the noise injection flag (called by the enforcer task).
pub fn set_noise_injection(active: bool) {
    INJECT_NOISE.store(active, Ordering::Relaxed);
}

/// Get seconds spent in degraded mode this session.
pub fn degraded_session_secs() -> u64 {
    DEGRADED_SESSION_SECS.load(Ordering::Relaxed)
}

/// Increment degraded session counter (called every second by enforcer).
pub fn tick_degraded_session() {
    DEGRADED_SESSION_SECS.fetch_add(1, Ordering::Relaxed);
}

/// Tick the trial timer by the given number of seconds.
/// Persists the updated state to disk.
pub fn tick_trial(elapsed_secs: u64) {
    let data_dir = match DATA_DIR.get() {
        Some(d) => d,
        None => return,
    };

    if let Some(mut state) = trial::read_trial(data_dir) {
        state.tick(elapsed_secs);
        if let Err(e) = trial::write_trial(data_dir, &state) {
            warn!("Failed to persist trial state: {e}");
        }

        // Check if trial just expired
        if state.is_expired() {
            info!("Trial expired — entering degraded mode");
            set_state(LicenseState::Degraded {
                reason: DegradedReason::TrialExpired,
            });
        } else {
            // Update the remaining time in the global state
            set_state(LicenseState::Trial {
                remaining_secs: state.remaining_secs(),
                total_secs: key::TRIAL_BUDGET_SECS,
            });
        }
    }
}

/// Activate a license key (online). Stores the key and activation token.
pub async fn activate(
    license_key: &str,
    component: &str,
    api_base: Option<&str>,
) -> anyhow::Result<LicenseState> {
    // Verify key signature offline first
    let payload = LicensePayload::from_key(license_key)
        .ok_or_else(|| anyhow::anyhow!("Invalid license key"))?;

    let data_dir = DATA_DIR
        .get()
        .ok_or_else(|| anyhow::anyhow!("License system not initialized"))?;

    let machine_hash = fingerprint::hex_fingerprint();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unknown".into());

    // Call activation server
    let token = api_client::activate(
        api_base,
        license_key,
        &machine_hash,
        component,
        &hostname,
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION"),
    )
    .await?;

    // Persist
    storage::write_license_key(data_dir, license_key)?;
    storage::write_activation_token(data_dir, &token)?;
    let _ = LICENSE_KEY.set(license_key.to_string());

    let state = LicenseState::Licensed {
        tier: payload.tier,
        update_expires_in_secs: payload.update_expires_in_secs(),
    };
    set_state(state.clone());

    info!(tier = payload.tier.label(), "License activated");
    Ok(state)
}

/// Deactivate this machine (online). Removes local license files.
pub async fn deactivate(api_base: Option<&str>) -> anyhow::Result<()> {
    let data_dir = DATA_DIR
        .get()
        .ok_or_else(|| anyhow::anyhow!("License system not initialized"))?;

    let license_key = storage::read_license_key(data_dir)
        .ok_or_else(|| anyhow::anyhow!("No license key found"))?;

    let machine_hash = fingerprint::hex_fingerprint();

    // Call deactivation server
    api_client::deactivate(api_base, &license_key, &machine_hash).await?;

    // Clean up local files
    storage::delete_activation_token(data_dir);
    storage::delete_license_key(data_dir);

    set_state(LicenseState::Unlicensed);
    info!("License deactivated");
    Ok(())
}

/// Re-validate the license online (call periodically, e.g., every 24h).
pub async fn revalidate(api_base: Option<&str>) -> anyhow::Result<LicenseState> {
    let data_dir = DATA_DIR
        .get()
        .ok_or_else(|| anyhow::anyhow!("License system not initialized"))?;

    let license_key = storage::read_license_key(data_dir)
        .ok_or_else(|| anyhow::anyhow!("No license key found"))?;

    let current_token = storage::read_activation_token(data_dir)
        .ok_or_else(|| anyhow::anyhow!("No activation token found"))?;

    let machine_hash = fingerprint::hex_fingerprint();

    match api_client::validate(api_base, &license_key, &machine_hash, &current_token).await {
        Ok(new_token) => {
            storage::write_activation_token(data_dir, &new_token)?;
            let payload = LicensePayload::from_key(&license_key);
            let state = if let Some(p) = payload {
                LicenseState::Licensed {
                    tier: p.tier,
                    update_expires_in_secs: p.update_expires_in_secs(),
                }
            } else {
                LicenseState::Degraded {
                    reason: DegradedReason::LicenseExpired,
                }
            };
            set_state(state.clone());
            debug!("License re-validated successfully");
            Ok(state)
        }
        Err(e) => {
            warn!("Re-validation failed: {e}");
            // Keep current state if within grace period
            Ok(current_state())
        }
    }
}

/// Get the default data directory for license files.
pub fn default_data_dir() -> PathBuf {
    storage::default_data_dir()
}

// ── Internal ────────────────────────────────────────────────────────────

/// Determine the initial license state at startup.
async fn determine_initial_state(data_dir: &Path, machine_hash: &str) -> LicenseState {
    // 1. Check for license key + activation token
    if let Some(license_key) = storage::read_license_key(data_dir) {
        let _ = LICENSE_KEY.set(license_key.clone());

        // Verify key signature offline
        let payload = match LicensePayload::from_key(&license_key) {
            Some(p) => p,
            None => {
                warn!("Stored license key has invalid signature");
                return LicenseState::Degraded {
                    reason: DegradedReason::LicenseExpired,
                };
            }
        };

        // Check activation token
        if let Some(token) = storage::read_activation_token(data_dir) {
            match validate_cached_token(&token, machine_hash) {
                Ok(()) => {
                    debug!(tier = payload.tier.label(), "License valid (cached token)");
                    return LicenseState::Licensed {
                        tier: payload.tier,
                        update_expires_in_secs: payload.update_expires_in_secs(),
                    };
                }
                Err(activation::TokenError::OfflineGraceExceeded { days_offline }) => {
                    warn!(days = days_offline, "Offline grace period exceeded");
                    return LicenseState::Degraded {
                        reason: DegradedReason::OfflineGraceExceeded,
                    };
                }
                Err(activation::TokenError::MachineMismatch) => {
                    warn!("Activation token machine mismatch");
                    // Fall through to try online activation
                }
            }
        }

        // No valid cached token — try online activation
        info!("Attempting online activation...");
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".into());

        match api_client::activate(
            None,
            &license_key,
            machine_hash,
            "client",
            &hostname,
            std::env::consts::OS,
            env!("CARGO_PKG_VERSION"),
        )
        .await
        {
            Ok(token) => {
                let _ = storage::write_activation_token(data_dir, &token);
                info!(tier = payload.tier.label(), "Online activation successful");
                return LicenseState::Licensed {
                    tier: payload.tier,
                    update_expires_in_secs: payload.update_expires_in_secs(),
                };
            }
            Err(e) => {
                warn!("Online activation failed: {e}");
                return LicenseState::Degraded {
                    reason: DegradedReason::ActivationRevoked,
                };
            }
        }
    }

    // 2. Check for existing trial
    if let Some(trial_state) = trial::read_trial(data_dir) {
        // Verify integrity
        if !trial_state.verify_checksum() {
            warn!("Trial file tampered — checksum mismatch");
            return LicenseState::Degraded {
                reason: DegradedReason::TamperDetected,
            };
        }

        // Check clock rollback
        if trial_state.detect_clock_rollback() {
            warn!("Clock rollback detected in trial state");
            return LicenseState::Degraded {
                reason: DegradedReason::TamperDetected,
            };
        }

        if trial_state.is_expired() {
            return LicenseState::Degraded {
                reason: DegradedReason::TrialExpired,
            };
        }

        return LicenseState::Trial {
            remaining_secs: trial_state.remaining_secs(),
            total_secs: key::TRIAL_BUDGET_SECS,
        };
    }

    // 3. Check for secondary marker (trial was deleted to reset)
    if trial::secondary_marker_exists() {
        warn!("Trial file deleted but secondary marker exists — trial expired");
        return LicenseState::Degraded {
            reason: DegradedReason::TamperDetected,
        };
    }

    // 4. First run — start trial
    info!("First run — starting 120-minute trial");
    let trial_state = TrialState::new();
    trial::set_secondary_marker();
    if let Err(e) = trial::write_trial(data_dir, &trial_state) {
        warn!("Failed to create trial file: {e}");
    }

    LicenseState::Trial {
        remaining_secs: trial_state.remaining_secs(),
        total_secs: key::TRIAL_BUDGET_SECS,
    }
}

/// Hostname — minimal wrapper avoiding external crate dependency.
pub(crate) mod hostname {
    use std::ffi::OsString;

    pub fn get() -> Result<OsString, std::io::Error> {
        #[cfg(unix)]
        {
            let output = std::process::Command::new("hostname")
                .output()?;
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Ok(OsString::from(name))
        }

        #[cfg(windows)]
        {
            std::env::var("COMPUTERNAME")
                .map(OsString::from)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "no hostname"))
        }
    }
}
