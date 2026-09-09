//! Console summing, at the level it is actually switched on: a whole project
//! rendered through the mixer, not the curve on its own.
//!
//! `mooloop_dsp::console` already holds the curve to its identity and its
//! ceiling. What those tests cannot see is whether the *engine* put the
//! encode and the decode in the right places, and that is the entire risk in
//! this feature: an encode without its decode is a mangled mix, and a decode
//! without its encode is a different mangled mix. Both would still pass every
//! test in the DSP module.
//!
//! `gain_structure_tests.rs` keeps holding for console-off, which stays the
//! default -- in particular `summing_stays_linear_however_the_faders_sit`,
//! which this feature must not weaken. Console-on gets its own tests here
//! rather than a tolerance added to that one.

use crate::render::RenderState;
use mooloop_core::{DeviceKind, NoteEvent, Project, ProjectChannel, SampleReference};

const SAMPLE_RATE: u32 = 48_000;

/// One channel of `kind` at unity with a single note, the shape
/// `gain_structure_tests.rs` uses.
fn one_note_channel(kind: DeviceKind, pitch: u8) -> ProjectChannel {
    let mut channel = match kind {
        DeviceKind::PolySynth => ProjectChannel::poly_synth(0, 1),
        DeviceKind::DrumSynth => ProjectChannel::drum_synth(0, 1),
        _ => ProjectChannel::sampler(0, 1),
    };
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

fn render_master(project: &Project, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    render_master_in_blocks(project, seconds, 1024)
}

/// The same render, at a chosen block size. Console summing happens between
/// strips inside one block, so a block-size dependence would mean the encode
/// and the decode had drifted apart across a boundary.
fn render_master_in_blocks(project: &Project, seconds: f32, block: usize) -> (Vec<f32>, Vec<f32>) {
    let mut render = RenderState::from_project(SAMPLE_RATE, project, &[]);
    render.play();
    let mut remaining = (SAMPLE_RATE as f32 * seconds) as usize;
    let mut left = Vec::with_capacity(remaining);
    let mut right = Vec::with_capacity(remaining);
    while remaining > 0 {
        let frames = remaining.min(block);
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

/// Two sustained notes a fifth apart, so the mix has a waveform rather than a
/// transient and the summing law has something to act on for two seconds.
fn two_channel_project(console: [bool; 2]) -> Project {
    let mut low = one_note_channel(DeviceKind::PolySynth, 40);
    let mut high = one_note_channel(DeviceKind::PolySynth, 47);
    low.setup.channel.console = console[0];
    high.setup.channel.console = console[1];
    Project {
        channels: vec![low, high],
        ..Project::default()
    }
}

/// **The claim the whole design rests on.** A strip on its own is not
/// coloured, so switching console on is not a hidden saturator -- the
/// character is entirely in what happens between strips.
///
/// Sample for sample, not "close enough in RMS": if the encode and the decode
/// are not exact inverses on this path, the difference is a distortion and it
/// has to show up here.
#[test]
fn one_console_strip_alone_nulls_against_console_off() {
    let mut on = two_channel_project([true, false]);
    on.channels.truncate(1);
    let mut off = two_channel_project([false, false]);
    off.channels.truncate(1);

    let (on_l, on_r) = render_master(&on, 2.0);
    let (off_l, off_r) = render_master(&off, 2.0);
    let peak = peak_of(&off_l);
    assert!(peak > 0.05, "the fixture rendered nothing to compare: {peak}");

    let error = worst_difference(&on_l, &off_l).max(worst_difference(&on_r, &off_r));
    println!("one strip alone: worst difference {error:.3e} against a {peak:.3} peak");
    assert!(
        error < 1e-6,
        "one console strip differed from console-off by {error:.3e}, which is \
         a colour it is not allowed to have"
    );
}

/// And the other half: two strips are audibly and measurably not the linear
/// sum. If this ever passes at 0.0 the feature has quietly become a no-op.
#[test]
fn two_console_strips_are_not_the_linear_sum() {
    let (on_l, _) = render_master(&two_channel_project([true, true]), 2.0);
    let (off_l, _) = render_master(&two_channel_project([false, false]), 2.0);

    let peak = peak_of(&off_l);
    let difference = worst_difference(&on_l, &off_l);
    println!(
        "two strips: worst difference {difference:.4} against a {peak:.3} linear peak \
         ({:.1}% of peak)",
        100.0 * difference / peak
    );
    assert!(
        difference > peak * 0.01,
        "two console strips differed from the linear sum by only {difference:.3e} \
         against a {peak:.3} peak -- the decode is not seeing the encode"
    );
}

/// Half on, half off. This is Adam's *"the decode stage is mixed with master
/// to pick up any channels that don't have it switched on"*, and it is the
/// reason a bus needs **two** input accumulators rather than one.
///
/// Stated as superposition between the two groups, which is the exact form of
/// "decode the encoded sum, then add the linear one": the mix of two console
/// strips and a linear one must equal the two console strips rendered alone,
/// plus the linear one rendered alone. Nothing weaker says it, because a
/// single-accumulator implementation -- decode everything at the summing
/// point -- would pass every other test in this file while putting a strip
/// whose switch is *off* through an `asin` it never opted into.
#[test]
fn the_linear_group_and_the_console_group_superpose() {
    // Three sustained notes. The first two are console-encoded and interact;
    // the third is linear and must arrive untouched.
    let project = |console: [bool; 3], muted: [bool; 3]| {
        let mut channels = Vec::new();
        for (index, pitch) in [40u8, 47, 52].into_iter().enumerate() {
            let mut channel = one_note_channel(DeviceKind::PolySynth, pitch);
            channel.setup.channel.console = console[index];
            channel.setup.channel.muted = muted[index];
            channels.push(channel);
        }
        Project {
            channels,
            ..Project::default()
        }
    };
    let consoles = [true, true, false];

    // Muted rather than removed, so all three renders walk the same graph --
    // the trick `gain_structure_tests::pad_and_drums` uses for the same
    // reason.
    let (all_l, _) = render_master(&project(consoles, [false; 3]), 2.0);
    let (encoded_l, _) = render_master(&project(consoles, [false, false, true]), 2.0);
    let (linear_l, _) = render_master(&project(consoles, [true, true, false]), 2.0);

    let peak = peak_of(&all_l);
    assert!(peak > 0.05, "the fixture rendered nothing to compare: {peak}");
    let error = all_l
        .iter()
        .zip(encoded_l.iter().zip(linear_l.iter()))
        .fold(0.0f32, |worst, (all, (encoded, linear))| {
            worst.max((all - encoded - linear).abs())
        });
    println!(
        "two console strips plus one linear one: superposition error {error:.3e}          against a {peak:.3} peak"
    );
    assert!(
        error < 1e-6,
        "the linear strip did not pass through the summing point untouched:          {error:.3e}. It is being decoded along with the encoded sum, which is          the single-accumulator bug this test exists for."
    );

    // And the encoded pair really is doing something, or the superposition
    // above would be the trivial linear case.
    let (all_linear_l, _) = render_master(&project([false; 3], [false; 3]), 2.0);
    let difference = worst_difference(&all_l, &all_linear_l);
    assert!(
        difference > peak * 0.01,
        "the console pair changed the mix by only {difference:.3e}"
    );
}

/// A console strip **on its own** is transparent even when linear strips
/// share its summing point, because it is alone in the encoded accumulator.
///
/// Worth its own test rather than folding into the null above: this is the
/// case a user meets first -- one strip switched on in an otherwise ordinary
/// mix -- and "nothing happened" is the correct answer, not a bug report.
/// Console summing is a property of strips that opted in *together*.
#[test]
fn one_console_strip_among_linear_ones_is_still_transparent() {
    let (mixed_l, mixed_r) = render_master(&two_channel_project([true, false]), 2.0);
    let (linear_l, linear_r) = render_master(&two_channel_project([false, false]), 2.0);
    let peak = peak_of(&linear_l);
    let error = worst_difference(&mixed_l, &linear_l).max(worst_difference(&mixed_r, &linear_r));
    println!("one of two switched on: {error:.3e} against a {peak:.3} peak");
    assert!(
        error < 1e-6,
        "one console strip beside a linear one differed by {error:.3e}"
    );
}

/// Console mode is out by default, and off is bit-identical to a tree
/// without it. This is what lets every characterization number in
/// `gain_structure_tests.rs` stand unchanged.
#[test]
fn console_off_is_the_default_and_changes_nothing() {
    let project = two_channel_project([false, false]);
    assert!(
        project.channels.iter().all(|c| !c.setup.channel.console),
        "a channel came into existence with console on"
    );
    assert!(
        project.buses.iter().all(|b| !b.bus.console),
        "a bus came into existence with console on"
    );
    // Two block sizes, because a project with no encoded feed must not have
    // allocated an accumulator that could hold audio across a boundary.
    let (a_l, _) = render_master_in_blocks(&project, 1.0, 1024);
    let (b_l, _) = render_master_in_blocks(&project, 1.0, 64);
    assert_eq!(
        worst_difference(&a_l, &b_l),
        0.0,
        "console-off rendered differently at two block sizes"
    );
}

/// Console-on renders the same at any block size, which is the same contract
/// `compensation_renders_the_same_at_any_block_size` holds the mixer to and
/// the reason a bounce can be trusted to match a live take: the offline
/// renderer is this path at a different block size.
#[test]
fn console_renders_the_same_at_any_block_size() {
    let project = two_channel_project([true, true]);
    let (reference_l, reference_r) = render_master_in_blocks(&project, 1.0, 1024);
    for block in [1, 17, 64, 512, 2048] {
        let (l, r) = render_master_in_blocks(&project, 1.0, block);
        let error = worst_difference(&reference_l, &l).max(worst_difference(&reference_r, &r));
        assert_eq!(
            error, 0.0,
            "console summing at {block}-frame blocks differed by {error:.3e}"
        );
    }
}

/// **Which fader is the drive and which is the volume**, which is how Adam
/// said he expects to use it: *"if I want those sources to be quieter I need
/// to turn down the mixer busses that have this analog/nonlinear summing mode
/// enabled."*
///
/// Nothing was built to make this true -- it falls out of the bus fader
/// sitting after the decode in the block order -- but it does have to be
/// true, so it is a test rather than a note.
#[test]
fn the_bus_fader_is_volume_and_the_channel_faders_are_drive() {
    let scaled = |project: &Project, gain: f32| {
        let (l, _) = render_master(project, 2.0);
        l.into_iter().map(|s| s * gain).collect::<Vec<_>>()
    };

    let reference = two_channel_project([true, true]);
    let (reference_l, _) = render_master(&reference, 2.0);
    let peak = peak_of(&reference_l);

    // Half on the master fader: the same waveform, half the size. The decode
    // has already happened by the time the fader is reached.
    let mut master_down = two_channel_project([true, true]);
    master_down.buses[0].bus.volume = 0.5;
    let (master_down_l, _) = render_master(&master_down, 2.0);
    let volume_error = worst_difference(&master_down_l, &scaled(&reference, 0.5));
    assert!(
        volume_error < 1e-6,
        "pulling the destination bus down changed the character by {volume_error:.3e}: \
         the fader is not after the decode"
    );

    // Half on both channel faders: *not* the same waveform, because they
    // arrive at the decode smaller and the summing law does less to them.
    let mut channels_down = two_channel_project([true, true]);
    for channel in &mut channels_down.channels {
        channel.setup.channel.volume = 0.5;
    }
    let (channels_down_l, _) = render_master(&channels_down, 2.0);
    let drive_error = worst_difference(&channels_down_l, &scaled(&reference, 0.5));
    println!(
        "bus fader: {volume_error:.3e} from a pure scale. channel faders: \
         {drive_error:.4} from a pure scale, against a {peak:.3} peak"
    );
    assert!(
        drive_error > peak * 0.001,
        "pulling the channel faders down was pure level: the drive control does \
         not control drive"
    );
}

/// Nesting, end to end: a group bus that is itself console-on encodes at its
/// own output and the master decodes it, with no parallel decoder and no
/// special case. This is what makes step 04's groups free.
#[test]
fn a_console_bus_nests_inside_another_summing_point() {
    let group_project = |bus_console: bool| {
        let mut project = two_channel_project([true, true]);
        for channel in &mut project.channels {
            channel.setup.channel.bus = 1;
        }
        project.buses[1].bus.console = bus_console;
        project
    };

    // A group whose own output is linear, versus one that is console-encoded
    // into the master. The group is alone at the master, so the encode and
    // the master's decode must cancel exactly -- the same null as a lone
    // channel, one level up.
    let (linear_l, _) = render_master(&group_project(false), 2.0);
    let (encoded_l, _) = render_master(&group_project(true), 2.0);
    let peak = peak_of(&linear_l);
    assert!(peak > 0.05, "the group fixture rendered nothing: {peak}");
    let error = worst_difference(&linear_l, &encoded_l);
    println!("lone console group into the master: {error:.3e} against a {peak:.3} peak");
    assert!(
        error < 1e-6,
        "a lone console-on group was coloured by {error:.3e} on its way through \
         the master, so nesting is not the identity it has to be"
    );

    // And the master refuses a switch of its own, since it feeds nothing: an
    // encode there would go into a sum nothing decodes.
    let mut master_on = group_project(false);
    master_on.buses[0].bus.console = true;
    let (master_on_l, _) = render_master(&master_on, 2.0);
    assert_eq!(
        worst_difference(&master_on_l, &linear_l),
        0.0,
        "a console switch on the master changed the mix"
    );
}
