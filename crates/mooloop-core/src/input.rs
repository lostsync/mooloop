//! A channel's audio input: where it records from.
//!
//! `docs/plans/archive/audio-recording/`. Every channel has one, beside its MIDI
//! input and independent of it (decision 10): the sidebar's AUDIO row picks
//! it, and it stays set. A channel is a dumb slot (decision 5) -- any channel
//! holds an audio input, and a device that has no use for one ignores it --
//! and any number of channels may hold one at once (decision 6). What records
//! is the sampler's own record button, from its Record page.

use crate::{ChannelId, TrackId};

/// Where a channel records audio from, as the project stores it.
///
/// **App sources are named by identity, never by seat** -- this is a field
/// that names another channel, and `channel-identity` spent a week converting
/// the old ones. The engine is index-addressed, so the session resolves an id
/// to a seat when it builds the routing ([`Self::resolve`]); a source that has
/// been deleted resolves to nothing and the picker says it is missing, as it
/// does for an unplugged MIDI port.
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
    /// The hardware input: what the audio driver delivers into the app
    /// (`audio-recording/01`). One stereo pair for now -- under JACK what
    /// feeds it is chosen in the JACK graph, as the MIDI input is -- so it is
    /// one value rather than a port name. When a driver offers several, this
    /// grows a name the way a MIDI port has one.
    Input,
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
    /// `tracks`, or `None` when it is `Off` or names nothing that exists.
    pub fn resolve(
        self,
        channels: impl IntoIterator<Item = ChannelId>,
        tracks: impl IntoIterator<Item = TrackId>,
    ) -> Option<AudioTap> {
        match self {
            Self::Off => None,
            Self::Input => Some(AudioTap::Input),
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
    /// The engine's input bus, filled from the driver at the top of each
    /// block.
    Input,
    Master,
    Track(u8),
    Channel(u8),
}

/// Every channel's audio input resolved to a seat, in bank order: `None` for
/// a channel with none, or with a source that no longer exists.
///
/// `channels` is every channel's identity and audio input, in bank order;
/// `tracks` every track's identity, master first. Sources are resolved
/// against the same bank the routing will index, so a move of either end
/// re-resolves the seat and the recording keeps naming the same source.
pub fn audio_input_taps(
    channels: &[(ChannelId, AudioInputSource)],
    tracks: &[TrackId],
) -> Vec<Option<AudioTap>> {
    channels
        .iter()
        .map(|(_, source)| {
            source.resolve(channels.iter().map(|(id, _)| *id), tracks.iter().copied())
        })
        .collect()
}

/// One row of the AUDIO picker: what it stands for and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSourceRow {
    pub source: AudioInputSource,
    pub label: String,
}

/// The rows the AUDIO picker lists: Off, the hardware input when the driver
/// has one (`input`, its label), the master, every track but the master, then
/// every channel -- the channel itself included, because resampling in place
/// is allowed. A channel set to the input on a driver that has none shows it
/// as missing, as an unplugged MIDI port does.
///
/// Generated from the project each time the menu opens, so a renamed track or
/// channel reads correctly without anything being stored. `tracks` includes
/// the master, first, as a bank does; it is listed once, as `Master`.
pub fn audio_source_rows<'a>(
    input: Option<&str>,
    tracks: impl IntoIterator<Item = (TrackId, &'a str)>,
    channels: impl IntoIterator<Item = (ChannelId, &'a str)>,
) -> Vec<AudioSourceRow> {
    let mut rows = vec![AudioSourceRow {
        source: AudioInputSource::Off,
        label: "Off".to_owned(),
    }];
    rows.extend(input.map(|label| AudioSourceRow {
        source: AudioInputSource::Input,
        label: label.to_owned(),
    }));
    rows.push(AudioSourceRow {
        source: AudioInputSource::Master,
        label: "Master".to_owned(),
    });
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

/// The AUDIO row's menu over `rows`.
///
/// Built here rather than in the markup for the reason
/// `MidiInputSource::picker_rows` gives: a menu's index and the value it
/// stands for are one fact.
pub struct AudioInputPicker<'a> {
    rows: &'a [AudioSourceRow],
}

impl<'a> AudioInputPicker<'a> {
    pub fn new(rows: &'a [AudioSourceRow]) -> Self {
        Self { rows }
    }

    pub fn labels(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.label.clone()).collect()
    }

    /// The row `source` occupies. A deleted source has no row and shows Off's;
    /// [`Self::is_missing`] says why.
    pub fn row(&self, source: AudioInputSource) -> usize {
        self.rows.iter().position(|row| row.source == source).unwrap_or(0)
    }

    /// What `row` stands for. Past the end reads as Off.
    pub fn pick(&self, row: usize) -> AudioInputSource {
        self.rows.get(row).map_or(AudioInputSource::Off, |row| row.source)
    }

    /// Whether `source` names something that is no longer in the song.
    pub fn is_missing(&self, source: AudioInputSource) -> bool {
        !source.is_off() && !self.rows.iter().any(|row| row.source == source)
    }
}

/// How a sampler's Record page records: until stopped, or for a clip length
/// (decision 8). Part of the sampler's saved state, so a preset and a song
/// keep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SamplerRecord {
    /// A take ends by itself after [`Self::bars`] when on, and runs until it
    /// is stopped when off.
    #[serde(default)]
    pub clip: bool,
    /// The clip length, in bars.
    #[serde(default = "one_bar")]
    pub bars: u8,
}

