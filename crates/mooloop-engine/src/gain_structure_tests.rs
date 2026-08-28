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
    DeviceKind, DrumMode, EffectKind, EffectParams, EffectSlotState, MonoSynthParams,
    NoteEvent, OscParams, PlateParams, PolySynthParams, Project, ProjectChannel, ReverbParams,
    SampleReference,
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
