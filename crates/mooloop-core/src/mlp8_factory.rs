//! The ML-P8 factory bank.
//!
//! Eight patches, defined here as data rather than as files, for the same two
//! reasons [`crate::mlm1_factory`] gives. The DSP tests need the same values
//! the preset seeder writes — a bank that only existed as TOML would have to
//! be parsed back to be tested, and the thing under test would be the parser.
//! And a patch is a set of parameters, so [`MlP8Params`] is its natural form;
//! the bundle on disk is a serialization of it, not the other way round.
//!
//! `docs/plans/poly-synth-v2/07-poly-factory-patches.md` names the list, and
//! the claim it is here to prove is ML-P8's whole argument: **the instrument
//! is a three-oscillator network with its own modulation, not a supersaw
//! followed by a chorus.** So the plan's standing constraint is a constraint
//! on this file: the first seven patches run at Unison 1x with Chorus OFF,
//! and if they do not sound clearly distinct the fix is the network, not a
//! duplicator. Five of the eight also leave Drift at 0.
//!
//! These are generator presets, not channel presets. ML-P8's modulation is
//! [`MlP8Params::routes`] and [`MlP8Params::lfo`] — inside the device, where
//! a polysynth needs it — so a patch carries no [`crate::modulation::ModRack`]
//! and needs no rescoping. That is the difference from the ML-M1 bank, whose
//! Sequence Bleep is nothing without a channel rack, and it is the same
//! choice the DS-01 bank makes.

use crate::mlp8::{
    osc_param, xmod_index, MlP8Chorus, MlP8FilterMode, MlP8LfoParams, MlP8LfoRetrigger,
    MlP8LfoWave, MlP8ModDest, MlP8ModSource, MlP8Route, MlP8Routes, MlP8Unison, PARAM_DRIVE,
    PARAM_FILTER_CUTOFF, PARAM_FILTER_RESONANCE, PARAM_NOISE_COLOR, PARAM_SUB_LEVEL,
    PARAM_VOICE_FEEDBACK, PARAM_XMOD_BASE,
};
use crate::{MlP8Params, OscParams, OscWave, SubOctave, SubSource, SubWave, SyncSource,
    OSC_OFFSET_PULSE_WIDTH, OSC_OFFSET_SEMITONES};

/// One factory patch: presentation metadata plus the complete parameter set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MlP8FactoryPatch {
    pub name: &'static str,
    pub category: &'static str,
    pub tags: &'static [&'static str],
    /// One line on what the patch is for, and what it demonstrates.
    pub description: &'static str,
    pub params: MlP8Params,
}

/// How many patches the bank ships. Named because the seeder's test asserts
/// the count and a bank that quietly shrank would otherwise still pass.
pub const BANK_SIZE: usize = 8;

/// The bank, in the order the plan lists it: the reference first, then the
/// six that make their case at Unison 1x, then the one that is about the
/// finishers.
pub fn patches() -> [MlP8FactoryPatch; BANK_SIZE] {
    [
        init_saw(),
        crosswire_brass(),
        furnace_stab(),
        cold_metal(),
        sub_pressure(),
        servo_pad(),
        broken_choir(),
        wide_machine(),
    ]
}

/// Every patch starts from the default and changes what it needs, so the diff
/// between this file and `MlP8Params::default()` *is* the "reachable from Init
/// Saw" claim, checkable by reading rather than by trusting a comment.
fn base() -> MlP8Params {
    MlP8Params::default()
}

/// Author one internal route under an explicit durable id.
///
/// The id is written down rather than minted because it is what an automation
/// lane persists: a patch that renumbered its routes between releases would
/// silently re-point every lane drawn against it. Ids are per patch and start
/// at zero, which is what [`MlP8Routes::default`] mints from anyway.
fn route(
    routes: &mut MlP8Routes,
    id: u16,
    source: MlP8ModSource,
    dest: MlP8ModDest,
    amount: f32,
) {
    let accepted = routes.upsert(MlP8Route {
        id,
        source,
        dest,
        amount,
    });
    debug_assert!(accepted, "factory route {id} was refused");
}

/// A route onto an authored parameter, which is most of them.
fn to_param(
    routes: &mut MlP8Routes,
    id: u16,
    source: MlP8ModSource,
    param: u32,
    amount: f32,
) {
    route(routes, id, source, MlP8ModDest::Param { id: param }, amount);
}

