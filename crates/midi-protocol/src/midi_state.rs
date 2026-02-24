/// Number of MIDI channels
pub const NUM_CHANNELS: usize = 16;
/// Number of MIDI notes
pub const NUM_NOTES: usize = 128;
/// Number of MIDI CCs
pub const NUM_CCS: usize = 128;

/// Complete MIDI state model for all 16 channels.
/// Serialization is handled by the journal module's custom binary format.
#[derive(Debug, Clone)]
pub struct MidiState {
    pub channels: [ChannelState; NUM_CHANNELS],
}

#[derive(Debug, Clone)]
pub struct ChannelState {
    /// Active notes: velocity > 0 means note is on
    pub notes: [u8; NUM_NOTES],
    /// Controller values (CC 0-127)
    pub cc: [u8; NUM_CCS],
    /// Current program number
    pub program: u8,
    /// Pitch bend (14-bit, center = 8192)
    pub pitch_bend: u16,
    /// Channel pressure (aftertouch)
    pub channel_pressure: u8,
}

impl Default for ChannelState {
    fn default() -> Self {
        Self {
            notes: [0; NUM_NOTES],
            cc: [0; NUM_CCS],
            program: 0,
            pitch_bend: 8192, // center position
            channel_pressure: 0,
        }
    }
}

impl Default for MidiState {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| ChannelState::default()),
        }
    }
}

