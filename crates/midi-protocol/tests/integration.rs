//! Integration tests for the midi-protocol crate.
//!
//! These tests exercise the public API across module boundaries,
//! verifying that packets, journal, pipeline, identity, and ring buffer
//! work correctly together and in isolation under realistic conditions.

use midi_protocol::identity::DeviceIdentity;
use midi_protocol::journal::{decode_journal, encode_journal};
use midi_protocol::midi_state::MidiState;
use midi_protocol::packets::{
    HeartbeatPacket, HostRole, IdentityPacket, MidiDataPacket,
};
use midi_protocol::pipeline::{PipelineConfig, VelocityCurve};
use midi_protocol::ringbuf::{midi_ring_buffer, SLOT_SIZE};

// ---------------------------------------------------------------------------
// 1. Packet serialization roundtrip -- MidiDataPacket
// ---------------------------------------------------------------------------

#[test]
fn midi_data_packet_roundtrip_no_journal() {
    let packet = MidiDataPacket {
        sequence: 1023,
        timestamp_us: 123_456_789_012,
        host_id: 2,
        midi_data: vec![0x90, 0x3C, 0x7F], // Note On C4 vel 127
        journal: None,
    };

    let mut buf = Vec::new();
    packet.serialize(&mut buf);

    let decoded = MidiDataPacket::deserialize(&buf)
        .expect("deserialization should succeed");

    assert_eq!(decoded.sequence, 1023);
    assert_eq!(decoded.timestamp_us, 123_456_789_012);
    assert_eq!(decoded.host_id, 2);
    assert_eq!(decoded.midi_data, vec![0x90, 0x3C, 0x7F]);
    assert!(decoded.journal.is_none());
}

#[test]
fn midi_data_packet_roundtrip_with_journal() {
    let journal_data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03];
    let packet = MidiDataPacket {
        sequence: 65535,
        timestamp_us: u64::MAX,
        host_id: 0,
        midi_data: vec![0xB0, 0x07, 0x64], // CC7 (Volume) = 100
        journal: Some(journal_data.clone()),
    };

    let mut buf = Vec::new();
    packet.serialize(&mut buf);

    let decoded = MidiDataPacket::deserialize(&buf)
        .expect("deserialization should succeed");

    assert_eq!(decoded.sequence, 65535);
    assert_eq!(decoded.timestamp_us, u64::MAX);
    assert_eq!(decoded.host_id, 0);
    assert_eq!(decoded.midi_data, vec![0xB0, 0x07, 0x64]);
    assert_eq!(decoded.journal, Some(journal_data));
}

#[test]
fn midi_data_packet_with_large_midi_payload() {
    // SysEx message spanning many bytes
    let mut sysex = vec![0xF0];
    sysex.extend(std::iter::repeat(0x42).take(200));
    sysex.push(0xF7);

    let packet = MidiDataPacket {
        sequence: 500,
        timestamp_us: 999,
        host_id: 1,
        midi_data: sysex.clone(),
        journal: None,
    };

    let mut buf = Vec::new();
    packet.serialize(&mut buf);

    let decoded = MidiDataPacket::deserialize(&buf).unwrap();
    assert_eq!(decoded.midi_data, sysex);
}

// ---------------------------------------------------------------------------
// 2. Heartbeat packet roundtrip
// ---------------------------------------------------------------------------

#[test]
fn heartbeat_packet_roundtrip_primary() {
    let packet = HeartbeatPacket {
        host_id: 1,
        role: HostRole::Primary,
        sequence: 12345,
        timestamp_us: 7_777_777,
    };

    let mut buf = [0u8; HeartbeatPacket::SIZE];
    packet.serialize(&mut buf);

    let decoded = HeartbeatPacket::deserialize(&buf)
        .expect("deserialization should succeed");

    assert_eq!(decoded.host_id, 1);
    assert_eq!(decoded.role, HostRole::Primary);
    assert_eq!(decoded.sequence, 12345);
    assert_eq!(decoded.timestamp_us, 7_777_777);
}

