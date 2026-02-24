/// Lock-free SPSC (Single-Producer, Single-Consumer) ring buffer for the MIDI hot path.
///
/// Design goals:
///   - Zero heap allocation after creation (all slots pre-allocated)
///   - Cache-line friendly: head and tail on separate cache lines to avoid false sharing
///   - Bounded: fixed capacity, oldest messages dropped on overflow (real-time priority)
///   - Paired with `tokio::sync::Notify` for async consumer wakeup
///
/// Typical flow:
///   Producer (USB reader thread):  `push(&midi_bytes)` → writes to slot, advances head
///   Consumer (broadcaster task):   `pop(&mut buf)` → reads from slot, advances tail
///
/// Safety: This is SPSC only. One thread calls push(), one thread calls pop().
/// Using it with multiple producers or consumers is undefined behavior.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Maximum MIDI message size per slot.
/// Covers all standard MIDI 1.0 messages. SysEx messages larger than this
/// will be truncated — in practice, MIDI controller SysEx rarely exceeds 128 bytes.
pub const SLOT_SIZE: usize = 256;

/// A single slot in the ring buffer.
#[repr(C)]
struct Slot {
    data: [u8; SLOT_SIZE],
    len: u16,
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            data: [0u8; SLOT_SIZE],
            len: 0,
        }
    }
}

/// Cache line size for padding to avoid false sharing.
const CACHE_LINE: usize = 64;

/// The ring buffer internals, shared between producer and consumer.
#[repr(C)]
pub struct MidiRingBufferInner {
    /// Write position (only modified by producer)
    head: AtomicUsize,
    _pad_head: [u8; CACHE_LINE - std::mem::size_of::<AtomicUsize>()],

    /// Read position (only modified by consumer)
    tail: AtomicUsize,
    _pad_tail: [u8; CACHE_LINE - std::mem::size_of::<AtomicUsize>()],

    /// Pre-allocated slots
    slots: Box<[UnsafeCell<Slot>]>,
    capacity: usize,
}

// SAFETY: SPSC contract — head is only written by producer, tail by consumer.
// Atomic operations provide the necessary memory ordering.
unsafe impl Send for MidiRingBufferInner {}
unsafe impl Sync for MidiRingBufferInner {}

impl MidiRingBufferInner {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0 && capacity.is_power_of_two(), "Capacity must be a power of two");

        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(UnsafeCell::new(Slot::default()));
        }

        Self {
            head: AtomicUsize::new(0),
            _pad_head: [0u8; CACHE_LINE - std::mem::size_of::<AtomicUsize>()],
            tail: AtomicUsize::new(0),
            _pad_tail: [0u8; CACHE_LINE - std::mem::size_of::<AtomicUsize>()],
            slots: slots.into_boxed_slice(),
            capacity,
        }
    }

    /// Push a MIDI message into the buffer. Returns true if successful, false if full.
    /// If the message exceeds SLOT_SIZE, it is truncated.
    ///
    /// SAFETY: Must only be called from the producer thread.
    #[inline]
    pub fn push(&self, data: &[u8]) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Acquire);

        // Check if buffer is full
        if head.wrapping_sub(tail) >= self.capacity {
            return false;
        }

        let idx = head & (self.capacity - 1); // Power-of-two mask
        let len = data.len().min(SLOT_SIZE);

        // SAFETY: We're the only producer writing to this slot, and the slot at `head`
        // is not being read by the consumer (consumer only reads at `tail`).
        unsafe {
            let slot = &mut *self.slots[idx].get();
            slot.data[..len].copy_from_slice(&data[..len]);
            slot.len = len as u16;
        }

        // Release ordering ensures the data write is visible before head advances
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Pop a MIDI message from the buffer into the provided buffer.
    /// Returns Some(len) if a message was available, None if empty.
    ///
    /// SAFETY: Must only be called from the consumer thread.
    #[inline]
    pub fn pop(&self, buf: &mut [u8; SLOT_SIZE]) -> Option<usize> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None; // Empty
        }

        let idx = tail & (self.capacity - 1);

        // SAFETY: We're the only consumer reading this slot, and the producer has moved
        // past it (head > tail).
        let len = unsafe {
            let slot = &*self.slots[idx].get();
            let len = slot.len as usize;
            buf[..len].copy_from_slice(&slot.data[..len]);
            len
        };

        // Release ordering ensures we've finished reading before advancing tail
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(len)
    }

    /// Number of messages currently in the buffer.
    #[inline]
    pub fn len(&self) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    /// Whether the buffer is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Producer half of the MIDI ring buffer.
/// Send this to the MIDI reader thread.
pub struct MidiProducer {
    inner: Arc<MidiRingBufferInner>,
    notify: Arc<tokio::sync::Notify>,
}

/// Consumer half of the MIDI ring buffer.
/// Used by the async broadcaster task.
pub struct MidiConsumer {
    inner: Arc<MidiRingBufferInner>,
    notify: Arc<tokio::sync::Notify>,
}

// Only the producer can push
unsafe impl Send for MidiProducer {}
// Only the consumer can pop
unsafe impl Send for MidiConsumer {}

/// Create a new MIDI ring buffer pair (producer, consumer).
/// Capacity must be a power of two.
pub fn midi_ring_buffer(capacity: usize) -> (MidiProducer, MidiConsumer) {
    let inner = Arc::new(MidiRingBufferInner::new(capacity));
    let notify = Arc::new(tokio::sync::Notify::new());

    let producer = MidiProducer {
        inner: Arc::clone(&inner),
        notify: Arc::clone(&notify),
    };
    let consumer = MidiConsumer {
        inner,
        notify,
    };

    (producer, consumer)
}

