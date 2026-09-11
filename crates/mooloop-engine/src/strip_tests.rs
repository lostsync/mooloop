//! The channel strip at the level it is actually used: a whole project
//! rendered through the mixer.
//!
//! `mooloop_dsp::strip` already holds the sections to their own claims -- a
//! band boosts its own frequency, `mix` at 0 is the dry signal, a voicing
//! reaches its harmonic profile. What those tests cannot see is whether the
//! *engine* put the strip in the right place, and that is the whole risk in
//! this feature. A strip that runs on the wrong side of the chain, or that
//! runs when its switches are out, or that stops a track going to sleep,
//! would pass every test in the DSP module.

use crate::render::RenderState;
use mooloop_core::mixer::{StripPin, STRIP_PIN};
use mooloop_core::strip::{StripParams, STRIP_COMP_IN, STRIP_EQ_IN, STRIP_PRE_IN};
use mooloop_core::{
    EffectParams, EffectSlotState, EngineCommand, EqParams, NoteEvent, PreampVoicing, Project,
    ProjectChannel, SampleReference,
};

const SAMPLE_RATE: u32 = 48_000;

/// One sustained note on a channel at unity, the shape the console and
/// gain-structure suites both use.
fn one_note_channel(pitch: u8) -> ProjectChannel {
    let mut channel = ProjectChannel::poly_synth(0, 1);
    if let Some(state) = channel.setup.sampler_state_mut() {
        state.sample = SampleReference::Builtin {
            id: "default_kick".into(),
        };
        state.params.output_gain = 1.0;
    }
    channel.setup.channel.volume = 1.0;
    channel.notes[0].push(NoteEvent::new(1, 0, 96 * 4, pitch, 127));
    channel
}

/// One channel on track 1, which is the track whose strip these tests move.
fn one_track_project() -> Project {
    let mut channel = one_note_channel(45);
    channel.setup.channel.bus = 1;
    let mut project = Project {
        channels: vec![channel],
        ..Project::default()
    };
    project.ensure_tracks(2);
    project
}

fn render_master(project: &Project, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut left = Vec::with_capacity(remaining);
    let mut right = Vec::with_capacity(remaining);
    while remaining > 0 {
        let frames = remaining.min(1_024);
        render.process_once_block(frames);
        let master = render.master();
        left.extend_from_slice(&master.l[..frames]);
        right.extend_from_slice(&master.r[..frames]);
        remaining -= frames;
    }
    (left, right)
}

fn worst_difference(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .fold(0.0f32, |worst, (x, y)| worst.max((x - y).abs()))
}

fn peak_of(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |p, s| p.max(s.abs()))
}

fn rms_of(samples: &[f32]) -> f32 {
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt()
}

/// **Acceptance case 1.** A project whose tracks all carry a default strip
/// renders bit-identically to the same project with the strip's fields set
/// to something that would be violent if anything ran them.
///
/// Sample for sample. This is what entitles a strip to exist on every track
/// and it is the one claim a tolerance would hide: a strip that ran at 0.001%
/// of its settings would pass an RMS comparison and be a bug.
#[test]
fn a_default_strip_changes_nothing_about_the_mix() {
    let plain = one_track_project();
    let mut loaded = one_track_project();
    loaded.buses[1].bus.strip = StripParams {
        voicing: PreampVoicing::Iron,
        drive_db: 18.0,
        threshold_db: -50.0,
        ratio: 20.0,
        makeup_db: 18.0,
        ..StripParams::default()
    };
    loaded.buses[1].bus.strip.bands[0].gain_db = 15.0;

    let (plain_l, plain_r) = render_master(&plain, 0.5);
    let (loaded_l, loaded_r) = render_master(&loaded, 0.5);
    assert!(peak_of(&plain_l) > 0.01, "the comparison ran on silence");
    assert_eq!(plain_l, loaded_l, "the left channel moved");
    assert_eq!(plain_r, loaded_r, "the right channel moved");
}