#[test]
fn heartbeat_packet_roundtrip_standby() {
    let packet = HeartbeatPacket {
        host_id: 2,
        role: HostRole::Standby,
        sequence: 0,
        timestamp_us: 0,
    };

    let mut buf = [0u8; HeartbeatPacket::SIZE];
    packet.serialize(&mut buf);

    let decoded = HeartbeatPacket::deserialize(&buf)
        .expect("deserialization should succeed");

    assert_eq!(decoded.host_id, 2);
    assert_eq!(decoded.role, HostRole::Standby);
    assert_eq!(decoded.sequence, 0);
    assert_eq!(decoded.timestamp_us, 0);
}

#[test]
fn heartbeat_packet_boundary_values() {
    let packet = HeartbeatPacket {
        host_id: 255,
        role: HostRole::Primary,
        sequence: u16::MAX,
        timestamp_us: u64::MAX,
    };

    let mut buf = [0u8; HeartbeatPacket::SIZE];
    packet.serialize(&mut buf);

    let decoded = HeartbeatPacket::deserialize(&buf).unwrap();
    assert_eq!(decoded.host_id, 255);
    assert_eq!(decoded.sequence, u16::MAX);
    assert_eq!(decoded.timestamp_us, u64::MAX);
}

// ---------------------------------------------------------------------------
// 3. Journal encode/decode
// ---------------------------------------------------------------------------

#[test]
fn journal_roundtrip_notes_and_ccs() {
    let mut state = MidiState::new();

    // Set notes on multiple channels
    state.process_message(&[0x90, 60, 100]); // Ch0 Note On C4 vel 100
    state.process_message(&[0x90, 64, 80]);  // Ch0 Note On E4 vel 80
    state.process_message(&[0x93, 48, 127]); // Ch3 Note On C3 vel 127

    // Set CC values
    state.process_message(&[0xB0, 1, 64]);   // Ch0 CC1 (Modulation) = 64
    state.process_message(&[0xB0, 7, 100]);  // Ch0 CC7 (Volume) = 100
    state.process_message(&[0xB3, 11, 90]);  // Ch3 CC11 (Expression) = 90

    let journal = encode_journal(&state);
    let decoded = decode_journal(&journal)
        .expect("journal decode should succeed");

    // Verify ch0 notes
    assert_eq!(decoded.channels[0].notes[60], 100);
    assert_eq!(decoded.channels[0].notes[64], 80);

    // Verify ch3 note
    assert_eq!(decoded.channels[3].notes[48], 127);

    // Verify ch0 CCs
    assert_eq!(decoded.channels[0].cc[1], 64);
    assert_eq!(decoded.channels[0].cc[7], 100);

    // Verify ch3 CC
    assert_eq!(decoded.channels[3].cc[11], 90);

    // Verify untouched channels remain default
    assert_eq!(decoded.channels[1].notes[60], 0);
    assert_eq!(decoded.channels[2].cc[1], 0);
    assert_eq!(decoded.active_note_count(), 3);
}

#[test]
fn journal_roundtrip_program_and_pitch_bend() {
    let mut state = MidiState::new();

    // Program change on ch0
    state.process_message(&[0xC0, 42]);
    // Pitch bend on ch0 (LSB=0, MSB=96 -> value=12288)
    state.process_message(&[0xE0, 0, 96]);
    // Channel pressure on ch5
    state.process_message(&[0xD5, 110]);

    let journal = encode_journal(&state);
    let decoded = decode_journal(&journal).unwrap();

    assert_eq!(decoded.channels[0].program, 42);
    assert_eq!(decoded.channels[0].pitch_bend, 12288);
    assert_eq!(decoded.channels[5].channel_pressure, 110);

    // Pitch bend on untouched channel should be centered
    assert_eq!(decoded.channels[1].pitch_bend, 8192);
}

#[test]
fn journal_empty_state_produces_minimal_output() {
    let state = MidiState::new();
    let journal = encode_journal(&state);

    // Empty state: only the 2-byte channel mask (all zeros)
    assert_eq!(journal.len(), 2);
    assert_eq!(journal, vec![0, 0]);

    // Decoding empty journal should produce a default state
    let decoded = decode_journal(&journal).unwrap();
    assert_eq!(decoded.active_note_count(), 0);
}

