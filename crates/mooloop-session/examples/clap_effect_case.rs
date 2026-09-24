//! Step 06's case with a real CLAP effect (`docs/plans/plugin-hosting/06`,
//! MOO-81): a third-party plugin on a drum loop, inserted through the
//! session, saved, reopened with the plugin present and with it missing,
//! and exported, all headless -- no JACK, no window. It writes the WAVs and
//! prints what it measured. **Listening is Adam's**; this is what an agent
//! can check instead.
//!
//! ```text
//! clap_effect_case --plugin /usr/lib64/clap/lsp-plugins.clap \
//!     --id in.lsp-plug.flanger_stereo [--out clap-case]
//! ```
//!
//! The plugin is found the way the app finds it. This binary is its own scan
//! child (`--scan-plugin`, exactly as `mooloop` is), so the library is loaded
//! first in a child process, and the song resolves it through the cache that
//! scan writes. `tests/clap_effect.rs` is the same case with the in-repo test
//! plugin, which is the one CI runs.
//!
//! What it measures:
//! 1. **The plugin is heard**: the export differs from the dry loop.
//! 2. **Deterministic**: two exports are identical.
//! 3. **Offline matches realtime block processing**: the plugin's processor
//!    run over the dry loop in 64-frame blocks (a live callback) and in
//!    512-frame blocks (the export's) gives the same audio.
//! 4. **Reopened present**: the saved song, reopened, exports the same file.
//! 5. **Reopened missing**: it opens, plays the placeholder (the dry loop,
//!    to the last bit), and keeps the plugin's slot unchanged when saved.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mooloop_core::{EffectParams, NoteEvent, PluginSlotId, Project, ProjectChannel};
use mooloop_dsp::{AudioNode, EventList, ProcessContext, StereoBus};
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::{ClapOpener, MAX_FRAMES};
use mooloop_plugin_host::scan::{self, ChildCommand, PluginCache, ScanConfig};
use mooloop_plugin_host::{AudioConfig, Lifeline, PluginOpener};
use mooloop_session::session::Session;

const RATE: u32 = 48_000;

struct Args {
    plugin: PathBuf,
    id: String,
    out: PathBuf,
}

fn args() -> Args {
    let mut plugin = None;
    let mut id = None;
    let mut out = PathBuf::from("clap-case");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--plugin" => plugin = args.next().map(PathBuf::from),
            "--id" => id = args.next(),
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let (Some(plugin), Some(id)) = (plugin, id) else {
        eprintln!("usage: clap_effect_case --plugin <file.clap> --id <clap id> [--out <dir>]");
        std::process::exit(2);
    };
    Args { plugin, id, out }
}

/// Takes every command and holds every node, as the engine would.
#[derive(Default)]
struct Engine {
    nodes: Vec<Box<dyn AudioNode + Send>>,
}

