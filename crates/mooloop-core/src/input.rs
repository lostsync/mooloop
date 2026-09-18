//! A channel's input: MIDI or audio, picked from one menu.
//!
//! `docs/plans/audio-recording/02-one-input-menu.md`. The sidebar's IN row
//! chooses what record-arm captures on a channel: notes from a MIDI input, or
//! audio from a source -- the master, a track, a channel, and (from step 01)
//! a hardware input. The document keeps the two halves in two fields,
//! `Channel::midi_input` and `Channel::audio_input`, and **at most one of them
//! is ever away from its default**; "one menu" is a presentation rule over
//! that pair, and [`ChannelInput`] is the value the menu stands for.

use crate::midi::{ChannelMidiInput, MidiInputSource, MidiPortInfo};
use crate::{ChannelId, DeviceKind, TrackId};

/// Where a channel records audio from, as the project stores it.
///
/// **App sources are named by identity, never by seat** -- this is a new
/// field that names another channel, and `channel-identity` spent a week
/// converting the old ones. The engine is index-addressed, so the session
/// resolves an id to a seat when it builds the routing
/// ([`Self::resolve`]); a source that has been deleted resolves to nothing
/// and the picker says it is missing, as it does for an unplugged MIDI port.
///
/// A hardware input (`Port`) arrives with step 01, when there is something
/// to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioInputSource {
    /// Records no audio. The default, and what every channel written before
    /// this field existed holds.
    #[default]
    Off,
    /// The master's output.
    Master,
    /// A track's output, after its fader and pan.
    Track(TrackId),
    /// A channel's output, after its fader and pan. May be the recording
    /// channel itself: resampling in place is the ordinary gesture, and it
    /// cannot feed back, because a take replaces the sample only once it
    /// stops (`00-status.md`, open question 7).
    Channel(ChannelId),
}

impl AudioInputSource {
    pub const fn is_off(&self) -> bool {
        matches!(self, Self::Off)
    }

    /// The seat this source is heard at in a bank of `channels` and
    /// `tracks`, or `None` when it names nothing that exists.
    pub fn resolve(
        self,
        channels: impl IntoIterator<Item = ChannelId>,
        tracks: impl IntoIterator<Item = TrackId>,
    ) -> Option<AudioTap> {
        match self {
            Self::Off => None,
            Self::Master => Some(AudioTap::Master),
            Self::Channel(id) => seat_of(channels, id).map(AudioTap::Channel),
            Self::Track(id) => seat_of(tracks, id).map(AudioTap::Track),
        }
    }
}

/// `serde(skip_serializing_if)` for [`AudioInputSource`].
pub fn audio_input_is_off(source: &AudioInputSource) -> bool {
    source.is_off()
}

fn seat_of<Id: PartialEq>(ids: impl IntoIterator<Item = Id>, id: Id) -> Option<u8> {
    ids.into_iter()
        .position(|held| held == id)
        .and_then(|seat| u8::try_from(seat).ok())
}

/// Where the audio thread reads a recording from: a strip's output, by seat.
///
/// The resolved twin of [`AudioInputSource`], in the relation
/// `MidiRouteSource` has to `MidiInputSource`: the stored one survives a
/// channel move, this one is `Copy` and indexes the engine directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioTap {
    Master,
    Track(u8),
    Channel(u8),
}

/// The one channel that records audio this generation, and what it records.
///
/// One rather than a table, because only one channel records audio at a time
/// (`02-one-input-menu.md`, "Session"): picking an audio input on a channel
/// clears it from whichever channel had one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRecordRoute {
    /// The recording channel's seat.
    pub channel: u8,
    pub tap: AudioTap,
}

/// Resolve the recording channel and its source to seats, for the engine.
///
/// `channels` is every channel's identity and audio input, in bank order;
/// `tracks` every track's identity, master first. The first channel with an
/// audio input records -- the session keeps it to one, and a hand-edited file
/// with two is repaired on load -- and `None` when none does or its source no
/// longer exists.
pub fn audio_record_route(
    channels: &[(ChannelId, AudioInputSource)],
    tracks: &[TrackId],
) -> Option<AudioRecordRoute> {
    let (seat, source) = channels
        .iter()
        .enumerate()
        .find_map(|(seat, (_, source))| (!source.is_off()).then_some((seat, *source)))?;
    let tap = source.resolve(
        channels.iter().map(|(id, _)| *id),
        tracks.iter().copied(),
    )?;
    Some(AudioRecordRoute {
        channel: u8::try_from(seat).ok()?,
        tap,
    })
}

