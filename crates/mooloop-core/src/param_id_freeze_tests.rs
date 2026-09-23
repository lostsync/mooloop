//! Every persisted parameter id, and the parameter that sits at it.
//!
//! An automation lane and a modulation route both store a parameter *id*, so an
//! id is an on-disk identifier. The tables say so repeatedly -- "never renumber
//! a shipped id, append instead" (`effect.rs`), "never renumbered, automation
//! and modulation routes persist them" (`ds01.rs`) -- and until this file
//! existed, one id space enforced it: the channel strip, which pins
//! `STRIP_FIRST` to a literal 16 and then asserts every descriptor sits at
//! `STRIP_FIRST + offset`.
//!
//! Everywhere else the ids were literals in their definitions and reached only
//! through their constants, so **every test moved with a renumber.** That is the
//! same fault the outlet tables had, found the day before this: a lookup by the
//! constant you meant to pin finds whatever the constant now points at.
//!
//! ## Why the whole table and not a sample
//!
//! The realistic mistake is not a renumber, it is an *insert*. The ids are a
//! hand-maintained dense run and putting a new parameter beside its relatives
//! is the natural thing to do -- which shifts every id after it. That leaves the
//! run dense, so a contiguity check passes; it leaves the length one longer, so
//! a length check cannot tell it from a legal append. The only thing that
//! notices is which name sits at which id, which is what this is.
//!
//! ## What the rest of the suite does not see
//!
//! Measured rather than assumed. Swap `FILTER_PARAM_MODE` and
//! `FILTER_PARAM_SLOPE` -- a pure renumber, both still backed by their fields,
//! the run still dense, the length unchanged -- and **all 251 tests that existed
//! before this file pass.** Only this one fails.
//!
//! An earlier attempt at that mutation *was* caught, and the reason is worth
//! knowing: it inserted a descriptor with no backing field, so
//! `descriptor_defaults_match_the_params_defaults` panicked on the missing
//! getter. That is an artefact of a careless mutation rather than coverage --
//! a real insert comes with its field.
//!
//! ## When this fails
//!
//! If you appended a parameter, add its row. If any *existing* row moved, an
//! id that projects have saved now means something else: stop and put it back.
//! The table was generated from the tables themselves, so regenerating it is
//! not a fix -- it is how the drift gets written down as if it were intended.

use crate::channel::DeviceKind;
use crate::effect::EffectKind;
use crate::modulation::ModulatorKind;

/// Which table an id belongs to. Ids are per-owner, so `0` means a different
/// parameter for every one of them.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Owner {
    Effect(EffectKind),
    Device(DeviceKind),
    Modulator(ModulatorKind),
}

use Owner::{Device, Effect, Modulator};

