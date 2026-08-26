//! Decoded MIDI input.
//!
//! These types are deliberately narrow: only the messages mooloop acts on,
//! already parsed out of their status bytes, carrying a sample offset into
//! the block they arrived in. Decoding happens once at the JACK boundary so
//! nothing downstream has to reason about running status or byte layout.

/// A relative-CC encoding. Controllers disagree about how a jog wheel or
/// endless encoder reports a turn, and the three conventions below are
/// mutually unintelligible — the same byte means opposite directions — so
/// this is configuration, not something to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelativeEncoding {
    /// 64 is no movement; the delta is `value - 64`, so `1..=127` spans
    /// -63..=+63. What Mackie/HUI-style jog wheels send.
    #[default]
    BinaryOffset,
    /// `1..=63` is +1..=+63, `65..=127` is -63..=-1, and 0/64 are no
    /// movement.
    TwosComplement,
    /// Bit 6 is the sign, bits 0-5 the magnitude: `0x01..=0x3F` positive,
    /// `0x41..=0x7F` negative.
    SignedBit,
}

impl RelativeEncoding {
    /// Movement reported by one relative-CC message, in encoder ticks.
    pub fn delta(self, value: u8) -> i8 {
        let value = value & 0x7F;
        match self {
            Self::BinaryOffset => value as i8 - 64,
            Self::TwosComplement | Self::SignedBit if value == 0 || value == 64 => 0,
            Self::TwosComplement => {
                if value < 64 {
                    value as i8
                } else {
                    -((128 - i16::from(value)) as i8)
                }
            }
            Self::SignedBit => {
                let magnitude = (value & 0x3F) as i8;
                if value & 0x40 == 0 {
                    magnitude
                } else {
                    -magnitude
                }
            }
        }
    }
}

/// One decoded MIDI message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiKind {
    /// A note-on with velocity 0 is normalized to `NoteOff` at decode time,
    /// since a great many controllers send it that way and nothing
    /// downstream should have to know that.
    NoteOn {
        note: u8,
        velocity: u8,
    },
    NoteOff {
        note: u8,
    },
    ControlChange {
        controller: u8,
        value: u8,
    },
    PitchBend {
        value: i16,
    },
}

/// A decoded message and where it landed inside the current block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiMessage {
    /// Frames into the block, `0..frames`.
    pub offset: u32,
    /// MIDI channel, `0..16`.
    pub channel: u8,
    pub kind: MidiKind,
}

impl MidiMessage {
    /// Decode one raw MIDI packet. Returns `None` for anything mooloop does
    /// not act on (system messages, aftertouch, program change) and for
    /// truncated packets, so the caller can simply skip them.
    pub fn decode(offset: u32, bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        // Running status is not reconstructed: JACK delivers whole messages,
        // so a packet without a status byte is malformed rather than
        // continued.
        if status < 0x80 {
            return None;
        }
        let channel = status & 0x0F;
        let kind = match status & 0xF0 {
            0x80 => MidiKind::NoteOff {
                note: *bytes.get(1)? & 0x7F,
            },
            0x90 => {
                let note = *bytes.get(1)? & 0x7F;
                let velocity = *bytes.get(2)? & 0x7F;
                if velocity == 0 {
                    MidiKind::NoteOff { note }
                } else {
                    MidiKind::NoteOn { note, velocity }
                }
            }
            0xB0 => MidiKind::ControlChange {
                controller: *bytes.get(1)? & 0x7F,
                value: *bytes.get(2)? & 0x7F,
            },
            0xE0 => {
                let low = i16::from(*bytes.get(1)? & 0x7F);
                let high = i16::from(*bytes.get(2)? & 0x7F);
                MidiKind::PitchBend {
                    value: ((high << 7) | low) - 8192,
                }
            }
            _ => return None,
        };
        Some(Self {
            offset,
            channel,
            kind,
        })
    }
}