impl CommandSink for Engine {
    fn send(&mut self, _cmd: mooloop_core::EngineCommand) -> bool {
        true
    }
    fn send_structural(&mut self, cmd: StructuralCommand) -> bool {
        match cmd {
            StructuralCommand::InstallEffect { node, .. }
            | StructuralCommand::ReplaceEffect { node, .. } => self.nodes.push(node),
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

/// Two bars of a drum loop on two drum-synth channels: kick, snare, hats.
fn drum_loop() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = 32;
    let hits: [&[(u32, u8)]; 2] = [
        &[(0, 36), (8, 38), (16, 36), (22, 36), (24, 38)],
        &[(0, 42), (2, 42), (4, 42), (6, 42), (10, 42), (12, 42), (14, 42), (18, 42), (20, 42), (26, 42), (28, 42), (30, 42)],
    ];
    for (index, hits) in hits.iter().enumerate() {
        let mut channel = ProjectChannel::drum_synth(index, 1);
        channel.setup.channel.volume = 0.5;
        for (n, &(step, pitch)) in hits.iter().enumerate() {
            channel.notes[0].push(NoteEvent::new(n as u32 + 1, step * 24, 12, pitch, 110));
        }
        project.channels.push(channel);
    }
    project.assign_channel_ids();
    project
}

fn snapshot(session: &Session) -> Project {
    let song = drum_loop();
    session.project_snapshot(song.bpm.into(), song.swing_percent.into())
}

fn render(
    project: &Project,
    plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>,
    path: &Path,
) -> Vec<f32> {
    let spec = ExportSpec {
        path: path.to_path_buf(),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 2.0,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    let summary = std::thread::scope(|scope| {
        scope
            .spawn(|| OfflineRenderer::render_with_plugins(project, &[], RATE, &spec, &ExportProgress::new(), plugins))
            .join()
            .expect("the export thread did not panic")
    })
    .expect("the export succeeds");
    println!(
        "  wrote {} ({} frames, {} of tail, {} overs, {} non-finite)",
        path.display(),
        summary.total_frames,
        summary.tail_frames,
        summary.overs,
        summary.non_finite_samples
    );
    hound::WavReader::open(path)
        .expect("a WAV")
        .into_samples::<f32>()
        .map(|sample| sample.expect("a float sample"))
        .collect()
}

fn export(session: &mut Session, path: &Path) -> Vec<f32> {
    let plugins = session.export_plugin_processors(RATE);
    render(&snapshot(session), plugins, path)
}

fn rms_db(samples: &[f32]) -> f64 {
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    10.0 * (sum / samples.len().max(1) as f64).max(1e-30).log10()
}

fn worst(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::INFINITY;
    }
    a.iter().zip(b).fold(0.0f32, |w, (x, y)| w.max((x - y).abs()))
}

/// Run `input` (interleaved stereo) through `node` in `block`-sized blocks,
/// on a thread of its own, the way the engine hands a processor audio.
fn process_in_blocks(mut node: Box<dyn AudioNode + Send>, input: &[f32], block: usize) -> Vec<f32> {
    let frames = input.len() / 2;
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                let mut bus = StereoBus::with_capacity(block);
                let events = EventList::empty();
                let mut out = Vec::with_capacity(input.len());
                let mut done = 0;
                while done < frames {
                    let n = block.min(frames - done);
                    for frame in 0..n {
                        bus.l[frame] = input[(done + frame) * 2];
                        bus.r[frame] = input[(done + frame) * 2 + 1];
                    }
                    let ctx = ProcessContext {
                        sample_rate: RATE,
                        frames: n,
                        playing: true,
                        bpm: 120.0,
                        position_ticks: done as f64 * 96.0 * 2.0 / f64::from(RATE),
                        position_frames: done as u64,
                    };
                    node.process(&ctx, &mut bus, &events, None);
                    for frame in 0..n {
                        out.push(bus.l[frame]);
                        out.push(bus.r[frame]);
                    }
                    done += n;
                }
                out
            })
            .join()
            .expect("the processing thread did not panic")
    })
}

fn verdict(ok: bool) -> &'static str {
    if ok {
        "PASS"
    } else {
        "FAIL"
    }
}