impl MidiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a raw MIDI message and update state accordingly.
    /// Returns true if state was changed.
    pub fn process_message(&mut self, data: &[u8]) -> bool {
        if data.is_empty() {
            return false;
        }

        let status = data[0];

        // Ignore System messages (0xF0-0xFF) except for channel-relevant ones
        if status >= 0xF0 {
            return false;
        }

        let msg_type = status & 0xF0;
        let channel = (status & 0x0F) as usize;

        match msg_type {
            // Note Off
            0x80 => {
                if data.len() >= 3 {
                    let note = data[1] as usize;
                    if note < NUM_NOTES {
                        self.channels[channel].notes[note] = 0;
                        return true;
                    }
                }
            }
            // Note On
            0x90 => {
                if data.len() >= 3 {
                    let note = data[1] as usize;
                    let velocity = data[2];
                    if note < NUM_NOTES {
                        // Velocity 0 = Note Off
                        self.channels[channel].notes[note] = velocity;
                        return true;
                    }
                }
            }
            // Polyphonic Aftertouch (not tracked in channel state)
            0xA0 => {}
            // Control Change
            0xB0 => {
                if data.len() >= 3 {
                    let cc_num = data[1] as usize;
                    let value = data[2];
                    if cc_num < NUM_CCS {
                        self.channels[channel].cc[cc_num] = value;

                        // Handle special CCs
                        match cc_num {
                            // All Sound Off
                            120 => {
                                self.channels[channel].notes = [0; NUM_NOTES];
                            }
                            // All Notes Off
                            123 => {
                                self.channels[channel].notes = [0; NUM_NOTES];
                            }
                            _ => {}
                        }
                        return true;
                    }
                }
            }
            // Program Change
            0xC0 => {
                if data.len() >= 2 {
                    self.channels[channel].program = data[1];
                    return true;
                }
            }
            // Channel Pressure
            0xD0 => {
                if data.len() >= 2 {
                    self.channels[channel].channel_pressure = data[1];
                    return true;
                }
            }
            // Pitch Bend
            0xE0 => {
                if data.len() >= 3 {
                    let lsb = data[1] as u16;
                    let msb = data[2] as u16;
                    self.channels[channel].pitch_bend = (msb << 7) | lsb;
                    return true;
                }
            }
            _ => {}
        }

        false
    }

    /// Generate MIDI messages to reconcile state after failover.
    /// Sends: All Notes Off on all channels, then restores CCs, programs,
    /// pitch bends, and re-triggers active notes.
    pub fn generate_reconciliation(&self) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();

        for ch in 0..NUM_CHANNELS {
            let channel = &self.channels[ch];
            let ch_byte = ch as u8;

            // First: All Sound Off (CC 120) to immediately silence
            messages.push(vec![0xB0 | ch_byte, 120, 0]);

            // Restore CC values (skip special CCs 120-127)
            for cc in 0..120 {
                if channel.cc[cc] != 0 {
                    messages.push(vec![0xB0 | ch_byte, cc as u8, channel.cc[cc]]);
                }
            }

            // Restore program
            if channel.program != 0 {
                messages.push(vec![0xC0 | ch_byte, channel.program]);
            }

            // Restore pitch bend (skip if centered)
            if channel.pitch_bend != 8192 {
                let lsb = (channel.pitch_bend & 0x7F) as u8;
                let msb = ((channel.pitch_bend >> 7) & 0x7F) as u8;
                messages.push(vec![0xE0 | ch_byte, lsb, msb]);
            }

            // Restore channel pressure
            if channel.channel_pressure != 0 {
                messages.push(vec![0xD0 | ch_byte, channel.channel_pressure]);
            }

            // Re-trigger active notes
            for note in 0..NUM_NOTES {
                if channel.notes[note] > 0 {
                    messages.push(vec![0x90 | ch_byte, note as u8, channel.notes[note]]);
                }
            }
        }

        messages
    }

    /// Count total active notes across all channels
    pub fn active_note_count(&self) -> usize {
        self.channels
            .iter()
            .flat_map(|ch| ch.notes.iter())
            .filter(|&&v| v > 0)
            .count()
    }

    /// Reset all state to defaults
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_on_off() {
        let mut state = MidiState::new();

        // Note On: channel 0, note 60 (C4), velocity 100
        state.process_message(&[0x90, 60, 100]);
        assert_eq!(state.channels[0].notes[60], 100);
        assert_eq!(state.active_note_count(), 1);

        // Note Off: channel 0, note 60
        state.process_message(&[0x80, 60, 0]);
        assert_eq!(state.channels[0].notes[60], 0);
        assert_eq!(state.active_note_count(), 0);
    }

    #[test]
    fn test_note_on_velocity_zero_is_off() {
        let mut state = MidiState::new();

        state.process_message(&[0x90, 60, 100]);
        assert_eq!(state.active_note_count(), 1);

        // Note On with velocity 0 = Note Off
        state.process_message(&[0x90, 60, 0]);
        assert_eq!(state.active_note_count(), 0);
    }

    #[test]
    fn test_cc() {
        let mut state = MidiState::new();

        // CC 1 (Modulation) on channel 0, value 64
        state.process_message(&[0xB0, 1, 64]);
        assert_eq!(state.channels[0].cc[1], 64);
    }

    #[test]
    fn test_all_notes_off() {
        let mut state = MidiState::new();

        state.process_message(&[0x90, 60, 100]);
        state.process_message(&[0x90, 64, 100]);
        state.process_message(&[0x91, 60, 100]); // channel 1
        assert_eq!(state.active_note_count(), 3);

        // All Notes Off on channel 0
        state.process_message(&[0xB0, 123, 0]);
        assert_eq!(state.channels[0].notes[60], 0);
        assert_eq!(state.channels[0].notes[64], 0);
        // Channel 1 unaffected
        assert_eq!(state.channels[1].notes[60], 100);
        assert_eq!(state.active_note_count(), 1);
    }

    #[test]
    fn test_program_change() {
        let mut state = MidiState::new();
        state.process_message(&[0xC0, 42]);
        assert_eq!(state.channels[0].program, 42);
    }

    #[test]
    fn test_pitch_bend() {
        let mut state = MidiState::new();
        // Pitch bend: LSB=0, MSB=96 → value = 96*128 = 12288
        state.process_message(&[0xE0, 0, 96]);
        assert_eq!(state.channels[0].pitch_bend, 12288);
    }

    #[test]
    fn test_reconciliation_generates_messages() {
        let mut state = MidiState::new();

        // Set some state
        state.process_message(&[0x90, 60, 100]); // Note On
        state.process_message(&[0xB0, 1, 64]); // CC1 = 64
        state.process_message(&[0xC0, 5]); // Program 5

        let messages = state.generate_reconciliation();

        // Should contain at least: All Sound Off, CC1 restore, Program restore, Note retrigger
        assert!(!messages.is_empty());

        // First message for ch0 should be All Sound Off
        assert_eq!(messages[0], vec![0xB0, 120, 0]);

        // Should contain CC1 restore
        assert!(messages.contains(&vec![0xB0, 1, 64]));

        // Should contain program change
        assert!(messages.contains(&vec![0xC0, 5]));

        // Should contain note retrigger
        assert!(messages.contains(&vec![0x90, 60, 100]));
    }

    #[test]
    fn test_multichannel() {
        let mut state = MidiState::new();

        state.process_message(&[0x90, 60, 100]); // Ch 0
        state.process_message(&[0x95, 64, 80]); // Ch 5
        state.process_message(&[0x9F, 48, 60]); // Ch 15

        assert_eq!(state.channels[0].notes[60], 100);
        assert_eq!(state.channels[5].notes[64], 80);
        assert_eq!(state.channels[15].notes[48], 60);
        assert_eq!(state.active_note_count(), 3);
    }
}