/// Generated once from the live tables, frozen from then on.
#[rustfmt::skip]
const FROZEN: &[(Owner, &[(u32, &str)])] = &[
    (Effect(EffectKind::Eq), &[
        // Rebuilt 2026-09-14 by `eq-v2/01`. The seven ids this row used to
        // hold -- 0 "Band", 1 "On", 2 "Freq", 3 "Gain", 4 "Q", 5 "Pass
        // Slope", 6 "Q Profile" -- are **retired, not renumbered**: they
        // meant "the selected target's", which is the model that was
        // removed, and there is nothing here that could honestly answer to
        // them. They stay spent, which is why this table starts at 16, and
        // `the_retired_eq_ids_address_nothing` holds the gap open so an
        // append never reaches down into it.
        (16, "B1 On"),
        (17, "B1 Freq"),
        (18, "B1 Gain"),
        (19, "B1 Q"),
        (20, "B1 Type"),
        (21, "B1 Q Prof"),
        (26, "B2 On"),
        (27, "B2 Freq"),
        (28, "B2 Gain"),
        (29, "B2 Q"),
        (30, "B2 Type"),
        (31, "B2 Q Prof"),
        (36, "B3 On"),
        (37, "B3 Freq"),
        (38, "B3 Gain"),
        (39, "B3 Q"),
        (40, "B3 Type"),
        (41, "B3 Q Prof"),
        (46, "B4 On"),
        (47, "B4 Freq"),
        (48, "B4 Gain"),
        (49, "B4 Q"),
        (50, "B4 Type"),
        (51, "B4 Q Prof"),
        (56, "B5 On"),
        (57, "B5 Freq"),
        (58, "B5 Gain"),
        (59, "B5 Q"),
        (60, "B5 Type"),
        (61, "B5 Q Prof"),
        (66, "B6 On"),
        (67, "B6 Freq"),
        (68, "B6 Gain"),
        (69, "B6 Q"),
        (70, "B6 Type"),
        (71, "B6 Q Prof"),
        (76, "B7 On"),
        (77, "B7 Freq"),
        (78, "B7 Gain"),
        (79, "B7 Q"),
        (80, "B7 Type"),
        (81, "B7 Q Prof"),
        (86, "HP On"),
        (87, "HP Freq"),
        (88, "HP Q"),
        (89, "HP Slope"),
        (96, "LP On"),
        (97, "LP Freq"),
        (98, "LP Q"),
        (99, "LP Slope"),
    ]),
    (Effect(EffectKind::Modulation), &[
        (0, "Mode"),
        (1, "Rate"),
        (2, "Depth"),
        (3, "Color"),
        (4, "Feedback"),
        (5, "Spread"),
        (6, "Tone"),
        (7, "Stages"),
    ]),
    (Effect(EffectKind::Filter), &[
        (0, "Cutoff"),
        (1, "Reso"),
        (2, "Mode"),
        (3, "Slope"),
        (4, "Drive"),
    ]),
    (Effect(EffectKind::Preamp), &[
        (0, "Drive"),
        (1, "Voicing"),
        (2, "Mix"),
        (3, "Output"),
    ]),
    (Effect(EffectKind::Drive), &[
        (0, "Drive"),
        (1, "Curve"),
        (2, "Tone"),
        (3, "Mix"),
        (4, "Out"),
    ]),
    (Effect(EffectKind::Bitcrush), &[
        (0, "Bits"),
        (1, "Rate"),
        (2, "Mix"),
        (3, "Style"),
    ]),
    (Effect(EffectKind::Delay), &[
        (0, "Time"),
        (1, "Fdbk"),
        (2, "Mode"),
        (3, "Cross"),
        (4, "Tone"),
        (5, "Mix"),
    ]),
    (Effect(EffectKind::Reverb), &[
        (8, "Size"),
        (9, "Decay"),
        (10, "Damp"),
        (11, "Pre"),
        (12, "Diffuse"),
        (13, "Width"),
        (14, "Mod"),
        (15, "Low Cut"),
    ]),
    (Effect(EffectKind::Plate), &[
        (0, "Size"),
        (1, "Decay"),
        (2, "Damp"),
        (3, "Width"),
        (4, "Pre"),
    ]),
    (Effect(EffectKind::Gate), &[
        (0, "Thresh"),
        (1, "Attack"),
        (2, "Hold"),
        (3, "Release"),
        (4, "Range"),
    ]),
    (Effect(EffectKind::Compressor), &[
        (0, "Thresh"),
        (1, "Ratio"),
        (2, "Attack"),
        (3, "Release"),
        (4, "Knee"),
        (5, "Makeup"),
        // Appended by MOO-142, 2026-09-23.
        (6, "Mix"),
    ]),
    (Effect(EffectKind::Limiter), &[
        (0, "Ceiling"),
        (1, "Release"),
        (2, "Gain"),
    ]),
    // Id 0 is **spent**. `Offset` was retired 2026-09-16 -- see
    // `BUFFER_PARAM_OFFSET_BEATS` -- and nothing may ever answer to 0 again,
    // because projects saved before that still name it and mean beats behind
    // the writer. A retirement is the one edit to this table that is not a
    // renumber: the row leaves, the id does not come back, and every row
    // after it moves down a position, which is safe only because a position
    // is read at runtime while an id is read off disk.
    //
    // Ids 3, 4 and 5 are spent too. `Rate`, `Length` and `Loop` went later
    // the same day with the turntable model that needed them, and a document
    // saved in between names them. What replaced them is new ids from 10 up,
    // never a reuse -- the same rule, applied three more times.
    //
    // `Quant Grid` became `Quant Start` in the same change, on the same id:
    // it is the same number answering the same question, now governing a
    // gesture's start as well as a freeze's. A rename is not a renumber, and
    // the name is recorded here because a lane picker shows it.
    (Effect(EffectKind::Buffer), &[
        (2, "Position"),
        (14, "Span"),
        (10, "Jump Back"),
        (13, "Stutter"),
        (1, "Crossfade"),
        (6, "Freeze"),
        (7, "Jump"),
        (11, "Reverse"),
        (12, "Stutter Gate"),
        (8, "Quantize"),
        (9, "Quant Start"),
    ]),
    (Effect(EffectKind::Chain), &[
        (0, "Mix"),
    ]),
    // The same single id, because a layer holds `ContainerParams` too and the
    // two kinds share one descriptor table. Ids are scoped by kind, so id 0
    // meaning Mix here and Mix there is one fact written once, not twice.
    (Effect(EffectKind::Layer), &[
        (0, "Mix"),
    ]),
    (Device(DeviceKind::Sampler), &[
        (0, "Start"),
        (1, "End"),
        (2, "Reverse"),
        (3, "Tune"),
        (4, "Fine"),
        (5, "Loop start"),
        (6, "Loop end"),
        (7, "Loop"),
        (8, "Attack"),
        (9, "Decay"),
        (10, "Sustain"),
        (11, "Release"),
        (12, "Cutoff"),
        (13, "Reso"),
        (14, "Env amt"),
        (15, "Drive"),
        (16, "Bits"),
        (17, "Rate"),
        (18, "Voice mode"),
        (19, "Voices"),
        (20, "Retrigger"),
        (21, "Root"),
        (22, "Output"),
        (23, "Filter attack"),
        (24, "Filter decay"),
        (25, "Filter sustain"),
        (26, "Filter release"),
        (27, "Stretch"),
        (28, "Stretch mode"),
        (29, "Stretch"),
        (30, "Grain"),
        (31, "Fit to tempo"),
        (32, "Bars"),
        (33, "Live tune"),
        (34, "Play mode"),
        (35, "Slice base"),
    ]),
    (Device(DeviceKind::DrumSynth), &[
        (0, "Mode"),
        (1, "Kick char"),
        (2, "Snare char"),
        (3, "Hat char"),
        (4, "Decay"),
        (5, "Tune"),
        (6, "Drive"),
        (7, "Punch"),
        (8, "Kick start"),
        (9, "Kick end"),
        (10, "Kick sweep"),
        (11, "Kick click"),
        (12, "Snare tone"),
        (13, "Snare tone 2"),
        (14, "Snare T2 mix"),
        (15, "Snare noise"),
        (16, "Snare N decay"),
        (17, "Snare color"),
        (18, "Hat high-pass"),
        (19, "Hat metallic"),
    ]),
    (Device(DeviceKind::MonoSynth), &[
        (0, "Glide"),
        (1, "Attack"),
        (2, "Decay"),
        (3, "Sustain"),
        (4, "Release"),
        (5, "Cutoff"),
        (6, "Reso"),
        (7, "Env amt"),
        (8, "Drive"),
        (9, "LFO wave"),
        (10, "LFO rate"),
        (11, "LFO pitch"),
        (12, "LFO filter"),
        (13, "LFO width"),
        (14, "LFO amp"),
        (100, "Osc 1 wave"),
        (101, "Semis"),
        (102, "Cents"),
        (103, "Level"),
        (104, "Width"),
        (110, "Osc 2 wave"),
        (111, "Semis"),
        (112, "Cents"),
        (113, "Level"),
        (114, "Width"),
        (120, "Osc 3 wave"),
        (121, "Semis"),
        (122, "Cents"),
        (123, "Level"),
        (124, "Width"),
    ]),
    (Device(DeviceKind::PolySynth), &[
        (0, "Glide"),
        (1, "Attack"),
        (2, "Decay"),
        (3, "Sustain"),
        (4, "Release"),
        (5, "Cutoff"),
        (6, "Reso"),
        (7, "Env amt"),
        (8, "Drive"),
        (9, "LFO wave"),
        (10, "LFO rate"),
        (11, "LFO pitch"),
        (12, "LFO filter"),
        (13, "LFO width"),
        (14, "LFO amp"),
        (100, "Osc 1 wave"),
        (101, "Semis"),
        (102, "Cents"),
        (103, "Level"),
        (104, "Width"),
        (110, "Osc 2 wave"),
        (111, "Semis"),
        (112, "Cents"),
        (113, "Level"),
        (114, "Width"),
        (120, "Osc 3 wave"),
        (121, "Semis"),
        (122, "Cents"),
        (123, "Level"),
        (124, "Width"),
        (15, "Voices"),
        (16, "Spread"),
        (17, "Mono"),
        (18, "Env trig"),
        (19, "Priority"),
    ]),
    (Device(DeviceKind::MlM1), &[
        (0, "Glide"),
        (1, "Attack"),
        (2, "Decay"),
        (3, "Sustain"),
        (4, "Release"),
        (5, "Cutoff"),
        (6, "Reso"),
        (7, "Env amt"),
        (8, "Drive"),
        (20, "F attack"),
        (21, "F decay"),
        (22, "F sustain"),
        (23, "F release"),
        (24, "Keytrack"),
        (25, "Glide mode"),
        (26, "Env trig"),
        (27, "Priority"),
        (28, "Model"),
        (29, "Accent"),
        (100, "Osc 1 wave"),
        (101, "Semis"),
        (102, "Cents"),
        (103, "Level"),
        (104, "Width"),
        (110, "Osc 2 wave"),
        (111, "Semis"),
        (112, "Cents"),
        (113, "Level"),
        (114, "Width"),
        (120, "Osc 3 wave"),
        (121, "Semis"),
        (122, "Cents"),
        (123, "Level"),
        (124, "Width"),
    ]),
    (Device(DeviceKind::MlP8), &[
        (0, "Osc 1 wave"),
        (1, "Semis"),
        (2, "Cents"),
        (3, "Level"),
        (4, "Width"),
        (5, "Osc 2 wave"),
        (6, "Semis"),
        (7, "Cents"),
        (8, "Level"),
        (9, "Width"),
        (10, "Osc 3 wave"),
        (11, "Semis"),
        (12, "Cents"),
        (13, "Level"),
        (14, "Width"),
        (15, "Attack"),
        (16, "Decay"),
        (17, "Sustain"),
        (18, "Release"),
        (19, "Glide"),
        (20, "Sub level"),
        (21, "Sub oct"),
        (22, "Sub wave"),
        (23, "Sub src"),
        (25, "Noise level"),
        (26, "Noise color"),
        (27, "XM 1>2"),
        (28, "XM 1>3"),
        (29, "XM 2>1"),
        (30, "XM 2>3"),
        (31, "XM 3>1"),
        (32, "XM 3>2"),
        (33, "N>Osc 1"),
        (34, "N>Osc 2"),
        (35, "N>Osc 3"),
        (36, "FB Osc 1"),
        (37, "FB Osc 2"),
        (38, "FB Osc 3"),
        (39, "Sync 1"),
        (40, "Sync 2"),
        (41, "Sync 3"),
        (42, "Mode"),
        (43, "Cutoff"),
        (44, "Reso"),
        (45, "Env amt"),
        (46, "Drive"),
        (47, "Keytrack"),
        (48, "F attack"),
        (49, "F decay"),
        (50, "F sustain"),
        (51, "F release"),
        (52, "Amp vel"),
        (53, "Filt vel"),
        (54, "Feedback"),
        (55, "LFO wave"),
        (56, "LFO sync"),
        (57, "LFO rate"),
        (58, "LFO div"),
        (59, "LFO phase"),
        (60, "LFO warp"),
        (61, "LFO slew"),
        (62, "LFO trig"),
        (64, "Drift"),
        (65, "Unison"),
        (66, "Detune"),
        (67, "Spread"),
        (68, "Chorus"),
        (69, "Volume"),
        (70, "Pan"),
    ]),
    (Device(DeviceKind::Ds01), &[
        (0, "Tune"),
        (1, "Level"),
        (2, "Choke grp"),
        (3, "Choke time"),
        (4, "Retrigger"),
        (5, "Vel amt"),
        (10, "Tone level"),
        (11, "Tone pitch"),
        (12, "Tone wave"),
        (13, "Partials"),
        (14, "Spread"),
        (15, "FM amt"),
        (16, "FM ratio"),
        (20, "Noise level"),
        (21, "Color"),
        (22, "Rate"),
        (23, "Morph"),
        (24, "Cutoff"),
        (25, "Reso"),
        (30, "Body level"),
        (31, "Body pitch"),
        (32, "Ratio"),
        (33, "Body decay"),
        (34, "Damping"),
        (35, "Excite"),
        (40, "Amp attack"),
        (41, "Amp hold"),
        (42, "Amp decay"),
        (43, "Amp curve"),
        (44, "Amp sustain"),
        (45, "Amp release"),
        (46, "Amp gate"),
        (50, "Pitch attack"),
        (51, "Pitch decay"),
        (52, "Pitch curve"),
        (53, "Pitch depth"),
        (60, "Noise attack"),
        (61, "Noise hold"),
        (62, "Noise decay"),
        (63, "Noise curve"),
        (64, "Noise sustain"),
        (65, "Noise release"),
        (66, "Noise gate"),
        (70, "Mod attack"),
        (71, "Mod hold"),
        (72, "Mod decay"),
        (73, "Mod curve"),
        (74, "Mod sustain"),
        (75, "Mod release"),
        (76, "Mod gate"),
        (80, "Repeats"),
        (81, "Spacing"),
        (82, "Spread"),
        (83, "Level step"),
        (84, "Pitch step"),
        (90, "Drive"),
        (91, "Character"),
        (92, "Bias"),
        (93, "Bits"),
        (94, "Output HP"),
        (100, "Row 1 src"),
        (101, "Row 1 dest"),
        (102, "Row 1 amt"),
        (103, "Row 1 curve"),
        (104, "Row 2 src"),
        (105, "Row 2 dest"),
        (106, "Row 2 amt"),
        (107, "Row 2 curve"),
        (108, "Row 3 src"),
        (109, "Row 3 dest"),
        (110, "Row 3 amt"),
        (111, "Row 3 curve"),
        (112, "Row 4 src"),
        (113, "Row 4 dest"),
        (114, "Row 4 amt"),
        (115, "Row 4 curve"),
        (116, "Row 5 src"),
        (117, "Row 5 dest"),
        (118, "Row 5 amt"),
        (119, "Row 5 curve"),
        (120, "Row 6 src"),
        (121, "Row 6 dest"),
        (122, "Row 6 amt"),
        (123, "Row 6 curve"),
        (124, "Row 7 src"),
        (125, "Row 7 dest"),
        (126, "Row 7 amt"),
        (127, "Row 7 curve"),
        (128, "Row 8 src"),
        (129, "Row 8 dest"),
        (130, "Row 8 amt"),
        (131, "Row 8 curve"),
    ]),
    (Device(DeviceKind::AuxIn), &[
        (0, "Source"),
        (1, "Outlet"),
        (2, "Level"),
    ]),
    (Modulator(ModulatorKind::Lfo), &[
        (0, "Rate"),
        (1, "Amount"),
        (2, "Waveform"),
        (3, "Phase"),
        (4, "Rate sync"),
        (5, "Rate division"),
        (6, "Retrigger"),
        (7, "Fade in"),
        (8, "Fade sync"),
        (9, "Fade division"),
        (10, "Smooth"),
        (11, "Pulse width"),
    ]),
    (Modulator(ModulatorKind::Envelope), &[
        (0, "Attack"),
        (1, "Attack sync"),
        (2, "Attack division"),
        (3, "Decay"),
        (4, "Decay sync"),
        (5, "Decay division"),
        (6, "Sustain"),
        (7, "Release"),
        (8, "Release sync"),
        (9, "Release division"),
        (10, "Amount"),
    ]),
    (Modulator(ModulatorKind::Step), &[
        (0, "Length"),
        (1, "Rate"),
        (2, "Glide"),
        (3, "Trigger"),
        (4, "Step 1"),
        (5, "Step 2"),
        (6, "Step 3"),
        (7, "Step 4"),
        (8, "Step 5"),
        (9, "Step 6"),
        (10, "Step 7"),
        (11, "Step 8"),
        (12, "Step 9"),
        (13, "Step 10"),
        (14, "Step 11"),
        (15, "Step 12"),
        (16, "Step 13"),
        (17, "Step 14"),
        (18, "Step 15"),
        (19, "Step 16"),
    ]),
    (Modulator(ModulatorKind::Random), &[
        (0, "Rate"),
        (1, "Rate sync"),
        (2, "Rate division"),
        (3, "Trigger"),
        (4, "Bipolar"),
        (5, "Chance"),
        (6, "Quantize"),
        (7, "Drunk"),
        (8, "Walk"),
    ]),
    (Modulator(ModulatorKind::Math), &[
        (0, "Input"),
        (1, "Operator"),
        (2, "Operand"),
        (3, "Low"),
        (4, "High"),
    ]),
];