impl MidiProducer {
    /// Push a MIDI message and notify the consumer.
    /// Returns true if the message was enqueued, false if the buffer is full.
    #[inline]
    pub fn push(&self, data: &[u8]) -> bool {
        let ok = self.inner.push(data);
        if ok {
            self.notify.notify_one();
        }
        ok
    }

    /// Push a MIDI message, dropping the oldest if full (real-time priority).
    /// Always succeeds — in a real-time system, we'd rather lose old data than block.
    #[inline]
    pub fn push_overwrite(&self, data: &[u8]) {
        if !self.inner.push(data) {
            // Buffer full — advance tail to make room (drop oldest)
            let tail = self.inner.tail.load(Ordering::Relaxed);
            self.inner.tail.store(tail.wrapping_add(1), Ordering::Release);
            // Retry
            let _ = self.inner.push(data);
        }
        self.notify.notify_one();
    }
}

impl MidiConsumer {
    /// Try to pop a message without blocking.
    #[inline]
    pub fn try_pop(&self, buf: &mut [u8; SLOT_SIZE]) -> Option<usize> {
        self.inner.pop(buf)
    }

    /// Wait for a message asynchronously. Returns the message length.
    pub async fn pop(&self, buf: &mut [u8; SLOT_SIZE]) -> usize {
        loop {
            if let Some(len) = self.inner.pop(buf) {
                return len;
            }
            self.notify.notified().await;
        }
    }

    /// Drain all available messages, calling the closure for each.
    /// Useful for batching multiple MIDI messages into a single network packet.
    pub fn drain(&self, mut f: impl FnMut(&[u8])) {
        let mut buf = [0u8; SLOT_SIZE];
        while let Some(len) = self.inner.pop(&mut buf) {
            f(&buf[..len]);
        }
    }

    /// Number of messages available.
    #[inline]
    pub fn available(&self) -> usize {
        self.inner.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_basic() {
        let (producer, consumer) = midi_ring_buffer(16);
        let data = [0x90, 0x3C, 0x7F]; // Note On C4 vel 127

        assert!(producer.push(&data));
        let mut buf = [0u8; SLOT_SIZE];
        let len = consumer.try_pop(&mut buf).unwrap();
        assert_eq!(len, 3);
        assert_eq!(&buf[..3], &data);
    }

    #[test]
    fn empty_returns_none() {
        let (_producer, consumer) = midi_ring_buffer(16);
        let mut buf = [0u8; SLOT_SIZE];
        assert!(consumer.try_pop(&mut buf).is_none());
    }

    #[test]
    fn full_buffer_rejects() {
        let (producer, _consumer) = midi_ring_buffer(4);
        assert!(producer.push(&[0x90, 0x3C, 0x7F]));
        assert!(producer.push(&[0x90, 0x3D, 0x7F]));
        assert!(producer.push(&[0x90, 0x3E, 0x7F]));
        assert!(producer.push(&[0x90, 0x3F, 0x7F]));
        // Buffer full
        assert!(!producer.push(&[0x90, 0x40, 0x7F]));
    }

    #[test]
    fn overwrite_drops_oldest() {
        let (producer, consumer) = midi_ring_buffer(4);
        producer.push_overwrite(&[0x01]);
        producer.push_overwrite(&[0x02]);
        producer.push_overwrite(&[0x03]);
        producer.push_overwrite(&[0x04]);
        // This should drop 0x01
        producer.push_overwrite(&[0x05]);

        let mut buf = [0u8; SLOT_SIZE];
        let len = consumer.try_pop(&mut buf).unwrap();
        assert_eq!(&buf[..len], &[0x02]); // 0x01 was dropped
    }

    #[test]
    fn fifo_order() {
        let (producer, consumer) = midi_ring_buffer(16);
        for i in 0u8..10 {
            producer.push(&[0x90, i, 0x7F]);
        }

        let mut buf = [0u8; SLOT_SIZE];
        for i in 0u8..10 {
            let len = consumer.try_pop(&mut buf).unwrap();
            assert_eq!(len, 3);
            assert_eq!(buf[1], i);
        }
        assert!(consumer.try_pop(&mut buf).is_none());
    }

    #[test]
    fn wraparound() {
        let (producer, consumer) = midi_ring_buffer(4);
        let mut buf = [0u8; SLOT_SIZE];

        // Fill and drain multiple times to exercise wraparound
        for round in 0u8..10 {
            for j in 0u8..4 {
                assert!(producer.push(&[round, j]));
            }
            for j in 0u8..4 {
                let len = consumer.try_pop(&mut buf).unwrap();
                assert_eq!(len, 2);
                assert_eq!(buf[0], round);
                assert_eq!(buf[1], j);
            }
        }
    }

    #[test]
    fn large_message_truncated() {
        let (producer, consumer) = midi_ring_buffer(4);
        let big = vec![0xF0; SLOT_SIZE + 100]; // Larger than slot
        assert!(producer.push(&big));

        let mut buf = [0u8; SLOT_SIZE];
        let len = consumer.try_pop(&mut buf).unwrap();
        assert_eq!(len, SLOT_SIZE); // Truncated to slot size
    }
}
