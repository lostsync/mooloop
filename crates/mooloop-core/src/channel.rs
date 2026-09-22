//! Channel model. A channel is a source, its own insert chain, and an output
//! stage that feeds one mixer bus. The buses themselves live in `mixer`.

/// Complete addressable channel bank.
///
/// Channel indices travel over the realtime bridge as `u8`, so this is a wire
/// format boundary, not a product-design limit. The engine preallocates this
/// bank so channel add/remove never allocates on the RT thread.
pub const MAX_CHANNELS: usize = u8::MAX as usize + 1;

/// Upper bound on stored patterns. Pattern IDs cross the realtime bridge as
/// `u8`, so 256 is the complete addressable bank rather than a UI limit.
pub const MAX_PATTERNS: usize = u8::MAX as usize + 1;

/// Complete addressable effect-chain bank. Chain slots cross the realtime
/// bridge as `u8`; this is therefore a protocol boundary, not an eight-device
/// product cap.
pub const MAX_EFFECTS_PER_CHANNEL: usize = u8::MAX as usize + 1;

/// Largest persisted linear gain for a channel/device output. This is the
/// +12 dB endpoint shared by the UI trim controls; defined in [`crate::gain`]
/// and re-exported here for its historical import path.
pub use crate::gain::MAX_LINEAR_GAIN;

/// Instrument kind for a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    Sampler,
    DrumSynth,
    MonoSynth,
    PolySynth,
    /// The ML-M1: the mono synth built around its filter and its note
    /// behaviour. `MonoSynth` is the older device it replaces, kept loadable
    /// until its channels have somewhere to migrate to.
    ///
    /// Serialized as `ml1`, not as the `rename_all` default `ml_m1`. The
    /// device shipped under the wrong name, and projects and channel presets
    /// saved before it was corrected carry the old spelling. A serialized
    /// variant name is an on-disk identifier like a parameter id, so it is
    /// frozen rather than corrected; the rename is a source and UI change
    /// only.
    #[serde(rename = "ml1")]
    MlM1,
    /// The ML-P8: eight voices around a three-oscillator network. A new kind
    /// rather than a reinterpretation of [`Self::PolySynth`], which stays as
    /// its own simpler device with its own saved projects.
    ///
    /// Serialized as `mlp8`, chosen rather than inherited: the `rename_all`
    /// default would spell it `ml_p8`, and the ML-M1's history is the reason
    /// to pick a serialized name on purpose the first time. It matches the
    /// preset directory slug, and it is frozen from here.
    #[serde(rename = "mlp8")]
    MlP8,
    /// The DS-01: one universal percussion voice, descriptor-addressed from
    /// its first commit. A new kind rather than a table over
    /// [`Self::DrumSynth`], whose parameters are a mode-union and so cannot
    /// carry ids that mean one thing — see `docs/plans/archive/drum-synth-v2/`.
    ///
    /// Serialized as `ds01`, chosen rather than inherited, for the reason the
    /// ML-M1 above records: an on-disk identifier is frozen the day it ships,
    /// so it is worth picking on purpose the first time.
    #[serde(rename = "ds01")]
    Ds01,
    /// Aux In: a source whose sound is another channel's published audio
    /// outlet. The consumer half of the typed audio edge; see
    /// [`crate::aux_in`].
    ///
    /// Serialized as `aux_in`, which is what `rename_all` would spell it
    /// anyway -- written out for the reason the two above give, so nobody has
    /// to derive an on-disk identifier from an attribute.
    #[serde(rename = "aux_in")]
    AuxIn,
}

impl DeviceKind {
    /// The name this device wears in the interface.
    ///
    /// These are product names, not on-disk identifiers -- `kind_slug` in
    /// `mooloop-ui`'s settings is the frozen thing, and `serde`'s renames
    /// above are the frozen thing for projects. This lives in the enum's own
    /// module because three crates want it: the preset browser titles a
    /// group with it, and both channel-creation paths build a default
    /// channel name out of it.
    ///
    /// `main.slint` holds the same eight strings once, in
    /// `SourceKinds.labels`, because a picker row is markup. That copy this
    /// cannot reach, but `mooloop-ui`'s `tests/source_kind_menu.rs` reads it
    /// out of the production markup and holds it against this table.
    pub fn label(self) -> &'static str {
        match self {
            Self::Sampler => "Sampler",
            Self::DrumSynth => "Drum Synth",
            Self::MonoSynth => "Mono Synth",
            Self::PolySynth => "Poly Synth",
            Self::MlM1 => "ML-M1",
            Self::MlP8 => "ML-P8",
            Self::Ds01 => "DS-01",
            Self::AuxIn => "Aux In",
        }
    }

    /// The name a channel takes when it is created at `index`, or when its
    /// device is swapped for this kind. One-based, because a channel is
    /// numbered the way it is displayed.
    ///
    /// **There were two of these tables and they had drifted.** Adding a
    /// channel produced `Drum Synth 3`; changing an existing channel's
    /// device to the same kind produced `Drum 3` -- same device, same slot,
    /// two names depending on how you got there. The three that disagreed
    /// were the three oldest kinds, so the newer five had been added to both
    /// copies correctly and the drift was invisible. Deriving the name from
    /// [`Self::label`] is what stops it recurring.
    pub fn default_channel_name(self, index: usize) -> String {
        format!("{} {}", self.label(), index + 1)
    }
}

