/// License enforcement task.
/// - Ticks trial timer every second
/// - Manages blackout/noise injection scheduling for degraded mode
/// - Re-validates license online every 24 hours

use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::health::TaskPulse;
use crate::ClientState;

/// Degraded mode escalation thresholds (seconds into degraded session).
const PHASE_1_END: u64 = 600; // 0-10 min: mild
const PHASE_2_END: u64 = 1800; // 10-30 min: moderate
const SESSION_KILL: u64 = 2700; // 45 min: terminate

pub async fn run(state: Arc<ClientState>, pulse: TaskPulse) -> anyhow::Result<()> {
    let mut tick_interval = tokio::time::interval(Duration::from_secs(1));
    let mut revalidation_interval = tokio::time::interval(Duration::from_secs(24 * 3600));
    // Skip the first immediate tick of the revalidation interval
    revalidation_interval.tick().await;

    let mut blackout_timer: Option<Instant> = None;
    let mut blackout_duration = Duration::ZERO;
    let mut last_blackout_end: Option<Instant> = None;

    loop {
        tokio::select! {
            _ = state.cancel.cancelled() => {
                midi_license::set_blackout(false);
                midi_license::set_noise_injection(false);
                return Ok(());
            }
            _ = tick_interval.tick() => {
                pulse.tick();
                let current = midi_license::current_state();

                match &current {
                    midi_license::state::LicenseState::Trial { .. } => {
                        midi_license::tick_trial(1);
                        midi_license::set_blackout(false);
                        midi_license::set_noise_injection(false);
                    }
                    midi_license::state::LicenseState::Degraded { .. } => {
                        midi_license::tick_degraded_session();
                        let secs = midi_license::degraded_session_secs();

                        // Check session kill
                        if secs >= SESSION_KILL {
                            info!("Degraded session limit reached (45 min) — requesting shutdown");
                            state.restart_requested.store(true, std::sync::atomic::Ordering::Relaxed);
                            state.cancel.cancel();
                            return Ok(());
                        }

                        // Manage blackout scheduling
                        handle_degraded_blackout(
                            secs,
                            &mut blackout_timer,
                            &mut blackout_duration,
                            &mut last_blackout_end,
                        );
                    }
                    _ => {
                        // Licensed or Unlicensed — no enforcement
                        midi_license::set_blackout(false);
                        midi_license::set_noise_injection(false);
                    }
                }
            }
            _ = revalidation_interval.tick() => {
                if midi_license::current_state().is_licensed() {
                    debug!("Running periodic license re-validation");
                    if let Err(e) = midi_license::revalidate(None).await {
                        warn!("License re-validation failed: {e}");
                    }
                }
            }
        }
    }
}

fn handle_degraded_blackout(
    degraded_secs: u64,
    blackout_timer: &mut Option<Instant>,
    blackout_duration: &mut Duration,
    last_blackout_end: &mut Option<Instant>,
) {
    // Determine phase parameters
    let (blackout_dur_secs, interval_secs) = if degraded_secs < PHASE_1_END {
        (3u64, 120u64) // Phase 1: 3s every 2 min
    } else if degraded_secs < PHASE_2_END {
        (5, 90) // Phase 2: 5s every 90s
    } else {
        (8, 60) // Phase 3: 8s every 60s
    };

    // Apply +/-20% jitter to interval
    let jitter_factor = 0.8 + (degraded_secs % 5) as f64 * 0.1; // pseudo-random jitter
    let effective_interval = Duration::from_secs_f64(interval_secs as f64 * jitter_factor);

    let now = Instant::now();

    // Check if we're currently in a blackout
    if let Some(start) = blackout_timer {
        if now.duration_since(*start) >= *blackout_duration {
            // Blackout ended
            midi_license::set_blackout(false);
            // Enable noise injection between blackouts in phase 2+
            midi_license::set_noise_injection(degraded_secs >= PHASE_1_END);
            *blackout_timer = None;
            *last_blackout_end = Some(now);
        }
        return; // Still in blackout
    }

    // Check if it's time for a new blackout
    let should_start = match last_blackout_end {
        Some(last_end) => now.duration_since(*last_end) >= effective_interval,
        None => degraded_secs >= 30, // First blackout after 30s grace
    };

    if should_start {
        debug!(
            phase = if degraded_secs < PHASE_1_END {
                1
            } else if degraded_secs < PHASE_2_END {
                2
            } else {
                3
            },
            duration_s = blackout_dur_secs,
            "Starting MIDI blackout"
        );
        midi_license::set_blackout(true);
        midi_license::set_noise_injection(false); // No noise during blackout
        *blackout_timer = Some(now);
        *blackout_duration = Duration::from_secs(blackout_dur_secs);
    }
}