/// What a channel's IN row is set to: one of the two halves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelInput {
    Midi(ChannelMidiInput),
    Audio(AudioInputSource),
}

impl ChannelInput {
    /// Which half of a stored pair is in charge. An audio source that is not
    /// `Off` wins, which is also how a hand-edited file holding both reads.
    pub fn of(midi: &ChannelMidiInput, audio: AudioInputSource) -> Self {
        if audio.is_off() {
            Self::Midi(midi.clone())
        } else {
            Self::Audio(audio)
        }
    }

    /// Write this into a stored pair, putting the other half back to its
    /// default -- the "at most one non-default" rule, in the one place it is
    /// spelled.
    pub fn store(self, midi: &mut ChannelMidiInput, audio: &mut AudioInputSource) {
        match self {
            Self::Midi(input) => {
                *midi = input;
                *audio = AudioInputSource::Off;
            }
            Self::Audio(source) => {
                *midi = ChannelMidiInput::default();
                *audio = source;
            }
        }
    }
}

/// Only a Sampler records audio: a take becomes the channel's sample, and no
/// other device has one (Adam, 2026-09-17).
pub const fn records_audio(kind: DeviceKind) -> bool {
    matches!(kind, DeviceKind::Sampler)
}

/// Apply the non-sampler rule to a stored pair after `kind` has changed.
///
/// A channel that stops being a Sampler while it has an audio input moves to
/// the **no-input** row -- MIDI `Off` -- not back to Follow Selection, and
/// switching back to Sampler does not restore the audio input. Returns
/// whether anything changed.
pub fn settle_input(
    kind: DeviceKind,
    midi: &mut ChannelMidiInput,
    audio: &mut AudioInputSource,
) -> bool {
    if records_audio(kind) || audio.is_off() {
        return false;
    }
    *audio = AudioInputSource::Off;
    *midi = ChannelMidiInput {
        source: MidiInputSource::Off,
        ..ChannelMidiInput::default()
    };
    true
}

/// One audio source row in the picker: what it stands for and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSourceRow {
    pub source: AudioInputSource,
    pub label: String,
}

/// The audio sources a picker lists: the master, then every track but the
/// master, then every channel.
///
/// Generated from the project each time the menu opens, so a renamed track or
/// channel reads correctly without anything being stored. `tracks` includes
/// the master, first, as a bank does; it is listed once, as `Master`.
pub fn audio_source_rows<'a>(
    tracks: impl IntoIterator<Item = (TrackId, &'a str)>,
    channels: impl IntoIterator<Item = (ChannelId, &'a str)>,
) -> Vec<AudioSourceRow> {
    let mut rows = vec![AudioSourceRow {
        source: AudioInputSource::Master,
        label: "Master".to_owned(),
    }];
    rows.extend(tracks.into_iter().skip(1).map(|(id, name)| AudioSourceRow {
        source: AudioInputSource::Track(id),
        label: format!("Track · {name}"),
    }));
    rows.extend(channels.into_iter().map(|(id, name)| AudioSourceRow {
        source: AudioInputSource::Channel(id),
        label: format!("Channel · {name}"),
    }));
    rows
}

/// What a picker row stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputRow {
    Midi(MidiInputSource),
    Audio(AudioInputSource),
    /// The heading between the two halves. Picking it changes nothing.
    Heading,
}

/// The IN row's menu: the MIDI rows, a heading, then the audio sources.
///
/// Built here rather than in the markup for the reason
/// [`MidiInputSource::picker_rows`] gives -- a menu's index and the value it
/// stands for are one fact -- and on top of those rows rather than beside
/// them, so the MIDI half of the numbering does not move.
pub struct InputPicker<'a> {
    ports: &'a [MidiPortInfo],
    sources: &'a [AudioSourceRow],
}

/// The heading row's text.
pub const AUDIO_HEADING: &str = "── Audio ──";

impl<'a> InputPicker<'a> {
    pub fn new(ports: &'a [MidiPortInfo], sources: &'a [AudioSourceRow]) -> Self {
        Self { ports, sources }
    }

    fn heading(&self) -> usize {
        MidiInputSource::picker_rows(self.ports).len()
    }

    pub fn rows(&self) -> Vec<String> {
        let mut rows = MidiInputSource::picker_rows(self.ports);
        rows.push(AUDIO_HEADING.to_owned());
        rows.extend(self.sources.iter().map(|row| row.label.clone()));
        rows
    }

