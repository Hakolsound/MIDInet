/// Input multiplexer for dual-controller redundancy.
///
/// Manages two MIDI input sources (SPSC ring buffer consumers) and presents
/// a single `pop()` interface to the broadcaster. Automatically switches to
/// the secondary controller when the primary fails.
///
/// The inactive controller's ring buffer is periodically drained to prevent
/// stale data accumulation.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use midi_protocol::ringbuf::{MidiConsumer, SLOT_SIZE};
use tokio::sync::{mpsc, Notify};
use tracing::{info, warn};

use crate::usb_reader::InputHealth;

/// Which input is currently active.
const INPUT_PRIMARY: u8 = 0;
const INPUT_SECONDARY: u8 = 1;

/// Input multiplexer for dual-controller active/passive failover.
pub struct InputMux {
    consumers: [MidiConsumer; 2],
    active: AtomicU8,
    /// Set when the active input switches — signals the broadcaster to
    /// force-attach a journal for state reconciliation.
    force_journal: AtomicBool,
    /// Wakes the broadcaster pop loop when a switch occurs (so it doesn't
    /// block waiting on the old consumer's Notify).
    switch_notify: Arc<Notify>,
    /// Timestamp of the last MIDI data received (any input).
    /// Used for activity-timeout failover.
    last_active_data: AtomicU64,
}

impl InputMux {
    pub fn new(primary: MidiConsumer, secondary: MidiConsumer) -> Self {
        Self {
            consumers: [primary, secondary],
            active: AtomicU8::new(INPUT_PRIMARY),
            force_journal: AtomicBool::new(false),
            switch_notify: Arc::new(Notify::new()),
            last_active_data: AtomicU64::new(now_nanos()),
        }
    }

    /// Read the next MIDI message from the active input.
    /// Drains the inactive input's buffer to prevent stale data buildup.
    /// Returns the number of bytes read into `buf`.
    pub async fn pop(&self, buf: &mut [u8; SLOT_SIZE]) -> usize {
        // Drain any pending data from the inactive consumer
        self.drain_inactive();

        // Race: wait for data on active consumer OR a switch event.
        // On switch, we loop back and read from the new active consumer.
        loop {
            let active_idx = self.active.load(Ordering::Acquire) as usize;

            tokio::select! {
                len = self.consumers[active_idx].pop(buf) => {
                    // Record data timestamp for activity-timeout tracking
                    self.last_active_data.store(now_nanos(), Ordering::Relaxed);
                    return len;
                }
                _ = self.switch_notify.notified() => {
                    // Active input changed — drain the now-inactive buffer
                    // and retry on the new active consumer
                    self.drain_inactive();
                    continue;
                }
            }
        }
    }

    /// Switch to the other input. Returns the new active index.
    /// Can be called by the health monitor (automatic) or externally (manual).
    pub fn switch(&self) -> u8 {
        let current = self.active.load(Ordering::Acquire);
        let new = if current == INPUT_PRIMARY {
            INPUT_SECONDARY
        } else {
            INPUT_PRIMARY
        };
        self.active.store(new, Ordering::Release);
        self.force_journal.store(true, Ordering::Release);
        self.switch_notify.notify_one();
        new
    }

    /// Check and clear the force-journal flag.
    /// The broadcaster calls this to know when to attach a journal
    /// for state reconciliation after an input switch.
    pub fn take_force_journal(&self) -> bool {
        self.force_journal.swap(false, Ordering::AcqRel)
    }

    /// Get the currently active input index (0 = primary, 1 = secondary).
    pub fn active_input(&self) -> u8 {
        self.active.load(Ordering::Relaxed)
    }

    /// Elapsed nanoseconds since the last MIDI data was received on the active input.
    pub fn active_idle_nanos(&self) -> u64 {
        let last = self.last_active_data.load(Ordering::Relaxed);
        now_nanos().saturating_sub(last)
    }

    /// Drain all pending messages from the inactive consumer.
    fn drain_inactive(&self) {
        let active = self.active.load(Ordering::Relaxed) as usize;
        let inactive = 1 - active;
        let mut discard = [0u8; SLOT_SIZE];
        while self.consumers[inactive].try_pop(&mut discard).is_some() {}
    }
}

fn now_nanos() -> u64 {
    // Monotonic clock — won't jump on NTP adjustments
    // We encode as u64 nanos from an arbitrary epoch (process start)
    // Fine for relative comparisons within the same process
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    Instant::now().duration_since(*epoch).as_nanos() as u64
}