#[test]
fn journal_all_channels_active() {
    let mut state = MidiState::new();

    // Set at least one note on every channel
    for ch in 0..16u8 {
        state.process_message(&[0x90 | ch, 60, 100]);
    }

    let journal = encode_journal(&state);
    let decoded = decode_journal(&journal).unwrap();

    for ch in 0..16 {
        assert_eq!(
            decoded.channels[ch].notes[60], 100,
            "channel {} note 60 should be 100",
            ch
        );
    }
    assert_eq!(decoded.active_note_count(), 16);
}

// ---------------------------------------------------------------------------
// 4. Pipeline channel filter
// ---------------------------------------------------------------------------

#[test]
fn pipeline_channel_filter_drops_disabled_channel() {
    let mut pipeline = PipelineConfig::default();
    // Disable channel 10 (index 9, 0-based -- MIDI "channel 10" is drum channel)
    pipeline.channel_filter[9] = false;

    // Channel 10 (0x99) messages should be dropped
    assert!(
        pipeline.process(&[0x99, 60, 100]).is_none(),
        "Note On ch10 should be filtered"
    );
    assert!(
        pipeline.process(&[0xB9, 1, 64]).is_none(),
        "CC on ch10 should be filtered"
    );
    assert!(
        pipeline.process(&[0xE9, 0, 64]).is_none(),
        "Pitch bend on ch10 should be filtered"
    );

    // Other channels pass through
    for ch in 0..16u8 {
        if ch == 9 {
            continue;
        }
        let msg = vec![0x90 | ch, 60, 100];
        assert!(
            pipeline.process(&msg).is_some(),
            "Channel {} should pass through",
            ch + 1
        );
    }
}

#[test]
fn pipeline_channel_filter_multiple_disabled() {
    let mut pipeline = PipelineConfig::default();
    pipeline.channel_filter[0] = false;  // Disable ch1
    pipeline.channel_filter[9] = false;  // Disable ch10
    pipeline.channel_filter[15] = false; // Disable ch16

    assert!(pipeline.process(&[0x90, 60, 100]).is_none()); // ch1
    assert!(pipeline.process(&[0x99, 60, 100]).is_none()); // ch10
    assert!(pipeline.process(&[0x9F, 60, 100]).is_none()); // ch16
    assert!(pipeline.process(&[0x91, 60, 100]).is_some()); // ch2 passes
}

// ---------------------------------------------------------------------------
// 5. Pipeline velocity curve
// ---------------------------------------------------------------------------

#[test]
fn pipeline_velocity_curve_exponential() {
    let mut pipeline = PipelineConfig::default();
    pipeline.velocity_curve = VelocityCurve::Exponential;

    // Exponential: v_out = (v_in/127)^2 * 127
    // For mid velocity 64: (64/127)^2 * 127 ~ 32 (lower than linear)
    let result = pipeline.process(&[0x90, 60, 64]).unwrap();
    let velocity_out = result[2];

    assert!(
        velocity_out < 64,
        "Exponential curve should compress mid-range velocity: got {}",
        velocity_out
    );

    // Max velocity should stay at max
    let result_max = pipeline.process(&[0x90, 60, 127]).unwrap();
    assert_eq!(result_max[2], 127);

    // Min velocity (1) should be attenuated but stay at least 1
    let result_min = pipeline.process(&[0x90, 60, 1]).unwrap();
    assert!(result_min[2] >= 1);
}

#[test]
fn pipeline_velocity_curve_linear_passthrough() {
    let pipeline = PipelineConfig::default();
    // Default is linear -- velocity should pass through unchanged
    assert_eq!(pipeline.velocity_curve, VelocityCurve::Linear);

    for vel in 1..=127u8 {
        let result = pipeline.process(&[0x90, 60, vel]).unwrap();
        assert_eq!(
            result[2], vel,
            "Linear curve should not modify velocity {}",
            vel
        );
    }
}

#[test]
fn pipeline_velocity_curve_only_applies_to_note_on() {
    let mut pipeline = PipelineConfig::default();
    pipeline.velocity_curve = VelocityCurve::Exponential;

    // Note Off (0x80) should not have velocity curve applied
    let result = pipeline.process(&[0x80, 60, 64]).unwrap();
    assert_eq!(result[2], 64, "Velocity curve should not apply to Note Off");

    // CC should not be affected
    let cc_result = pipeline.process(&[0xB0, 7, 64]).unwrap();
    assert_eq!(cc_result[2], 64, "Velocity curve should not apply to CC");
}