/// One mixer channel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    pub name: String,
    pub kind: DeviceKind,
    pub muted: bool,
    /// Whether this channel is soloed.
    ///
    /// **Solo in place**, the same ruling a track's solo follows: soloing
    /// silences the *other* channels rather than opening a monitor path, so
    /// a soloed channel is still heard through its own volume, its pan and
    /// the track it feeds. What it silences is [`solo_silenced`], and that is
    /// derived rather than stored, for the reason a track's is.
    ///
    /// Defaulted and omitted when false, so a song written before channel
    /// solo existed loads and saves byte-identically.
    #[serde(default, skip_serializing_if = "is_unsoloed")]
    pub solo: bool,
    /// Linear output volume in [0, `MAX_LINEAR_GAIN`] (+12 dB).
    pub volume: f32,
    /// Stereo pan in [-1, 1].
    pub pan: f32,
    /// Mixer bus this channel feeds. Defaulted on load so songs written before
    /// the mixer existed land on the master.
    #[serde(default)]
    pub bus: u8,
    /// The colour the user gave this channel, or `None` for one nobody has
    /// chosen. Content rather than theme: it is stored as a colour the song
    /// owns, never as an index into the current palette, so a song looks the
    /// same under every scheme. A malformed one reads as `None` rather than
    /// failing the load -- see [`crate::color::deserialize_lenient`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::color::deserialize_lenient"
    )]
    pub color: Option<crate::color::ProjectColor>,
    /// Which MIDI input, and which MIDI channel on it, plays this channel.
    ///
    /// Defaulted, and its default is what mooloop did before the field
    /// existed: follow the selection, on every channel. So this is an
    /// addition rather than a change -- every song written before it opens
    /// playing exactly as it used to, and a song that has never been
    /// configured does not grow a table of defaults on disk.
    #[serde(default, skip_serializing_if = "is_default_midi_input")]
    pub midi_input: crate::midi::ChannelMidiInput,
    /// Where this channel records audio from, if anywhere: the AUDIO row,
    /// beside and independent of `midi_input`. Any channel may hold one,
    /// whatever its device. Defaulted and omitted when `Off`, so a song
    /// written before it loads and saves byte-identically.
    #[serde(default, skip_serializing_if = "crate::audio_input_is_off")]
    pub audio_input: crate::AudioInputSource,
}

fn is_default_midi_input(input: &crate::midi::ChannelMidiInput) -> bool {
    input == &crate::midi::ChannelMidiInput::default()
}

/// `skip_serializing_if` for [`Channel::solo`]. Named rather than spelled as
/// a negation at the field, because the field's absence has to keep meaning
/// "not soloed" in every direction it is read.
fn is_unsoloed(solo: &bool) -> bool {
    !*solo
}

/// Which channels a solo silences, indexed by channel.
///
/// All false while nothing is soloed, which is the case that has to cost
/// nothing: a bank with no solo anywhere derives all false, matches what was
/// last sent, and never reaches the engine.
///
/// **Flat, where a track's is a graph walk.** [`crate::mixer::solo_silenced`]
/// keeps a soloed track's feeders and its destination audible, because a
/// track's audio *is* made of what feeds it. Channels do not feed each other:
/// a channel is a source, so the only honest reading is that soloing one
/// silences the rest. The one edge that looks like a counter-example is an
/// Aux In reading another channel's published outlet, and it is not one -- a
/// producer publishes whether or not it is heard (`docs/CURRENT.md`, "A muted
/// producer publishes too"), so silencing a source does not take its outlet
/// away from whoever is reading it.
///
/// Answers only for the `count` channels the caller has; a seat past the end
/// of the bank is not soloed and is not silenced either.
pub fn solo_silenced(soloed: impl IntoIterator<Item = bool>) -> [bool; MAX_CHANNELS] {
    let mut flags = [false; MAX_CHANNELS];
    let mut count = 0;
    for (slot, solo) in flags.iter_mut().zip(soloed) {
        *slot = solo;
        count += 1;
    }
    if !flags[..count].iter().any(|&solo| solo) {
        return flags;
    }
    for slot in flags[..count].iter_mut() {
        *slot = !*slot;
    }
    flags
}

