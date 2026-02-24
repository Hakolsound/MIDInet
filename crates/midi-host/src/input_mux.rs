/// Input multiplexer for dual-controller redundancy.
///
/// Manages two MIDI input sources (SPSC ring buffer consumers) and presents
/// a single `pop()` interface to the broadcaster. Automatically switches to
/// the secondary controller when the primary fails.
///
/// The inactive controller's ring buffer is periodically drained to prevent
/// stale data accumulation.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

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
}

impl InputMux {
    pub fn new(primary: MidiConsumer, secondary: MidiConsumer) -> Self {
        Self {
            consumers: [primary, secondary],
            active: AtomicU8::new(INPUT_PRIMARY),
            force_journal: AtomicBool::new(false),
            switch_notify: Arc::new(Notify::new()),
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
    fn switch(&self) -> u8 {
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

    /// Drain all pending messages from the inactive consumer.
    fn drain_inactive(&self) {
        let active = self.active.load(Ordering::Relaxed) as usize;
        let inactive = 1 - active;
        let mut discard = [0u8; SLOT_SIZE];
        while self.consumers[inactive].try_pop(&mut discard).is_some() {}
    }
}

/// Run the health monitor that listens for input health events
/// and triggers switchover when the active input fails.
///
/// `input_index` in the received tuple identifies which input (0 or 1)
/// reported the health event.
pub async fn run_health_monitor(
    mux: Arc<InputMux>,
    mut health_rx: mpsc::Receiver<(u8, InputHealth)>,
    input_switch_count: Arc<std::sync::atomic::AtomicU64>,
) {
    while let Some((index, health)) = health_rx.recv().await {
        match health {
            InputHealth::Active => {
                info!(input = index, "Input controller active");
            }
            InputHealth::Error(ref msg) | InputHealth::Disconnected(ref msg) => {
                let active = mux.active_input();
                if index == active {
                    warn!(
                        input = index,
                        error = %msg,
                        "Active input failed — switching to {}",
                        if active == INPUT_PRIMARY { "secondary" } else { "primary" }
                    );
                    let new = mux.switch();
                    input_switch_count.fetch_add(1, Ordering::Relaxed);
                    info!(new_active = new, "Input failover complete");
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
}