#[test]
fn pipeline_velocity_curve_note_on_velocity_zero_not_transformed() {
    let mut pipeline = PipelineConfig::default();
    pipeline.velocity_curve = VelocityCurve::Exponential;

    // Note On with velocity 0 means Note Off -- velocity should not be transformed
    let result = pipeline.process(&[0x90, 60, 0]).unwrap();
    assert_eq!(result[2], 0, "Velocity 0 (Note Off) should not be transformed");
}

#[test]
fn pipeline_velocity_curve_scurve() {
    let mut pipeline = PipelineConfig::default();
    pipeline.velocity_curve = VelocityCurve::SCurve;

    // S-curve: smoothstep(v) = v^2 * (3 - 2v)
    // At midpoint (0.5): 0.25 * 2.0 = 0.5 -- midpoint should be approximately unchanged
    let result = pipeline.process(&[0x90, 60, 64]).unwrap();
    let vel_out = result[2];
    // S-curve at midpoint is close to 0.5, so output should be close to 64
    assert!(
        (vel_out as i16 - 64).unsigned_abs() <= 5,
        "S-curve midpoint should be near 64, got {}",
        vel_out
    );

    // Low values should be compressed (less than linear)
    let result_low = pipeline.process(&[0x90, 60, 20]).unwrap();
    assert!(
        result_low[2] <= 20,
        "S-curve should compress low velocities: got {}",
        result_low[2]
    );
}

// ---------------------------------------------------------------------------
// 6. Pipeline transpose
// ---------------------------------------------------------------------------

#[test]
fn pipeline_transpose_up_one_octave() {
    let mut pipeline = PipelineConfig::default();
    pipeline.transpose[0] = 12; // +1 octave on channel 0

    let result = pipeline.process(&[0x90, 60, 100]).unwrap();
    assert_eq!(result[1], 72, "C4 (60) + 12 semitones = C5 (72)");
}

#[test]
fn pipeline_transpose_down_one_octave() {
    let mut pipeline = PipelineConfig::default();
    pipeline.transpose[0] = -12; // -1 octave on channel 0

    let result = pipeline.process(&[0x90, 72, 100]).unwrap();
    assert_eq!(result[1], 60, "C5 (72) - 12 semitones = C4 (60)");
}

#[test]
fn pipeline_transpose_out_of_range_drops_message() {
    let mut pipeline = PipelineConfig::default();
    pipeline.transpose[0] = 48; // +4 octaves

    // Note 100 + 48 = 148 > 127 -- should be dropped
    assert!(
        pipeline.process(&[0x90, 100, 100]).is_none(),
        "Transposed note >127 should be dropped"
    );

    // Also for negative: note 10 - 24 = -14 < 0
    pipeline.transpose[0] = -24;
    assert!(
        pipeline.process(&[0x90, 10, 100]).is_none(),
        "Transposed note <0 should be dropped"
    );
}

#[test]
fn pipeline_transpose_per_channel() {
    let mut pipeline = PipelineConfig::default();
    pipeline.transpose[0] = 12;  // Ch1: +1 octave
    pipeline.transpose[1] = -12; // Ch2: -1 octave
    pipeline.transpose[2] = 0;   // Ch3: no change

    let r0 = pipeline.process(&[0x90, 60, 100]).unwrap();
    let r1 = pipeline.process(&[0x91, 60, 100]).unwrap();
    let r2 = pipeline.process(&[0x92, 60, 100]).unwrap();

    assert_eq!(r0[1], 72);
    assert_eq!(r1[1], 48);
    assert_eq!(r2[1], 60);
}

#[test]
fn pipeline_transpose_applies_to_note_off() {
    let mut pipeline = PipelineConfig::default();
    pipeline.transpose[0] = 7; // +7 semitones

    let result = pipeline.process(&[0x80, 60, 64]).unwrap();
    assert_eq!(result[1], 67, "Transpose should apply to Note Off too");
}

// ---------------------------------------------------------------------------
// 7. Pipeline channel remap
// ---------------------------------------------------------------------------

#[test]
fn pipeline_channel_remap_basic() {
    let mut pipeline = PipelineConfig::default();
    pipeline.channel_remap[0] = 5; // Map ch0 -> ch5

    let result = pipeline.process(&[0x90, 60, 100]).unwrap();
    assert_eq!(result[0] & 0x0F, 5, "Channel should be remapped to 5");
    assert_eq!(result[0] & 0xF0, 0x90, "Message type should be preserved");
}