/// The reference patch, and the honest zero the other seven are measured
/// from: one saw at the calibrated level, eight clean voices, a filter that
/// is open and out of the way, and nothing switched on.
///
/// It is `MlP8Params::default()` unchanged on purpose. The gain contract is
/// calibrated against exactly this signal (`crate::gain`), so a bank whose
/// reference patch differed from the device's own default would be measuring
/// something the contract does not describe.
fn init_saw() -> MlP8FactoryPatch {
    MlP8FactoryPatch {
        name: "Init Saw",
        category: "Init",
        tags: &["init", "reference", "saw"],
        description: "One saw at the reference level. The zero the rest of the bank moves from.",
        params: base(),
    }
}

/// **Filter Env drives XMOD, and velocity shapes both the filter and the amp.**
///
/// The brass character is not the filter sweep: it is the *spectrum* opening
/// as the envelope pushes oscillator 1 into oscillator 2's phase, so the
/// harmonics arrive before the filter does. A filter sweep alone is a filter
/// sweep on any synth; this is the network doing it, which is why the route
/// from Filter Env to `XM 1>2` is the patch.
fn crosswire_brass() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Saw,
        level: 0.7,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Saw,
        semitones: 0.0,
        cents: 7.0,
        level: 0.5,
        ..OscParams::default()
    };
    // A modest static amount, so the route has somewhere to travel from
    // rather than starting at the floor.
    params.xmod[xmod_index(0, 1)] = 12.0;
    params.filter_mode = MlP8FilterMode::Lp12;
    params.filter_cutoff = 0.42;
    params.filter_resonance = 0.18;
    params.filter_env_amount = 0.55;
    params.filter_attack = 0.02;
    params.filter_decay = 0.5;
    params.filter_sustain = 0.35;
    params.filter_release = 0.25;
    params.filter_velocity = 0.45;
    params.amp_velocity = 0.8;
    params.attack = 0.012;
    params.decay = 0.35;
    params.sustain = 0.75;
    params.release = 0.25;

    let mut routes = MlP8Routes::default();
    // The patch. The envelope opens the crosswire, so playing harder both
    // brightens the filter and drives the network harder.
    to_param(&mut routes, 0, MlP8ModSource::FilterEnv, PARAM_XMOD_BASE + xmod_index(0, 1) as u32, 46.0);
    // A little key tracking on the same crosswire, so the top of the keyboard
    // does not simply get thinner.
    to_param(&mut routes, 1, MlP8ModSource::Key, PARAM_XMOD_BASE + xmod_index(0, 1) as u32, 14.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Crosswire Brass",
        category: "Keys",
        tags: &["brass", "xmod", "velocity", "env"],
        description: "Filter Env opens XM 1>2, so the spectrum arrives before the filter does.",
        params,
    }
}

/// **Voice Feedback, drive and LP24 make a hard percussive stab.**
///
/// The loop around the filter is what makes this more than a plucked saw: at
/// this depth it rings the resonance rather than merely thickening it, and the
/// drive sits *inside* the loop so the ring is bounded by the shaper instead
/// of by luck. Amp sustain is zero, so the whole sound is the transient.
fn furnace_stab() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Saw,
        level: 0.62,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Pulse,
        semitones: -12.0,
        cents: 0.0,
        level: 0.34,
        pulse_width: 0.28,
    };
    params.filter_mode = MlP8FilterMode::Lp24;
    params.filter_cutoff = 0.30;
    params.filter_resonance = 0.62;
    params.filter_env_amount = 0.72;
    params.filter_attack = 0.001;
    params.filter_decay = 0.16;
    params.filter_sustain = 0.0;
    params.filter_release = 0.1;
    params.drive = 0.55;
    params.voice_feedback = 0.42;
    params.attack = 0.001;
    params.decay = 0.22;
    params.sustain = 0.0;
    params.release = 0.12;
    params.amp_velocity = 0.9;
    // Headroom: a resonant loop with drive in it is the loudest thing this
    // bank does, and the plan asks factory patches to leave some.
    params.master_volume = 0.8;

    let mut routes = MlP8Routes::default();
    // Harder playing tightens the loop rather than only opening the filter,
    // which is what makes the stab's edge follow the hand.
    to_param(&mut routes, 0, MlP8ModSource::Velocity, PARAM_VOICE_FEEDBACK, 22.0);
    to_param(&mut routes, 1, MlP8ModSource::AmpEnv, PARAM_DRIVE, 18.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Furnace Stab",
        category: "Keys",
        tags: &["stab", "feedback", "drive", "lp24"],
        description: "Voice Feedback and drive inside the loop, under a 24 dB filter. All transient.",
        params,
    }
}

