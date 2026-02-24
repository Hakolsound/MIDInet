/// Failover management for the host daemon.
/// Handles primary/standby negotiation and manual switch triggers.
/// Thread-safe — uses interior mutability so it can be shared via Arc.

use std::sync::Mutex;
use std::time::Instant;

use midi_protocol::packets::HostRole;
use tokio::sync::watch;
use tracing::info;

pub struct FailoverManager {
    lockout_seconds: u64,
    role_tx: watch::Sender<HostRole>,
    last_switch: Mutex<Option<Instant>>,
}

impl FailoverManager {
    pub fn new(lockout_seconds: u64, role_tx: watch::Sender<HostRole>) -> Self {
        Self {
            lockout_seconds,
            role_tx,
            last_switch: Mutex::new(None),
        }
    }

    /// Check if a switch is allowed based on safety measures (lockout period)
    pub fn can_switch(&self) -> bool {
        if let Ok(guard) = self.last_switch.lock() {
            if let Some(last) = *guard {
                let lockout = std::time::Duration::from_secs(self.lockout_seconds);
                if last.elapsed() < lockout {
                    return false;
                }
            }
        }
        true
    }

    /// Trigger a failover switch. Returns true if the switch was performed.
    pub fn trigger_switch(&self, role_tx: &watch::Sender<HostRole>) -> bool {
        if !self.can_switch() {
            info!("Switch blocked by lockout period");
            return false;
        }

        let current_role = *role_tx.borrow();
        let new_role = match current_role {
            HostRole::Primary => HostRole::Standby,
            HostRole::Standby => HostRole::Primary,
        };

        role_tx.send(new_role).ok();

        if let Ok(mut guard) = self.last_switch.lock() {
            *guard = Some(Instant::now());
        }

        info!(
            from = ?current_role,
            to = ?new_role,
            "Failover switch triggered"
        );

        true
    }

    /// Get the current role
    pub fn current_role(&self) -> HostRole {
        *self.role_tx.borrow()
    }
}
