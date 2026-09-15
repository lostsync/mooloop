//! Decoded MIDI input.
//!
//! These types are deliberately narrow: only the messages mooloop acts on,
//! already parsed out of their status bytes, carrying a sample offset into
//! the block they arrived in and the port they arrived on. Decoding happens
//! once at the driver boundary so nothing downstream has to reason about
//! running status or byte layout.
//!
//! **Clock (`0xF8`) is still dropped.** It is the one message that arrives in
//! bulk -- twenty-four per beat, forever, from any device that sends it -- and
//! nothing syncs to it yet, so decoding it would only spend the block's MIDI
//! scratch. Start, Continue, Stop and Song Position are decoded, because they
//! are transport gestures rather than a stream, and the transport acts on
//! them (`docs/CONTROL_SURFACES.md`). Clock lands the day there is a clock to
//! drive.

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
    /// `0xFA`. Start playing from the beginning.
    Start,
    /// `0xFB`. Resume from where the transport stands.
    Continue,
    /// `0xFC`. Stop, holding position -- which is why this is `Continue`'s
    /// partner rather than `Start`'s: MIDI's stop is a pause.
    Stop,
    /// `0xF2`, carrying a position in MIDI beats, which are sixteenth notes
    /// rather than quarters. The conversion lives in
    /// [`Self::song_position_ticks`] so nothing downstream has to remember
    /// that.
    SongPosition { beats: u16 },
}

/// The channel a system message carries, which is no channel at all.
///
/// System messages have no channel nibble; their status byte spends it on the
/// message type. A channel filter must therefore never match one, and giving
/// them a value no filter accepts is how that is enforced rather than
/// remembered -- `0` would have been a lie that an Omni filter and a
/// channel-1 filter would both have believed.
pub const SYSTEM_CHANNEL: u8 = 0xFF;

impl MidiKind {
    /// Whether this is a transport gesture rather than something addressed to
    /// a channel. Transport messages are routed by the transport's own
    /// external-sync setting, never by a channel's input filter.
    pub const fn is_transport(self) -> bool {
        matches!(
            self,
            Self::Start | Self::Continue | Self::Stop | Self::SongPosition { .. }
        )
    }
}

/// One MIDI input port, as the driver numbers them **for this run**.
///
/// Deliberately not persisted. A port's index moves the moment a device is
/// unplugged, so a project names its port by the string the driver reports
/// and [`MidiInputSource::resolve`] turns that name into one of these against
/// the ports that actually exist. What crosses to the audio thread is then a
/// small integer comparison rather than a string one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MidiPortId(pub u16);

impl MidiPortId {
    /// The port a driver that has only one uses. JACK's single auto-connected
    /// `midi_in` is this; see [`MidiPortInfo`].
    pub const FIRST: Self = Self(0);
}

/// One port as the driver reports it, for the input picker and for
/// resolution.
///
/// `name` is the identity: it is what a project stores and what the picker
/// shows. A driver that merges every hardware source into one port -- which
/// is what JACK's `midi_in` does today -- reports exactly one of these, and
/// the picker is then honest about there being one thing to pick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiPortInfo {
    pub id: MidiPortId,
    pub name: String,
}

/// Which MIDI channel a channel listens on.
///
/// The picker's index and this enum are the same fact, so the conversion is
/// written once here rather than in the markup that draws the menu --
/// `AGENTS.md`'s duplication note is about exactly this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiChannelFilter {
    /// Every channel, which is what a keyboard with no channel setting wants.
    #[default]
    Omni,
    /// One channel, stored `0..16` and shown `1..=16`.
    One(u8),
}

/// Menu rows for a channel filter: Omni, then the sixteen channels.
pub const MIDI_CHANNEL_FILTER_ROWS: usize = 17;

impl MidiChannelFilter {
    pub const fn accepts(self, channel: u8) -> bool {
        match self {
            // `SYSTEM_CHANNEL` is not a channel, so Omni does not mean it.
            Self::Omni => channel < 16,
            Self::One(one) => one == channel,
        }
    }

    /// The row this filter occupies in the picker: 0 is Omni, 1..=16 are the
    /// channels.
    pub const fn row(self) -> usize {
        match self {
            Self::Omni => 0,
            Self::One(one) => one as usize + 1,
        }
    }

