//! Gain-structure characterization tests.
//!
//! These lock in TODAY'S levels before docs/plans/gain-structure/ changes
//! them. They are intentionally *characterization*, not correctness: when a
//! plan step (04, 05, 06, 07) deliberately moves a number, update the
//! expected range here in the same commit rather than treating the diff as a
//! regression. Measured values are transcribed into
//! `docs/plans/gain-structure/00-status.md` so later steps can compare
//! without re-running anything.

use crate::render::RenderState;
use mooloop_core::{
    DeviceKind, DrumMode, EffectKind, EffectParams, EffectSlotState, FilterParams, MonoSynthParams,
    NoteEvent, OscParams, PlateParams, PolySynthParams, Project, ProjectChannel, ReverbParams,
    SampleReference, MAX_LINEAR_GAIN,
};

const SAMPLE_RATE: u32 = 48_000;

/// Render the project offline and return the master peak in dBFS.
fn peak_dbfs(project: &Project, seconds: f32) -> f32 {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut peak = 0.0f32;
    while remaining > 0 {
        let frames = remaining.min(1024);
        render.process_once_block(frames);
        let master = render.master();
        peak = peak.max(master.l[..frames].iter().fold(0.0f32, |p, s| p.max(s.abs())));
        peak = peak.max(master.r[..frames].iter().fold(0.0f32, |p, s| p.max(s.abs())));
        remaining -= frames;
    }
    20.0 * peak.max(1e-6).log10()
}

/// One channel of `kind` at unity volume with a single default-velocity note
/// at tick 0. Samplers get the builtin kick so they render without data.
fn one_note_channel(kind: DeviceKind) -> ProjectChannel {
    let mut channel = match kind {
        DeviceKind::Sampler => ProjectChannel::sampler(0, 1),
        DeviceKind::DrumSynth => ProjectChannel::drum_synth(0, 1),
        DeviceKind::MonoSynth => ProjectChannel::mono_synth(0, 1),
        DeviceKind::PolySynth => ProjectChannel::poly_synth(0, 1),
    };
    if let Some(state) = channel.setup.sampler_state_mut() {
        state.sample = SampleReference::Builtin {
            id: "default_kick".into(),
        };
    }
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, 96, 60, 127));
    channel
}

fn single_channel_project(channel: ProjectChannel) -> Project {
    Project {
        channels: vec![channel],
        ..Project::default()
    }
}

fn osc_levels(level: f32, count: usize) -> [OscParams; 3] {
    let mut osc = std::array::from_fn(|_| OscParams::default());
    for voice in osc.iter_mut().take(count) {
        voice.level = level;
    }
    osc
}

#[test]
fn source_peak_at_unity_hits_the_reference_level() {
    // Step 05 calibrated every generator so its default patch, at default
    // velocity and a unity channel, peaks within ~1 dB of
    // `gain::REFERENCE_PEAK_DBFS`. The sampler's case is the builtin kick,
    // the plan's "known test asset": user samples are the channel trim's
    // job.
    for kind in [
        DeviceKind::Sampler,
        DeviceKind::DrumSynth,
        DeviceKind::MonoSynth,
        DeviceKind::PolySynth,
    ] {
        let peak = peak_dbfs(&single_channel_project(one_note_channel(kind)), 2.0);
        println!("source peak at unity, {kind:?}: {peak:.1} dBFS");
        assert!(
            (-13.0..=-11.0).contains(&peak),
            "{kind:?} default patch peaked at {peak:.1} dBFS, want ~-12"
        );
    }
}

#[test]
fn kick_and_snare_reproduces_adam_s_measurement() {
    // Adam's case: default kick and snare on the downbeat, default channel
    // volume, default patches. Was -4.2 live / hot pre-calibration; with the
    // reference level both hits peak near -12 and their sum lands "somewhere
    // near -9". Assert a range, not a point.
    let kick = one_note_channel(DeviceKind::DrumSynth);
    let mut snare = one_note_channel(DeviceKind::DrumSynth);
    snare.setup.drum_synth_state_mut().unwrap().params.mode = DrumMode::Snare;
    snare.notes[0][0].start_tick = 0;
    let project = Project {
        channels: vec![kick, snare],
        ..Project::default()
    };
    let peak = peak_dbfs(&project, 2.0);
    println!("kick + snare master peak (downbeat, unity): {peak:.1} dBFS");
    assert!(
        (-12.0..=-5.0).contains(&peak),
        "kick + snare peaked at {peak:.1} dBFS"
    );
}

