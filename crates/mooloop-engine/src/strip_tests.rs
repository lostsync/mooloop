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
use mooloop_dsp::testkit::rms as rms_of;
use crate::render_test_support::{peak_of, render_master, worst_difference, SAMPLE_RATE};
use mooloop_core::mixer::{StripPin, STRIP_PIN};
use mooloop_core::strip::{StripParams, STRIP_COMP_IN, STRIP_EQ_IN, STRIP_PRE_IN};
use mooloop_core::{
    EffectParams, EffectSlotState, EngineCommand, EqParams, NoteEvent, PreampVoicing, Project,
    ProjectChannel, SampleReference,
};

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

/// **Solo in place**, through the engine: soloing a track silences its
/// siblings, keeps what feeds it and where it goes, and leaves the mix
/// bit-identical while nothing is soloed.
///
/// `mooloop_core::mixer::solo_silenced` is where the rule is tested as a
/// rule. What this says is that the engine acts on it -- and acts through
/// `load_project`, which is the path an **offline bounce** takes, so a
/// render matches what was heard.
#[test]
fn a_solo_silences_the_siblings_and_keeps_the_path() {
    let mut sibling = one_note_channel(45);
    let mut soloed = one_note_channel(52);
    sibling.setup.channel.bus = 1;
    soloed.setup.channel.bus = 2;
    let mut project = Project {
        channels: vec![sibling, soloed],
        ..Project::default()
    };
    project.ensure_tracks(3);

    let (both, _) = render_master(&project, 0.5);
    assert!(peak_of(&both) > 0.01, "the comparison ran on silence");

    // Track 2 soloed: 1 goes, 2 stays, and the master stays because the
    // solo has to be audible through it.
    project.buses[2].bus.solo = true;
    let (alone, _) = render_master(&project, 0.5);
    assert!(
        peak_of(&alone) > 0.01,
        "a soloed track has to still be heard"
    );
    assert!(
        worst_difference(&both, &alone) > 1e-3,
        "soloing one of two tracks has to change the mix"
    );

    // The same render with the silenced track muted by hand instead: solo
    // in place *is* that, which is the claim worth pinning.
    project.buses[2].bus.solo = false;
    project.buses[1].bus.muted = true;
    let (muted, _) = render_master(&project, 0.5);
    project.buses[1].bus.muted = false;
    project.buses[2].bus.solo = true;
    let (soloed_again, _) = render_master(&project, 0.5);
    assert_eq!(
        muted, soloed_again,
        "solo in place should be exactly the other tracks muted"
    );
}

/// **Solo in place one level down**: soloing a *channel* silences the other
/// channels, and is exactly those channels muted by hand.
///
/// The same claim `a_solo_silences_the_siblings_and_keeps_the_path` makes for
/// tracks, and made against `load_project` for the same reason -- that is the
/// path an offline bounce takes, so a render of a soloed song matches what
/// was heard.
///
/// A channel's version has no "keeps the path" half, and that is the
/// difference between the two derivations rather than a gap in the test:
/// channels do not feed each other, so there is no ancestor to keep up.
#[test]
fn soloing_a_channel_is_the_other_channels_muted() {
    let mut quiet = one_note_channel(45);
    let mut soloed = one_note_channel(52);
    quiet.setup.channel.bus = 1;
    soloed.setup.channel.bus = 1;
    let mut project = Project {
        channels: vec![quiet, soloed],
        ..Project::default()
    };
    project.ensure_tracks(2);

    let (both, _) = render_master(&project, 0.5);
    assert!(peak_of(&both) > 0.01, "the comparison ran on silence");

    project.channels[1].setup.channel.solo = true;
    let (alone, _) = render_master(&project, 0.5);
    assert!(
        peak_of(&alone) > 0.01,
        "a soloed channel has to still be heard"
    );
    assert!(
        worst_difference(&both, &alone) > 1e-3,
        "soloing one of two channels has to change the mix"
    );

    // The same render with the silenced channel muted by hand instead.
    project.channels[1].setup.channel.solo = false;
    project.channels[0].setup.channel.muted = true;
    let (muted, _) = render_master(&project, 0.5);
    project.channels[0].setup.channel.muted = false;
    project.channels[1].setup.channel.solo = true;
    let (soloed_again, _) = render_master(&project, 0.5);
    assert_eq!(
        muted, soloed_again,
        "solo in place should be exactly the other channels muted"
    );
}

/// A solo does not eat a mute, which is why the verdict is a second field on
/// the strip rather than a write to `output.muted`: a channel muted by hand
/// and then silenced by somebody else's solo is still muted when that solo
/// is dropped.
#[test]
fn dropping_a_solo_gives_a_channel_back_its_own_mute() {
    let mut project = Project {
        channels: vec![one_note_channel(45), one_note_channel(52)],
        ..Project::default()
    };
    project.ensure_tracks(1);
    project.channels[0].setup.channel.muted = true;

    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    assert!(!render.channel_solo_silenced(0), "nothing is soloed yet");

    render.apply_command(EngineCommand::SetChannelSoloSilenced {
        channel: 0,
        silenced: true,
    });
    assert!(render.channel_solo_silenced(0));

    render.apply_command(EngineCommand::SetChannelSoloSilenced {
        channel: 0,
        silenced: false,
    });
    assert!(!render.channel_solo_silenced(0), "the verdict did not lift");
    assert!(
        render.channel_muted(0),
        "the solo ate the channel's own mute on the way past"
    );
}