/// The longest clip the Record page offers, in bars.
pub const MAX_RECORD_BARS: u8 = 64;

/// What the RECORD page shows, as the window's `sampler-record-state`
/// integer and the label on its button.
///
/// The phase a take is in is `mooloop_engine::TakePhase`; this is what the
/// interface makes of it, and it lives in core because both the crate that
/// encodes it (`mooloop-ui`) and the crate that converts into it
/// (`mooloop-engine`) depend on core. The `.slint` ladder in
/// `sampler-device.slint` decodes these numbers -- Slint cannot import a Rust
/// enum, so the copy is unavoidable and
/// `crates/mooloop-ui/tests/record_page.rs` holds the two together instead.
///
/// Adding a variant means adding an arm to that ladder; the test fails until
/// both move. See MOO-54.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum RecordFace {
    /// Nothing is recording on this channel: the button offers to start one.
    Idle = 0,
    /// Armed, waiting for the next bar line -- the pre-roll.
    Waiting = 1,
    /// Recording; the button offers to stop.
    Recording = 2,
}

impl RecordFace {
    /// Every variant, for a caller that needs to check it has them all.
    pub const ALL: [Self; 3] = [Self::Idle, Self::Waiting, Self::Recording];

    /// The label the RECORD button carries in this state.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "REC",
            Self::Waiting => "WAIT",
            Self::Recording => "STOP",
        }
    }

    /// The number the window property carries in this state.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// The state that number names, or `None` if nothing does.
    pub fn from_i32(value: i32) -> Option<Self> {
        Self::ALL.into_iter().find(|face| face.as_i32() == value)
    }
}

fn one_bar() -> u8 {
    1
}

impl Default for SamplerRecord {
    /// Records until stopped, with a one-bar clip ready for when Clip is on.
    fn default() -> Self {
        Self { clip: false, bars: 1 }
    }
}

impl SamplerRecord {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The clip length a take is armed with, in ticks, or `None` when the
    /// take runs until it is stopped.
    pub fn clip_ticks(&self) -> Option<u32> {
        self.clip.then(|| u32::from(self.bars.clamp(1, MAX_RECORD_BARS)) * crate::TICKS_PER_BAR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<AudioSourceRow> {
        audio_source_rows(
            Some("System Capture"),
            [(TrackId(0), "Master"), (TrackId(4), "Drums")],
            [(ChannelId(7), "Kick"), (ChannelId(2), "Bass")],
        )
    }

    #[test]
    fn the_rows_are_off_the_input_the_master_the_tracks_then_the_channels() {
        let rows = rows();
        assert_eq!(
            AudioInputPicker::new(&rows).labels(),
            ["Off", "System Capture", "Master", "Track · Drums", "Channel · Kick", "Channel · Bass"]
        );
    }

    #[test]
    fn every_row_round_trips() {
        let rows = rows();
        let picker = AudioInputPicker::new(&rows);
        for row in 0..rows.len() {
            assert_eq!(picker.row(picker.pick(row)), row);
        }
    }

    /// A source is an identity: the picker finds it wherever its row went,
    /// and one that was deleted is missing rather than landing on whoever
    /// took its place.
    #[test]
    fn a_source_is_found_by_identity_and_a_deleted_one_is_missing() {
        let kick = AudioInputSource::Channel(ChannelId(7));
        let reordered = audio_source_rows(
            None,
            [(TrackId(0), "Master")],
            [(ChannelId(2), "Bass"), (ChannelId(7), "Kick")],
        );
        let picker = AudioInputPicker::new(&reordered);
        assert_eq!(picker.labels()[picker.row(kick)], "Channel · Kick");
        assert!(!picker.is_missing(kick));

        let gone = audio_source_rows(None, [(TrackId(0), "Master")], [(ChannelId(2), "Bass")]);
        // No driver input: a channel set to it says it is missing.
        assert!(AudioInputPicker::new(&gone).is_missing(AudioInputSource::Input));
        let picker = AudioInputPicker::new(&gone);
        assert!(picker.is_missing(kick));
        assert_eq!(picker.row(kick), 0);
        assert!(!picker.is_missing(AudioInputSource::Off));
    }

    /// Every channel resolves on its own: several may record, one may record
    /// another, and a deleted source resolves to nothing without disturbing
    /// the rest.
    #[test]
    fn every_channel_resolves_its_own_source_by_identity() {
        let tracks = [TrackId(0), TrackId(4)];
        let channels = [
            (ChannelId(7), AudioInputSource::Master),
            (ChannelId(2), AudioInputSource::Channel(ChannelId(7))),
            (ChannelId(3), AudioInputSource::Track(TrackId(4))),
            (ChannelId(5), AudioInputSource::Channel(ChannelId(9))),
            (ChannelId(6), AudioInputSource::Off),
            (ChannelId(8), AudioInputSource::Input),
        ];
        assert_eq!(
            audio_input_taps(&channels, &tracks),
            [
                Some(AudioTap::Master),
                Some(AudioTap::Channel(0)),
                Some(AudioTap::Track(1)),
                None,
                None,
                Some(AudioTap::Input),
            ]
        );

        // After a move the same inputs name the seats their sources hold now.
        let moved = [channels[1], channels[0]];
        assert_eq!(
            audio_input_taps(&moved, &tracks),
            [Some(AudioTap::Channel(1)), Some(AudioTap::Master)]
        );
    }
}
