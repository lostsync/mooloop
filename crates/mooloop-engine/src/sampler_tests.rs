//! The sampler through the engine.
//!
//! `mooloop_dsp::sampler` tests the voice against its own `process`. These
//! test what is only true of the assembled program: that a loop fitted to
//! the project's tempo plays for the same length, sample for sample, offline
//! as it does live (MOO-39), which is what "song and offline render timing
//! match the realtime transport" means for a stretched loop.

use std::sync::Arc;

use crate::offline::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};
use crate::render::RenderState;
use crate::render_test_support::SAMPLE_RATE;
use mooloop_core::{LoopMode, NoteEvent, Project, ProjectChannel, SamplerParams};
use mooloop_dsp::SampleData;

/// A second and a half of a pitched, decaying tone every quarter second: a
/// loop the stretcher has something to lock onto, not a sine.
fn loop_sample() -> Arc<SampleData> {
    let len = (SAMPLE_RATE as f32 * 1.5) as usize;
    let frames = (0..len)
        .map(|n| {
            let t = n as f32 / SAMPLE_RATE as f32;
            let since_hit = t % 0.25;
            let value = 0.6
                * (std::f32::consts::TAU * 180.0 * t).sin()
                * (-since_hit / 0.06).exp();
            [value, value]
        })
        .collect();
    Arc::new(SampleData {
        frames,
        sample_rate: SAMPLE_RATE,
        root_note: 60,
    })
}

/// One sampler channel holding a note for the whole two-bar pattern, with
/// its loop fitted to one bar at the project's tempo.
fn fitted_loop(bpm: u16) -> Project {
    let mut channel = ProjectChannel::sampler(0, 1);
    channel.setup.channel.volume = 1.0;
    if let Some(state) = channel.setup.sampler_state_mut() {
        state.params = SamplerParams {
            loop_mode: LoopMode::Forward,
            loop_start: 0.0,
            loop_end: 1.0,
            output_gain: 1.0,
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
    }
    channel.notes[0].push(NoteEvent::new(1, 0, 32 * mooloop_core::TICKS_PER_STEP, 60, 127));
    Project {
        bpm,
        channels: vec![channel],
        pattern_lengths: vec![32],
        ..Project::default()
    }
}

/// **A fitted loop renders the same offline as it plays live** (MOO-39).
/// At two tempos, the export of the two-bar pattern is compared sample for
/// sample with the live engine playing the same pattern from its top, block
/// by block. And the loop really is fitted: its hits land a bar's worth
/// of hits apart at either tempo, not the sample's own spacing.
#[test]
fn a_fitted_loop_renders_the_same_offline_as_live() {
    let sample = loop_sample();
    let temp = tempfile::tempdir().expect("a temp dir");
    for bpm in [96u16, 128] {
        let project = fitted_loop(bpm);
        let wav = temp.path().join(format!("fit-{bpm}.wav"));
        OfflineRenderer::render(
            &project,
            &[Some(sample.clone())],
            SAMPLE_RATE,
            &ExportSpec {
                path: wav.clone(),
                scope: RenderScope::Pattern { index: 0 },
                tail_seconds: 0.0,
                format: ExportFormat::Wav(WavEncoding::Float32),
            },
        )
        .expect("it renders offline");
        let exported: Vec<f32> = hound::WavReader::open(&wav)
            .expect("the export is readable")
            .samples::<f32>()
            .map(|sample| sample.expect("a decoded sample"))
            .collect();

        let mut live_project = project.clone();
        live_project.playback_mode = mooloop_core::PlaybackMode::Pattern;
        live_project.current_pattern = 0;
        let mut live_state = RenderState::from_project(SAMPLE_RATE, &live_project, &[Some(sample.clone())]);
        live_state.play();
        let frames = exported.len() / 2;
        let mut live = Vec::with_capacity(frames * 2 + 1_024);
        while live.len() < frames * 2 {
            live_state.process_block(512);
            let master = live_state.master();
            for frame in 0..512 {
                live.push(master.l[frame]);
                live.push(master.r[frame]);
            }
        }
        live.truncate(frames * 2);

        let peak = exported.iter().fold(0.0_f32, |p, s| p.max(s.abs()));
        assert!(peak > 0.05, "at {bpm} BPM the loop has to be sounding");
        // Every frame but the last, as the layer render found: the export's
        // last frame is where the pattern ends and the live loop's is where
        // it wraps to its top.
        let worst = exported[..exported.len() - 2]
            .iter()
            .zip(&live[..exported.len() - 2])
            .fold(0.0_f32, |w, (a, b)| w.max((a - b).abs()));
        assert!(worst < 1.0e-6, "at {bpm} BPM offline and live differ by {worst}");

        // Two bars long, whatever the tempo: the pattern is 32 steps.
        let bar = mooloop_core::frames_per_bar(SAMPLE_RATE, f64::from(bpm));
        assert!(
            (frames as f64 - 2.0 * bar).abs() < 2.0,
            "at {bpm} BPM two bars rendered as {frames} frames, a bar being {bar}"
        );
        // And the loop repeats once a bar: the second bar's 10 ms energy
        // envelope is the first's. Envelopes, not samples, because a
        // stretched tone lands at a different phase on each pass while its
        // hits land where the bar puts them. An unfitted loop (1.5 s against
        // a 2.5 s or 1.875 s bar) puts its hits elsewhere and fails this.
        let bar = bar.round() as usize;
        let left: Vec<f32> = exported.iter().step_by(2).copied().collect();
        let window = SAMPLE_RATE as usize / 100;
        let envelope = |signal: &[f32]| -> Vec<f64> {
            signal
                .chunks(window)
                .map(|chunk| chunk.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>().sqrt())
                .collect()
        };
        let skip = SAMPLE_RATE as usize / 10;
        let (first, second) = (
            envelope(&left[skip..bar]),
            envelope(&left[bar + skip..2 * bar]),
        );
        let n = first.len().min(second.len());
        let mean = |v: &[f64]| v[..n].iter().sum::<f64>() / n as f64;
        let (ma, mb) = (mean(&first), mean(&second));
        let cov: f64 = (0..n).map(|i| (first[i] - ma) * (second[i] - mb)).sum();
        let va: f64 = (0..n).map(|i| (first[i] - ma).powi(2)).sum();
        let vb: f64 = (0..n).map(|i| (second[i] - mb).powi(2)).sum();
        let correlation = cov / (va * vb).sqrt().max(1.0e-12);
        assert!(
            correlation > 0.8,
            "at {bpm} BPM the second bar is not the first again ({correlation})"
        );
    }
}
