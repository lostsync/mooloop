//! One channel's live state.
//!
//! The project model as the application edits it, rather than as it is
//! serialized: decoded audio, per-kind generator parameters kept side by side
//! so switching source never loses the others, and the pattern-indexed note
//! and automation banks.

use mooloop_core::{
    AutomationLane, AuxInParams, ChannelId, DeviceKind, Ds01Params, EffectSlotState,
    GeneratorParams,
    MlM1Params,
    MlP8Params, ModRack, MonoSynthParams, NoteEvent, NoteId, PolySynthParams, Project,
    ProjectChannel, SampleCommit, SampleReference, SamplerParams, SliceMap, DrumSynthParams,
    MASTER_BUS, MAX_CHANNELS,
};
use mooloop_dsp::SampleData;
use std::path::PathBuf;
use std::sync::Arc;

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
    pub kind: DeviceKind,
    pub muted: bool,
    pub volume: f32,
    pub pan: f32,
    pub params: SamplerParams,
    pub drum_params: DrumSynthParams,
    pub mono_params: MonoSynthParams,
    pub mlm1_params: MlM1Params,
    pub mlp8_params: MlP8Params,
    pub ds01_params: Ds01Params,
    pub aux_in_params: AuxInParams,
    pub poly_params: PolySynthParams,
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

    /// This channel's generator parameters in their addressable form. The
    /// `ChannelState` keeps one struct per kind so switching sources does not
    /// lose the others; only the active kind is addressable.
    pub fn generator_params(&self) -> GeneratorParams {
        match self.kind {
            DeviceKind::Sampler => GeneratorParams::Sampler(self.params),
            DeviceKind::MonoSynth => GeneratorParams::MonoSynth(self.mono_params),
            DeviceKind::PolySynth => GeneratorParams::PolySynth(self.poly_params),
            DeviceKind::MlM1 => GeneratorParams::MlM1(self.mlm1_params),
            DeviceKind::MlP8 => GeneratorParams::MlP8(self.mlp8_params),
            DeviceKind::Ds01 => GeneratorParams::Ds01(self.ds01_params),
            DeviceKind::AuxIn => GeneratorParams::AuxIn(self.aux_in_params),
            DeviceKind::DrumSynth => GeneratorParams::DrumSynth(self.drum_params),
        }
    }

    /// Write one of the current generator's parameters by descriptor id,
    /// returning the value that was actually stored -- the descriptor's own
    /// clamp, not the one asked for.
    ///
    /// `generator_params` hands out a copy, because the eight device kinds are
    /// separate fields rather than one box, so a caller cannot write through
    /// it. This puts the copy back on the field the current kind reads from,
    /// which is the only reason it exists.
    pub fn set_generator_param(&mut self, id: u32, value: f32) -> Option<f32> {
        let mut params = self.generator_params();
        let written = params.set(id, value)?;
        match params {
            GeneratorParams::Sampler(params) => self.params = params,
            GeneratorParams::DrumSynth(params) => self.drum_params = params,
            GeneratorParams::MonoSynth(params) => self.mono_params = params,
            GeneratorParams::PolySynth(params) => self.poly_params = params,
            GeneratorParams::MlM1(params) => self.mlm1_params = params,
            GeneratorParams::MlP8(params) => self.mlp8_params = params,
            GeneratorParams::Ds01(params) => self.ds01_params = params,
            GeneratorParams::AuxIn(params) => self.aux_in_params = params,
        }
        Some(written)
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
            kind: DeviceKind::Sampler,
            muted: false,
            volume: 0.8,
            pan: 0.0,
            params: SamplerParams::default(),
            drum_params: DrumSynthParams::default(),
            mono_params: MonoSynthParams::default(),
            mlm1_params: MlM1Params::default(),
            mlp8_params: MlP8Params::default(),
            ds01_params: Ds01Params::default(),
            aux_in_params: AuxInParams::default(),
            poly_params: PolySynthParams::default(),
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

    pub fn create_note(
        &mut self,
        pattern: usize,
        start_tick: u32,
        duration_ticks: u32,
        note: u8,
    ) -> NoteEvent {
        let event = NoteEvent::new(self.next_note_id, start_tick, duration_ticks, note, 100);
        self.next_note_id = self.next_note_id.wrapping_add(1).max(1);
        self.notes[pattern].push(event);
        self.notes[pattern].sort_by_key(|note| (note.start_tick, note.id));
        event
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
        let first = channel.create_note(0, 0, DEFAULT_NOTE_DURATION_TICKS, 60);
        let second = channel.create_note(0, TICKS_PER_64TH, DEFAULT_NOTE_DURATION_TICKS, 62);
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