fn live(owner: Owner) -> &'static [crate::effect::ParamDescriptor] {
    match owner {
        Effect(kind) => kind.descriptors(),
        Device(kind) => kind.descriptors(),
        Modulator(kind) => kind.descriptors(),
    }
}

#[test]
fn every_persisted_parameter_id_is_where_it_was() {
    for (owner, frozen) in FROZEN {
        let live = live(*owner);
        for (position, (id, name)) in frozen.iter().enumerate() {
            let Some(descriptor) = live.get(position) else {
                panic!(
                    "{owner:?} has {} parameters where {} were frozen; \
                     {name:?} (id {id}) is gone, and any project that automated \
                     it still names id {id}",
                    live.len(),
                    frozen.len(),
                );
            };
            assert_eq!(
                (descriptor.id, descriptor.name),
                (*id, *name),
                "{owner:?} position {position} holds id {} named {:?}; it was \
                 id {id} named {name:?}. Projects have saved {id}, so whatever \
                 answers to {id} now is what their lanes and routes will move.",
                descriptor.id,
                descriptor.name,
            );
        }
    }
}

/// A new kind has to be written down here, or its ids are frozen by nothing.
///
/// The count rather than the names, because the names are in `FROZEN` already
/// and this is the tripwire for a kind that never got a row at all -- which is
/// how four effect faces came to be outside the face-agreement test.
#[test]
fn every_kind_with_a_table_is_frozen() {
    let owners: Vec<Owner> = EffectKind::ALL
        .iter()
        .map(|kind| Effect(*kind))
        .chain(
            [
                DeviceKind::Sampler,
                DeviceKind::DrumSynth,
                DeviceKind::MonoSynth,
                DeviceKind::PolySynth,
                DeviceKind::MlM1,
                DeviceKind::MlP8,
                DeviceKind::Ds01,
                DeviceKind::AuxIn,
            ]
            .iter()
            .map(|kind| Device(*kind)),
        )
        .chain(ModulatorKind::ALL.iter().map(|kind| Modulator(*kind)))
        .collect();

    for owner in &owners {
        assert!(
            FROZEN.iter().any(|(frozen, _)| frozen == owner),
            "{owner:?} has a descriptor table and no frozen row; its ids are \
             persisted and nothing is holding them"
        );
    }
    assert_eq!(
        FROZEN.len(),
        owners.len(),
        "FROZEN lists a kind that no longer has a table"
    );
}

/// Every parameter a device offers, counted once, so the scale of what the
/// table above is holding is visible rather than implied.
#[test]
fn the_frozen_table_covers_every_described_parameter() {
    let frozen: usize = FROZEN.iter().map(|(_, rows)| rows.len()).sum();
    let living: usize = FROZEN.iter().map(|(owner, _)| live(*owner).len()).sum();
    assert_eq!(
        frozen, living,
        "{living} parameters are described and {frozen} are frozen"
    );
}