    /// The row `input` occupies. An audio source that is not listed -- a
    /// deleted channel or track -- has no row and shows the heading's, and
    /// [`Self::is_missing`] says why; the MIDI half keeps its own fallback.
    pub fn row(&self, input: &ChannelInput) -> usize {
        match input {
            ChannelInput::Midi(midi) => midi.source.row(self.ports),
            ChannelInput::Audio(source) => self
                .sources
                .iter()
                .position(|row| row.source == *source)
                .map_or(self.heading(), |at| self.heading() + 1 + at),
        }
    }

    /// What `row` stands for.
    pub fn pick(&self, row: usize) -> InputRow {
        let heading = self.heading();
        match row {
            row if row < heading => InputRow::Midi(MidiInputSource::from_row(row, self.ports)),
            row if row == heading => InputRow::Heading,
            row => self
                .sources
                .get(row - heading - 1)
                .map_or(InputRow::Heading, |source| InputRow::Audio(source.source)),
        }
    }

    /// Whether `input` names something the menu cannot show: an unplugged
    /// MIDI port, or an audio source that has been deleted.
    pub fn is_missing(&self, input: &ChannelInput) -> bool {
        match input {
            ChannelInput::Midi(midi) => midi.source.is_missing(self.ports),
            ChannelInput::Audio(source) => {
                !source.is_off() && !self.sources.iter().any(|row| row.source == *source)
            }
        }
    }

