//! Step 07's case with a real CLAP effect (`docs/plans/plugin-hosting/07`,
//! MOO-82): a third-party filter on a drum loop with its cutoff automated
//! through the session, saved, reopened and exported, and played through
//! the executor the way a live callback plays it -- all headless, no JACK,
//! no window. It writes the WAVs and prints what it measured. **Listening
//! is Adam's**; this is what an agent can check instead.
//!
//! ```text
//! clap_automation_case --plugin /usr/lib64/clap/lsp-plugins.clap \
//!     [--id in.lsp-plug.filter_stereo] [--param freq] \
//!     [--set 'type=<value>'] [--out clap-automation]
//! ```
//!
//! `--param` picks the automated parameter: the first automatable,
//! continuous one whose name holds that text. `--set name=value` holds any
//! other parameter at a plain value with a flat lane, for a filter whose
//! default mode is off; repeat it as needed. The binary prints the plugin's
//! whole parameter list first, stepped positions' texts included, so the
//! right values can be read off a first run.
//!
//! The plugin is found the way the app finds it: this binary is its own scan
//! child, so the library is loaded first in a child process.
//! `tests/plugin_params.rs` is the same case with the in-repo test plugin,
//! which is the one CI runs.
//!
//! What it measures:
//! 1. **The automation lands**: the cutoff lane falls across the loop, and
//!    each of the twelve identical hats on the filter's channel is darker
//!    than the one before it, against the same song with the lane held flat
//!    (the energy of a steep high-pass over each hat's sixteenth).
//! 2. **Offline matches realtime**: the export against the same song played
//!    through the executor in 64- and 512-frame callbacks, to below the
//!    smallest normal float (the executor flushes subnormals and an export
//!    does not yet: MOO-223).
//! 3. **Deterministic**, **saved and reopened with no repairs**, and the
//!    reopened song exports the same file.
//! 4. **Reopened missing**: it plays the dry loop and keeps the plugin's
//!    slot and its lanes byte for byte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mooloop_core::{
    EffectParams, EffectTarget, NoteEvent, ParamAddr, PluginParamInfo, PluginSlotId, Project,
    ProjectChannel, TICKS_PER_STEP,
};
use mooloop_dsp::AudioNode;
use mooloop_engine::live_check::play_through_executor;
use mooloop_engine::{
    CommandSink, ExportFormat, ExportProgress, ExportSpec, OfflineRenderer, RenderScope,
    StructuralCommand, WavEncoding,
};
use mooloop_plugin_host::clap::ClapOpener;
use mooloop_plugin_host::scan::{self, ChildCommand, PluginCache, ScanConfig};
use mooloop_session::plugin_params::plugin_normalized;
use mooloop_session::session::Session;

const RATE: u32 = 48_000;
/// Two bars of sixteenths.
const STEPS: u16 = 32;

struct Args {
    plugin: PathBuf,
    id: String,
    param: String,
    set: Vec<(String, f64)>,
    out: PathBuf,
}