#[test]
fn channel_summing_is_honest_today() {
    // N identical mono-synth channels, unity volume. Assert only that the
    // peak grows with N; the measured table goes into 00-status.md and is the
    // before-picture for step 05's headroom claim.
    let mut peaks = Vec::new();
    for n in [1usize, 2, 4, 8] {
        let channels: Vec<ProjectChannel> = (0..n)
            .map(|index| {
                let mut channel = one_note_channel(DeviceKind::MonoSynth);
                channel.setup.channel.name = format!("Synth {index}");
                channel
            })
            .collect();
        let project = Project {
            channels,
            ..Project::default()
        };
        let peak = peak_dbfs(&project, 2.0);
        println!("sum of {n} identical channels: {peak:.1} dBFS");
        peaks.push(peak);
    }
    for window in peaks.windows(2) {
        assert!(window[1] > window[0], "peak did not grow: {peaks:?}");
    }
}

#[test]
fn oscillator_summing_gain_matches_today() {
    for kind in [DeviceKind::MonoSynth, DeviceKind::PolySynth] {
        let build = |count: usize| {
            let mut channel = one_note_channel(kind);
            match kind {
                DeviceKind::MonoSynth => {
                    channel.setup.mono_synth_state_mut().unwrap().params = {
                        let mut params = MonoSynthParams::default();
                        params.osc = osc_levels(1.0, count);
                        params
                    };
                }
                DeviceKind::PolySynth => {
                    channel.setup.poly_synth_state_mut().unwrap().params = {
                        let mut params = PolySynthParams::default();
                        params.osc = osc_levels(1.0, count);
                        params
                    };
                }
                _ => unreachable!(),
            }
            single_channel_project(channel)
        };
        let one = peak_dbfs(&build(1), 2.0);
        let three = peak_dbfs(&build(3), 2.0);
        println!("{kind:?}: one osc at full {one:.1} dBFS, three {three:.1} dBFS, delta {:.1} dB", three - one);
        assert!(
            (6.0..=12.0).contains(&(three - one)),
            "{kind:?} oscillator summing delta was {:.1} dB",
            three - one
        );
    }
}

#[test]
fn reverb_wet_path_is_level_matched_now() {
    // Step 07 energy-normalized the IR and level-matched the plate. Wet at
    // 100% should sit within a few dB of the dry signal. Two probes: the
    // broadband kick (transient, whole-spectrum) and the synth patch
    // (narrowband — each partial samples the IR response at one point, so
    // its wet/dry ratio legitimately swings both ways; it is a sanity bound
    // here, not a loudness meter).
    for effect in [
        (EffectKind::Reverb, EffectParams::Reverb(ReverbParams::default())),
        (EffectKind::Plate, EffectParams::Plate(PlateParams::default())),
    ] {
        for kind in [DeviceKind::DrumSynth, DeviceKind::MonoSynth] {
            let with_effect = |wet_dry: f32| {
                let mut channel = one_note_channel(kind);
                let mut slot = EffectSlotState::new(effect.1);
                slot.wet_dry = wet_dry;
                channel.setup.effects.push(slot);
                single_channel_project(channel)
            };
            let bypass = peak_dbfs(&single_channel_project(one_note_channel(kind)), 3.0);
            let dry_only = peak_dbfs(&with_effect(0.0), 3.0);
            let wet_only = peak_dbfs(&with_effect(1.0), 3.0);
            println!(
                "{:?} on {kind:?}: bypass {bypass:.1} dBFS, 0% wet {dry_only:.1}, 100% wet {wet_only:.1}, wet/dry {:.1} dB",
                effect.0,
                wet_only - bypass
            );
            let bound = if kind == DeviceKind::MonoSynth {
                // Narrowband probe: each partial samples the IR's low-heavy
                // tail spectrum at one point, so it legitimately reads a
                // long way hotter than broadband material. Bound it, but
                // wide — see the reverb's IR_ENERGY_TARGET note.
                15.0
            } else {
                4.0
            };
            assert!(
                (wet_only - bypass).abs() < bound,
                "{:?} on {kind:?}: wet path {:.1} dB off dry; level matching lost?",
                effect.0,
                wet_only - bypass
            );
        }
    }
}

