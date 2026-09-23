//! One channel's live state.
//!
//! The project model as the application edits it, rather than as it is
//! serialized: decoded audio, the running generator's parameters, and the
//! pattern-indexed note and automation banks.

use mooloop_core::{
    AutomationLane, AuxInParams, ChannelId, DeviceKind, Ds01Params, EffectSlotState,
    GeneratorParams,
    MlM1Params,
    MlP8Params, ModRack, MonoSynthParams, NoteEvent, NoteId, PolySynthParams, Project,
    ProjectChannel, SampleCommit, SampleReference, SamplerParams, SliceMap, DrumSynthParams,
    MASTER_BUS, MAX_CHANNELS, MAX_NOTES_PER_CHANNEL_PATTERN,
};
use mooloop_dsp::SampleData;
use std::path::PathBuf;
use std::sync::Arc;

/// One typed reader and one typed writer per generator kind, for the device
/// faces, which each know which kind they draw.
///
/// The reader gives the kind's defaults when the channel runs another kind:
/// a face that is not showing still gets drawn from something, which is what
/// the eight separate fields used to give it. The writer gives `None`, and
/// says so in the log: a write aimed at a kind the channel is not running
/// used to land in a field nothing read, silently, and now it lands nowhere
/// but not silently (the Buffer lesson in `AGENTS.md`, "Trace the press").
macro_rules! typed_generator_access {
    ($($variant:ident, $params:ty, $get:ident, $get_mut:ident;)*) => {
        $(
            pub fn $get(&self) -> $params {
                match self.generator {
                    GeneratorParams::$variant(params) => params,
                    _ => <$params>::default(),
                }
            }

            pub fn $get_mut(&mut self) -> Option<&mut $params> {
                let kind = self.kind();
                match &mut self.generator {
                    GeneratorParams::$variant(params) => Some(params),
                    _ => {
                        mooloop_core::log_warn!(
                            "session",
                            "{} write refused: the channel runs {kind:?}",
                            stringify!($variant)
                        );
                        None
                    }
                }
            }
        )*
    };
}

pub struct ChannelState {
    /// This channel's durable identity, carried in both directions so a mint
    /// is never rewound by a round trip through the document -- the same
    /// reason and the same treatment `next_device_id` below gets.
    ///
    /// [`ChannelId::UNASSIGNED`] until the channel joins a song: a
    /// [`ChannelState::new`] built for a rack that is about to add it is not
    /// addressable yet, and `Session::add_channel` mints on the way in.
    pub id: ChannelId,
    pub name: String,
    /// Which MIDI input, and which MIDI channel on it, plays this channel.
    pub midi_input: mooloop_core::ChannelMidiInput,
    /// Where this channel records audio from: the AUDIO row, independent of
    /// `midi_input`. Kept whatever the channel's device.
    pub audio_input: mooloop_core::AudioInputSource,
    /// The colour the user gave this channel, or `None` for one nobody has
    /// coloured. Content, like the name: it survives a save and it survives a
    /// change of source device.
    pub color: Option<mooloop_core::ProjectColor>,
    pub muted: bool,
    /// Whether this channel is soloed. What that *silences* is derived by the
    /// pump's `sync_channel_solo` and never stored here, for the reason a
    /// track's is: it is a property of the whole bank.
    pub solo: bool,
    pub volume: f32,
    pub pan: f32,
    /// The running generator's parameters, and so which generator it is
    /// ([`Self::kind`]).
    ///
    /// One block, not one per kind (MOO-192). Eight fields used to sit here
    /// "so switching sources does not lose the others", but nothing ever
    /// read the others: a source change resets the kind it switches to, and
    /// every install rebuilds the channel from a document that carries only
    /// the running kind (`tests/source_switch.rs`). Crate-private, and the
    /// kind is read from it rather than stored beside it, so the two cannot
    /// disagree.
    pub(crate) generator: GeneratorParams,
    pub sample_name: String,
    pub sample_description: String,
    pub sample_duration: f32,
    pub sample_path: Option<PathBuf>,
    /// The folder the sample was *browsed from*, as a path to the file that
    /// was picked there. Where `sample_path` says where the bytes are now,
    /// this says where "next sample" should look.
    ///
    /// They are the same thing until a song is saved with Embed Assets on,
    /// which is the default: the app writes the resolved bundle paths back
    /// into the live session, and `sample_path.parent()` then names
    /// `<song>-assets/samples/`. With four sampler channels embedded that
    /// directory is `00-kick.wav, 01-snare.wav, 02-hat.wav, 03-clap.wav`, so
    /// "next sample" on the kick loaded the snare out of the bundle -- a kick
    /// picked from a fifty-file drum folder lost the other forty-nine the
    /// first time the song was saved.
    ///
    /// So this is the field the save's write-back must not touch, which is
    /// the whole of why it is a second field rather than a change to what is
    /// stored: only `project_snapshot` needs the bundle-relative form.
    pub sample_browse_path: Option<PathBuf>,
    pub sample_embedded: bool,
    /// The decoded source. Authoritative, and never what the engine plays
    /// once a stretch has been committed.
    pub sample_data: Option<Arc<SampleData>>,
    /// The committed render, when there is one. This is what is published,
    /// drawn, and measured against, so the waveform, the markers, and the
    /// start/end fractions all live in one coordinate system.
    pub committed_sample: Option<Arc<SampleData>>,
    /// What the committed render was baked from, and what the editor looked
    /// like before it. `None` means the published buffer is the source.
    pub commit: Option<Box<SampleCommit>>,
    /// Slice boundaries into the *published* buffer, in frames, so they move
    /// with the waveform under any zoom.
    pub slices: SliceMap,
    /// The Record page's clip settings, saved with the sampler.
    pub record: mooloop_core::SamplerRecord,
    pub waveform: Vec<f32>,
    pub can_previous_sample: bool,
    pub can_next_sample: bool,
    pub notes: Vec<Vec<NoteEvent>>,
    /// Pattern-indexed automation lanes, parallel to `notes`. A lane is kept
    /// even when the editor is not showing it, so switching the visible lane
    /// never destroys what is behind it.
    pub automation: Vec<Vec<AutomationLane>>,
    pub next_note_id: NoteId,
    pub effects: Vec<EffectSlotState>,
    /// Next device identity to mint for `effects`. Mirrors
    /// `ChannelSetup::next_device_id`, and travels with the chain in both
    /// directions so a mint is never rewound by a round trip through the
    /// document.
    pub next_device_id: u32,
    pub modulation: ModRack,
    /// Mixer bus this channel feeds; 0 is the master.
    pub bus: u8,
}

