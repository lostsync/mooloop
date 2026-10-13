//! The ML-M1 factory patches play as they did when they carried a rack
//! (song patch step 09): each patch, saved by the bank as a patch fragment
//! and loaded onto a channel, renders sample for sample what the same
//! patch's rack lifted onto that channel renders.

use std::path::Path;

use mooloop_core::{mlm1_factory, ChannelSetup, NoteEvent, PatternPlacement, Project, TICKS_PER_STEP};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};
use mooloop_project::{list_presets, load_bundle, seed_mlm1_bank, LoadedDocument};

const SAMPLE_RATE: u32 = 48_000;

/// A one-pattern song whose first channel plays `setup` over a bar of
/// overlapping notes, so every kind of module hears notes start and end.
fn song(setup: ChannelSetup) -> Project {
    let mut project = Project::default();
    project.assign_channel_ids();
    project.replace_channel_setup(0, setup);
    let steps = u32::from(project.pattern_lengths[0]);
    project.channels[0].notes[0] = (0..steps)
        .filter(|step| step % 3 == 0)
        .map(|step| NoteEvent::new(step + 1, step * TICKS_PER_STEP, TICKS_PER_STEP * 4, (40 + step * 5 % 30) as u8, 100))
        .collect();
    project.channels[0].next_note_id = steps + 1;
    project.playlist = vec![PatternPlacement {
        pattern: 0,
        start_tick: 0,
    }];
    project
}

fn render(project: &Project, path: &Path) -> Vec<f32> {
    OfflineRenderer::render(
        project,
        &[],
        SAMPLE_RATE,
        &ExportSpec {
            path: path.to_path_buf(),
            scope: RenderScope::Song,
            tail_seconds: 0.5,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("it renders");
    hound::WavReader::open(path)
        .expect("readable")
        .samples::<f32>()
        .map(|sample| sample.expect("a sample"))
        .collect()
}

#[test]
fn every_factory_patch_renders_as_its_rack_did() {
    let temp = tempfile::tempdir().unwrap();
    let bank = temp.path().join("channels");
    seed_mlm1_bank(&bank).unwrap();
    let listed = list_presets(&bank);
    let mut carried = 0;
    for patch in mlm1_factory::patches() {
        let summary = listed
            .iter()
            .find(|found| found.name == patch.name)
            .unwrap_or_else(|| panic!("{} was not seeded", patch.name));
        let LoadedDocument::Channel(saved) = load_bundle(&summary.path).unwrap().document else {
            panic!("{} did not load as a channel", patch.name);
        };
        assert!(saved.preset_modulation.is_none(), "{} saved a rack", patch.name);
        carried += usize::from(saved.preset_patch.is_some());

        let mut rack = ChannelSetup::mlm1_with_params(patch.name, patch.params);
        rack.preset_modulation = (!patch.modulation.is_empty()).then_some(patch.modulation);

        let fragment = render(&song(*saved), &temp.path().join("fragment.wav"));
        let lifted = render(&song(rack), &temp.path().join("rack.wav"));
        assert!(fragment.iter().any(|sample| *sample != 0.0), "{} is silent", patch.name);
        assert_eq!(fragment.len(), lifted.len(), "{} changed length", patch.name);
        let first = fragment.iter().zip(&lifted).position(|(a, b)| a.to_bits() != b.to_bits());
        assert_eq!(first, None, "{} differs from its rack from sample {first:?}", patch.name);
    }
    assert!(carried > 0, "no factory patch carries modulation to test");
}