#[test]
fn equal_power_blend_preserves_energy_when_decorrelated() {
    // What equal-power buys: at 50% the blend of a wet path decorrelated
    // from dry carries the average of the two energies, where a linear
    // fade would dip ~3 dB. The kick's delayed copy barely overlaps its
    // own decay, so delay at 375 ms makes a decorrelated pair.
    let mut channel = one_note_channel(DeviceKind::DrumSynth);
    let mut slot = EffectSlotState::new(EffectParams::Delay(
        mooloop_core::DelayParams {
            time_ms: 375.0,
            mix: 1.0,
            feedback: 0.0,
            ..mooloop_core::DelayParams::default()
        },
    ));
    slot.wet_dry = 1.0;
    channel.setup.effects.push(slot);
    let energy = |wet_dry: f32| {
        let mut channel = channel.clone();
        channel.setup.effects[0].wet_dry = wet_dry;
        let mut render = RenderState::from_project(SAMPLE_RATE, &single_channel_project(channel), &[]);
        render.play();
        let mut remaining = (SAMPLE_RATE as f32 * 2.0) as usize;
        let mut energy = 0.0f64;
        while remaining > 0 {
            let frames = remaining.min(1024);
            render.process_once_block(frames);
            let master = render.master();
            energy += master.l[..frames]
                .iter()
                .chain(master.r[..frames].iter())
                .map(|s| (*s as f64) * (*s as f64))
                .sum::<f64>();
            remaining -= frames;
        }
        energy
    };
    let dry = energy(0.0);
    let wet = energy(1.0);
    let mid = energy(0.5);
    println!(
        "equal-power 50%: dry {dry:.1}, wet {wet:.1}, mid {mid:.1} (want ~{})",
        (dry + wet) / 2.0
    );
    let expected = (dry + wet) / 2.0;
    assert!(
        (mid - expected).abs() < expected * 0.2,
        "50% blend carried {mid:.1}, expected ~{expected:.1}"
    );
}

#[test]
fn fader_travel_is_tapered_in_db() {
    // Step 04 flipped the mixer fader from travel==linear-gain to the shared
    // dB taper: unity sits at three-quarter travel, the top of the throw is
    // +6 dB. The stored value stays linear; only the mapping changed.
    assert!(
        mooloop_core::gain::fader_position_to_db(0.75).abs() < 0.05,
        "travel 0.75 should read 0 dB"
    );
    assert!(
        (mooloop_core::gain::fader_position_to_db(1.0) - 6.0).abs() < 0.05,
        "full throw should read +6 dB"
    );
    // The stored-gain identity is gone: 0.75 linear is no longer what
    // three-quarter travel produces.
    assert!((20.0 * 0.75f32.log10() - -2.5).abs() < 0.05, "sanity");
}

// --- Level-dependent gain: is the summing path linear? ----------------------
//
// Adam's report: raising the pad's fader makes the drums quieter, whatever
// bus the pad is assigned to. Two categories of cause, and they need
// different fixes. Control-dependent: something derives a scalar from the
// track gains (`1.0 / max(gains)` and friends), so moving one fader moves
// everything. Signal-dependent: something in the shared path reacts to the
// audio, so a loud pad pushes a drum transient down. The tests below
// separate them, and they are also the regression fence for the summing
// contract in `docs/GAIN_STRUCTURE.md`: "no summing point normalizes by its
// input count".

/// Render the project offline and keep the master's samples, not just its
/// peak. Superposition can only be checked sample by sample.
fn render_master(project: &Project, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut left = Vec::with_capacity(remaining);
    let mut right = Vec::with_capacity(remaining);
    while remaining > 0 {
        let frames = remaining.min(1024);
        render.process_once_block(frames);
        let master = render.master();
        left.extend_from_slice(&master.l[..frames]);
        right.extend_from_slice(&master.r[..frames]);
        remaining -= frames;
    }
    (left, right)
}

fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |p, s| p.max(s.abs()))
}

