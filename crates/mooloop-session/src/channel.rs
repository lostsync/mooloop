//! One channel's live state.
//!
//! The project model as the application edits it, rather than as it is
//! serialized: decoded audio, per-kind generator parameters kept side by side
//! so switching source never loses the others, and the pattern-indexed note
//! and automation banks.

use mooloop_core::{
    AutomationLane, DeviceKind, Ds01Params, EffectSlotState, GeneratorParams, MlM1Params,
    MlP8Params, ModRack, MonoSynthParams, NoteEvent, NoteId, PolySynthParams, Project,
    ProjectChannel, SampleCommit, SampleReference, SamplerParams, SliceMap, DrumSynthParams,
    MASTER_BUS, MAX_CHANNELS,
};
use mooloop_dsp::SampleData;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ChannelState {
    pub name: String,
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
    pub poly_params: PolySynthParams,
    pub sample_name: String,
    pub sample_description: String,
    pub sample_duration: f32,
    pub sample_path: Option<PathBuf>,
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
            DeviceKind::DrumSynth => GeneratorParams::DrumSynth,
        }
    }

    /// A brand new sampler channel is silent and empty until a sample is
    /// loaded or a project assigns one.
    pub fn new(index: usize) -> Self {
        Self {
            name: format!("Sampler {}", index + 1),
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
            poly_params: PolySynthParams::default(),
            sample_name: String::new(),
            sample_description: String::new(),
            sample_duration: 0.0,
            sample_path: None,
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
            modulation: ModRack::default(),
            bus: MASTER_BUS,
        }
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
}