/// **Bidirectional XMOD plus sync reaches a stable inharmonic spectrum.**
///
/// Two oscillators modulating each other is the case a one-directional FM
/// pair cannot make: the spectrum is not a carrier with sidebands but a
/// mutual solution, and it stays put rather than wandering because oscillator
/// 3 is hard-synced to oscillator 1. That is what makes this a bell instead of
/// a noise. The filter is nearly out of the way on purpose — the timbre is the
/// network's, not the filter's.
fn cold_metal() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Sine,
        level: 0.58,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Sine,
        semitones: 7.0,
        cents: 0.0,
        level: 0.42,
        ..OscParams::default()
    };
    params.osc[2] = OscParams {
        wave: OscWave::Saw,
        semitones: 19.0,
        cents: 3.0,
        level: 0.22,
        ..OscParams::default()
    };
    // Both directions, at different depths: equal amounts sound like one
    // effect, and the asymmetry is what gives the spectrum a direction.
    params.xmod[xmod_index(0, 1)] = 38.0;
    params.xmod[xmod_index(1, 0)] = 24.0;
    // Synced to oscillator 1, so the inharmonic partial is locked to the
    // played pitch rather than beating against it.
    params.sync_source[2] = SyncSource::Osc1;
    params.filter_mode = MlP8FilterMode::Hp12;
    params.filter_cutoff = 0.22;
    params.filter_resonance = 0.1;
    params.filter_env_amount = 0.0;
    params.attack = 0.001;
    params.decay = 1.4;
    params.sustain = 0.12;
    params.release = 0.9;
    params.amp_velocity = 0.85;
    params.master_volume = 0.9;

    let mut routes = MlP8Routes::default();
    // The strike: the amplitude envelope pulls the crosswire back as the note
    // decays, so the attack is metallic and the tail is not.
    to_param(&mut routes, 0, MlP8ModSource::AmpEnv, PARAM_XMOD_BASE + xmod_index(0, 1) as u32, 34.0);
    to_param(&mut routes, 1, MlP8ModSource::Velocity, PARAM_XMOD_BASE + xmod_index(1, 0) as u32, 26.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Cold Metal",
        category: "Bell",
        tags: &["bell", "xmod", "sync", "inharmonic"],
        description: "Bidirectional XM with Osc 3 synced: an inharmonic spectrum that stays put.",
        params,
    }
}

/// **The derived sub stays solid underneath noise-modulated carriers.**
///
/// The sub is derived from oscillator 1 rather than being a fourth
/// oscillator, so the test is whether it survives having its source's phase
/// modulated by noise. It does, because the derivation happens before the
/// noise reaches the carrier — which is the whole reason it is a derived sub
/// and not another voice to detune.
fn sub_pressure() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Saw,
        level: 0.5,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Pulse,
        semitones: 0.0,
        cents: -6.0,
        level: 0.3,
        pulse_width: 0.42,
    };
    params.sub_level = 0.62;
    params.sub_octave = SubOctave::Minus1;
    params.sub_wave = SubWave::Sine;
    params.sub_source = SubSource::Osc1;
    params.noise_level = 0.06;
    params.noise_color = -35.0;
    params.noise_to_osc[0] = 22.0;
    params.noise_to_osc[1] = 34.0;
    params.filter_mode = MlP8FilterMode::Lp12;
    params.filter_cutoff = 0.34;
    params.filter_resonance = 0.12;
    params.filter_env_amount = 0.3;
    params.filter_keytrack = 0.6;
    params.filter_attack = 0.004;
    params.filter_decay = 0.3;
    params.filter_sustain = 0.4;
    params.attack = 0.004;
    params.decay = 0.3;
    params.sustain = 0.8;
    params.release = 0.2;
    params.master_volume = 0.85;

    let mut routes = MlP8Routes::default();
    // The noise into the carriers is a texture, not a constant: the envelope
    // makes it a breath at the front of the note.
    to_param(&mut routes, 0, MlP8ModSource::AmpEnv, crate::mlp8::PARAM_NOISE_TO_OSC_BASE + 1, -24.0);
    to_param(&mut routes, 1, MlP8ModSource::Velocity, PARAM_NOISE_COLOR, 30.0);
    // The sub itself does not move with any of it.
    to_param(&mut routes, 2, MlP8ModSource::Key, PARAM_SUB_LEVEL, -18.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Sub Pressure",
        category: "Bass",
        tags: &["bass", "sub", "noise", "keytrack"],
        description: "A derived sub that holds while noise modulates the carriers above it.",
        params,
    }
}