/// A sustained low note: the "pad" whose fader is the one being moved.
fn pad_channel(volume: f32) -> ProjectChannel {
    let mut channel = one_note_channel(DeviceKind::PolySynth);
    channel.setup.channel.volume = volume;
    // Two seconds at 120 BPM, low enough that its waveform spends most of
    // each cycle away from zero — the bias a shaper would ride on.
    channel.notes[0][0] = NoteEvent::new(1, 0, 96 * 4, 40, 127);
    channel
}

/// A kick landing a beat in, on top of the pad's sustain.
fn drum_channel() -> ProjectChannel {
    let mut channel = one_note_channel(DeviceKind::DrumSynth);
    channel.notes[0][0].start_tick = 96;
    channel
}

/// The pad and the drums together, with either one optionally silenced by
/// its mute rather than by removing it, so all three renders walk the same
/// graph.
fn pad_and_drums(pad_volume: f32, pad_muted: bool, drums_muted: bool) -> Project {
    let mut pad = pad_channel(pad_volume);
    pad.setup.channel.muted = pad_muted;
    let mut drums = drum_channel();
    drums.setup.channel.muted = drums_muted;
    Project {
        channels: vec![pad, drums],
        ..Project::default()
    }
}

/// Send the pad down a chain of insert buses that ends at the master. A
/// channel that feeds the master directly never exercises `mix_into`, the
/// bus-to-bus hop, so the straight-to-master case alone would leave the bus
/// walk's own summing untested.
fn route_pad_through(mut project: Project, chain: &[u8]) -> Project {
    let Some((&first, rest)) = chain.split_first() else {
        return project;
    };
    project.channels[0].setup.channel.bus = first;
    let mut current = first;
    for &next in rest {
        project.buses[current as usize].bus.output = next;
        current = next;
    }
    project.buses[current as usize].bus.output = mooloop_core::MASTER_BUS;
    project
}

/// Largest sample-by-sample departure from superposition: how far
/// `(pad + drums)` rendered together sits from the two rendered apart and
/// added. Zero for any linear mixer, whatever the faders are set to and
/// whatever the pad is routed through.
fn superposition_error(pad_volume: f32, chain: &[u8]) -> (f32, f32) {
    let project = |pad_muted, drums_muted| {
        route_pad_through(pad_and_drums(pad_volume, pad_muted, drums_muted), chain)
    };
    let (both_l, _) = render_master(&project(false, false), 2.5);
    let (pad_l, _) = render_master(&project(false, true), 2.5);
    let (drums_l, _) = render_master(&project(true, false), 2.5);
    let error = both_l
        .iter()
        .zip(pad_l.iter().zip(drums_l.iter()))
        .fold(0.0f32, |worst, (both, (pad, drums))| {
            worst.max((both - pad - drums).abs())
        });
    (error, peak_of(&both_l))
}

#[test]
fn summing_stays_linear_however_the_faders_sit() {
    // The control-dependent hypothesis, tested directly: if any headroom
    // scalar were derived from the track gains, moving the pad's fader
    // would change what the drums contribute, and superposition would fail
    // at every fader position but one. Sweep the pad from silence to the
    // +12 dB ceiling, both straight to the master and down a two-hop bus
    // chain, so the channel sum and the bus-to-bus sum are both covered.
    for chain in [&[][..], &[5, 2][..]] {
        for pad_volume in [0.0, 0.5, 1.0, 2.0, MAX_LINEAR_GAIN] {
            let (error, peak) = superposition_error(pad_volume, chain);
            println!(
                "pad via {chain:?}, fader {pad_volume:.2} linear: \
                 superposition error {error:.3e} against a {peak:.3} peak"
            );
            assert!(
                error < 1e-6,
                "pad fader at {pad_volume} via {chain:?} broke superposition by \
                 {error:.3e}: something in the summing path depends on the \
                 other track's gain"
            );
        }
    }
}