/// Every section switched in with `Moo` and nothing set is still the mix,
/// to the smoothers' tolerance -- which is the null case the voicing exists
/// to be.
#[test]
fn moo_with_every_section_in_is_the_same_mix() {
    let plain = one_track_project();
    let mut switched = one_track_project();
    switched.buses[1].bus.strip = StripParams {
        pre_in: true,
        eq_in: true,
        comp_in: true,
        threshold_db: 0.0,
        knee_db: 0.0,
        ..StripParams::default()
    };
    let (plain_l, _) = render_master(&plain, 0.5);
    let (switched_l, _) = render_master(&switched, 0.5);
    let worst = worst_difference(&plain_l, &switched_l);
    assert!(
        worst < 1e-5,
        "Moo with everything in should be transparent: worst {worst} against a peak of {}",
        peak_of(&plain_l)
    );
}

/// The EQ section reaches the mix when it is switched in, and not before.
/// Two renders of the same project differing only in `eq_in`.
#[test]
fn the_eq_reaches_the_mix_only_once_it_is_switched_in() {
    let mut out = one_track_project();
    out.buses[1].bus.strip.bands[3].gain_db = 15.0;
    let mut is_in = out.clone();
    is_in.buses[1].bus.strip.eq_in = true;

    let (out_l, _) = render_master(&out, 0.5);
    let (in_l, _) = render_master(&is_in, 0.5);
    let plain = render_master(&one_track_project(), 0.5).0;
    assert_eq!(
        out_l, plain,
        "a band set while the section is out is not audible"
    );
    assert!(
        worst_difference(&in_l, &plain) > 1e-3,
        "a 15 dB low shelf switched in has to be audible"
    );
}

/// The compressor reduces level through the mixer, which is the same claim
/// the DSP test makes one level down -- here to say the engine hands it the
/// track's audio rather than something else.
#[test]
fn the_compressor_reaches_the_mix() {
    let plain = one_track_project();
    let mut squashed = one_track_project();
    squashed.buses[1].bus.strip = StripParams {
        comp_in: true,
        threshold_db: -40.0,
        ratio: 20.0,
        attack_ms: 1.0,
        ..StripParams::default()
    };
    let (plain_l, _) = render_master(&plain, 0.5);
    let (squashed_l, _) = render_master(&squashed, 0.5);
    assert!(
        rms_of(&squashed_l) < rms_of(&plain_l) * 0.8,
        "20:1 over a -40 dB threshold should be obvious: {} against {}",
        rms_of(&squashed_l),
        rms_of(&plain_l)
    );
}

/// **Acceptance case 8.** The pin decides whether the strip runs before or
/// after the track's own devices, and the same project rendered both ways
/// has to differ -- so this fails if the branch is dropped and the strip is
/// simply written in one place.
///
/// The proof is a device that does not commute with the strip: a steep
/// low-pass where the strip's compressor is looking. At the head the
/// compressor detects the whole signal and clamps hard; at the tail it
/// detects what is left after the filter and barely works at all.
///
/// `STRIP_PIN` is the policy and is asserted separately, at the end: the
/// point of a constant is that one edit moves both the audio and the rack's
/// drawing, and the point of this test is that the two orders are really two
/// orders.
#[test]
fn the_pin_decides_whether_the_strip_runs_before_the_tracks_devices() {
    let mut project = one_track_project();
    project.buses[1].bus.strip = StripParams {
        comp_in: true,
        threshold_db: -40.0,
        ratio: 20.0,
        attack_ms: 1.0,
        ..StripParams::default()
    };
    let mut eq = EqParams::default();
    for band in &mut eq.bands {
        band.enabled = false;
    }
    eq.low_pass.enabled = true;
    eq.low_pass.frequency_hz = 60.0;
    eq.low_pass.slope = mooloop_core::EqSlope::Db36;
    project.buses[1].effects = vec![EffectSlotState::new(EffectParams::Eq(eq))];
    // A chain reaches the engine with identities assigned; the engine
    // `debug_assert!`s it, because a route or a lane on an unidentified
    // device resolves to nothing.
    project.assign_device_ids();

    let render_at = |pin: StripPin| -> Vec<f32> {
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.set_strip_pin(pin);
        render.play();
        let mut samples = Vec::new();
        for _ in 0..24 {
            render.process_once_block(1_024);
            samples.extend_from_slice(&render.master().l[..1_024]);
        }
        samples
    };
    let head = render_at(StripPin::Head);
    let tail = render_at(StripPin::Tail);
    assert!(peak_of(&tail) > 1e-4, "the comparison ran on silence");
    assert!(
        worst_difference(&head, &tail) > 1e-4,
        "the two orders rendered the same audio, so the pin is not being read"
    );
    assert!(
        rms_of(&head) < rms_of(&tail),
        "at the head the compressor sees the whole signal and clamps harder: {} against {}",
        rms_of(&head),
        rms_of(&tail)
    );
    assert_eq!(
        STRIP_PIN,
        StripPin::Head,
        "the pinned position is the head; if this is deliberately moved, the \
         rack's pinned row moves with it and this line is the record of it"
    );
}