/// Split a 7-bit CC into `buckets` evenly-sized steps, `0..buckets`. This is
/// how a continuous controller addresses a small set of choices — four bars
/// selected by 31/63/95/127, say — without the top of the range falling off
/// the end.
pub fn cc_bucket(value: u8, buckets: u8) -> u8 {
    if buckets <= 1 {
        return 0;
    }
    let value = u16::from(value & 0x7F);
    let buckets = u16::from(buckets);
    ((value * buckets) / 128).min(buckets - 1) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_on_with_zero_velocity_decodes_as_note_off() {
        let on = MidiMessage::decode(7, &[0x90, 60, 100]).unwrap();
        assert_eq!(
            on.kind,
            MidiKind::NoteOn {
                note: 60,
                velocity: 100
            }
        );
        assert_eq!(on.offset, 7);
        let off = MidiMessage::decode(0, &[0x90, 60, 0]).unwrap();
        assert_eq!(off.kind, MidiKind::NoteOff { note: 60 });
    }

    #[test]
    fn channel_and_unhandled_status_are_decoded_or_skipped() {
        let cc = MidiMessage::decode(0, &[0xB3, 21, 64]).unwrap();
        assert_eq!(cc.channel, 3);
        assert_eq!(
            cc.kind,
            MidiKind::ControlChange {
                controller: 21,
                value: 64
            }
        );
        // Program change, aftertouch, clock, and truncated packets.
        assert!(MidiMessage::decode(0, &[0xC0, 1]).is_none());
        assert!(MidiMessage::decode(0, &[0xF8]).is_none());
        assert!(MidiMessage::decode(0, &[0x90, 60]).is_none());
        assert!(MidiMessage::decode(0, &[]).is_none());
        assert!(MidiMessage::decode(0, &[60, 100]).is_none());
    }

    #[test]
    fn pitch_bend_centres_on_zero() {
        assert_eq!(
            MidiMessage::decode(0, &[0xE0, 0, 64]).unwrap().kind,
            MidiKind::PitchBend { value: 0 }
        );
        assert_eq!(
            MidiMessage::decode(0, &[0xE0, 0, 0]).unwrap().kind,
            MidiKind::PitchBend { value: -8192 }
        );
    }

    /// The three conventions read the same byte as opposite directions,
    /// which is exactly why the encoding has to be configured rather than
    /// sniffed.
    #[test]
    fn relative_encodings_disagree_about_the_same_byte() {
        assert_eq!(RelativeEncoding::BinaryOffset.delta(65), 1);
        assert_eq!(RelativeEncoding::BinaryOffset.delta(63), -1);
        assert_eq!(RelativeEncoding::BinaryOffset.delta(64), 0);
        assert_eq!(RelativeEncoding::BinaryOffset.delta(127), 63);
        assert_eq!(RelativeEncoding::BinaryOffset.delta(1), -63);

        assert_eq!(RelativeEncoding::TwosComplement.delta(1), 1);
        assert_eq!(RelativeEncoding::TwosComplement.delta(127), -1);
        assert_eq!(RelativeEncoding::TwosComplement.delta(63), 63);
        assert_eq!(RelativeEncoding::TwosComplement.delta(65), -63);
        assert_eq!(RelativeEncoding::TwosComplement.delta(64), 0);

        assert_eq!(RelativeEncoding::SignedBit.delta(1), 1);
        assert_eq!(RelativeEncoding::SignedBit.delta(0x41), -1);
        assert_eq!(RelativeEncoding::SignedBit.delta(0x3F), 63);
        assert_eq!(RelativeEncoding::SignedBit.delta(0x7F), -63);
    }

    #[test]
    fn cc_buckets_split_the_range_evenly_and_reach_the_top() {
        // Four bars addressed by 31/63/95/127, the quarters of the range.
        for (value, expected) in [(0, 0), (31, 0), (32, 1), (63, 1), (95, 2), (127, 3)] {
            assert_eq!(cc_bucket(value, 4), expected, "value {value}");
        }
        assert_eq!(cc_bucket(127, 1), 0);
        assert_eq!(cc_bucket(127, 3), 2);
    }
}

/// Where a mapped control change lands in the event tuple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BufferCcTarget {
    /// Window length, bucketed into `bars` whole bars.
    WindowBars { bars: u8 },
    /// Jump distance, bucketed into whole beats back.
    OffsetBeats { beats: u8 },
    /// Repeat count, bucketed.
    Repeat { max: u8 },
    /// Relative scrub. Not a tuple field: it drives the head directly, the
    /// way a platter does, rather than re-firing an event per message.
    Scrub { encoding: RelativeEncoding },
}

/// One CC assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BufferCcMapping {
    pub controller: u8,
    pub target: BufferCcTarget,
}

/// One note assignment: the note says *what* edit, and note-on/off says how
/// long, so every mapped note fires with a `Gate` duration.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BufferNoteMapping {
    pub note: u8,
    pub event: crate::BufferEvent,
}

/// Fixed ceilings. A performance mapping is a small set of gestures under
/// the hands, not an arbitrary table, and bounding it keeps the whole map
/// `Copy` and safe to hand to the realtime thread.
pub const MAX_BUFFER_NOTE_MAPPINGS: usize = 16;
pub const MAX_BUFFER_CC_MAPPINGS: usize = 8;

/// How MIDI input drives one buffer insert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BufferMidiMap {
    pub enabled: bool,
    /// `None` listens on every channel.
    pub channel: Option<u8>,
    pub target: crate::EffectTarget,
    pub slot: u8,
    pub notes: [Option<BufferNoteMapping>; MAX_BUFFER_NOTE_MAPPINGS],
    pub controls: [Option<BufferCcMapping>; MAX_BUFFER_CC_MAPPINGS],
    /// Velocity's influence on the crossfade, in ms at velocity 1. Velocity
    /// 127 always lands on zero, so a hard hit is a hard edit and a soft one
    /// is declicked.
    pub velocity_crossfade_ms: f32,
}

impl BufferMidiMap {
    pub fn new(target: crate::EffectTarget, slot: u8) -> Self {
        Self {
            enabled: true,
            channel: None,
            target,
            slot,
            notes: [None; MAX_BUFFER_NOTE_MAPPINGS],
            controls: [None; MAX_BUFFER_CC_MAPPINGS],
            velocity_crossfade_ms: 6.0,
        }
    }

    pub fn accepts(&self, message: &MidiMessage) -> bool {
        self.enabled
            && self
                .channel
                .is_none_or(|channel| channel == message.channel)
    }

    /// The event a note-on fires, with velocity applied to the crossfade.
    pub fn note_event(&self, note: u8, velocity: u8) -> Option<crate::BufferEvent> {
        let mapping = self
            .notes
            .iter()
            .flatten()
            .find(|mapping| mapping.note == note)?;
        let mut event = mapping.event;
        event.duration = crate::BufferDuration::Gate;
        let softness = 1.0 - f32::from(velocity.min(127)) / 127.0;
        event.crossfade_ms = self.velocity_crossfade_ms * softness;
        Some(event)
    }

    pub fn cc_target(&self, controller: u8) -> Option<BufferCcTarget> {
        self.controls
            .iter()
            .flatten()
            .find(|mapping| mapping.controller == controller)
            .map(|mapping| mapping.target)
    }
}