/// **The device's own LFO and per-voice envelopes move it, with no channel
/// routes at all.**
///
/// This is the patch that has to be convincing without the modulation shelf,
/// because ML-P8's argument is that it makes complete sounds on its own. The
/// LFO is slewed and warped rather than a plain sine, and it reaches three
/// different destinations at three depths; the per-voice envelopes do the
/// rest. Retrigger is per chord, so a held pad breathes together instead of
/// each note starting its own cycle.
fn servo_pad() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Saw,
        level: 0.46,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Pulse,
        semitones: 0.0,
        cents: 9.0,
        level: 0.4,
        pulse_width: 0.5,
    };
    params.osc[2] = OscParams {
        wave: OscWave::Triangle,
        semitones: -12.0,
        cents: -5.0,
        level: 0.3,
        ..OscParams::default()
    };
    params.filter_mode = MlP8FilterMode::Lp12;
    params.filter_cutoff = 0.4;
    params.filter_resonance = 0.28;
    params.filter_env_amount = 0.35;
    params.filter_attack = 0.4;
    params.filter_decay = 1.2;
    params.filter_sustain = 0.5;
    params.filter_release = 1.0;
    params.attack = 0.35;
    params.decay = 0.8;
    params.sustain = 0.85;
    params.release = 1.1;
    params.amp_velocity = 0.5;
    params.lfo = MlP8LfoParams {
        wave: MlP8LfoWave::Triangle,
        synced: false,
        rate_hz: 0.42,
        phase: 0.0,
        // Asymmetric, so the rise and the fall take different times and the
        // movement does not read as a metronome.
        warp: -0.35,
        slew: 0.28,
        retrigger: MlP8LfoRetrigger::Chord,
        ..MlP8LfoParams::default()
    };
    params.master_volume = 0.85;

    let mut routes = MlP8Routes::default();
    to_param(&mut routes, 0, MlP8ModSource::Lfo, PARAM_FILTER_CUTOFF, 22.0);
    to_param(&mut routes, 1, MlP8ModSource::Lfo, osc_param(1, OSC_OFFSET_PULSE_WIDTH), 34.0);
    // Opposite sign on the third oscillator's tuning, so the LFO widens the
    // patch rather than moving all of it the same way.
    to_param(&mut routes, 2, MlP8ModSource::Lfo, osc_param(2, OSC_OFFSET_SEMITONES), -6.0);
    to_param(&mut routes, 3, MlP8ModSource::FilterEnv, PARAM_FILTER_RESONANCE, 18.0);
    // Per voice, so the notes of a chord sit at slightly different levels.
    route(&mut routes, 4, MlP8ModSource::Velocity, MlP8ModDest::VcaLevel, -20.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Servo Pad",
        category: "Pad",
        tags: &["pad", "lfo", "internal", "no-rack"],
        description: "Moves entirely on its own LFO and envelopes. No channel modulation at all.",
        params,
    }
}

/// **Three differently tuned oscillators interacting, not merely stacking.**
///
/// A stack of three detuned saws is a supersaw and would prove nothing. Here
/// each oscillator modulates the next around a ring at small depths, so the
/// three tunings *interfere* — the beating is in the spectrum rather than only
/// in the amplitude, and muting any one of them changes the other two.
fn broken_choir() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Triangle,
        level: 0.44,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Saw,
        semitones: 12.0,
        cents: -11.0,
        level: 0.34,
        ..OscParams::default()
    };
    params.osc[2] = OscParams {
        wave: OscWave::Pulse,
        semitones: 7.0,
        cents: 14.0,
        level: 0.32,
        pulse_width: 0.62,
    };
    // A ring rather than a star: 1 into 2, 2 into 3, 3 back into 1. Small
    // depths, because the point is interference and not a new timbre.
    params.xmod[xmod_index(0, 1)] = 16.0;
    params.xmod[xmod_index(1, 2)] = 14.0;
    params.xmod[xmod_index(2, 0)] = 11.0;
    // Just enough self-feedback on the triangle to give the ring something
    // with harmonics to work on.
    params.osc_feedback[0] = 18.0;
    params.filter_mode = MlP8FilterMode::Bp12;
    params.filter_cutoff = 0.52;
    params.filter_resonance = 0.2;
    params.filter_env_amount = 0.22;
    params.filter_keytrack = 0.5;
    params.filter_attack = 0.12;
    params.filter_decay = 0.6;
    params.filter_sustain = 0.6;
    params.attack = 0.09;
    params.decay = 0.5;
    params.sustain = 0.8;
    params.release = 0.5;
    params.amp_velocity = 0.6;
    // Drift, not Detune: the three tunings are authored, and this makes the
    // eight slots disagree about them very slightly.
    params.drift = 0.22;
    params.master_volume = 0.85;

    let mut routes = MlP8Routes::default();
    to_param(&mut routes, 0, MlP8ModSource::Gate, PARAM_XMOD_BASE + xmod_index(2, 0) as u32, 12.0);
    to_param(&mut routes, 1, MlP8ModSource::Key, osc_param(2, OSC_OFFSET_PULSE_WIDTH), -22.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Broken Choir",
        category: "Pad",
        tags: &["choir", "xmod", "drift", "interference"],
        description: "Three tunings modulating each other in a ring. Muting one changes the others.",
        params,
    }
}