    /// The filter a picker row names. Out-of-range rows read as Omni rather
    /// than failing: a menu index is not a value worth refusing a project
    /// over.
    pub const fn from_row(row: usize) -> Self {
        match row {
            0 => Self::Omni,
            row if row <= 16 => Self::One(row as u8 - 1),
            _ => Self::Omni,
        }
    }

    pub fn label(self) -> String {
        match self {
            Self::Omni => "Omni".to_owned(),
            Self::One(one) => (one + 1).to_string(),
        }
    }
}

/// Where a channel takes MIDI from, as the project stores it.
///
/// [`Self::FollowSelection`] is the default and is what mooloop did before
/// this field existed: a keyboard plays whichever channel is selected. That
/// makes the field a pure addition -- a project written before it loads with
/// every channel behaving exactly as it used to.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiInputSource {
    /// Sounds when it is the selected channel, from any port.
    #[default]
    FollowSelection,
    /// Never sounds from MIDI, selected or not.
    Off,
    /// Every input port, whether or not the channel is selected. This is what
    /// makes a multitimbral setup work: several channels listening at once,
    /// told apart by [`MidiChannelFilter`].
    AllPorts,
    /// One port, by the name the driver reported for it.
    Port(String),
}

/// This run's id for the port called `name`, if it is plugged in.
///
/// The one place a stored port name meets a live port table. Both a channel's
/// input ([`MidiInputSource`]) and a control binding's port filter
/// ([`MidiPortFilter`]) go through here, so neither can develop its own
/// opinion about what matching a port means.
pub fn resolve_port_name(name: &str, ports: &[MidiPortInfo]) -> Option<MidiPortId> {
    ports.iter().find(|port| port.name == name).map(|port| port.id)
}

impl MidiInputSource {
    /// Turn a stored port name into this run's port id.
    ///
    /// A name no current port carries resolves to [`MidiRouteSource::Off`]:
    /// the *project* keeps the name, so plugging the device back in restores
    /// the routing, but for this run the channel is silent rather than
    /// listening to whatever happens to hold that index now.
    pub fn resolve(&self, ports: &[MidiPortInfo]) -> MidiRouteSource {
        match self {
            Self::FollowSelection => MidiRouteSource::FollowSelection,
            Self::Off => MidiRouteSource::Off,
            Self::AllPorts => MidiRouteSource::AllPorts,
            Self::Port(name) => resolve_port_name(name, ports)
                .map_or(MidiRouteSource::Off, MidiRouteSource::Port),
        }
    }

    /// The port this names, for a picker that lists ports separately from the
    /// two modes that are not ports.
    pub fn port_name(&self) -> Option<&str> {
        match self {
            Self::Port(name) => Some(name),
            _ => None,
        }
    }

    /// The rows an input picker shows: the two modes that are not ports, then
    /// every port.
    ///
    /// Written here rather than in the markup that draws the menu, with
    /// [`Self::from_row`] and [`Self::row`] beside it, because a menu's index
    /// and the value it stands for are one fact. Spelling the rows in `.slint`
    /// and reading them back in Rust is the shape `AGENTS.md` opens on.
    pub fn picker_rows(ports: &[MidiPortInfo]) -> Vec<String> {
        let mut rows = vec![
            "Follow Selection".to_owned(),
            "Off".to_owned(),
            "All Inputs".to_owned(),
        ];
        rows.extend(ports.iter().map(|port| port.name.clone()));
        rows
    }

    /// The row this input occupies. A port that is no longer plugged in has no
    /// row, so the picker shows [`Self::FollowSelection`]'s -- the interface
    /// says the device is missing separately, rather than the menu silently
    /// landing on whichever port now sits where it used to.
    pub fn row(&self, ports: &[MidiPortInfo]) -> usize {
        match self {
            Self::FollowSelection => 0,
            Self::Off => 1,
            Self::AllPorts => 2,
            Self::Port(name) => ports
                .iter()
                .position(|port| &port.name == name)
                .map_or(0, |index| index + 3),
        }
    }

    /// The input a picker row names.
    pub fn from_row(row: usize, ports: &[MidiPortInfo]) -> Self {
        match row {
            1 => Self::Off,
            2 => Self::AllPorts,
            row if row >= 3 => ports
                .get(row - 3)
                .map_or(Self::FollowSelection, |port| Self::Port(port.name.clone())),
            _ => Self::FollowSelection,
        }
    }