impl ChannelState {
    /// What this channel actually plays and the editor actually draws: the
    /// committed render when there is one, the decoded source otherwise.
    ///
    /// Every measurement against the audio -- the waveform, zero-crossing
    /// snapping, the fit-to-tempo guess, the frame count the markers are
    /// expressed in -- goes through here, so there is one coordinate system
    /// rather than two.
    pub fn published_sample(&self) -> Option<&Arc<SampleData>> {
        self.committed_sample.as_ref().or(self.sample_data.as_ref())
    }

    /// Which generator this channel runs.
    pub fn kind(&self) -> DeviceKind {
        self.generator.kind()
    }

    /// This channel's generator parameters in their addressable form.
    pub fn generator_params(&self) -> GeneratorParams {
        self.generator
    }

    /// The generator's parameters, mutably, whatever its kind.
    pub fn generator_params_mut(&mut self) -> &mut GeneratorParams {
        &mut self.generator
    }

    /// Replaces the generator, kind and all. A source change passes the new
    /// kind's defaults; an install passes what the document carries.
    pub fn set_generator(&mut self, params: GeneratorParams) {
        self.generator = params;
    }

    /// Write one of the current generator's parameters by descriptor id,
    /// returning the value that was actually stored -- the descriptor's own
    /// clamp, not the one asked for.
    pub fn set_generator_param(&mut self, id: u32, value: f32) -> Option<f32> {
        self.generator.set(id, value)
    }

    typed_generator_access! {
        Sampler, SamplerParams, sampler_params, sampler_params_mut;
        DrumSynth, DrumSynthParams, drum_params, drum_params_mut;
        MonoSynth, MonoSynthParams, mono_params, mono_params_mut;
        PolySynth, PolySynthParams, poly_params, poly_params_mut;
        MlM1, MlM1Params, mlm1_params, mlm1_params_mut;
        MlP8, MlP8Params, mlp8_params, mlp8_params_mut;
        Ds01, Ds01Params, ds01_params, ds01_params_mut;
        AuxIn, AuxInParams, aux_in_params, aux_in_params_mut;
    }

