use crate::midi_state::{MidiState, NUM_CHANNELS, NUM_CCS, NUM_NOTES};

/// Compact binary journal for MIDI state recovery.
/// Inspired by RFC 6295 MIDI journal but simplified for our use case.
///
/// The journal is a compact snapshot of the current MIDI state that can be
/// appended to data packets periodically. On failover or packet loss,
/// receivers use the journal to reconstruct full state.
///
/// Format:
/// [channel_mask: 2 bytes] — bitmask of channels that have non-default state
/// For each set channel:
///   [flags: 1 byte] — which state types are present
///   [active_notes: variable] — compressed note bitmap + velocities
///   [cc_values: variable] — non-zero CC numbers + values
///   [program: 1 byte] — if flag set
///   [pitch_bend: 2 bytes] — if flag set (and not center)
///   [channel_pressure: 1 byte] — if flag set

const FLAG_HAS_NOTES: u8 = 0x01;
const FLAG_HAS_CC: u8 = 0x02;
const FLAG_HAS_PROGRAM: u8 = 0x04;
const FLAG_HAS_PITCH_BEND: u8 = 0x08;
const FLAG_HAS_PRESSURE: u8 = 0x10;

/// Encode the current MIDI state into a compact journal.
pub fn encode_journal(state: &MidiState) -> Vec<u8> {
    let mut buf = Vec::with_capacity(256);

    // Channel mask: which channels have non-default state
    let mut channel_mask: u16 = 0;
    for ch in 0..NUM_CHANNELS {
        if has_state(&state.channels[ch]) {
            channel_mask |= 1 << ch;
        }
    }

    buf.extend_from_slice(&channel_mask.to_be_bytes());

    for ch in 0..NUM_CHANNELS {
        if channel_mask & (1 << ch) == 0 {
            continue;
        }

        let channel = &state.channels[ch];
        let mut flags: u8 = 0;

        // Determine what state to encode
        let active_notes: Vec<(u8, u8)> = (0..NUM_NOTES)
            .filter(|&n| channel.notes[n] > 0)
            .map(|n| (n as u8, channel.notes[n]))
            .collect();

        let non_zero_cc: Vec<(u8, u8)> = (0..NUM_CCS)
            .filter(|&c| channel.cc[c] != 0)
            .map(|c| (c as u8, channel.cc[c]))
            .collect();

        if !active_notes.is_empty() {
            flags |= FLAG_HAS_NOTES;
        }
        if !non_zero_cc.is_empty() {
            flags |= FLAG_HAS_CC;
        }
        if channel.program != 0 {
            flags |= FLAG_HAS_PROGRAM;
        }
        if channel.pitch_bend != 8192 {
            flags |= FLAG_HAS_PITCH_BEND;
        }
        if channel.channel_pressure != 0 {
            flags |= FLAG_HAS_PRESSURE;
        }

        buf.push(flags);

        // Encode active notes: [count(1)] [note, velocity] pairs
        if flags & FLAG_HAS_NOTES != 0 {
            buf.push(active_notes.len() as u8);
            for (note, vel) in &active_notes {
                buf.push(*note);
                buf.push(*vel);
            }
        }

        // Encode CC values: [count(1)] [cc_num, value] pairs
        if flags & FLAG_HAS_CC != 0 {
            buf.push(non_zero_cc.len() as u8);
            for (cc, val) in &non_zero_cc {
                buf.push(*cc);
                buf.push(*val);
            }
        }

        if flags & FLAG_HAS_PROGRAM != 0 {
            buf.push(channel.program);
        }

        if flags & FLAG_HAS_PITCH_BEND != 0 {
            buf.extend_from_slice(&channel.pitch_bend.to_be_bytes());
        }

        if flags & FLAG_HAS_PRESSURE != 0 {
            buf.push(channel.channel_pressure);
        }
    }

    buf
}

