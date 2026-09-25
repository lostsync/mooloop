//! Step 10's listening case (MOO-85): a CLAP instrument as a channel's
//! source, playing a one-bar melody from the channel's pattern, rendered
//! the way the app exports, and saved as a song Adam can open.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/clap-instrument \
//!   cargo run -p mooloop-session --example clap_instrument_case -- target/clap-instrument
//! ```
//!
//! The instrument is the in-repo test sine (`mooloop.test.sine`), the one
//! CLAP instrument every machine has. `--real <id>` also opens an installed
//! instrument by id through this machine's scan cache
//! (`~/.config/mooloop/plugins.toml`), plays the same melody through it and
//! writes `real.wav`: run the built binary on the laptop for that
//! (`--real in.lsp-plug.sampler_stereo`). Writes `instrument.wav` (the export),
//! `missing.wav` (the same song with the plugin missing: silence), and
//! `instrument.mooloop` (the song; it names the test sine by id, so it
//! plays in the app only where the scanner has found the test plugin).
//! Prints each note's first sounding frame against where the pattern puts
//! it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mooloop_core::{NoteEvent, PluginFormat, PluginRef, PluginSlotId, Project, ProjectChannel};
use mooloop_dsp::{AudioNode, SourceNode};
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::ClapOpener;
use mooloop_plugin_host::scan::PluginCache;
use mooloop_session::session::Session;
use mooloop_test_plugin as test_plugin;

const RATE: u32 = 48_000;
const FRAMES_PER_STEP: usize = 6_000;
const MELODY: [(u32, u32, u8); 6] = [(0, 2, 57), (3, 1, 60), (4, 2, 64), (8, 3, 69), (12, 1, 67), (14, 4, 64)];

fn library() -> PathBuf {
    let exe = std::env::current_exe().expect("the example has a path");
    let dir = exe.parent().expect("the example is in a directory");
    let name = format!(
        "{}mooloop_test_plugin{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let up = dir.parent().unwrap_or(dir);
    for candidate in [dir.to_path_buf(), up.to_path_buf(), up.join("deps")] {
        let path = candidate.join(&name);
        if path.is_file() {
            return path;
        }
    }
    panic!("{name} is not next to {}", exe.display());
}

fn cache(library: &Path) -> PluginCache {
    let path = library.display().to_string().replace('\\', "/");
    PluginCache::from_toml(&format!(
        "version = 1\n\n[[file]]\npath = \"{path}\"\nmodified-ns = 0\nsize = 0\n\n\
         [[file.plugin]]\npath = \"{path}\"\nformat = \"clap\"\nid = \"{id}\"\nname = \"Test Sine\"\n\
         vendor = \"{vendor}\"\nfeatures = [\"instrument\"]\naudio-inputs = []\naudio-outputs = [2]\nnote-inputs = 1\n",
        id = test_plugin::SINE_ID,
        vendor = test_plugin::VENDOR,
    ))
    .expect("a cache the scanner could have written")
}

fn sine() -> PluginRef {
    PluginRef {
        format: PluginFormat::Clap,
        id: test_plugin::SINE_ID.to_owned(),
        name: "Test Sine".to_owned(),
        vendor: test_plugin::VENDOR.to_owned(),
        version: String::new(),
    }
}

fn melody() -> Project {
    let mut project = Project {
        bpm: 120,
        ..Project::default()
    };
    project.channels.clear();
    project.pattern_lengths[0] = 16;
    let mut channel = ProjectChannel::drum_synth(0, 1);
    channel.setup.channel.volume = 0.8;
    for (id, (step, steps, key)) in MELODY.into_iter().enumerate() {
        channel.notes[0].push(NoteEvent::new(id as u32 + 1, step * 24, steps * 24 - 12, key, 100));
    }
    project.channels.push(channel);
    project.assign_channel_ids();
    project
}

#[derive(Default)]
struct Engine {
    sources: Vec<Box<dyn SourceNode + Send>>,
    nodes: Vec<Box<dyn AudioNode + Send>>,
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallSource { node, .. } => self.sources.push(node),
            StructuralCommand::HostSourceProcessor { node: Some(node), .. } => self.nodes.push(node),
            _ => {}
        }
        true
    }
    fn send_deferred(&mut self, _cmd: mooloop_core::EngineCommand, _when: mooloop_core::MusicalEdge) -> bool {
        true
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
}