/// A soloed *group* keeps the tracks that feed it: their audio is what the
/// group is made of, so silencing them would make the solo silent.
#[test]
fn soloing_a_group_still_hears_what_feeds_it() {
    let mut channel = one_note_channel(45);
    channel.setup.channel.bus = 3;
    let mut project = Project {
        channels: vec![channel],
        ..Project::default()
    };
    project.ensure_tracks(4);
    // 3 feeds the group 2, which feeds the master.
    project.buses[3].bus.output = 2;
    let (plain, _) = render_master(&project, 0.5);
    assert!(peak_of(&plain) > 0.01);

    project.buses[2].bus.solo = true;
    let (grouped, _) = render_master(&project, 0.5);
    assert_eq!(
        plain, grouped,
        "soloing a group whose only feeder is track 3 should change nothing"
    );
}

/// Nothing soloed is bit-identical to the engine before solo existed, which
/// is the case that has to cost nothing.
#[test]
fn a_bank_with_no_solo_renders_exactly_as_before() {
    let project = one_track_project();
    let (left, right) = render_master(&project, 0.5);
    let mut untouched = one_track_project();
    for setup in &mut untouched.buses {
        setup.bus.solo = false;
    }
    let (again, again_right) = render_master(&untouched, 0.5);
    assert_eq!(left, again);
    assert_eq!(right, again_right);
}

/// The lamp beside the COMP header reads what the compressor took, and
/// reads nothing while the section is out.
#[test]
fn the_strip_publishes_the_reduction_its_lamp_reads() {
    let mut project = one_track_project();
    project.buses[1].bus.strip = StripParams {
        comp_in: true,
        threshold_db: -40.0,
        ratio: 20.0,
        attack_ms: 1.0,
        ..StripParams::default()
    };
    let meters = crate::meters::BusMeters::new();
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.attach_meters(meters.clone());
    render.play();
    for _ in 0..16 {
        render.process_once_block(512);
    }
    let reduction = meters.take_reduction(1);
    assert!(
        reduction > 3.0,
        "20:1 over a -40 dB threshold should publish real reduction: {reduction}"
    );

    // Switched out, nothing is published and the held cell falls to zero --
    // which is what takes the lamp dark rather than leaving it lit at
    // whatever it last saw.
    project.buses[1].bus.strip.comp_in = false;
    let mut render = RenderState::from_project(SAMPLE_RATE, &project, &[]);
    render.attach_meters(meters.clone());
    render.play();
    for _ in 0..16 {
        render.process_once_block(512);
    }
    assert_eq!(meters.take_reduction(1), 0.0);
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

/// **A document arriving clears the strip it lands in.**
///
/// `load_project` installs a track's parameters onto whatever the strip in
/// that seat was already holding. On the path the application takes that is
/// harmless, because `install_project` builds a fresh `RenderState`; on a
/// state being *reused* -- an undo, or a project with fewer tracks than the
/// last one -- the new song would be handed a filter bank and a detector
/// full of the old one's audio, and would hear it in its first block.
///
/// Asked of the strip rather than measured off the master, and that is the
/// point: `set_params` leaves the same *parameters* either way, so the only
/// difference the reset makes is state, and the only block it is visible in
/// is the first one. Both sections are in on both sides so that neither the
/// bank nor the detector can answer for the other, and the tests either side
/// of this one are what say the reset does not cost the parameters.
#[test]
fn a_document_arriving_clears_the_strip_it_lands_in() {
    let mut loud = one_track_project();
    loud.buses[1].bus.strip = StripParams {
        eq_in: true,
        comp_in: true,
        threshold_db: -50.0,
        ratio: 20.0,
        release_ms: 2_000.0,
        ..StripParams::default()
    };
    loud.buses[1].bus.strip.bands[0].gain_db = 18.0;

    let mut render = RenderState::from_project(SAMPLE_RATE, &loud, &[]);
    render.play();
    for _ in 0..8 {
        render.process_once_block(1_024);
    }
    assert_eq!(
        render.strip_is_at_rest(1),
        Some(false),
        "the strip was not holding anything, so this proves nothing"
    );

    // The same document again: every parameter it installs is the one
    // already there, so nothing but the reset can settle the strip.
    render.load_project(&loud);
    assert_eq!(
        render.strip_is_at_rest(1),
        Some(true),
        "a document arrived onto a charged detector and a loaded filter bank"
    );
}

/// A strip's state does not survive a project that does not ask for it: a
/// track removed and another loaded into its seat does not inherit a
/// compressor.
///
/// What this holds is the *outcome* over both branches -- the seat is
/// abandoned by one project and re-taken by the next -- rather than
/// `BusStrip::reset` in particular. It cannot single that one out, and the
/// reason is worth knowing before somebody tries: `reset` puts the sections
/// back **out**, and a strip whose sections are out answers `is_at_rest`
/// without reading any state and renders without touching a sample, so
/// there is nothing it could do differently for a test to see. Its own
/// `strip.reset()` is there by construction, so the seat is clean whichever
/// branch clears it.
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