fn args() -> Args {
    let mut plugin = None;
    let mut id = "in.lsp-plug.filter_stereo".to_owned();
    let mut param = "freq".to_owned();
    let mut set = Vec::new();
    let mut out = PathBuf::from("clap-automation");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--plugin" => plugin = args.next().map(PathBuf::from),
            "--id" => id = args.next().unwrap_or(id),
            "--param" => param = args.next().unwrap_or(param),
            "--set" => {
                let pair = args.next().unwrap_or_default();
                let Some((name, value)) = pair.split_once('=') else {
                    eprintln!("--set wants name=value, not {pair}");
                    std::process::exit(2);
                };
                let Ok(value) = value.trim().parse() else {
                    eprintln!("--set {name}: {value} is not a number");
                    std::process::exit(2);
                };
                set.push((name.trim().to_lowercase(), value));
            }
            "--out" => out = args.next().map(PathBuf::from).unwrap_or(out),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(plugin) = plugin else {
        eprintln!("usage: clap_automation_case --plugin <file.clap> [--id <clap id>] [--param <name>] [--set name=value] [--out <dir>]");
        std::process::exit(2);
    };
    Args {
        plugin,
        id,
        param: param.to_lowercase(),
        set,
        out,
    }
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

/// The steps channel 1's closed hats fall on: the filter's channel, and
/// twelve identical hits for the measurement to hold against each other.
const HAT_STEPS: [u32; 12] = [0, 2, 4, 6, 10, 12, 14, 18, 20, 26, 28, 30];

/// Two bars of a drum loop on two drum-synth channels: hats on the first,
/// which the filter goes on, and kick and snare on the second.
fn drum_loop() -> Project {
    let mut project = Project::default();
    project.channels.clear();
    project.pattern_lengths[0] = STEPS;
    let hats: Vec<(u32, u8)> = HAT_STEPS.iter().map(|&step| (step, 42)).collect();
    let hits: [&[(u32, u8)]; 2] = [&hats, &[(0, 36), (8, 38), (16, 36), (22, 36), (24, 38)]];
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

fn render(project: &Project, plugins: BTreeMap<PluginSlotId, Box<dyn AudioNode + Send>>, path: &Path) -> Vec<f32> {
    let spec = ExportSpec {
        path: path.to_path_buf(),
        scope: RenderScope::Pattern { index: 0 },
        tail_seconds: 0.0,
        format: ExportFormat::Wav(WavEncoding::Float32),
    };
    std::thread::scope(|scope| {
        scope
            .spawn(|| OfflineRenderer::render_with_plugins(project, &[], RATE, &spec, &ExportProgress::new(), plugins))
            .join()
            .expect("the export thread did not panic")
    })
    .expect("the export succeeds");
    println!("  wrote {}", path.display());
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

/// Interleaved stereo as a float WAV, for listening to what the executor
/// played beside what the export rendered.
fn write_wav(path: &Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("a WAV to write");
    for &sample in samples {
        writer.write_sample(sample).expect("a sample written");
    }
    writer.finalize().expect("the WAV closes");
}

fn worst(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::INFINITY;
    }
    a.iter().zip(b).fold(0.0f32, |w, (x, y)| w.max((x - y).abs()))
}

/// How bright each hat is with the lane against with it held flat: the
/// energy of the second difference (a steep high-pass, what a falling
/// low-pass cutoff takes away) over the sixteenth each hat starts, wet over
/// flat. Twelve identical hits, so a cutoff that falls across the loop is a
/// ratio that falls hat by hat. The kick and snare are on the other channel,
/// in both renders alike, so they only pull a ratio toward one.
fn hat_brightness(wet: &[f32], flat: &[f32], bpm: f64) -> Vec<(u32, f64)> {
    let frames_per_step = f64::from(RATE) * 60.0 / bpm / 4.0;
    let bright = |samples: &[f32], from: usize, to: usize| {
        (from.max(2)..to.min(samples.len() / 2))
            .map(|frame| {
                let at = |n: usize| f64::from(samples[n * 2]) + f64::from(samples[n * 2 + 1]);
                let edge = at(frame) - 2.0 * at(frame - 1) + at(frame - 2);
                edge * edge
            })
            .sum::<f64>()
    };
    HAT_STEPS
        .iter()
        .map(|&step| {
            let from = (f64::from(step) * frames_per_step) as usize;
            let to = (f64::from(step + 1) * frames_per_step) as usize;
            (step, bright(wet, from, to) / bright(flat, from, to).max(1.0e-30))
        })
        .collect()
}

fn verdict(ok: bool) -> &'static str {
    if ok {
        "PASS"
    } else {
        "FAIL"
    }
}

fn describe(session: &mut Session, slot: PluginSlotId, params: &[PluginParamInfo]) {
    println!("parameters ({}):", params.len());
    for info in params {
        let mut line = format!(
            "  {:>10}  {:<32} {} .. {} (default {})",
            info.id, info.name, info.min, info.max, info.default
        );
        if let Some(steps) = info.stepped {
            let instance = session.plugin_rack.instance_mut(slot).expect("hosted");
            let texts: Vec<String> = (0..steps.min(24))
                .map(|step| {
                    let value = info.min + f64::from(step);
                    format!("{value}={}", instance.value_text(info.id, value).unwrap_or_default())
                })
                .collect();
            line.push_str(&format!(" stepped [{}]", texts.join(", ")));
        }
        if !info.automatable {
            line.push_str(" (not automatable)");
        }
        if info.hidden {
            line.push_str(" (hidden)");
        }
        println!("{line}");
    }
}

fn main() {
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
    let summary = scan::scan(&config, &mut cache, |_, _, _| {});
    cache.save(&cache_path).expect("the cache saves");
    println!("scan: {summary:?}");
    let Some(found) = cache.plugins().find(|found| found.plugin.id == args.id).cloned() else {
        eprintln!("{} is not in {}", args.id, args.plugin.display());
        std::process::exit(1);
    };
    println!("plugin: {} ({}) {}", found.plugin.name, found.plugin.vendor, found.plugin.version);

    // 2. The dry loop, and the session with the effect on channel 1.
    let dry_song = drum_loop();
    let dry = render(&dry_song, BTreeMap::new(), &args.out.join("dry.wav"));
    let mut session = Session::default();
    session.set_plugin_opener(Box::new(ClapOpener::new(cache_path.clone())));
    session.replace_project(&dry_song, &[]);
    let mut engine = Engine::default();
    let inserted = session
        .insert_plugin_effect(found.plugin.clone(), 0, &mut engine)
        .expect("the device is inserted");
    let EffectParams::Plugin(slot) = inserted.params else {
        unreachable!("a plugin device");
    };
    if let Some(problem) = session.plugin_problem(slot) {
        eprintln!("the plugin could not be hosted: {problem}");
        std::process::exit(1);
    }
    let params = session.plugins[&slot].params.clone();
    describe(&mut session, slot, &params);
    let Some(cutoff) = params
        .iter()
        .find(|info| {
            info.automatable && info.stepped.is_none() && !info.hidden && info.name.to_lowercase().contains(&args.param)
        })
        .cloned()
    else {
        eprintln!("no automatable, continuous parameter's name holds {:?}", args.param);
        std::process::exit(1);
    };
    println!("automating: {} (id {}), {} .. {}", cutoff.name, cutoff.id, cutoff.min, cutoff.max);

    // 3. The lanes, drawn through the session as the roll draws them: the
    // cutoff falls from near the top of its range to near the bottom, and
    // each `--set` parameter is held flat at its value.
    let device = session.channels[0].effects[0].id;
    let length = i32::from(STEPS) * TICKS_PER_STEP as i32;
    let address = |id| ParamAddr::plugin_param(EffectTarget::Channel(0), device, id);
    for (name, value) in &args.set {
        let Some(info) = params.iter().find(|info| info.name.to_lowercase().contains(name.as_str())) else {
            eprintln!("--set: no parameter's name holds {name:?}");
            std::process::exit(1);
        };
        let level = plugin_normalized(info, *value);
        session.open_automation_lane_at(address(info.id)).expect("a lane opens");
        session.create_automation_point(0, level).expect("a point");
        session.create_automation_point(length - 1, level).expect("a point");
        println!("holding: {} at {value} (lane at {level:.4})", info.name);
    }
    let (high, low) = (0.95f32, 0.35f32);
    session.open_automation_lane_at(address(cutoff.id)).expect("the cutoff's lane opens");
    session.create_automation_point(0, high).expect("a point");
    session.create_automation_point(length - 1, low).expect("a point");
    println!(
        "cutoff lane: {:.1} -> {:.1}",
        cutoff.min + f64::from(high) * (cutoff.max - cutoff.min),
        cutoff.min + f64::from(low) * (cutoff.max - cutoff.min)
    );

    // 4. Exported with the lane, and with it held flat at its start.
    let wet = export(&mut session, &args.out.join("wet.wav"));
    let again = export(&mut session, &args.out.join("wet-again.wav"));
    let flat_song = {
        let mut song = snapshot(&session);
        for lane in song.channels[0].automation[0].iter_mut() {
            if lane.target == address(cutoff.id) {
                lane.clear();
                let _ = lane.upsert(mooloop_core::AutomationPoint::new(1, 0, high));
            }
        }
        song
    };
    let plugins = session.export_plugin_processors(RATE);
    let flat = render(&flat_song, plugins, &args.out.join("flat.wav"));
    let hats = hat_brightness(&wet, &flat, f64::from(drum_loop().bpm));
    let falls = hats.windows(2).all(|pair| pair[1].1 <= pair[0].1 * 1.02)
        && hats[hats.len() - 1].1 < hats[0].1 * 0.5;
    println!(
        "[{}] the automation lands: each hat's brightness against the flat lane, in order: {}",
        verdict(falls && wet != flat),
        hats.iter()
            .map(|(step, ratio)| format!("{step}:{ratio:.3}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    println!("[{}] deterministic: two exports identical", verdict(wet == again));

    // 5. Offline against realtime: the same song through the executor.
    let frames = wet.len() / 2;
    let mut parity = true;
    for block in [64usize, 512] {
        let plugins = session.export_plugin_processors(RATE);
        let played = play_through_executor(&snapshot(&session), plugins, RATE, frames, block);
        let difference = worst(&played, &wet);
        // Below the smallest normal float, and nowhere else: the executor's
        // thread flushes subnormals and the export's does not (MOO-223).
        parity &= difference < f32::MIN_POSITIVE;
        let path = args.out.join(format!("live-{block}.wav"));
        write_wav(&path, &played);
        let first = played
            .iter()
            .zip(&wet)
            .position(|(a, b)| a != b)
            .map_or_else(|| "none".to_owned(), |sample| format!("frame {}", sample / 2));
        println!(
            "  executor at {block} frames against the export: largest difference {difference:e}, first at {first} ({})",
            path.display()
        );
    }
    println!("[{}] offline == realtime", verdict(parity));

    // 6. Saved and reopened with the plugin present.
    let song_path = args.out.join("clap-automation.mooloop");
    session.capture_plugin_states();
    let saved = snapshot(&session);
    mooloop_project::save_song(&song_path, &saved, mooloop_project::AssetMode::Referenced).expect("it saves");
    let report = mooloop_project::load_bundle(&song_path).expect("it reopens");
    let repairs_empty = report.repairs.is_empty();
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        unreachable!("a song");
    };
    println!(
        "[{}] saved and reopened with no repairs, plugin slot and lanes intact",
        verdict(repairs_empty && reopened.plugins == saved.plugins && reopened.channels[0].automation == saved.channels[0].automation)
    );
    let mut present = Session::default();
    present.set_plugin_opener(Box::new(ClapOpener::new(cache_path.clone())));
    present.replace_project(&reopened, &[]);
    let mut engine = Engine::default();
    present.service_plugins(&mut engine);
    let reopened_wet = export(&mut present, &args.out.join("reopened.wav"));
    println!(
        "[{}] reopened present: export identical (largest difference {:.9})",
        verdict(present.plugin_problem(slot).is_none() && reopened_wet == wet),
        worst(&reopened_wet, &wet)
    );

    // 7. Reopened with the plugin missing.
    let mut missing = Session::default();
    missing.set_plugin_opener(Box::new(ClapOpener::with_cache(PluginCache::default())));
    missing.replace_project(&reopened, &[]);
    let mut engine = Engine::default();
    missing.service_plugins(&mut engine);
    let placeholder = export(&mut missing, &args.out.join("missing.wav"));
    let kept = snapshot(&missing);
    println!(
        "[{}] reopened missing: plays the dry loop (largest difference {:.9}), slot and lanes kept: {}",
        verdict(
            worst(&placeholder, &dry) < 1.0e-6
                && kept.plugins == reopened.plugins
                && kept.channels[0].automation == reopened.channels[0].automation
        ),
        worst(&placeholder, &dry),
        kept.plugins == reopened.plugins && kept.channels[0].automation == reopened.channels[0].automation
    );
}