    /// Whether this names a port that is not plugged in, so the interface can
    /// say so rather than leaving a channel mysteriously silent.
    pub fn is_missing(&self, ports: &[MidiPortInfo]) -> bool {
        self.port_name()
            .is_some_and(|name| resolve_port_name(name, ports).is_none())
    }
}

/// Which port a control binding listens to.
///
/// Deliberately not [`MidiInputSource`]: a binding has no notion of following
/// the selection or of being switched off -- a binding that should not fire is
/// deleted. Flattening the two into one enum would put two cases in front of
/// every `match` that cannot happen, and the picker that lists a channel's
/// four choices as one menu is the reason the other type is flat. What they
/// must not disagree about is what *matching a port* means, and they do not:
/// both call [`resolve_port_name`].
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiPortFilter {
    /// Any port. A desk that presents several ports, or a user with one
    /// controller, wants this.
    #[default]
    Any,
    /// One port, by the name the driver reported for it.
    Named(String),
}

/// [`MidiPortFilter`] with the name resolved, for matching against a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiPortMatch {
    #[default]
    Any,
    Port(MidiPortId),
    /// The binding names a port that is not plugged in. Distinct from `Any`
    /// so the binding is inert rather than promiscuous, and distinct from a
    /// resolved port so the interface can say *why* it is inert.
    Missing,
}

impl MidiPortFilter {
    pub fn resolve(&self, ports: &[MidiPortInfo]) -> MidiPortMatch {
        match self {
            Self::Any => MidiPortMatch::Any,
            Self::Named(name) => resolve_port_name(name, ports)
                .map_or(MidiPortMatch::Missing, MidiPortMatch::Port),
        }
    }
}

impl MidiPortMatch {
    pub const fn accepts(self, port: MidiPortId) -> bool {
        match self {
            Self::Any => true,
            Self::Port(one) => one.0 == port.0,
            Self::Missing => false,
        }
    }
}

/// [`MidiInputSource`] with the port name already resolved.
///
/// Two types for one idea, on purpose: the stored one is addressed by a name
/// that survives a reboot, and this one is `Copy`, allocation-free, and
/// comparable in a branch the audio thread runs per message. `resolve` is the
/// single crossing between them and
/// `channel_input_resolves_a_port_name_once` is what holds them together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MidiRouteSource {
    #[default]
    FollowSelection,
    Off,
    AllPorts,
    Port(MidiPortId),
}

/// One channel's MIDI input, as the project stores it.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ChannelMidiInput {
    #[serde(default)]
    pub source: MidiInputSource,
    #[serde(default)]
    pub channel: MidiChannelFilter,
}

/// One channel's MIDI input as the audio thread sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MidiInputRoute {
    pub source: MidiRouteSource,
    pub channel: MidiChannelFilter,
}

impl MidiInputRoute {
    /// Whether this route claims `message` **without** consulting the
    /// selection. `FollowSelection` never claims here: the selected channel
    /// is the engine's fallback, applied once for a message no explicit route
    /// wanted, so that selecting a channel does not double a note that an
    /// explicit route already played.
    pub fn claims(self, message: &MidiMessage) -> bool {
        if message.kind.is_transport() || !self.channel.accepts(message.channel) {
            return false;
        }
        match self.source {
            MidiRouteSource::FollowSelection | MidiRouteSource::Off => false,
            MidiRouteSource::AllPorts => true,
            MidiRouteSource::Port(port) => port == message.port,
        }
    }

    /// Whether this route sounds when its channel is the selected one.
    pub fn follows_selection(self, message: &MidiMessage) -> bool {
        !message.kind.is_transport()
            && self.source == MidiRouteSource::FollowSelection
            && self.channel.accepts(message.channel)
    }
}

impl ChannelMidiInput {
    pub fn resolve(&self, ports: &[MidiPortInfo]) -> MidiInputRoute {
        MidiInputRoute {
            source: self.source.resolve(ports),
            channel: self.channel,
        }
    }
}

/// A decoded message, where it landed inside the current block, and which
/// port it came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiMessage {
    /// Frames into the block, `0..frames`.
    pub offset: u32,
    /// The input this arrived on.
    pub port: MidiPortId,
    /// MIDI channel, `0..16`, or [`SYSTEM_CHANNEL`] for a message that has
    /// none.
    pub channel: u8,
    pub kind: MidiKind,
}