#[test]
fn pipeline_channel_remap_preserves_message_types() {
    let mut pipeline = PipelineConfig::default();
    pipeline.channel_remap[0] = 3; // Map ch0 -> ch3

    // Note On
    let note_on = pipeline.process(&[0x90, 60, 100]).unwrap();
    assert_eq!(note_on[0], 0x93);

    // Note Off
    let note_off = pipeline.process(&[0x80, 60, 0]).unwrap();
    assert_eq!(note_off[0], 0x83);

    // CC
    let cc = pipeline.process(&[0xB0, 1, 64]).unwrap();
    assert_eq!(cc[0], 0xB3);

    // Pitch Bend
    let pb = pipeline.process(&[0xE0, 0, 64]).unwrap();
    assert_eq!(pb[0], 0xE3);

    // Program Change
    let pc = pipeline.process(&[0xC0, 42]).unwrap();
    assert_eq!(pc[0], 0xC3);
}

#[test]
fn pipeline_channel_remap_no_remap_when_0xff() {
    let pipeline = PipelineConfig::default();
    // Default remap is all 0xFF = no remap

    let result = pipeline.process(&[0x94, 60, 100]).unwrap();
    assert_eq!(
        result[0] & 0x0F,
        4,
        "Channel should be unchanged when remap is 0xFF"
    );
}

#[test]
fn pipeline_channel_remap_multiple_channels() {
    let mut pipeline = PipelineConfig::default();
    pipeline.channel_remap[0] = 15;  // ch0 -> ch15
    pipeline.channel_remap[15] = 0;  // ch15 -> ch0 (swap)

    let r0 = pipeline.process(&[0x90, 60, 100]).unwrap();
    let r15 = pipeline.process(&[0x9F, 60, 100]).unwrap();

    assert_eq!(r0[0], 0x9F, "Ch0 should map to ch15");
    assert_eq!(r15[0], 0x90, "Ch15 should map to ch0");
}

// ---------------------------------------------------------------------------
// 8. Identity serialization (serde/bincode roundtrip)
// ---------------------------------------------------------------------------

#[test]
fn identity_bincode_roundtrip() {
    let identity = DeviceIdentity {
        name: "Akai APC40 mkII".to_string(),
        manufacturer: "Akai Professional".to_string(),
        vendor_id: 0x09E8,
        product_id: 0x0029,
        sysex_identity: [
            0x47, 0x73, 0x00, 0x19, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00,
        ],
        port_count_in: 1,
        port_count_out: 2,
    };

    let encoded = bincode::serialize(&identity)
        .expect("bincode serialization should succeed");
    let decoded: DeviceIdentity = bincode::deserialize(&encoded)
        .expect("bincode deserialization should succeed");

    assert_eq!(decoded.name, "Akai APC40 mkII");
    assert_eq!(decoded.manufacturer, "Akai Professional");
    assert_eq!(decoded.vendor_id, 0x09E8);
    assert_eq!(decoded.product_id, 0x0029);
    assert_eq!(decoded.sysex_identity, identity.sysex_identity);
    assert_eq!(decoded.port_count_in, 1);
    assert_eq!(decoded.port_count_out, 2);
    assert!(decoded.is_valid());
}

#[test]
fn identity_default_not_valid() {
    let identity = DeviceIdentity::default();
    assert!(!identity.is_valid());

    // Roundtrip the default too
    let encoded = bincode::serialize(&identity).unwrap();
    let decoded: DeviceIdentity = bincode::deserialize(&encoded).unwrap();
    assert_eq!(decoded.name, "Unknown MIDI Device");
    assert!(!decoded.is_valid());
}

