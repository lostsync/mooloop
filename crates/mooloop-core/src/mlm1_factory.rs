//! The ML-M1 factory bank.
//!
//! Six patches, defined here as data rather than as files, for two reasons.
//! The DSP tests need the same values the preset seeder writes — a bank that
//! only existed as TOML would have to be parsed back to be tested, and the
//! thing under test would be the parser. And a patch is a set of parameters,
//! so `MlM1Params` is its natural form; the bundle on disk is a serialization
//! of it, not the other way round.
//!
//! Each patch exists to prove something, per
//! `docs/plans/mono-synth-v2/08-mono-factory-patches.md`, and the step's
//! standing requirement is that every one of them stays a few knob moves from
//! the default saw. Where a patch needed a setting the defaults made awkward
//! to reach, that is recorded in `00-status.md` as a finding rather than
//! worked around here.

use crate::modulation::{
    ModLfoParams, ModLfoWaveform, ModPolarity, ModRack, ModRoute, ModTimeDivision, ModulatorParams,
    ParamAddr, ParamOwner,
};
use crate::{
    synth_osc_param, EffectTarget, EnvTrigger, FilterModel, GlideMode, MlM1Params, NotePriority,
    OscWave, OSC_OFFSET_PULSE_WIDTH, SYNTH_PARAM_FILTER_CUTOFF,
};

/// The channel a factory patch's modulation routes are written against.
///
/// Routes name their destination channel absolutely, so a stored rack is only
/// correct on the channel it was saved from. The bank picks channel 0 and the
/// loader re-scopes on the way in; see [`ModRack`] and
/// `mooloop_project::rescope_modulation`.
const AUTHORED_SCOPE: EffectTarget = EffectTarget::Channel(0);

/// One factory patch: presentation metadata plus the complete channel-level
/// state an ML-M1 instrument needs.
///
/// `modulation` is part of the patch rather than an afterthought because the
/// ML-M1 has no device-local LFO by design — general modulation is channel
/// state (`crate::mlm1`). A bank that could not carry a rack could not ship
/// Sequence Bleep at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FactoryPatch {
    pub name: &'static str,
    pub category: &'static str,
    pub tags: &'static [&'static str],
    /// One line on what the patch is for, and what it demonstrates.
    pub description: &'static str,
    pub params: MlM1Params,
    pub modulation: ModRack,
}

/// The bank, in the order it should be presented.
pub fn patches() -> [FactoryPatch; 6] {
    [
        round_bass(),
        rubber_bass(),
        acid_line(),
        snap_pluck(),
        porta_lead(),
        sequence_bleep(),
    ]
}

/// Every patch starts from the default and changes what it needs, so the diff
/// between this file and `MlM1Params::default()` *is* the "few knob moves"
/// claim, checkable by reading rather than by trusting a comment.
fn base() -> MlM1Params {
    MlM1Params::default()
}

/// Ladder weight and low-end stability under resonance.
///
/// The pairing of a low cutoff with resonance past half is the whole patch:
/// on a filter that loses its bass to resonance this is a thin buzz, and on
/// the Ladder it stays a bass note. The sub oscillator makes that audible
/// rather than merely measurable, and keeps its default detune, which beats
/// slowly against the octave instead of doubling it exactly.
fn round_bass() -> FactoryPatch {
    let mut params = base();
    params.osc[2].level = 0.55;
    params.filter_cutoff = 0.28;
    params.filter_resonance = 0.55;
    params.filter_env_amount = 0.35;
    params.filter_decay = 0.35;
    params.filter_sustain = 0.25;
    params.filter_keytrack = 0.35;
    params.drive = 0.25;
    params.sustain = 0.85;
    FactoryPatch {
        name: "Round Bass",
        category: "ML-M1",
        tags: &["bass", "ladder"],
        description: "Ladder weight: low cutoff, high resonance, bass intact.",
        params,
        modulation: ModRack::default(),
    }
}

/// Filter envelope, resonance and pre-drive working on each other.
///
/// Everything here is one stage feeding the next: the envelope sweeps into a
/// resonant peak, and the drive ahead of the filter decides how hard the peak
/// is hit. Turning any one of the three down audibly changes what the other
/// two do — that interaction is the patch.
fn rubber_bass() -> FactoryPatch {
    let mut params = base();
    params.osc[2].wave = OscWave::Pulse;
    params.osc[2].level = 0.4;
    params.filter_cutoff = 0.18;
    params.filter_resonance = 0.75;
    params.filter_env_amount = 0.6;
    params.filter_decay = 0.18;
    params.filter_sustain = 0.0;
    params.filter_keytrack = 0.5;
    params.drive = 0.45;
    params.accent = 0.3;
    params.sustain = 0.9;
    FactoryPatch {
        name: "Rubber Bass",
        category: "ML-M1",
        tags: &["bass", "ladder", "envelope"],
        description: "Envelope into resonance, with drive setting how hard it lands.",
        params,
        modulation: ModRack::default(),
    }
}