/// Run the health monitor that listens for input health events,
/// triggers switchover when the active input fails, and enforces
/// activity-timeout failover.
///
/// `shared_input_active` is written on every switch so the SharedState
/// stays in sync with the mux.
///
/// `activity_timeout` triggers a switch if the active input produces no
/// MIDI data for the given duration (Duration::ZERO = disabled).
pub async fn run_health_monitor(
    mux: Arc<InputMux>,
    mut health_rx: mpsc::Receiver<(u8, InputHealth)>,
    input_switch_count: Arc<AtomicU64>,
    shared_input_active: Arc<AtomicU8>,
    dual_input_enabled: bool,
    activity_timeout: Duration,
) {
    let activity_check_enabled = dual_input_enabled && !activity_timeout.is_zero();
    let activity_check_interval = if activity_check_enabled {
        // Check at half the timeout interval, minimum 500ms
        Duration::from_millis((activity_timeout.as_millis() as u64 / 2).max(500))
    } else {
        Duration::from_secs(3600) // effectively disabled
    };

    let mut activity_ticker = tokio::time::interval(activity_check_interval);
    // Don't pile up ticks if processing is slow
    activity_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Track per-input health for the admin state reporting
    let mut input_health: [InputHealthState; 2] = [InputHealthState::Unknown, InputHealthState::Unknown];

    loop {
        tokio::select! {
            event = health_rx.recv() => {
                let Some((index, health)) = event else {
                    info!("Health channel closed — monitor exiting");
                    return;
                };

                match health {
                    InputHealth::Active => {
                        info!(input = index, "Input controller active");
                        input_health[index as usize] = InputHealthState::Active;
                    }
                    InputHealth::Reconnecting => {
                        info!(input = index, "Input controller reconnecting");
                        input_health[index as usize] = InputHealthState::Reconnecting;
                    }
                    InputHealth::Error(ref msg) | InputHealth::Disconnected(ref msg) => {
                        let is_error = matches!(health, InputHealth::Error(_));
                        input_health[index as usize] = if is_error {
                            InputHealthState::Error
                        } else {
                            InputHealthState::Disconnected
                        };

                        let active = mux.active_input();
                        if index == active && dual_input_enabled {
                            // Check if the other input is healthy enough to switch to
                            let other = 1 - index;
                            if input_health[other as usize] == InputHealthState::Active {
                                warn!(
                                    input = index,
                                    error = %msg,
                                    "Active input failed — switching to {}",
                                    if active == INPUT_PRIMARY { "secondary" } else { "primary" }
                                );
                                do_switch(&mux, &input_switch_count, &shared_input_active);
                            } else {
                                warn!(
                                    input = index,
                                    error = %msg,
                                    other_health = ?input_health[other as usize],
                                    "Active input failed — other input not healthy, staying put"
                                );
                            }
                        } else if index == active {
                            warn!(
                                input = index,
                                error = %msg,
                                "Input failed — single-controller mode, no failover available"
                            );
                        } else {
                            warn!(
                                input = index,
                                error = %msg,
                                "Standby input failed — no redundancy available"
                            );
                        }
                    }
                }
            }

            _ = activity_ticker.tick(), if activity_check_enabled => {
                let idle_nanos = mux.active_idle_nanos();
                let idle_duration = Duration::from_nanos(idle_nanos);

                if idle_duration >= activity_timeout {
                    let active = mux.active_input();
                    let other = 1 - active;

                    // Only switch if the other input is healthy
                    if input_health[other as usize] == InputHealthState::Active {
                        warn!(
                            active_input = active,
                            idle_secs = idle_duration.as_secs_f32(),
                            timeout_secs = activity_timeout.as_secs_f32(),
                            "Active input silent for too long — switching to {}",
                            if active == INPUT_PRIMARY { "secondary" } else { "primary" }
                        );
                        do_switch(&mux, &input_switch_count, &shared_input_active);
                    }
                }
            }
        }
    }
}

fn do_switch(
    mux: &Arc<InputMux>,
    input_switch_count: &Arc<AtomicU64>,
    shared_input_active: &Arc<AtomicU8>,
) {
    let new = mux.switch();
    input_switch_count.fetch_add(1, Ordering::Relaxed);
    shared_input_active.store(new, Ordering::Relaxed);
    info!(new_active = new, "Input failover complete");
}

/// Internal health state per input, tracked by the health monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputHealthState {
    Unknown,
    Active,
    Reconnecting,
    Error,
    Disconnected,
}