    /// A brand new sampler channel is silent and empty until a sample is
    /// loaded or a project assigns one.
    pub fn new(index: usize) -> Self {
        Self {
            id: ChannelId::UNASSIGNED,
            name: DeviceKind::Sampler.default_channel_name(index),
            midi_input: mooloop_core::ChannelMidiInput::default(),
            audio_input: mooloop_core::AudioInputSource::Off,
            color: None,
            muted: false,
            solo: false,
            volume: mooloop_core::DEFAULT_CHANNEL_VOLUME,
            pan: 0.0,
            generator: DeviceKind::Sampler.default_generator_params(),
            sample_name: String::new(),
            sample_description: String::new(),
            sample_duration: 0.0,
            sample_path: None,
            sample_browse_path: None,
            sample_embedded: false,
            sample_data: None,
            committed_sample: None,
            commit: None,
            slices: SliceMap::default(),
            record: mooloop_core::SamplerRecord::default(),
            waveform: Vec::new(),
            can_previous_sample: false,
            can_next_sample: false,
            notes: vec![Vec::new()],
            automation: vec![Vec::new()],
            next_note_id: 1,
            effects: Vec::new(),
            next_device_id: 0,
            modulation: ModRack::default(),
            bus: MASTER_BUS,
        }
    }

    /// This channel wearing `id`, for the constructors, which build a channel
    /// before it has joined a rack.
    pub fn with_id(mut self, id: ChannelId) -> Self {
        self.id = id;
        self
    }

    /// Whether `pattern` can take `count` more notes.
    ///
    /// The engine stores [`MAX_NOTES_PER_CHANNEL_PATTERN`] notes per channel
    /// per pattern, preallocated so the callback never grows them
    /// (`docs/CAPACITY_POLICY.md`), and refuses the rest -- so a note past it
    /// would be drawn and silent, and the save-time integrity pass would
    /// refuse the song (MOO-133). Every verb that adds notes asks this first.
    pub fn has_room_for(&self, pattern: usize, count: usize) -> bool {
        self.notes[pattern].len().saturating_add(count) <= MAX_NOTES_PER_CHANNEL_PATTERN
    }

    /// Adds a note, or `None` when the pattern is full
    /// ([`Self::has_room_for`]).
    pub fn create_note(
        &mut self,
        pattern: usize,
        start_tick: u32,
        duration_ticks: u32,
        note: u8,
    ) -> Option<NoteEvent> {
        if !self.has_room_for(pattern, 1) {
            return None;
        }
        let event = NoteEvent::new(self.next_note_id, start_tick, duration_ticks, note, 100);
        self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
        self.notes[pattern].push(event);
        self.notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
        Some(event)
    }
}

/// In-memory channel clipboard. It intentionally keeps decoded sample data
/// alongside the serializable channel so pasting never needs to re-read audio
/// on the UI thread.
#[derive(Clone)]
pub struct ChannelClipboard {
    pub channel: ProjectChannel,
    pub sample: Option<Arc<SampleData>>,
}

pub fn copied_channel_name(project: &Project, source_name: &str) -> String {
    let base = if source_name.trim().is_empty() {
        "Channel".to_string()
    } else {
        format!("{source_name} copy")
    };
    if !project
        .channels
        .iter()
        .any(|channel| channel.setup.channel.name == base)
    {
        return base;
    }
    for suffix in 2..=MAX_CHANNELS {
        let candidate = format!("{base} {suffix}");
        if !project
            .channels
            .iter()
            .any(|channel| channel.setup.channel.name == candidate)
        {
            return candidate;
        }
    }
    base
}