    /// The rows an audio input cannot be picked from on a channel of `kind`:
    /// all of them, unless it is a Sampler. For the markup to grey out; a
    /// pick of one is refused by the session either way.
    pub fn is_enabled(&self, row: usize, kind: DeviceKind) -> bool {
        match self.pick(row) {
            InputRow::Midi(_) => true,
            InputRow::Heading => false,
            InputRow::Audio(_) => records_audio(kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::MidiPortId;

    fn ports() -> Vec<MidiPortInfo> {
        vec![MidiPortInfo {
            id: MidiPortId::FIRST,
            name: "Keys".to_owned(),
        }]
    }

    fn sources() -> Vec<AudioSourceRow> {
        audio_source_rows(
            [(TrackId(0), "Master"), (TrackId(4), "Drums")],
            [(ChannelId(7), "Kick"), (ChannelId(2), "Bass")],
        )
    }

    /// Written before the rows changed, as the step asks: the MIDI half keeps
    /// the numbering it had, and the audio half follows a heading.
    #[test]
    fn the_rows_are_the_midi_rows_then_a_heading_then_the_sources() {
        let (ports, sources) = (ports(), sources());
        let picker = InputPicker::new(&ports, &sources);
        assert_eq!(
            picker.rows(),
            [
                "Follow Selection",
                "Off",
                "All Inputs",
                "Keys",
                AUDIO_HEADING,
                "Master",
                "Track · Drums",
                "Channel · Kick",
                "Channel · Bass",
            ]
        );
    }

    #[test]
    fn every_row_round_trips() {
        let (ports, sources) = (ports(), sources());
        let picker = InputPicker::new(&ports, &sources);
        for row in 0..picker.rows().len() {
            let input = match picker.pick(row) {
                InputRow::Midi(source) => ChannelInput::Midi(ChannelMidiInput {
                    source,
                    ..ChannelMidiInput::default()
                }),
                InputRow::Audio(source) => ChannelInput::Audio(source),
                InputRow::Heading => {
                    assert_eq!(row, 4, "only one heading");
                    continue;
                }
            };
            assert_eq!(picker.row(&input), row, "{input:?}");
        }
    }

    /// A source is an identity: the picker finds it wherever its row went,
    /// and one that was deleted is missing rather than landing on whoever
    /// took its place.
    #[test]
    fn a_source_is_found_by_identity_and_a_deleted_one_is_missing() {
        let ports = ports();
        let kick = ChannelInput::Audio(AudioInputSource::Channel(ChannelId(7)));
        let reordered = audio_source_rows(
            [(TrackId(0), "Master")],
            [(ChannelId(2), "Bass"), (ChannelId(7), "Kick")],
        );
        let picker = InputPicker::new(&ports, &reordered);
        assert_eq!(picker.rows()[picker.row(&kick)], "Channel · Kick");
        assert!(!picker.is_missing(&kick));

        let gone = audio_source_rows([(TrackId(0), "Master")], [(ChannelId(2), "Bass")]);
        let picker = InputPicker::new(&ports, &gone);
        assert!(picker.is_missing(&kick));
        assert_eq!(picker.pick(picker.row(&kick)), InputRow::Heading);
    }

    #[test]
    fn audio_rows_are_enabled_only_on_a_sampler() {
        let (ports, sources) = (ports(), sources());
        let picker = InputPicker::new(&ports, &sources);
        for kind in [
            DeviceKind::Sampler,
            DeviceKind::DrumSynth,
            DeviceKind::MonoSynth,
            DeviceKind::PolySynth,
            DeviceKind::MlM1,
            DeviceKind::MlP8,
            DeviceKind::Ds01,
            DeviceKind::AuxIn,
        ] {
            for row in 0..picker.rows().len() {
                let expected = match picker.pick(row) {
                    InputRow::Midi(_) => true,
                    InputRow::Heading => false,
                    InputRow::Audio(_) => kind == DeviceKind::Sampler,
                };
                assert_eq!(picker.is_enabled(row, kind), expected, "{kind:?} row {row}");
            }
        }
    }

    #[test]
    fn storing_one_half_resets_the_other() {
        let mut midi = ChannelMidiInput {
            source: MidiInputSource::AllPorts,
            ..ChannelMidiInput::default()
        };
        let mut audio = AudioInputSource::Off;
        ChannelInput::Audio(AudioInputSource::Master).store(&mut midi, &mut audio);
        assert_eq!((midi.clone(), audio), (ChannelMidiInput::default(), AudioInputSource::Master));
        assert_eq!(ChannelInput::of(&midi, audio), ChannelInput::Audio(AudioInputSource::Master));

        ChannelInput::Midi(ChannelMidiInput::default()).store(&mut midi, &mut audio);
        assert!(audio.is_off());
    }

    /// Leaving the Sampler with an audio input lands on Off, not on Follow
    /// Selection; with a MIDI input it changes nothing.
    #[test]
    fn leaving_the_sampler_drops_an_audio_input_to_off() {
        let mut midi = ChannelMidiInput::default();
        let mut audio = AudioInputSource::Master;
        assert!(!settle_input(DeviceKind::Sampler, &mut midi, &mut audio));
        assert!(settle_input(DeviceKind::DrumSynth, &mut midi, &mut audio));
        assert_eq!(audio, AudioInputSource::Off);
        assert_eq!(midi.source, MidiInputSource::Off);

        let mut midi = ChannelMidiInput::default();
        let mut audio = AudioInputSource::Off;
        assert!(!settle_input(DeviceKind::DrumSynth, &mut midi, &mut audio));
        assert_eq!(midi, ChannelMidiInput::default());
    }

    /// The route follows identities: after a move the same recording names
    /// new seats, and a deleted source records nothing.
    #[test]
    fn the_route_names_the_seats_the_ids_hold_now() {
        let tracks = [TrackId(0), TrackId(4)];
        let recording = |sources: [(ChannelId, AudioInputSource); 2]| {
            audio_record_route(&sources, &tracks)
        };
        let kick = (ChannelId(7), AudioInputSource::Off);
        let sampler = (ChannelId(2), AudioInputSource::Channel(ChannelId(7)));
        assert_eq!(
            recording([kick, sampler]),
            Some(AudioRecordRoute { channel: 1, tap: AudioTap::Channel(0) })
        );
        assert_eq!(
            recording([sampler, kick]),
            Some(AudioRecordRoute { channel: 0, tap: AudioTap::Channel(1) })
        );
        assert_eq!(
            audio_record_route(&[sampler], &tracks),
            None,
            "a deleted source records nothing"
        );
        assert_eq!(recording([kick, (ChannelId(2), AudioInputSource::Off)]), None);
    }

    #[test]
    fn a_source_resolves_to_the_seat_its_id_holds_now() {
        let channels = [ChannelId(7), ChannelId(2)];
        let tracks = [TrackId(0), TrackId(4)];
        assert_eq!(
            AudioInputSource::Channel(ChannelId(2)).resolve(channels, tracks),
            Some(AudioTap::Channel(1))
        );
        assert_eq!(
            AudioInputSource::Track(TrackId(4)).resolve(channels, tracks),
            Some(AudioTap::Track(1))
        );
        assert_eq!(AudioInputSource::Master.resolve(channels, tracks), Some(AudioTap::Master));
        assert_eq!(AudioInputSource::Channel(ChannelId(9)).resolve(channels, tracks), None);
        assert_eq!(AudioInputSource::Off.resolve(channels, tracks), None);
    }
}