impl Channel {
    pub fn new(name: impl Into<String>, kind: DeviceKind) -> Self {
        Self {
            name: name.into(),
            kind,
            muted: false,
            solo: false,
            // Genuinely at unity: the operating-level headroom comes from
            // source calibration (`gain::REFERENCE_PEAK_DBFS`), not from a
            // quiet default fader.
            volume: 1.0,
            pan: 0.0,
            bus: crate::MASTER_BUS,
            color: None,
            midi_input: crate::midi::ChannelMidiInput::default(),
            audio_input: crate::AudioInputSource::Off,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind's interface name, listed rather than derived, so a ninth
    /// device cannot be added without someone writing down what it is called.
    #[test]
    fn every_kind_has_an_interface_name() {
        for (kind, label) in [
            (DeviceKind::Sampler, "Sampler"),
            (DeviceKind::DrumSynth, "Drum Synth"),
            (DeviceKind::MonoSynth, "Mono Synth"),
            (DeviceKind::PolySynth, "Poly Synth"),
            (DeviceKind::MlM1, "ML-M1"),
            (DeviceKind::MlP8, "ML-P8"),
            (DeviceKind::Ds01, "DS-01"),
            (DeviceKind::AuxIn, "Aux In"),
        ] {
            assert_eq!(kind.label(), label);
        }
    }

    /// The default name is the label plus a one-based slot number, and it is
    /// the *same* name whichever path asked for it -- which is the property
    /// the two drifted copies did not have.
    #[test]
    fn a_default_channel_name_is_its_label_and_its_slot() {
        assert_eq!(DeviceKind::DrumSynth.default_channel_name(0), "Drum Synth 1");
        assert_eq!(DeviceKind::MlP8.default_channel_name(7), "ML-P8 8");
    }

    /// Nothing soloed silences nothing, which is the case that has to cost
    /// nothing -- it is derived every pump tick.
    #[test]
    fn a_bank_with_no_channel_soloed_silences_nothing() {
        assert_eq!(solo_silenced([false, false, false]), [false; MAX_CHANNELS]);
    }

    /// A solo silences the other channels and not itself, and two solos add
    /// rather than fight.
    #[test]
    fn soloing_channels_silences_the_ones_left_out() {
        let silenced = solo_silenced([false, true, false, true]);
        assert!(silenced[0], "not soloed, so silenced");
        assert!(!silenced[1], "the soloed channel");
        assert!(silenced[2], "not soloed, so silenced");
        assert!(!silenced[3], "the other soloed channel");
    }

    /// A seat past the end of the bank is not a channel, so it is neither
    /// soloed nor silenced. Read the other way round this is what keeps a
    /// solo from silencing channels that do not exist, which is how a bank
    /// that grew would have arrived already quiet.
    #[test]
    fn a_solo_says_nothing_about_seats_the_bank_does_not_have() {
        let silenced = solo_silenced([true, false]);
        assert!(!silenced[0]);
        assert!(silenced[1]);
        assert!(
            silenced[2..].iter().all(|&silenced| !silenced),
            "seats past the bank were silenced"
        );
    }

    /// A channel's own solo is what is stored; what it silences is not. A
    /// song written before channel solo existed loads with none, and one
    /// with nothing soloed saves without the field.
    #[test]
    fn an_unsoloed_channel_is_absent_from_the_manifest() {
        let mut channel = Channel::new("Kick", DeviceKind::Sampler);
        let text = toml::to_string(&channel).expect("a channel serializes");
        assert!(
            !text.contains("solo"),
            "an unsoloed channel wrote a field: {text}"
        );
        let back: Channel = toml::from_str(&text).expect("and reads back");
        assert!(!back.solo);

        channel.solo = true;
        let text = toml::to_string(&channel).expect("a soloed channel serializes");
        assert!(
            text.contains("solo = true"),
            "a soloed channel wrote nothing: {text}"
        );
        let back: Channel = toml::from_str(&text).expect("and reads back");
        assert!(back.solo);
    }
}
