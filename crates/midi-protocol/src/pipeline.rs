/// MIDI processing pipeline.
/// Applies filters, remaps, velocity curves, and transforms to MIDI data.
/// Shared between host (outbound) and client (inbound + feedback) paths.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// Enable/disable specific MIDI channels (index 0-15 = channels 1-16)
    #[serde(default = "default_channels")]
    pub channel_filter: [bool; 16],

    /// Message type filter
    #[serde(default)]
    pub message_filter: MessageFilter,

    /// Channel remap: index = source channel, value = destination channel (0xFF = no remap)
    #[serde(default = "default_channel_remap")]
    pub channel_remap: [u8; 16],

    /// Note transpose per channel (signed, -48 to +48 semitones)
    #[serde(default)]
    pub transpose: [i8; 16],

    /// Velocity curve type
    #[serde(default)]
    pub velocity_curve: VelocityCurve,

    /// SysEx passthrough
    #[serde(default = "default_true")]
    pub sysex_passthrough: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageFilter {
    pub note_on_off: bool,
    pub control_change: bool,
    pub program_change: bool,
    pub pitch_bend: bool,
    pub aftertouch: bool,
    pub sysex: bool,
    pub clock: bool,
}

impl Default for MessageFilter {
    fn default() -> Self {
        Self {
            note_on_off: true,
            control_change: true,
            program_change: true,
            pitch_bend: true,
            aftertouch: true,
            sysex: true,
            clock: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub enum VelocityCurve {
    #[default]
    Linear,
    Logarithmic,
    Exponential,
    SCurve,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            channel_filter: [true; 16],
            message_filter: MessageFilter::default(),
            channel_remap: [0xFF; 16],
            transpose: [0; 16],
            velocity_curve: VelocityCurve::default(),
            sysex_passthrough: true,
        }
    }
}

fn default_channels() -> [bool; 16] {
    [true; 16]
}

fn default_channel_remap() -> [u8; 16] {
    [0xFF; 16] // 0xFF = no remap
}

fn default_true() -> bool {
    true
}

impl PipelineConfig {
    /// Process a MIDI message through the pipeline.
    /// Returns None if the message should be filtered out.
    /// Returns Some(processed_data) if the message should be forwarded.
    pub fn process(&self, data: &[u8]) -> Option<Vec<u8>> {
        if data.is_empty() {
            return None;
        }

        let status = data[0];

        // System messages (0xF0-0xFF)
        if status >= 0xF0 {
            return self.process_system_message(data);
        }

        let msg_type = status & 0xF0;
        let channel = (status & 0x0F) as usize;

        // Channel filter
        if !self.channel_filter[channel] {
            return None;
        }

        // Message type filter
        match msg_type {
            0x80 | 0x90 => {
                if !self.message_filter.note_on_off {
                    return None;
                }
            }
            0xA0 | 0xD0 => {
                if !self.message_filter.aftertouch {
                    return None;
                }
            }
            0xB0 => {
                if !self.message_filter.control_change {
                    return None;
                }
            }
            0xC0 => {
                if !self.message_filter.program_change {
                    return None;
                }
            }
            0xE0 => {
                if !self.message_filter.pitch_bend {
                    return None;
                }
            }
            _ => {}
        }

        let mut result = data.to_vec();

        // Channel remap
        let dest_channel = if self.channel_remap[channel] != 0xFF {
            self.channel_remap[channel] & 0x0F
        } else {
            channel as u8
        };
        result[0] = msg_type | dest_channel;

        // Apply note processing (transpose, velocity curve)
        match msg_type {
            0x80 | 0x90 => {
                if result.len() >= 3 {
                    // Transpose
                    let transpose = self.transpose[channel];
                    if transpose != 0 {
                        let note = result[1] as i16 + transpose as i16;
                        if !(0..=127).contains(&note) {
                            return None; // Note out of range after transpose
                        }
                        result[1] = note as u8;
                    }

                    // Velocity curve (only for Note On with velocity > 0)
                    if msg_type == 0x90 && result[2] > 0 {
                        result[2] = apply_velocity_curve(result[2], self.velocity_curve);
                    }
                }
            }
            _ => {}
        }

        Some(result)
    }