/// Decode a journal back into a MIDI state.
pub fn decode_journal(data: &[u8]) -> Option<MidiState> {
    if data.len() < 2 {
        return None;
    }

    let mut state = MidiState::new();
    let channel_mask = u16::from_be_bytes([data[0], data[1]]);
    let mut offset = 2;

    for ch in 0..NUM_CHANNELS {
        if channel_mask & (1 << ch) == 0 {
            continue;
        }

        if offset >= data.len() {
            return None;
        }

        let flags = data[offset];
        offset += 1;

        // Decode notes
        if flags & FLAG_HAS_NOTES != 0 {
            if offset >= data.len() {
                return None;
            }
            let count = data[offset] as usize;
            offset += 1;
            if offset + count * 2 > data.len() {
                return None;
            }
            for i in 0..count {
                let note = data[offset + i * 2] as usize;
                let vel = data[offset + i * 2 + 1];
                if note < NUM_NOTES {
                    state.channels[ch].notes[note] = vel;
                }
            }
            offset += count * 2;
        }

        // Decode CCs
        if flags & FLAG_HAS_CC != 0 {
            if offset >= data.len() {
                return None;
            }
            let count = data[offset] as usize;
            offset += 1;
            if offset + count * 2 > data.len() {
                return None;
            }
            for i in 0..count {
                let cc_num = data[offset + i * 2] as usize;
                let val = data[offset + i * 2 + 1];
                if cc_num < NUM_CCS {
                    state.channels[ch].cc[cc_num] = val;
                }
            }
            offset += count * 2;
        }

        // Decode program
        if flags & FLAG_HAS_PROGRAM != 0 {
            if offset >= data.len() {
                return None;
            }
            state.channels[ch].program = data[offset];
            offset += 1;
        }

        // Decode pitch bend
        if flags & FLAG_HAS_PITCH_BEND != 0 {
            if offset + 2 > data.len() {
                return None;
            }
            state.channels[ch].pitch_bend =
                u16::from_be_bytes([data[offset], data[offset + 1]]);
            offset += 2;
        }

        // Decode channel pressure
        if flags & FLAG_HAS_PRESSURE != 0 {
            if offset >= data.len() {
                return None;
            }
            state.channels[ch].channel_pressure = data[offset];
            offset += 1;
        }
    }

    Some(state)
}

fn has_state(ch: &crate::midi_state::ChannelState) -> bool {
    ch.notes.iter().any(|&v| v > 0)
        || ch.cc.iter().any(|&v| v != 0)
        || ch.program != 0
        || ch.pitch_bend != 8192
        || ch.channel_pressure != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_state_journal() {
        let state = MidiState::new();
        let journal = encode_journal(&state);
        // Only channel mask (2 bytes, all zeros)
        assert_eq!(journal.len(), 2);
        assert_eq!(journal, vec![0, 0]);
    }

    #[test]
    fn test_journal_roundtrip() {
        let mut state = MidiState::new();

        // Set some state
        state.process_message(&[0x90, 60, 100]); // Note On ch0
        state.process_message(&[0x95, 64, 80]); // Note On ch5
        state.process_message(&[0xB0, 1, 64]); // CC1 ch0
        state.process_message(&[0xC0, 42]); // Program ch0
        state.process_message(&[0xE0, 0, 96]); // Pitch Bend ch0

        let journal = encode_journal(&state);
        let decoded = decode_journal(&journal).unwrap();

        // Verify ch0
        assert_eq!(decoded.channels[0].notes[60], 100);
        assert_eq!(decoded.channels[0].cc[1], 64);
        assert_eq!(decoded.channels[0].program, 42);
        assert_eq!(decoded.channels[0].pitch_bend, 96 << 7); // 12288

        // Verify ch5
        assert_eq!(decoded.channels[5].notes[64], 80);

        // Verify other channels are default
        assert_eq!(decoded.channels[1].notes[60], 0);
        assert_eq!(decoded.active_note_count(), 2);
    }

    #[test]
    fn test_journal_size_efficient() {
        let mut state = MidiState::new();

        // Single note on channel 0
        state.process_message(&[0x90, 60, 100]);

        let journal = encode_journal(&state);
        // channel_mask(2) + flags(1) + note_count(1) + note_pair(2) = 6 bytes
        assert_eq!(journal.len(), 6);
    }

    #[test]
    fn test_decode_invalid_data() {
        assert!(decode_journal(&[]).is_none());
        assert!(decode_journal(&[0]).is_none());
    }
}