fn render(project: &Project, plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>, path: &Path) -> Vec<f32> {
    let spec = ExportSpec {
        path: path.to_path_buf(),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 0.5,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    std::thread::scope(|scope| {
        scope
            .spawn(|| OfflineRenderer::render_with_plugins(project, &[], RATE, &spec, &ExportProgress::new(), plugins))
            .join()
            .expect("the export thread did not panic")
    })
    .expect("the export succeeds");
    hound::WavReader::open(path)
        .expect("a WAV")
        .into_samples::<f32>()
        .map(|sample| sample.expect("a float sample"))
        .collect()
}

/// Play the melody through the installed instrument `id`, found through
/// this machine's scan cache, into `real.wav`.
fn real(id: &str, out: &Path) {
    let home = std::env::var_os("HOME").expect("a home directory");
    let cache_path = Path::new(&home).join(".config/mooloop/plugins.toml");
    let cache = PluginCache::load(&cache_path);
    let Some(found) = cache.plugins().find(|found| found.plugin.id == id) else {
        println!("{id}: not in {}", cache_path.display());
        return;
    };
    println!(
        "{id}: {} by {}, audio in {:?} out {:?}, note inputs {}; as a source: {}",
        found.plugin.name,
        found.plugin.vendor,
        found.audio_inputs,
        found.audio_outputs,
        found.note_inputs,
        found.source_refusal().unwrap_or_else(|| "accepted".into())
    );
    let plugin = found.plugin.clone();
    let song = melody();
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::new(cache_path)));
    session.replace_project(&song, &[]);
    let slot = session.set_plugin_source(0, plugin, &mut Engine::default()).expect("channel 0");
    if let Some(problem) = session.plugin_problem(slot) {
        println!("{id}: not hosted as a source: {problem}");
        return;
    }
    let saved = session.project_snapshot(song.bpm.into(), song.swing_percent.into());
    let plugins = session.export_plugin_processors(RATE);
    let played = render(&saved, plugins, &out.join("real.wav"));
    let peak = played.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    println!("{id}: real.wav peak {peak:.4}{}", if peak == 0.0 { " (silent)" } else { "" });
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let real_id = args.iter().position(|arg| arg == "--real").and_then(|at| args.get(at + 1)).cloned();
    let out = PathBuf::from(
        args.iter()
            .find(|arg| !arg.starts_with("--") && Some(*arg) != real_id.as_ref())
            .cloned()
            .unwrap_or_else(|| "target/clap-instrument".into()),
    );
    std::fs::create_dir_all(&out).expect("the output directory");
    let song = melody();

    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::with_cache(cache(&library()))));
    session.replace_project(&song, &[]);
    let mut engine = Engine::default();
    let slot = session.set_plugin_source(0, sine(), &mut engine).expect("channel 0");
    if let Some(problem) = session.plugin_problem(slot) {
        panic!("the test sine is not hosted: {problem}");
    }
    session.capture_plugin_states();
    let saved = session.project_snapshot(song.bpm.into(), song.swing_percent.into());
    mooloop_project::save_song(&out.join("instrument.mooloop"), &saved, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");

    let plugins = session.export_plugin_processors(RATE);
    let played = render(&saved, plugins, &out.join("instrument.wav"));
    let missing = render(&saved, BTreeMap::new(), &out.join("missing.wav"));
    let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    println!("instrument.wav: peak {:.3}; missing.wav: peak {:.3}", peak(&played), peak(&missing));

    // Where each note is first heard, against where the pattern puts it
    // (the sine starts at phase 0, so one frame late for every note).
    let mut quiet = 100;
    let mut heard = Vec::new();
    for (frame, pair) in played.chunks(2).enumerate() {
        if pair[0] == 0.0 {
            quiet += 1;
        } else {
            if quiet >= 100 {
                heard.push(frame);
            }
            quiet = 0;
        }
    }
    for (index, (step, _, key)) in MELODY.iter().enumerate() {
        let wanted = *step as usize * FRAMES_PER_STEP + 1;
        let got = heard.iter().copied().min_by_key(|frame| frame.abs_diff(wanted));
        println!("note {index} (key {key}): pattern frame {wanted}, heard at {got:?}");
    }
    if let Some(id) = real_id {
        real(&id, &out);
    }
}