fn main() {
    // The scan child, when this binary is launched as one.
    if let Some(status) = scan::run_child_from_args() {
        std::process::exit(status);
    }
    let args = args();
    std::fs::create_dir_all(&args.out).expect("the output directory");
    let cache_path = args.out.join("plugins.toml");

    // 1. Scan the plugin's directory out of process, as the app does.
    let dir = args.plugin.parent().expect("the plugin is in a directory").to_path_buf();
    let config = ScanConfig {
        search_paths: vec![dir],
        timeout: Duration::from_secs(60),
        child: ChildCommand::current_exe().expect("this binary's path"),
    };
    let mut cache = PluginCache::load(&cache_path);
    let summary = scan::scan(&config, &mut cache, |_, _, path| println!("  scanning {}", path.display()));
    cache.save(&cache_path).expect("the cache saves");
    println!("scan: {summary:?}");
    let Some(found) = cache.plugins().find(|found| found.plugin.id == args.id).cloned() else {
        eprintln!("{} is not in {}", args.id, args.plugin.display());
        std::process::exit(1);
    };
    println!(
        "plugin: {} ({}) {} in {:?} out {:?}",
        found.plugin.name, found.plugin.vendor, found.plugin.version, found.audio_inputs, found.audio_outputs
    );
    let reference = found.plugin.clone();

    // 2. The dry loop, and the session with the effect inserted on the
    // kick-and-snare channel.
    let dry_song = drum_loop();
    let dry = render(&dry_song, BTreeMap::new(), &args.out.join("dry.wav"));
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::new(cache_path.clone())));
    session.replace_project(&dry_song, &[]);
    let mut engine = Engine::default();
    let inserted = session
        .insert_plugin_effect(reference.clone(), 0, &mut engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    if let Some(problem) = session.plugin_problem(slot) {
        eprintln!("the plugin could not be hosted: {problem}");
        std::process::exit(1);
    }
    let latency = session.plugin_rack.latency_frames(slot).unwrap_or(0);
    println!(
        "hosted: {} parameters, latency {latency} frames",
        session.plugins[&slot].params.len()
    );

    // 3. Exported twice.
    let wet = export(&mut session, &args.out.join("wet.wav"));
    let again = export(&mut session, &args.out.join("wet-again.wav"));
    let heard = wet != dry;
    let deterministic = wet == again;
    println!(
        "[{}] heard: dry {:.2} dB RMS, wet {:.2} dB RMS, largest difference {:.6}",
        verdict(heard),
        rms_db(&dry),
        rms_db(&wet),
        worst(&wet, &dry)
    );
    println!("[{}] deterministic: two exports identical", verdict(deterministic));

    // 4. Offline against realtime block processing: the processor over the
    // dry loop in live-sized and export-sized blocks.
    let mut opener = ClapOpener::new(cache_path.clone());
    let state = session.plugins[&slot].state.0.clone();
    let config = AudioConfig {
        sample_rate: RATE,
        max_frames: MAX_FRAMES,
    };
    let mut blocks = Vec::new();
    for block in [64usize, 512, 333] {
        let mut instance = opener.open(&reference, &state, config).expect("a second instance");
        let lifeline = Lifeline::new();
        let node = instance.build_processor(lifeline.tie()).expect("a processor");
        blocks.push((block, process_in_blocks(node, &dry, block)));
    }
    let parity = blocks.windows(2).all(|pair| pair[0].1 == pair[1].1);
    println!(
        "[{}] block processing: 64 vs 512 frames differ by {:.9}, 512 vs 333 by {:.9} \
         (a plugin that moves its modulation once a block, as LSP's flanger does, differs here by design)",
        verdict(parity),
        worst(&blocks[0].1, &blocks[1].1),
        worst(&blocks[1].1, &blocks[2].1)
    );

    // 5. Saved and reopened with the plugin present.
    let song_path = args.out.join("clap-case.mooloop");
    session.capture_plugin_states();
    let saved = snapshot(&session);
    mooloop_project::save_song(&song_path, &saved, mooloop_project::AssetMode::Referenced).expect("it saves");
    let report = mooloop_project::load_bundle(&song_path).expect("it reopens");
    let repairs_empty = report.repairs.is_empty();
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        unreachable!("a song");
    };
    println!("[{}] saved and reopened with no repairs", verdict(repairs_empty && reopened.plugins == saved.plugins));
    let mut present = Session::default();
    present.set_plugin_opener(Box::new(ClapOpener::new(cache_path.clone())));
    present.replace_project(&reopened, &[]);
    let mut engine = Engine::default();
    present.service_plugins(&mut engine);
    let reopened_wet = export(&mut present, &args.out.join("reopened.wav"));
    println!(
        "[{}] reopened present: hosted again ({} processor swapped in), export identical",
        verdict(present.plugin_problem(slot).is_none() && reopened_wet == wet),
        engine.nodes.len()
    );

    // 6. Reopened with the plugin missing.
    let mut missing = Session::default();
    missing.set_plugin_opener(Box::new(ClapOpener::with_cache(PluginCache::default())));
    missing.replace_project(&reopened, &[]);
    let mut engine = Engine::default();
    missing.service_plugins(&mut engine);
    let placeholder = export(&mut missing, &args.out.join("missing.wav"));
    let kept = snapshot(&missing);
    println!(
        "[{}] reopened missing: {:?}, plays the dry loop (largest difference {:.9}), slot kept unchanged: {}",
        verdict(worst(&placeholder, &dry) < 1.0e-6 && kept.plugins == reopened.plugins),
        missing.plugin_problem(slot).map(|p| p.to_string()),
        worst(&placeholder, &dry),
        kept.plugins == reopened.plugins
    );
}