/// Level of the drums' own contribution over the 50 ms after their onset:
/// the difference the drum channel makes to the master, in RMS. The window
/// matters — a nonlinearity's gain swings with the pad's waveform, so a peak
/// would report the single most favourable instant rather than what is heard.
fn drum_contribution_db(
    pad_volume: f32,
    master: &[EffectSlotState],
    pad_channel: &[EffectSlotState],
) -> f32 {
    let chain = |pad_muted, drums_muted| {
        let mut project = pad_and_drums(pad_volume, pad_muted, drums_muted);
        project.buses[0].effects = master.to_vec();
        project.channels[0].setup.effects = pad_channel.to_vec();
        project
    };
    // The kick lands on beat 2: tick 96 at 120 BPM.
    let onset = (SAMPLE_RATE / 2) as usize;
    let window = onset..onset + SAMPLE_RATE as usize / 20;
    let (both, _) = render_master(&chain(false, false), 2.5);
    let (pad_only, _) = render_master(&chain(false, true), 2.5);
    let (drums_only, _) = render_master(&chain(true, false), 2.5);
    let rms = |samples: &[f32]| {
        (samples
            .iter()
            .map(|s| (*s as f64) * (*s as f64))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    };
    let difference: Vec<f32> = both[window.clone()]
        .iter()
        .zip(pad_only[window.clone()].iter())
        .map(|(both, pad)| both - pad)
        .collect();
    20.0 * (rms(&difference).max(1e-12) / rms(&drums_only[window]).max(1e-12)).log10() as f32
}

#[test]
fn a_shared_saturation_stage_is_what_ducks_one_track_under_another() {
    // The signal-dependent hypothesis, and the only mechanism in this
    // codebase that produces Adam's symptom. `apply_drive` (mooloop-dsp
    // `filter.rs`) is the one static nonlinearity in the shared path, and
    // the filter effect carries it, so a filter with drive up on the master
    // bus sits under everything. A sustained pad then biases the shaper onto
    // its compressive region and the drum transient riding on top is
    // multiplied by the local slope. Raising the pad's fader deepens it, and
    // no bus assignment escapes it, because every bus drains to the master.
    let driven_filter = |drive: f32| {
        vec![EffectSlotState::new(EffectParams::Filter(FilterParams {
            // Wide open: the drive stage is the only thing under test.
            cutoff_hz: 18_000.0,
            drive,
            ..FilterParams::default()
        }))]
    };

    // With nothing shared, the pad's fader cannot touch the drums at all —
    // the same fact `summing_stays_linear_however_the_faders_sit` proves
    // sample by sample, stated in the terms the symptom was reported in.
    for pad_volume in [1.0, 2.0, MAX_LINEAR_GAIN] {
        let clean = drum_contribution_db(pad_volume, &[], &[]);
        println!("bare master, pad fader {pad_volume:.2}: drums {clean:+.2} dB");
        assert!(
            clean.abs() < 0.01,
            "a bare master moved the drums by {clean:.2} dB at pad fader {pad_volume}"
        );
    }

    // With a driven filter on the master, the pad's fader is a ducking
    // control over the drums.
    let mut previous = 0.0f32;
    for pad_volume in [1.0, 2.0, MAX_LINEAR_GAIN] {
        let ducked = drum_contribution_db(pad_volume, &driven_filter(0.6), &[]);
        println!("master filter at drive 0.6, pad fader {pad_volume:.2}: drums {ducked:+.2} dB");
        assert!(
            ducked < previous - 0.1,
            "pad fader {pad_volume} left the drums at {ducked:+.2} dB, \
             no deeper than the {previous:+.2} dB of the fader below it"
        );
        previous = ducked;
    }
    assert!(
        previous < -2.0,
        "the deepest duck was only {previous:+.2} dB; the mechanism should be \
         plainly audible at the +12 dB end of the pad's fader"
    );

    // Placement is the whole mechanism, not the effect. The same filter at
    // the same drive on the pad's own channel runs on the pad's buffer
    // before anything is summed (`RenderState::process_block`: the chain
    // processes `strip.bus`, and only then is it added into the destination
    // bus), so it cannot reach the drums however hard the pad is driven
    // into it.
    for pad_volume in [1.0, 2.0, MAX_LINEAR_GAIN] {
        let on_the_channel = drum_contribution_db(pad_volume, &[], &driven_filter(0.6));
        println!(
            "pad-channel filter at drive 0.6, pad fader {pad_volume:.2}: \
             drums {on_the_channel:+.2} dB"
        );
        assert!(
            on_the_channel.abs() < 0.01,
            "a filter on the pad's own channel moved the drums by \
             {on_the_channel:+.2} dB at pad fader {pad_volume}"
        );
    }
}