/// Polarity inverts the track and nothing else: the same samples, negated.
/// Exactly, because it is a multiply by -1.
#[test]
fn polarity_inverts_the_track_and_changes_nothing_else() {
    let plain = one_track_project();
    let mut flipped = one_track_project();
    flipped.buses[1].bus.polarity = true;
    let (plain_l, plain_r) = render_master(&plain, 0.5);
    let (flipped_l, flipped_r) = render_master(&flipped, 0.5);
    assert!(peak_of(&plain_l) > 0.01);
    for (index, (plain, flipped)) in plain_l.iter().zip(flipped_l.iter()).enumerate() {
        assert_eq!(*flipped, -*plain, "frame {index}");
    }
    for (plain, flipped) in plain_r.iter().zip(flipped_r.iter()) {
        assert_eq!(*flipped, -*plain);
    }
}

/// Two tracks carrying the same signal, one of them inverted, cancel at the
/// master. The reason polarity is worth having at all, and a test that would
/// fail if the flip happened after the track's own summing point.
#[test]
fn an_inverted_track_cancels_its_twin() {
    let mut left = one_note_channel(45);
    let mut right = one_note_channel(45);
    left.setup.channel.bus = 1;
    right.setup.channel.bus = 2;
    let mut project = Project {
        channels: vec![left, right],
        ..Project::default()
    };
    project.ensure_tracks(3);
    let doubled = peak_of(&render_master(&project, 0.5).0);
    project.buses[2].bus.polarity = true;
    let cancelled = peak_of(&render_master(&project, 0.5).0);
    assert!(doubled > 0.01, "the comparison ran on silence");
    assert!(
        cancelled < doubled * 1e-3,
        "an inverted twin should cancel: {cancelled} against {doubled}"
    );
}

/// **Acceptance case 9.** Idle skipping is the mechanism that makes a
/// sleeping mixer cheap, and its whole claim is that it changes nothing. A
/// strip switched in must not break that -- and it is the plausible way to
/// break it, because a compressor holding a release is a reason for a track
/// *not* to sleep and `BusStrip::is_resting` has to know.
#[test]
fn a_strip_that_is_in_renders_the_same_with_idle_skipping_either_way() {
    let mut project = one_track_project();
    project.buses[1].bus.strip = StripParams {
        pre_in: true,
        eq_in: true,
        comp_in: true,
        voicing: PreampVoicing::Iron,
        drive_db: 6.0,
        threshold_db: -30.0,
        release_ms: 400.0,
        ..StripParams::default()
    };
    project.buses[1].bus.strip.bands[1].gain_db = 9.0;

    let render_with = |skip: bool| {
        let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
        render.set_idle_skipping(skip);
        render.play();
        let mut samples = Vec::new();
        // Long enough that the note ends and the strip has to decide
        // whether the track may sleep while its detector is still letting
        // go.
        for _ in 0..120 {
            render.process_once_block(512);
            samples.extend_from_slice(&render.master().l[..512]);
        }
        samples
    };
    let skipping = render_with(true);
    let running = render_with(false);
    assert!(peak_of(&running) > 0.01, "the comparison ran on silence");
    assert_eq!(skipping, running, "idle skipping changed the render");
}