#[test]
fn identity_sysex_reply_format() {
    let identity = DeviceIdentity {
        name: "TestDevice".to_string(),
        manufacturer: "TestMfr".to_string(),
        vendor_id: 0x1234,
        product_id: 0x5678,
        sysex_identity: [
            0x47, 0x73, 0x00, 0x19, 0x00,
            0x01, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00,
        ],
        port_count_in: 1,
        port_count_out: 1,
    };

    let reply = identity.sysex_identity_reply();
    assert_eq!(reply[0], 0xF0, "SysEx start");
    assert_eq!(reply[1], 0x7E, "Universal Non-Realtime");
    assert_eq!(reply[2], 0x7F, "Device ID");
    assert_eq!(reply[3], 0x06, "General Information");
    assert_eq!(reply[4], 0x02, "Identity Reply sub-ID");
    assert_eq!(*reply.last().unwrap(), 0xF7, "SysEx end");
}

// ---------------------------------------------------------------------------
// 9. Ring buffer concurrent test (tokio tasks)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ring_buffer_concurrent_1000_messages() {
    let (producer, consumer) = midi_ring_buffer(1024); // Power of two >= 1000
    let message_count: usize = 1000;

    // Spawn producer task
    let producer_handle = tokio::spawn(async move {
        for i in 0..message_count {
            // Encode the sequence number as a 3-byte MIDI-like message
            // Use push_overwrite to never block
            let seq = i as u16;
            let msg = [
                0x90,                          // Note On status
                (seq & 0x7F) as u8,            // Low 7 bits as note
                ((seq >> 7) & 0x7F) as u8,     // High 7 bits as velocity
            ];
            producer.push_overwrite(&msg);
            // Small yield to simulate realistic producer pace
            if i % 100 == 0 {
                tokio::task::yield_now().await;
            }
        }
    });

    // Spawn consumer task
    let consumer_handle = tokio::spawn(async move {
        let mut received = Vec::with_capacity(message_count);
        let mut buf = [0u8; SLOT_SIZE];

        while received.len() < message_count {
            let len = consumer.pop(&mut buf).await;
            assert_eq!(len, 3, "Each message should be 3 bytes");
            let seq = (buf[1] as u16) | ((buf[2] as u16) << 7);
            received.push(seq);
        }

        received
    });

    producer_handle.await.expect("producer task should not panic");
    let received = consumer_handle.await.expect("consumer task should not panic");

    // Verify all messages received in order
    assert_eq!(received.len(), message_count);
    for i in 0..message_count {
        assert_eq!(
            received[i], i as u16,
            "Message {} should have sequence {}, got {}",
            i, i, received[i]
        );
    }
}

#[tokio::test]
async fn ring_buffer_async_pop_wakes_on_push() {
    let (producer, consumer) = midi_ring_buffer(16);

    // Spawn consumer that waits for a message
    let consumer_handle = tokio::spawn(async move {
        let mut buf = [0u8; SLOT_SIZE];
        let len = consumer.pop(&mut buf).await;
        assert_eq!(len, 3);
        assert_eq!(&buf[..3], &[0x90, 60, 100]);
    });

    // Small delay to ensure consumer is waiting
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // Push a message -- this should wake the consumer
    assert!(producer.push(&[0x90, 60, 100]));

    consumer_handle
        .await
        .expect("consumer should receive the message and complete");
}

#[tokio::test]
async fn ring_buffer_drain_collects_all() {
    let (producer, consumer) = midi_ring_buffer(64);

    // Push several messages
    for i in 0u8..10 {
        producer.push(&[0x90, i, 127]);
    }

    let mut collected = Vec::new();
    consumer.drain(|data| {
        collected.push(data.to_vec());
    });

    assert_eq!(collected.len(), 10);
    for (i, msg) in collected.iter().enumerate() {
        assert_eq!(msg, &vec![0x90, i as u8, 127]);
    }

    // Buffer should be empty now
    assert_eq!(consumer.available(), 0);
}

// ---------------------------------------------------------------------------
// Cross-module integration: journal embedded in MidiDataPacket
// ---------------------------------------------------------------------------