    fn process_system_message(&self, data: &[u8]) -> Option<Vec<u8>> {
        match data[0] {
            0xF0 => {
                // SysEx
                if self.sysex_passthrough {
                    Some(data.to_vec())
                } else {
                    None
                }
            }
            0xF8 => {
                // MIDI Clock
                if self.message_filter.clock {
                    Some(data.to_vec())
                } else {
                    None
                }
            }
            0xFA | 0xFB | 0xFC => {
                // Start, Continue, Stop
                if self.message_filter.clock {
                    Some(data.to_vec())
                } else {
                    None
                }
            }
            _ => Some(data.to_vec()),
        }
    }
}

fn apply_velocity_curve(velocity: u8, curve: VelocityCurve) -> u8 {
    let v = velocity as f32 / 127.0;
    let result = match curve {
        VelocityCurve::Linear => v,
        VelocityCurve::Logarithmic => (v.ln() / (1.0f32).ln() + 1.0).max(0.0), // log curve
        VelocityCurve::Exponential => v * v,
        VelocityCurve::SCurve => {
            // Smooth S-curve using smoothstep
            let t = v.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        }
    };
    (result * 127.0).round().clamp(1.0, 127.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_passthrough_default() {
        let pipeline = PipelineConfig::default();

        // Note On should pass through unchanged
        let result = pipeline.process(&[0x90, 60, 100]);
        assert_eq!(result, Some(vec![0x90, 60, 100]));
    }

    #[test]
    fn test_channel_filter() {
        let mut pipeline = PipelineConfig::default();
        pipeline.channel_filter[0] = false; // Disable channel 1

        // Channel 0 should be filtered
        assert!(pipeline.process(&[0x90, 60, 100]).is_none());

        // Channel 1 should pass
        assert!(pipeline.process(&[0x91, 60, 100]).is_some());
    }

    #[test]
    fn test_message_type_filter() {
        let mut pipeline = PipelineConfig::default();
        pipeline.message_filter.control_change = false;

        // CC should be filtered
        assert!(pipeline.process(&[0xB0, 1, 64]).is_none());

        // Note On should pass
        assert!(pipeline.process(&[0x90, 60, 100]).is_some());
    }

    #[test]
    fn test_channel_remap() {
        let mut pipeline = PipelineConfig::default();
        pipeline.channel_remap[0] = 5; // Remap channel 1 -> 6

        let result = pipeline.process(&[0x90, 60, 100]).unwrap();
        assert_eq!(result[0], 0x95); // Channel 5 (6th channel, 0-indexed)
    }

    #[test]
    fn test_transpose() {
        let mut pipeline = PipelineConfig::default();
        pipeline.transpose[0] = 12; // +1 octave

        let result = pipeline.process(&[0x90, 60, 100]).unwrap();
        assert_eq!(result[1], 72); // C4 -> C5
    }

    #[test]
    fn test_transpose_out_of_range() {
        let mut pipeline = PipelineConfig::default();
        pipeline.transpose[0] = 48; // +4 octaves

        // Note 100 + 48 = 148 > 127 -> filtered out
        assert!(pipeline.process(&[0x90, 100, 100]).is_none());
    }

    #[test]
    fn test_velocity_curves() {
        // Linear: input = output
        assert_eq!(apply_velocity_curve(64, VelocityCurve::Linear), 64);
        assert_eq!(apply_velocity_curve(127, VelocityCurve::Linear), 127);

        // Exponential: lower velocities are softer
        let exp = apply_velocity_curve(64, VelocityCurve::Exponential);
        assert!(exp < 64); // Exponential compresses lower values

        // S-Curve: midrange is compressed
        let s = apply_velocity_curve(64, VelocityCurve::SCurve);
        assert!(s > 0 && s <= 127);
    }

    #[test]
    fn test_sysex_filter() {
        let mut pipeline = PipelineConfig::default();
        pipeline.sysex_passthrough = false;

        // SysEx should be filtered
        assert!(pipeline.process(&[0xF0, 0x7E, 0x7F, 0xF7]).is_none());

        // Enable passthrough
        pipeline.sysex_passthrough = true;
        assert!(pipeline.process(&[0xF0, 0x7E, 0x7F, 0xF7]).is_some());
    }
}