/// The Acid model, Accent, legato slide and a fast filter decay together.
///
/// The patch the performance controls were built for, and the one that leaves
/// the oscillators entirely alone: it is the default single saw, and every
/// difference from the default patch is the filter and the performance
/// switches. `Legato` env trigger means an overlapping note slides and does
/// not retrigger, so which notes overlap is the part being played, and Accent
/// is high because how hard they are hit is the other half of it.
fn acid_line() -> FactoryPatch {
    let mut params = base();
    params.filter_model = FilterModel::Acid;
    params.filter_cutoff = 0.22;
    params.filter_resonance = 0.85;
    params.filter_env_amount = 0.55;
    params.filter_decay = 0.14;
    params.filter_sustain = 0.0;
    params.drive = 0.4;
    params.accent = 0.8;
    params.glide = 0.06;
    params.env_trigger = EnvTrigger::Legato;
    params.sustain = 1.0;
    params.release = 0.06;
    FactoryPatch {
        name: "Acid Line",
        category: "ML-M1",
        tags: &["bass", "acid", "accent", "legato"],
        description: "Acid model with accent and slide; overlap notes to play it.",
        params,
        modulation: ModRack::default(),
    }
}

/// Fast filter decay, keytrack, and a focused mono response.
///
/// Keytrack is most of this patch. A pluck voiced by ear at the bottom of the
/// keyboard is a dull thump two octaves up unless the cutoff follows the
/// note, so this is the patch that fails loudly if keytracking regresses.
fn snap_pluck() -> FactoryPatch {
    let mut params = base();
    params.osc[0].wave = OscWave::Pulse;
    params.osc[0].pulse_width = 0.35;
    params.osc[1].level = 0.35;
    params.filter_cutoff = 0.3;
    params.filter_resonance = 0.5;
    params.filter_env_amount = 0.5;
    params.filter_decay = 0.09;
    params.filter_sustain = 0.0;
    params.filter_release = 0.06;
    params.filter_keytrack = 0.8;
    params.accent = 0.4;
    params.sustain = 0.0;
    FactoryPatch {
        name: "Snap Pluck",
        category: "ML-M1",
        tags: &["pluck", "keytrack", "short"],
        description: "Short and focused, cutoff tracking the keyboard.",
        params,
        modulation: ModRack::default(),
    }
}

/// The held-note stack, note priority, both glide modes and the legato
/// envelope trigger.
///
/// `High` priority and `Always` glide are the settings that make the stack
/// audible: hold a low note, play above it, then release the top one — the
/// voice slides back down to what is still held rather than cutting off.
/// Nothing else in the bank exercises the fallback path.
fn porta_lead() -> FactoryPatch {
    let mut params = base();
    params.osc[1].level = 0.7;
    params.osc[2].level = 0.3;
    params.filter_cutoff = 0.5;
    params.filter_resonance = 0.25;
    params.filter_env_amount = 0.25;
    params.filter_decay = 0.6;
    params.filter_keytrack = 0.5;
    params.accent = 0.35;
    params.glide = 0.12;
    params.glide_mode = GlideMode::Always;
    params.env_trigger = EnvTrigger::Legato;
    params.priority = NotePriority::High;
    params.release = 0.25;
    FactoryPatch {
        name: "Porta Lead",
        category: "ML-M1",
        tags: &["lead", "glide", "legato", "priority"],
        description: "Sliding lead; hold a note under it and release to hear the stack.",
        params,
        modulation: ModRack::default(),
    }
}