#[test]
fn journal_inside_midi_data_packet_roundtrip() {
    // Build a realistic MIDI state
    let mut state = MidiState::new();
    state.process_message(&[0x90, 60, 100]); // Note On C4
    state.process_message(&[0xB0, 7, 100]);  // Volume CC
    state.process_message(&[0xC0, 10]);       // Program change
    state.process_message(&[0xE0, 0, 80]);    // Pitch bend

    // Encode journal
    let journal_bytes = encode_journal(&state);

    // Package into a MidiDataPacket
    let packet = MidiDataPacket {
        sequence: 42,
        timestamp_us: 1_000_000,
        host_id: 1,
        midi_data: vec![0x90, 65, 90], // A new Note On in this packet
        journal: Some(journal_bytes.clone()),
    };

    // Serialize the full packet
    let mut wire = Vec::new();
    packet.serialize(&mut wire);

    // Deserialize
    let decoded_packet = MidiDataPacket::deserialize(&wire).unwrap();

    // Verify MIDI data
    assert_eq!(decoded_packet.midi_data, vec![0x90, 65, 90]);

    // Decode the journal from the deserialized packet
    let journal_data = decoded_packet.journal.unwrap();
    assert_eq!(journal_data, journal_bytes);

    let decoded_state = decode_journal(&journal_data).unwrap();
    assert_eq!(decoded_state.channels[0].notes[60], 100);
    assert_eq!(decoded_state.channels[0].cc[7], 100);
    assert_eq!(decoded_state.channels[0].program, 10);
    assert_eq!(decoded_state.channels[0].pitch_bend, 80 << 7); // 10240
}

// ---------------------------------------------------------------------------
// Pipeline combined transforms
// ---------------------------------------------------------------------------

#[test]
fn pipeline_combined_remap_transpose_velocity() {
    let mut pipeline = PipelineConfig::default();
    pipeline.channel_remap[0] = 3;           // ch0 -> ch3
    pipeline.transpose[0] = 12;              // +1 octave (applies using SOURCE channel)
    pipeline.velocity_curve = VelocityCurve::Exponential;

    let result = pipeline.process(&[0x90, 60, 64]).unwrap();

    // Channel remapped to 3
    assert_eq!(result[0], 0x93);
    // Note transposed from 60 to 72
    assert_eq!(result[1], 72);
    // Velocity compressed by exponential curve (64 -> ~32)
    assert!(result[2] < 64);
    assert!(result[2] > 0);
}

#[test]
fn pipeline_default_config_is_passthrough() {
    let pipeline = PipelineConfig::default();

    // All message types should pass through unchanged
    let messages: Vec<Vec<u8>> = vec![
        vec![0x90, 60, 100],    // Note On
        vec![0x80, 60, 0],      // Note Off
        vec![0xB0, 1, 64],      // CC
        vec![0xC0, 42],         // Program Change
        vec![0xE0, 0, 64],      // Pitch Bend
        vec![0xD0, 80],         // Channel Pressure
        vec![0xA0, 60, 80],     // Polyphonic Aftertouch
        vec![0xF0, 0x7E, 0xF7], // SysEx
        vec![0xF8],             // MIDI Clock
    ];

    for msg in &messages {
        let result = pipeline.process(msg);
        assert_eq!(
            result.as_ref(),
            Some(msg),
            "Default pipeline should pass through {:02X?}",
            msg
        );
    }
}

// ---------------------------------------------------------------------------
// IdentityPacket (custom binary format) roundtrip
// ---------------------------------------------------------------------------

#[test]
fn identity_packet_roundtrip() {
    let packet = IdentityPacket {
        host_id: 1,
        device_name: "Novation Launchpad Pro".to_string(),
        manufacturer: "Novation".to_string(),
        vendor_id: 0x1235,
        product_id: 0x0051,
        sysex_identity: [
            0x00, 0x20, 0x29, // Novation manufacturer ID
            0x51, 0x00,       // Family
            0x00, 0x00,       // Model
            0x00, 0x00, 0x00, 0x00, // Version
            0x00, 0x00, 0x00, 0x00,
        ],
        port_count_in: 2,
        port_count_out: 2,
    };

    let mut buf = Vec::new();
    packet.serialize(&mut buf);

    let decoded = IdentityPacket::deserialize(&buf)
        .expect("IdentityPacket deserialization should succeed");

    assert_eq!(decoded.host_id, 1);
    assert_eq!(decoded.device_name, "Novation Launchpad Pro");
    assert_eq!(decoded.manufacturer, "Novation");
    assert_eq!(decoded.vendor_id, 0x1235);
    assert_eq!(decoded.product_id, 0x0051);
    assert_eq!(decoded.sysex_identity, packet.sysex_identity);
    assert_eq!(decoded.port_count_in, 2);
    assert_eq!(decoded.port_count_out, 2);
}
