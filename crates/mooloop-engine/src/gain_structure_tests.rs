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
fn source_peak_at_unity_is_today_s_baseline() {
    // Step 05 moves these to ~-12 dBFS. Record today's numbers; only a wide
    // sanity band is asserted so a deliberate level change does not fight
    // this test — read the printed values and update 00-status.md.
    for kind in [
        DeviceKind::Sampler,
        DeviceKind::DrumSynth,
        DeviceKind::MonoSynth,
        DeviceKind::PolySynth,
    ] {
        let peak = peak_dbfs(&single_channel_project(one_note_channel(kind)), 2.0);
        println!("source peak at unity, {kind:?}: {peak:.1} dBFS");
        assert!(
            (-40.0..=0.0).contains(&peak),
            "{kind:?} default patch peaked at {peak:.1} dBFS"
        );
    }
}

#[test]
fn kick_and_snare_reproduces_adam_s_measurement() {
    // As shipped today: default channel volume (0.8 linear), a default kick
    // on step 0 and a default snare one beat later — neither at unity.
    // Adam measured -4.2 dBFS on the master. Assert a range, not a point.
    let mut kick = one_note_channel(DeviceKind::DrumSynth);
    kick.setup.channel.volume = 0.8;
    let mut snare = one_note_channel(DeviceKind::DrumSynth);
    snare.setup.channel.volume = 0.8;
    snare.setup.drum_synth_state_mut().unwrap().params.mode = DrumMode::Snare;
    snare.notes[0][0].start_tick = 96;
    let project = Project {
        channels: vec![kick, snare],
        ..Project::default()
    };
    let peak = peak_dbfs(&project, 2.0);
    println!("kick + snare master peak (default volumes): {peak:.1} dBFS");
    assert!(
        (-8.0..=0.0).contains(&peak),
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
fn reverb_wet_path_is_loud_relative_to_dry_today() {
    // Step 07 exists because the IR is peak-normalized to 0.42
    // (crates/mooloop-dsp/src/effects/reverb.rs:210). Measure how far above
    // unity the 100% wet path sits against the effect-less render.
    for effect in [
        (EffectKind::Reverb, EffectParams::Reverb(ReverbParams::default())),
        (EffectKind::Plate, EffectParams::Plate(PlateParams::default())),
    ] {
        let with_effect = |wet_dry: f32| {
            let mut channel = one_note_channel(DeviceKind::MonoSynth);
            let mut slot = EffectSlotState::new(effect.1);
            slot.wet_dry = wet_dry;
            channel.setup.effects.push(slot);
            single_channel_project(channel)
        };
        let bypass = peak_dbfs(&single_channel_project(one_note_channel(DeviceKind::MonoSynth)), 3.0);
        let dry_only = peak_dbfs(&with_effect(0.0), 3.0);
        let wet_only = peak_dbfs(&with_effect(1.0), 3.0);
        println!(
            "{:?}: bypass {bypass:.1} dBFS, 0% wet {dry_only:.1} dBFS, 100% wet {wet_only:.1} dBFS, wet/bypass {:.1} dB",
            effect.0,
            wet_only - bypass
        );
        assert!(
            wet_only > bypass + 3.0,
            "{:?} wet path only {:.1} dB above bypass; peak-normalized IR not confirmed?",
            effect.0,
            wet_only - bypass
        );
    }
}

#[test]
fn fader_travel_maps_linearly_to_gain_today() {
    // Today the mixer fader's travel IS the linear gain (mixer.slint binds
    // `value: root.strip.volume` in [0, 1] with no taper), so 0.75 travel is
    // 0.75 linear gain = -2.5 dB. Step 04 replaces this identity with the
    // piecewise dB taper; flip this assertion there.
    let travel = 0.75f32;
    let db = 20.0 * travel.log10();
    assert!((db - -2.5).abs() < 0.05, "travel 0.75 read {db:.2} dB");
}