impl MidiMessage {
    /// Where a Song Position Pointer lands, in ticks at `ppq`. Its unit is
    /// the MIDI beat, which is a **sixteenth** note, so four of them are a
    /// quarter and a bar of 4/4 is sixteen. Taking `ppq` rather than reaching
    /// for the default keeps the one place this arithmetic is written honest
    /// about a project whose resolution is not 96.
    pub fn song_position_ticks(beats: u16, ppq: u32) -> u32 {
        u32::from(beats) * (ppq / 4)
    }

    /// Decode one raw MIDI packet. Returns `None` for anything mooloop does
    /// not act on (aftertouch, program change, clock, system exclusive) and
    /// for truncated packets, so the caller can simply skip them.
    pub fn decode(port: MidiPortId, offset: u32, bytes: &[u8]) -> Option<Self> {
        let status = *bytes.first()?;
        // Running status is not reconstructed: JACK delivers whole messages,
        // so a packet without a status byte is malformed rather than
        // continued.
        if status < 0x80 {
            return None;
        }
        // System messages first, because they have no channel nibble: `0xFA`
        // is Start, not "note-off on channel 10".
        if status >= 0xF0 {
            let kind = match status {
                0xF2 => {
                    let low = u16::from(*bytes.get(1)? & 0x7F);
                    let high = u16::from(*bytes.get(2)? & 0x7F);
                    MidiKind::SongPosition {
                        beats: (high << 7) | low,
                    }
                }
                0xFA => MidiKind::Start,
                0xFB => MidiKind::Continue,
                0xFC => MidiKind::Stop,
                _ => return None,
            };
            return Some(Self {
                offset,
                port,
                channel: SYSTEM_CHANNEL,
                kind,
            });
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
            port,
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

    /// Every test here decodes from one port; the port is a routing fact, not
    /// a decoding one.
    const PORT: MidiPortId = MidiPortId::FIRST;

    fn decode(offset: u32, bytes: &[u8]) -> Option<MidiMessage> {
        MidiMessage::decode(PORT, offset, bytes)
    }

    #[test]
    fn note_on_with_zero_velocity_decodes_as_note_off() {
        let on = decode(7, &[0x90, 60, 100]).unwrap();
        assert_eq!(
            on.kind,
            MidiKind::NoteOn {
                note: 60,
                velocity: 100
            }
        );
        assert_eq!(on.offset, 7);
        let off = decode(0, &[0x90, 60, 0]).unwrap();
        assert_eq!(off.kind, MidiKind::NoteOff { note: 60 });
    }

    #[test]
    fn channel_and_unhandled_status_are_decoded_or_skipped() {
        let cc = decode(0, &[0xB3, 21, 64]).unwrap();
        assert_eq!(cc.channel, 3);
        assert_eq!(
            cc.kind,
            MidiKind::ControlChange {
                controller: 21,
                value: 64
            }
        );
        // Program change, aftertouch, clock, and truncated packets.
        assert!(decode(0, &[0xC0, 1]).is_none());
        assert!(decode(0, &[0xF8]).is_none());
        assert!(decode(0, &[0x90, 60]).is_none());
        assert!(decode(0, &[]).is_none());
        assert!(decode(0, &[60, 100]).is_none());
    }

    #[test]
    fn pitch_bend_centres_on_zero() {
        assert_eq!(
            decode(0, &[0xE0, 0, 64]).unwrap().kind,
            MidiKind::PitchBend { value: 0 }
        );
        assert_eq!(
            decode(0, &[0xE0, 0, 0]).unwrap().kind,
            MidiKind::PitchBend { value: -8192 }
        );
    }

    /// `0xFA` is Start, and it is emphatically not a note-off on channel 10.
    /// That is the whole reason system status is matched before the channel
    /// nibble is read.
    #[test]
    fn transport_messages_carry_no_channel() {
        for (bytes, kind) in [
            (&[0xFA][..], MidiKind::Start),
            (&[0xFB][..], MidiKind::Continue),
            (&[0xFC][..], MidiKind::Stop),
        ] {
            let message = decode(0, bytes).expect("a transport message decodes");
            assert_eq!(message.kind, kind);
            assert_eq!(message.channel, SYSTEM_CHANNEL);
            assert!(message.kind.is_transport());
        }
        // Clock is still dropped; see this module's header for why.
        assert!(decode(0, &[0xF8]).is_none());
        // And system exclusive, and tune request.
        assert!(decode(0, &[0xF0, 0x7E, 0xF7]).is_none());
        assert!(decode(0, &[0xF6]).is_none());
    }

    /// Song Position counts sixteenths, so bar 2 of 4/4 is beat 16, and at
    /// 96 PPQ that is tick 384 -- four quarters in, not sixteen.
    #[test]
    fn song_position_counts_sixteenths_not_quarters() {
        let message = decode(0, &[0xF2, 16, 0]).expect("a song position decodes");
        assert_eq!(message.kind, MidiKind::SongPosition { beats: 16 });
        assert!(message.kind.is_transport());
        assert_eq!(MidiMessage::song_position_ticks(16, 96), 384);
        assert_eq!(MidiMessage::song_position_ticks(0, 96), 0);
        // Both bytes are read: 128 beats needs the high one.
        assert_eq!(
            decode(0, &[0xF2, 0, 1]).unwrap().kind,
            MidiKind::SongPosition { beats: 128 }
        );
    }

    /// Omni means every *channel*, and a system message is not on one. A
    /// filter that accepted [`SYSTEM_CHANNEL`] would let an Omni channel play
    /// a note off the back of a Start message.
    #[test]
    fn omni_does_not_mean_system() {
        for channel in 0..16 {
            assert!(MidiChannelFilter::Omni.accepts(channel));
        }
        assert!(!MidiChannelFilter::Omni.accepts(SYSTEM_CHANNEL));
        assert!(!MidiChannelFilter::One(0).accepts(SYSTEM_CHANNEL));
        assert!(MidiChannelFilter::One(9).accepts(9));
        assert!(!MidiChannelFilter::One(9).accepts(8));
    }

    /// The picker's rows and the filter are one fact. This is the test that
    /// keeps them one: the markup asks for a row and gets a filter back.
    #[test]
    fn every_channel_filter_row_round_trips() {
        assert_eq!(MidiChannelFilter::from_row(0), MidiChannelFilter::Omni);
        assert_eq!(MidiChannelFilter::Omni.row(), 0);
        assert_eq!(MidiChannelFilter::Omni.label(), "Omni");
        for row in 1..MIDI_CHANNEL_FILTER_ROWS {
            let filter = MidiChannelFilter::from_row(row);
            assert_eq!(filter, MidiChannelFilter::One(row as u8 - 1));
            assert_eq!(filter.row(), row);
            assert_eq!(filter.label(), row.to_string());
        }
        // Row 16 is channel 16, and there is no row 17.
        assert_eq!(
            MidiChannelFilter::from_row(16),
            MidiChannelFilter::One(15)
        );
        assert_eq!(MidiChannelFilter::from_row(17), MidiChannelFilter::Omni);
    }

    fn ports() -> Vec<MidiPortInfo> {
        vec![
            MidiPortInfo {
                id: MidiPortId(0),
                name: "Launchkey MK3".to_owned(),
            },
            MidiPortInfo {
                id: MidiPortId(1),
                name: "SH-01A".to_owned(),
            },
        ]
    }

    /// The stored form names a port that survives a reboot; the resolved form
    /// is what the audio thread compares. This is the one crossing between
    /// them, which is why the two enums are allowed to exist.
    #[test]
    fn channel_input_resolves_a_port_name_once() {
        let ports = ports();
        assert_eq!(
            MidiInputSource::Port("SH-01A".to_owned()).resolve(&ports),
            MidiRouteSource::Port(MidiPortId(1))
        );
        // A device that is not plugged in is silent for this run, and the
        // project keeps the name so plugging it back in restores the routing.
        assert_eq!(
            MidiInputSource::Port("A device nobody owns".to_owned()).resolve(&ports),
            MidiRouteSource::Off
        );
        assert_eq!(
            MidiInputSource::AllPorts.resolve(&ports),
            MidiRouteSource::AllPorts
        );
        assert_eq!(
            MidiInputSource::FollowSelection.resolve(&ports),
            MidiRouteSource::FollowSelection
        );
        assert_eq!(MidiInputSource::Off.resolve(&ports), MidiRouteSource::Off);
    }

    /// A route claims by port and channel; the selection is a separate
    /// question, asked only of the channels that did not claim. Without that
    /// split, selecting a channel that already listens to All Inputs would
    /// play every note twice.
    #[test]
    fn a_route_claims_by_port_and_channel_not_by_selection() {
        let message = |port: u16, channel: u8| MidiMessage {
            offset: 0,
            port: MidiPortId(port),
            channel,
            kind: MidiKind::NoteOn {
                note: 60,
                velocity: 100,
            },
        };
        let route = |source, channel| MidiInputRoute { source, channel };

        let any = route(MidiRouteSource::AllPorts, MidiChannelFilter::Omni);
        assert!(any.claims(&message(0, 0)));
        assert!(any.claims(&message(1, 9)));
        assert!(!any.follows_selection(&message(0, 0)));

        let one_port = route(
            MidiRouteSource::Port(MidiPortId(1)),
            MidiChannelFilter::Omni,
        );
        assert!(one_port.claims(&message(1, 3)));
        assert!(!one_port.claims(&message(0, 3)));

        let one_channel = route(MidiRouteSource::AllPorts, MidiChannelFilter::One(2));
        assert!(one_channel.claims(&message(0, 2)));
        assert!(!one_channel.claims(&message(0, 3)));

        let following = route(MidiRouteSource::FollowSelection, MidiChannelFilter::One(2));
        assert!(!following.claims(&message(0, 2)));
        assert!(following.follows_selection(&message(0, 2)));
        assert!(!following.follows_selection(&message(0, 3)));

        let off = route(MidiRouteSource::Off, MidiChannelFilter::Omni);
        assert!(!off.claims(&message(0, 0)));
        assert!(!off.follows_selection(&message(0, 0)));

        // No channel route ever claims a transport message, whatever its
        // filter says.
        let start = MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: SYSTEM_CHANNEL,
            kind: MidiKind::Start,
        };
        assert!(!any.claims(&start));
        assert!(!following.follows_selection(&start));
    }

    /// Every row the input picker draws round-trips to the value it stands for.
    /// This is the test that lets the markup ask for a row and get an input
    /// back rather than holding its own copy of the list.
    #[test]
    fn every_input_picker_row_round_trips() {
        let ports = ports();
        let rows = MidiInputSource::picker_rows(&ports);
        assert_eq!(
            rows,
            vec![
                "Follow Selection",
                "Off",
                "All Inputs",
                "Launchkey MK3",
                "SH-01A",
            ]
        );
        for (row, _) in rows.iter().enumerate() {
            let source = MidiInputSource::from_row(row, &ports);
            assert_eq!(source.row(&ports), row, "row {row}");
        }
        assert_eq!(
            MidiInputSource::from_row(3, &ports),
            MidiInputSource::Port("Launchkey MK3".to_owned())
        );
        // Past the end is the default rather than a panic: a menu index is
        // not a value worth refusing over.
        assert_eq!(
            MidiInputSource::from_row(99, &ports),
            MidiInputSource::FollowSelection
        );

        // A port that went away has no row, so the picker falls back rather
        // than landing on whichever port now holds its old index.
        let unplugged = MidiInputSource::Port("SH-01A".to_owned());
        assert_eq!(unplugged.row(&ports[..1]), 0);
        assert!(unplugged.is_missing(&ports[..1]));
        assert!(!unplugged.is_missing(&ports));
        assert!(!MidiInputSource::AllPorts.is_missing(&[]));
    }

    /// The field is an addition, not a change: a channel written before it
    /// existed loads as the behaviour it had.
    #[test]
    fn a_channel_with_no_stored_input_follows_the_selection() {
        let input: ChannelMidiInput =
            toml::from_str("").expect("an empty table is a whole default");
        assert_eq!(input, ChannelMidiInput::default());
        assert_eq!(input.source, MidiInputSource::FollowSelection);
        assert_eq!(input.channel, MidiChannelFilter::Omni);
        assert_eq!(
            input.resolve(&ports()),
            MidiInputRoute {
                source: MidiRouteSource::FollowSelection,
                channel: MidiChannelFilter::Omni,
            }
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