/// A sample-and-hold channel LFO, pulse-width modulation, and a plain
/// source-to-destination route.
///
/// The one patch in the bank with a rack, and the reason the bank is
/// channel-scoped: the ML-M1 has no device-local LFO, so this movement can
/// only come from channel modulation. Two slots, because one route would not
/// show that the matrix takes more than one.
fn sequence_bleep() -> FactoryPatch {
    let mut params = base();
    params.osc[0].wave = OscWave::Pulse;
    params.filter_model = FilterModel::Clean;
    params.filter_cutoff = 0.45;
    params.filter_resonance = 0.35;
    params.filter_env_amount = 0.3;
    params.filter_decay = 0.12;
    params.filter_sustain = 0.0;
    params.filter_keytrack = 0.6;
    params.decay = 0.12;
    params.sustain = 0.0;
    params.release = 0.05;

    let mut modulation = ModRack::default();
    // Stepped random, retriggered, one step per sixteenth: the cutoff lands
    // somewhere new on each note rather than drifting between them.
    modulation.install(0, ModulatorParams::Lfo(ModLfoParams {
        waveform: ModLfoWaveform::Random,
        tempo_sync: true,
        rate_division: ModTimeDivision::Sixteenth,
        retrigger: true,
        depth: 1.0,
        ..ModLfoParams::default()
    }));
    // Free-running and slow, so the pulse width drifts across the sequence
    // instead of resetting with it. Unipolar because a pulse width either
    // side of centre sounds the same, and only one direction is interesting.
    modulation.install(1, ModulatorParams::Lfo(ModLfoParams {
        waveform: ModLfoWaveform::Sine,
        rate_hz: 0.35,
        depth: 1.0,
        ..ModLfoParams::default()
    }));
    modulation.add_route(ModRoute::to_slot(
        0,
        ParamAddr {
            scope: AUTHORED_SCOPE,
            owner: ParamOwner::Source,
            param: SYNTH_PARAM_FILTER_CUTOFF,
        },
        0.3,
        ModPolarity::Bipolar,
    ));
    modulation.add_route(ModRoute::to_slot(
        1,
        ParamAddr {
            scope: AUTHORED_SCOPE,
            owner: ParamOwner::Source,
            param: synth_osc_param(0, OSC_OFFSET_PULSE_WIDTH),
        },
        0.35,
        ModPolarity::Unipolar,
    ));

    FactoryPatch {
        name: "Sequence Bleep",
        category: "ML-M1",
        tags: &["sequence", "modulation", "pwm"],
        description: "Sample-and-hold cutoff and drifting pulse width, from the channel rack.",
        params,
        modulation,
    }
}

/// How many parameters a patch changes from [`MlM1Params::default`].
///
/// The plan's standing requirement on the bank is that every patch is a few
/// knob moves from the default saw, with fifteen precise settings named as
/// the point at which the defaults or the ranges are the problem rather than
/// the patch. That is only a requirement if something counts, so this does.
///
/// Oscillators count per changed field, not per oscillator: turning up a sub
/// is one move, and retuning it as well is two.
pub fn moves_from_default(params: &MlM1Params) -> usize {
    let base = MlM1Params::default();
    let mut moves = 0;

    for (osc, default) in params.osc.iter().zip(base.osc.iter()) {
        moves += usize::from(osc.wave != default.wave);
        moves += usize::from(osc.semitones != default.semitones);
        moves += usize::from(osc.cents != default.cents);
        moves += usize::from(osc.level != default.level);
        moves += usize::from(osc.pulse_width != default.pulse_width);
    }

    let scalars = [
        (params.glide, base.glide),
        (params.attack, base.attack),
        (params.decay, base.decay),
        (params.sustain, base.sustain),
        (params.release, base.release),
        (params.filter_cutoff, base.filter_cutoff),
        (params.filter_resonance, base.filter_resonance),
        (params.filter_env_amount, base.filter_env_amount),
        (params.drive, base.drive),
        (params.filter_attack, base.filter_attack),
        (params.filter_decay, base.filter_decay),
        (params.filter_sustain, base.filter_sustain),
        (params.filter_release, base.filter_release),
        (params.filter_keytrack, base.filter_keytrack),
        (params.accent, base.accent),
    ];
    moves += scalars.iter().filter(|(a, b)| a != b).count();

    moves += usize::from(params.glide_mode != base.glide_mode);
    moves += usize::from(params.env_trigger != base.env_trigger);
    moves += usize::from(params.priority != base.priority);
    moves += usize::from(params.filter_model != base.filter_model);
    moves
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition of done, counted. A patch that drifts past this has
    /// stopped being reachable, and the plan is explicit that the finding
    /// then belongs to the defaults or the ranges rather than to the patch.
    #[test]
    fn every_patch_is_a_few_knob_moves_from_the_default_saw() {
        for patch in patches() {
            let moves = moves_from_default(&patch.params);
            assert!(
                moves < 15,
                "{} takes {moves} moves from the default patch",
                patch.name
            );
        }
    }

    /// Six patches, distinctly named, in one category. The browser groups by
    /// category and the seeder names bundle directories after the patch, so a
    /// duplicate name would silently drop a patch from the bank.
    #[test]
    fn the_bank_is_six_distinctly_named_patches() {
        let patches = patches();
        assert_eq!(patches.len(), 6);
        for (index, patch) in patches.iter().enumerate() {
            assert!(!patch.name.is_empty());
            assert!(!patch.description.is_empty());
            assert_eq!(patch.category, "ML-M1");
            assert!(
                !patches[..index]
                    .iter()
                    .any(|other| other.name == patch.name),
                "{} appears twice",
                patch.name
            );
        }
    }

    /// Every route has to point at a slot that exists, or it is a silent
    /// no-op that would look like a broken patch rather than a broken bank.
    #[test]
    fn every_route_names_a_populated_modulator_slot() {
        for patch in patches() {
            for route in patch.modulation.routes.iter().flatten() {
                let slot = patch.modulation.params(route.source_slot as usize);
                assert!(
                    slot.is_some(),
                    "{} routes from empty slot {}",
                    patch.name,
                    route.source_slot
                );
            }
        }
    }
}