/// **The finishers, and the only patch in the bank allowed to use them.**
///
/// Unison, Detune, Spread, Drift and the chorus all at once, over a patch
/// that is deliberately simple underneath — because the point is to hear what
/// the finishers do, not to hear them rescue something. Unison 4x spends the
/// eight slots rather than growing them, so this plays two notes at a time and
/// that is the honest trade rather than a limitation to hide.
fn wide_machine() -> MlP8FactoryPatch {
    let mut params = base();
    params.osc[0] = OscParams {
        wave: OscWave::Saw,
        level: 0.6,
        ..OscParams::default()
    };
    params.osc[1] = OscParams {
        wave: OscWave::Saw,
        semitones: -12.0,
        cents: 5.0,
        level: 0.34,
        ..OscParams::default()
    };
    params.filter_mode = MlP8FilterMode::Lp12;
    params.filter_cutoff = 0.55;
    params.filter_resonance = 0.14;
    params.filter_env_amount = 0.28;
    params.filter_attack = 0.05;
    params.filter_decay = 0.8;
    params.filter_sustain = 0.55;
    params.attack = 0.02;
    params.decay = 0.4;
    params.sustain = 0.8;
    params.release = 0.4;
    params.unison = MlP8Unison::X4;
    params.detune = 0.42;
    params.spread = 0.7;
    params.drift = 0.3;
    params.chorus = MlP8Chorus::Two;
    // Unison spends the pool rather than growing it, but four voices at full
    // level still *sum* -- the device does not normalise them, by design, and
    // the plan says so. So the patch pays for its own width here rather than
    // asking the instrument to quietly turn four voices down.
    params.master_volume = 0.5;

    let mut routes = MlP8Routes::default();
    // The one route, so the width is the finishers' and not a modulation
    // trick standing in for them.
    to_param(&mut routes, 0, MlP8ModSource::FilterEnv, PARAM_FILTER_CUTOFF, 20.0);
    params.routes = routes;

    MlP8FactoryPatch {
        name: "Wide Machine",
        category: "Lead",
        tags: &["unison", "detune", "spread", "chorus"],
        description: "Unison 4x, Detune, Spread and Chorus II over a deliberately plain patch.",
        params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mlp8;

    /// Names are what a browser shows and what a file is called, so a
    /// duplicate would silently collapse two patches into one bundle.
    #[test]
    fn every_patch_has_its_own_name() {
        let bank = patches();
        for (index, patch) in bank.iter().enumerate() {
            assert!(!patch.name.is_empty());
            assert!(!patch.description.is_empty(), "{} has no line", patch.name);
            for other in &bank[index + 1..] {
                assert_ne!(patch.name, other.name, "two patches called {}", patch.name);
            }
        }
    }

    /// Every value has to be inside the range its own descriptor states, or
    /// the doctor would repair a shipped patch on the way in — which is the
    /// bank telling the user their factory content was broken.
    #[test]
    fn every_value_is_inside_its_descriptor() {
        for patch in patches() {
            for descriptor in mlp8::DESCRIPTORS.iter() {
                let value = mlp8::get(&patch.params, descriptor.id)
                    .unwrap_or_else(|| panic!("{} has no value for {}", patch.name, descriptor.id));
                assert!(
                    value >= descriptor.min && value <= descriptor.max,
                    "{}: {} is {value}, outside {}..{}",
                    patch.name,
                    descriptor.name,
                    descriptor.min,
                    descriptor.max
                );
            }
        }
    }

    /// A route's amount is a signed percentage and its destination has to be
    /// one the device accepts. An out-of-range amount would be clamped on the
    /// way in, which is the same "your factory content was broken" message
    /// the descriptor check above exists to avoid.
    #[test]
    fn every_authored_route_is_legal_and_durable() {
        for patch in patches() {
            let mut seen: Vec<u16> = Vec::new();
            for route in patch.params.routes.iter() {
                assert!(
                    route.dest.is_legal(),
                    "{}: route {} names a structural destination",
                    patch.name,
                    route.id
                );
                assert!(
                    (-100.0..=100.0).contains(&route.amount),
                    "{}: route {} is {}%",
                    patch.name,
                    route.id,
                    route.amount
                );
                assert!(
                    route.amount != 0.0,
                    "{}: route {} does nothing",
                    patch.name,
                    route.id
                );
                assert!(
                    !seen.contains(&route.id),
                    "{}: route id {} is used twice",
                    patch.name,
                    route.id
                );
                seen.push(route.id);
            }
        }
    }

    /// The plan's standing constraint, and the reason it is a test rather
    /// than a note: **a patch that needs a duplicator to be interesting has
    /// not proved the network.** Seven of the eight run one voice per note
    /// with the chorus off, and only Wide Machine — which is *about* the
    /// finishers — is allowed to use them.
    #[test]
    fn the_bank_makes_its_case_before_the_finishers() {
        let bank = patches();
        let plain = bank
            .iter()
            .filter(|patch| {
                patch.params.unison == MlP8Unison::X1 && patch.params.chorus == MlP8Chorus::Off
            })
            .count();
        assert!(
            plain >= 7,
            "only {plain} patches stand up at Unison 1x with Chorus OFF"
        );
        // And the plan asks at least four to leave Drift alone as well, so
        // the bank is not uniformly smeared.
        let undrifted = bank.iter().filter(|patch| patch.params.drift == 0.0).count();
        assert!(undrifted >= 4, "only {undrifted} patches leave Drift at 0");

        // The one that is allowed the finishers actually uses them, or the
        // count above is satisfied by a bank that simply never turns them on.
        let wide = wide_machine().params;
        assert_ne!(wide.unison, MlP8Unison::X1);
        assert_ne!(wide.chorus, MlP8Chorus::Off);
        assert!(wide.detune > 0.0 && wide.spread > 0.0 && wide.drift > 0.0);
    }

    /// Init Saw is the reference patch, and the gain contract is calibrated
    /// against exactly the device's default. A bank whose reference differed
    /// from that default would be measuring something the contract does not
    /// describe.
    #[test]
    fn init_saw_is_the_device_default() {
        assert_eq!(init_saw().params, MlP8Params::default());
    }

    /// Four patches have to demonstrate four *materially different* internal
    /// modulation relationships, which is what the step asks for and what
    /// stops the bank being one idea eight times. Compared by the set of
    /// (source, destination) pairs, because that is the relationship; the
    /// depths are taste.
    #[test]
    fn the_modulating_patches_use_different_relationships() {
        let shapes: Vec<Vec<(MlP8ModSource, MlP8ModDest)>> =
            [crosswire_brass(), furnace_stab(), cold_metal(), servo_pad()]
                .iter()
                .map(|patch| {
                    patch
                        .params
                        .routes
                        .iter()
                        .map(|route| (route.source, route.dest))
                        .collect()
                })
                .collect();
        for (index, shape) in shapes.iter().enumerate() {
            assert!(!shape.is_empty(), "patch {index} has no internal routes");
            for other in &shapes[index + 1..] {
                assert_ne!(shape, other, "two patches route the same way");
            }
        }
    }

    /// Every patch has to be reachable from Init Saw, which means it differs
    /// from the default rather than being one. A patch identical to another
    /// is the same failure seen from the other side.
    #[test]
    fn every_patch_after_the_first_is_its_own_sound() {
        let bank = patches();
        for (index, patch) in bank.iter().enumerate().skip(1) {
            assert_ne!(
                patch.params,
                MlP8Params::default(),
                "{} is the default patch",
                patch.name
            );
            for other in &bank[index + 1..] {
                assert_ne!(
                    patch.params, other.params,
                    "{} and {} are the same patch",
                    patch.name, other.name
                );
            }
        }
    }
}