/// The command path, end to end: a parameter moved through
/// `EngineCommand::SetStripParam` has to reach the audio, and the switches
/// have to be switches.
///
/// Two renderers of the same project in lockstep, one of which is sent the
/// commands. Comparing a renderer against *itself* a moment later cannot
/// work here -- the note is decaying, so the level falls between any two
/// measurements whether a command arrived or not, which is what the first
/// version of this test measured.
#[test]
fn a_strip_parameter_arrives_by_command() {
    let project = one_track_project();
    let mut moved = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    let mut control = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    moved.play();
    control.play();

    let advance =
        |moved: &mut RenderState, control: &mut RenderState| -> (Vec<f32>, Vec<f32>) {
            let mut a = Vec::new();
            let mut b = Vec::new();
            for _ in 0..8 {
                moved.process_once_block(512);
                a.extend_from_slice(&moved.master().l[..512]);
                control.process_once_block(512);
                b.extend_from_slice(&control.master().l[..512]);
            }
            (a, b)
        };

    let (a, b) = advance(&mut moved, &mut control);
    assert_eq!(
        a, b,
        "two renderers of one project disagreed before anything moved"
    );
    assert!(peak_of(&a) > 0.01, "the comparison ran on silence");

    moved.apply_command(EngineCommand::SetStripParam {
        bus: 1,
        param: mooloop_core::strip::strip_band_param(3, mooloop_core::STRIP_BAND_GAIN),
        value: 18.0,
    });
    let (a, b) = advance(&mut moved, &mut control);
    assert_eq!(
        a, b,
        "a band set while the section is out reached the audio"
    );

    moved.apply_command(EngineCommand::SetStripParam {
        bus: 1,
        param: STRIP_EQ_IN,
        value: 1.0,
    });
    let (a, b) = advance(&mut moved, &mut control);
    assert!(
        worst_difference(&a, &b) > 1e-3,
        "an 18 dB low shelf switched in has to be heard"
    );
}

/// The master is a track and gets a strip like any other. A mix-bus
/// compressor is an ordinary thing to want, and analog sum -- which the
/// master refuses for a mechanical reason -- is the only one of these
/// controls it does not have.
#[test]
fn the_master_has_a_strip_of_its_own() {
    let plain = one_track_project();
    let mut bussed = one_track_project();
    bussed.buses[0].bus.strip = StripParams {
        comp_in: true,
        threshold_db: -40.0,
        ratio: 20.0,
        attack_ms: 1.0,
        ..StripParams::default()
    };
    let (plain_l, _) = render_master(&plain, 0.5);
    let (bussed_l, _) = render_master(&bussed, 0.5);
    assert!(
        rms_of(&bussed_l) < rms_of(&plain_l) * 0.8,
        "the master's own compressor has to work: {} against {}",
        rms_of(&bussed_l),
        rms_of(&plain_l)
    );
}

/// A strip's state does not survive a project that does not ask for it:
/// `BusStrip::reset` puts the sections back out, so a track removed and
/// another loaded into its seat does not inherit a compressor.
#[test]
fn a_track_reused_by_a_shorter_project_loses_its_strip() {
    let mut loud = one_track_project();
    loud.buses[1].bus.strip = StripParams {
        comp_in: true,
        threshold_db: -50.0,
        ratio: 20.0,
        ..StripParams::default()
    };
    let mut render = RenderState::from_project(SAMPLE_RATE, &loud, &[]);
    // The same graph, handed a project with only the master: the spare
    // strip is kept rather than freed, and has to be reset rather than
    // carried.
    let bare = Project::default();
    render.load_project(&bare);
    render.load_project(&one_track_project());
    render.play();
    let mut samples = Vec::new();
    for _ in 0..24 {
        render.process_once_block(1_024);
        samples.extend_from_slice(&render.master().l[..1_024]);
    }
    let reference = render_master(&one_track_project(), 0.5).0;
    assert_eq!(
        samples[..reference.len()],
        reference[..],
        "a reloaded track kept the strip of the project before it"
    );
}

/// Every switch is reachable by command and every one of them is off in a
/// default project. Cheap, and it is the fact the whole cost argument rests
/// on: three booleans, all false, on every track a song has.
#[test]
fn every_section_starts_out_on_every_track() {
    let project = {
        let mut project = one_track_project();
        project.ensure_tracks(8);
        project
    };
    for setup in &project.buses {
        assert!(
            setup.bus.strip.is_out(),
            "{} arrived switched in",
            setup.bus.name
        );
        assert!(!setup.bus.polarity);
    }
    for id in [STRIP_PRE_IN, STRIP_EQ_IN, STRIP_COMP_IN] {
        assert_eq!(StripParams::default().get(id), Some(0.0));
    }
}