pub fn apply_sample_references(
    channels: &mut [ChannelState],
    references: impl IntoIterator<Item = Option<SampleReference>>,
) {
    for (channel, sample) in channels.iter_mut().zip(references) {
        match sample {
            // `sample_browse_path` is deliberately untouched on both arms:
            // this runs after a save, and where the bytes went is not where
            // the user was browsing.
            Some(SampleReference::Builtin { .. } | SampleReference::Empty) => {
                channel.sample_path = None;
                channel.sample_embedded = false;
            }
            Some(SampleReference::File { path, embedded }) => {
                channel.sample_path = Some(path);
                channel.sample_embedded = embedded;
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{DEFAULT_NOTE_DURATION_TICKS, TICKS_PER_64TH};

    /// A new channel starts at one volume wherever it is made. Shaped
    /// against the tree where the session's channel, the strip's volume
    /// descriptor and the engine's reset said 0.8 while core said 1.0: a
    /// channel added from the toolbar started 1.9 dB under the gain
    /// contract's calibration, and double-clicking its knob, which resets to
    /// 0 dB, made it louder. Unity is pinned too, because the knob's
    /// double-click and the mixer fader's reset are both written as unity in
    /// the markup.
    #[test]
    fn a_new_channel_starts_at_the_one_default_volume() {
        use mooloop_core::{Channel, DeviceKind, DEFAULT_CHANNEL_VOLUME, STRIP_DESCRIPTORS};
        assert_eq!(DEFAULT_CHANNEL_VOLUME, 1.0);
        assert_eq!(ChannelState::new(0).volume, DEFAULT_CHANNEL_VOLUME);
        assert_eq!(
            Channel::new("Kick", DeviceKind::Sampler).volume,
            DEFAULT_CHANNEL_VOLUME
        );
        let volume = STRIP_DESCRIPTORS
            .iter()
            .find(|descriptor| descriptor.id == mooloop_core::STRIP_PARAM_VOLUME)
            .expect("the strip describes its volume");
        assert_eq!(volume.default, DEFAULT_CHANNEL_VOLUME);
    }

    #[test]
    fn copied_channel_names_are_readable_and_unique() {
        let mut project = Project::default();
        project.channels[0].setup.channel.name = "Kick".into();
        assert_eq!(copied_channel_name(&project, "Kick"), "Kick copy");

        let mut first_copy = project.channels[0].clone();
        first_copy.setup.channel.name = "Kick copy".into();
        project.channels.push(first_copy);
        assert_eq!(copied_channel_name(&project, "Kick"), "Kick copy 2");
    }

    #[test]
    fn channel_assigns_stable_note_ids() {
        let mut channel = ChannelState::new(0);
        let first = channel.create_note(0, 0, DEFAULT_NOTE_DURATION_TICKS, 60).expect("room");
        let second = channel.create_note(0, TICKS_PER_64TH, DEFAULT_NOTE_DURATION_TICKS, 62).expect("room");
        assert_ne!(first.id, second.id);
        assert_eq!(channel.notes[0][0].id, first.id);
        assert_eq!(channel.notes[0][1].id, second.id);
    }

    #[test]
    fn saved_bundle_sample_paths_replace_external_paths() {
        let mut channel = ChannelState::new(0);
        channel.sample_path = Some(PathBuf::from("/samples/source.wav"));

        apply_sample_references(
            std::slice::from_mut(&mut channel),
            [Some(SampleReference::File {
                path: PathBuf::from("/songs/beat.mooloop-assets/samples/00-source.wav"),
                embedded: true,
            })],
        );

        assert_eq!(
            channel.sample_path,
            Some(PathBuf::from(
                "/songs/beat.mooloop-assets/samples/00-source.wav"
            ))
        );
        assert!(channel.sample_embedded);
    }

    /// **A save does not move where "next sample" is looking.**
    ///
    /// The write-back above is correct and necessary -- the bytes really are
    /// in the bundle now -- and it used to take the browse folder with it,
    /// because there was only one path. So a kick picked out of a fifty-file
    /// drum folder, saved once with Embed Assets on (the default), had arrows
    /// that walked `<song>-assets/samples/` instead: with four sampler
    /// channels embedded that directory is `00-kick.wav, 01-snare.wav,
    /// 02-hat.wav, 03-clap.wav`, so "next sample" on the kick loaded the
    /// snare out of the song's own bundle.
    #[test]
    fn a_save_does_not_move_the_browse_origin() {
        let mut channel = ChannelState::new(0);
        channel.sample_path = Some(PathBuf::from("/samples/drums/kick.wav"));
        channel.sample_browse_path = Some(PathBuf::from("/samples/drums/kick.wav"));

        apply_sample_references(
            std::slice::from_mut(&mut channel),
            [Some(SampleReference::File {
                path: PathBuf::from("/songs/beat.mooloop-assets/samples/00-kick.wav"),
                embedded: true,
            })],
        );

        assert_eq!(
            channel.sample_browse_path,
            Some(PathBuf::from("/samples/drums/kick.wav")),
            "the save moved the folder the arrows walk"
        );

        // And a channel whose sample was cleared keeps nothing to walk.
        apply_sample_references(
            std::slice::from_mut(&mut channel),
            [Some(SampleReference::Empty)],
        );
        assert_eq!(
            channel.sample_browse_path,
            Some(PathBuf::from("/samples/drums/kick.wav")),
            "clearing the reference is still not a browse"
        );
        assert_eq!(channel.sample_path, None);
    }
}
