//! The effect factory banks: a few starting places for every effect kind.
//!
//! Defined as data rather than as files, for the reasons `mlm1_factory` gives:
//! a patch is a set of parameters, so an [`EffectSlotState`] is its natural
//! form, and a bank that only existed as TOML would have to be parsed back to
//! be tested. The seeder in `mooloop_project::factory` serializes these into
//! `presets/effects/<kind>/` on first run.
//!
//! Every patch starts from [`EffectSlotState::of_kind`] and changes what it
//! needs, so the diff between this file and the kind's defaults *is* the
//! patch, checkable by reading. Each one is a recognisable use of the device
//! -- a slapback, a jet flange, a drum gate -- rather than a demonstration of
//! its range, because a starting place the user recognises is one they will
//! reach for, and one they will not is a demo.
//!
//! Nothing here names a channel, a route, or a bus. An effect preset is one
//! rack row with relative addressing, and a patch that needed modulation to
//! mean something would be a different unit (`docs/plans/preset-system/`).

use crate::{
    BitcrushStyle, DelayMode, DelayTimeDivision, DriveCurve, EffectKind, EffectParams,
    EffectSlotState, EqBandKind, EqPassFilter, EqQProfile, EqSlope, FilterMode, FilterSlope,
    ModulationMode,
};

/// One factory patch: presentation metadata plus the complete row state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectFactoryPatch {
    pub name: &'static str,
    /// Every shipped patch is `"Factory"`. The list sorts by category first
    /// and an unnamed category sorts ahead, so a user's own presets lead.
    pub category: &'static str,
    pub tags: &'static [&'static str],
    /// One line on what the patch is for.
    pub description: &'static str,
    pub effect: EffectSlotState,
}

const CATEGORY: &str = "Factory";

/// The bank for `kind`, in the order it should be presented.
pub fn patches(kind: EffectKind) -> Vec<EffectFactoryPatch> {
    match kind {
        EffectKind::Eq => eq(),
        EffectKind::Modulation => modulation(),
        EffectKind::Filter => filter(),
        EffectKind::Drive => drive(),
        EffectKind::Bitcrush => bitcrush(),
        EffectKind::Delay => delay(),
        EffectKind::Reverb => reverb(),
        EffectKind::Plate => plate(),
        EffectKind::Gate => gate(),
        EffectKind::Compressor => compressor(),
        EffectKind::Limiter => limiter(),
        EffectKind::Buffer => buffer(),
    }
}

/// A patch of `kind` whose parameters `edit` has adjusted from the defaults.
/// The host settings (bypass, blend, trims) start where a freshly inserted
/// device starts, so a patch is what the row *does*, not how loud it is.
fn patch(
    kind: EffectKind,
    name: &'static str,
    tags: &'static [&'static str],
    description: &'static str,
    edit: impl FnOnce(&mut EffectSlotState),
) -> EffectFactoryPatch {
    let mut effect = EffectSlotState::of_kind(kind);
    edit(&mut effect);
    debug_assert_eq!(effect.kind(), kind, "{name} changed its own kind");
    EffectFactoryPatch {
        name,
        category: CATEGORY,
        tags,
        description,
        effect,
    }
}

// --- EQ -------------------------------------------------------------------

fn eq() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::EqParams)| {
        patch(EffectKind::Eq, name, tags, description, |effect| {
            if let EffectParams::Eq(params) = &mut effect.params {
                // The defaults switch three bands on at 0 dB; a patch states
                // its own shape from a flat start.
                for band in &mut params.bands {
                    band.enabled = false;
                    band.gain_db = 0.0;
                }
                edit(params);
            }
        })
    };
    vec![
        with("Low Cut", &["cleanup", "highpass"], "Everything below 80 Hz goes, 18 dB an octave.", |p| {
            p.high_pass = EqPassFilter {
                enabled: true,
                frequency_hz: 80.0,
                q: 0.707,
                slope: EqSlope::Db18,
            };
        }),
        with("Bass Lift", &["bass", "shelf"], "A 4 dB low shelf under 100 Hz.", |p| {
            p.bands[0].enabled = true;
            p.bands[0].kind = EqBandKind::LowShelf;
            p.bands[0].frequency_hz = 100.0;
            p.bands[0].gain_db = 4.0;
            p.selected_target = 0;
        }),
        with("Mud Scoop", &["cut", "mid"], "A wide 4 dB dip at 300 Hz.", |p| {
            p.bands[1].enabled = true;
            p.bands[1].frequency_hz = 300.0;
            p.bands[1].gain_db = -4.0;
            p.bands[1].q = 1.2;
        }),
        with("Presence", &["mid", "boost"], "3 dB at 3 kHz and a touch of shelf above 8 kHz.", |p| {
            p.bands[1].enabled = true;
            p.bands[1].frequency_hz = 3_000.0;
            p.bands[1].gain_db = 3.0;
            p.bands[1].q = 1.0;
            p.bands[2].enabled = true;
            p.bands[2].kind = EqBandKind::HighShelf;
            p.bands[2].frequency_hz = 8_000.0;
            p.bands[2].gain_db = 2.0;
        }),
        with("De-Harsh", &["cut", "proportional"], "A narrow proportional-Q cut at 4 kHz.", |p| {
            p.bands[1].enabled = true;
            p.bands[1].frequency_hz = 4_000.0;
            p.bands[1].gain_db = -3.0;
            p.bands[1].q = 2.0;
            p.bands[1].q_profile = EqQProfile::Proportional;
        }),
        with("Air", &["shelf", "treble"], "A 3.5 dB high shelf from 12 kHz.", |p| {
            p.bands[2].enabled = true;
            p.bands[2].kind = EqBandKind::HighShelf;
            p.bands[2].frequency_hz = 12_000.0;
            p.bands[2].gain_db = 3.5;
            p.selected_target = 2;
        }),
        with("Telephone", &["bandpass", "lo-fi"], "Steep passes at 400 Hz and 3 kHz.", |p| {
            p.high_pass = EqPassFilter {
                enabled: true,
                frequency_hz: 400.0,
                q: 0.707,
                slope: EqSlope::Db24,
            };
            p.low_pass = EqPassFilter {
                enabled: true,
                frequency_hz: 3_000.0,
                q: 0.707,
                slope: EqSlope::Db24,
            };
        }),
    ]
}

// --- Modulation -------------------------------------------------------------

fn modulation() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::ModulationParams)| {
        patch(EffectKind::Modulation, name, tags, description, |effect| {
            if let EffectParams::Modulation(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Soft Chorus", &["chorus", "wide"], "A slow, wide chorus that thickens without wobble.", |p| {
            p.mode = ModulationMode::Chorus;
            p.rate_hz = 0.3;
            p.depth = 0.4;
            p.color = 0.5;
            p.spread = 0.7;
        }),
        with("Jet Flange", &["flange", "feedback"], "Deep flange with the feedback most of the way up.", |p| {
            p.mode = ModulationMode::Flange;
            p.rate_hz = 0.12;
            p.depth = 0.8;
            p.color = 0.3;
            p.feedback = 0.7;
            p.spread = 0.5;
        }),
        with("Four Stage Phase", &["phaser", "classic"], "The four-stage stompbox sweep.", |p| {
            p.mode = ModulationMode::Phaser;
            p.rate_hz = 0.5;
            p.depth = 0.7;
            p.color = 0.45;
            p.feedback = 0.35;
            p.stages = 4;
        }),
        with("Slow Phase", &["phaser", "deep"], "Twelve stages, a six-second sweep, plenty of feedback.", |p| {
            p.mode = ModulationMode::Phaser;
            p.rate_hz = 0.06;
            p.depth = 0.9;
            p.color = 0.4;
            p.feedback = 0.55;
            p.stages = 12;
        }),
        with("Ensemble", &["ensemble", "strings"], "The string-machine ensemble, spread fully.", |p| {
            p.mode = ModulationMode::Ensemble;
            p.rate_hz = 0.6;
            p.depth = 0.55;
            p.color = 0.5;
            p.spread = 1.0;
            p.tone = 0.7;
        }),
        with("Double Track", &["adt", "double"], "Artificial double tracking, barely moving.", |p| {
            p.mode = ModulationMode::Adt;
            p.rate_hz = 0.15;
            p.depth = 0.3;
            p.color = 0.6;
            p.spread = 0.8;
        }),
    ]
}

// --- Filter -----------------------------------------------------------------

fn filter() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::FilterParams)| {
        patch(EffectKind::Filter, name, tags, description, |effect| {
            if let EffectParams::Filter(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Warm Low-Pass", &["lowpass", "warm"], "24 dB low-pass at 1.8 kHz with a little drive.", |p| {
            p.mode = FilterMode::LowPass;
            p.slope = FilterSlope::Db24;
            p.cutoff_hz = 1_800.0;
            p.resonance = 0.15;
            p.drive = 0.15;
        }),
        with("Acid Squelch", &["lowpass", "resonant"], "Low cutoff, high resonance, driven hard.", |p| {
            p.mode = FilterMode::LowPass;
            p.slope = FilterSlope::Db24;
            p.cutoff_hz = 600.0;
            p.resonance = 0.8;
            p.drive = 0.5;
        }),
        with("Telephone", &["bandpass", "lo-fi"], "A resonant band-pass at 1.4 kHz.", |p| {
            p.mode = FilterMode::BandPass;
            p.slope = FilterSlope::Db12;
            p.cutoff_hz = 1_400.0;
            p.resonance = 0.5;
        }),
        with("Rumble Cut", &["highpass", "cleanup"], "A gentle high-pass at 90 Hz.", |p| {
            p.mode = FilterMode::HighPass;
            p.slope = FilterSlope::Db12;
            p.cutoff_hz = 90.0;
            p.resonance = 0.05;
        }),
        with("Air Only", &["highpass", "thin"], "Steep high-pass at 4 kHz: hats and air, nothing else.", |p| {
            p.mode = FilterMode::HighPass;
            p.slope = FilterSlope::Db24;
            p.cutoff_hz = 4_000.0;
            p.resonance = 0.2;
        }),
    ]
}

// --- Drive ------------------------------------------------------------------

fn drive() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::DriveParams)| {
        patch(EffectKind::Drive, name, tags, description, |effect| {
            if let EffectParams::Drive(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Tape Warmth", &["tape", "subtle"], "Tape curve, light drive, a shade darker.", |p| {
            p.curve = DriveCurve::Tape;
            p.drive = 3.0;
            p.tone = -0.2;
            p.mix = 0.7;
        }),
        with("Soft Push", &["soft", "saturation"], "Soft saturation with a little brightness.", |p| {
            p.curve = DriveCurve::Soft;
            p.drive = 4.0;
            p.tone = 0.1;
        }),
        with("Hard Clip", &["hard", "distortion"], "Hard clipping, pulled back at the output.", |p| {
            p.curve = DriveCurve::Hard;
            p.drive = 8.0;
            p.tone = 0.2;
            p.output = 0.8;
        }),
        with("Fold Screech", &["fold", "extreme"], "Wavefolding well past full scale.", |p| {
            p.curve = DriveCurve::Fold;
            p.drive = 12.0;
            p.tone = 0.4;
            p.mix = 0.85;
            p.output = 0.7;
        }),
        with("Parallel Grit", &["hard", "parallel"], "Heavy clipping blended a third of the way in.", |p| {
            p.curve = DriveCurve::Hard;
            p.drive = 16.0;
            p.tone = -0.3;
            p.mix = 0.35;
        }),
    ]
}

// --- Bitcrush ---------------------------------------------------------------

fn bitcrush() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::BitcrushParams)| {
        patch(EffectKind::Bitcrush, name, tags, description, |effect| {
            if let EffectParams::Bitcrush(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("8-Bit", &["crush", "chip"], "Eight bits, a quarter of the sample rate.", |p| {
            p.style = BitcrushStyle::Crush;
            p.bits = 8.0;
            p.downsample = 4.0;
        }),
        with("Lo-Fi Sampler", &["dither", "sampler"], "Twelve dithered bits at a vintage sampler's rate.", |p| {
            p.style = BitcrushStyle::Dither;
            p.bits = 12.0;
            p.downsample = 3.6;
            p.mix = 0.8;
        }),
        with("Mu Crunch", &["mu", "companded"], "Companded six bits: detail stays, peaks crush.", |p| {
            p.style = BitcrushStyle::Mu;
            p.bits = 6.0;
            p.downsample = 2.0;
        }),
        with("Aliased Glide", &["glide", "alias"], "Twelfth-rate hold with interpolation, ten bits.", |p| {
            p.style = BitcrushStyle::Glide;
            p.bits = 10.0;
            p.downsample = 12.0;
        }),
        with("Dust", &["dither", "noise"], "Four dithered bits, half blended: the signal in noise.", |p| {
            p.style = BitcrushStyle::Dither;
            p.bits = 4.0;
            p.downsample = 1.0;
            p.mix = 0.5;
        }),
    ]
}

// --- Delay ------------------------------------------------------------------

fn delay() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::DelayParams)| {
        patch(EffectKind::Delay, name, tags, description, |effect| {
            if let EffectParams::Delay(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Dotted Eighth", &["sync", "rhythmic"], "The tempo-locked dotted eighth.", |p| {
            p.tempo_sync = true;
            p.time_division = DelayTimeDivision::DottedEighth;
            p.feedback = 0.4;
            p.tone = 0.55;
            p.mix = 0.3;
        }),
        with("Ping Pong", &["sync", "stereo"], "Quarter notes alternating sides.", |p| {
            p.tempo_sync = true;
            p.time_division = DelayTimeDivision::Quarter;
            p.feedback = 0.45;
            p.cross = 1.0;
            p.mix = 0.35;
        }),
        with("Slapback", &["short", "rockabilly"], "One bright repeat at 95 ms.", |p| {
            p.tempo_sync = false;
            p.time_ms = 95.0;
            p.feedback = 0.08;
            p.tone = 0.8;
            p.mix = 0.3;
        }),
        with("Tape Echo", &["tape", "dark"], "Tape mode, darkening repeats.", |p| {
            p.mode = DelayMode::Tape;
            p.tempo_sync = false;
            p.time_ms = 420.0;
            p.feedback = 0.55;
            p.tone = 0.4;
            p.mix = 0.3;
        }),
        with("Reverse Wash", &["reverse", "ambient"], "Half-note windows played backwards.", |p| {
            p.mode = DelayMode::Reverse;
            p.tempo_sync = true;
            p.time_division = DelayTimeDivision::Half;
            p.feedback = 0.6;
            p.tone = 0.5;
            p.mix = 0.45;
        }),
        with("Dub Runaway", &["dub", "feedback"], "Triplet tape repeats on the edge of running away.", |p| {
            p.mode = DelayMode::Tape;
            p.tempo_sync = true;
            p.time_division = DelayTimeDivision::EighthTriplet;
            p.feedback = 0.85;
            p.cross = 0.5;
            p.tone = 0.35;
            p.mix = 0.4;
        }),
    ]
}

// --- Reverb -----------------------------------------------------------------

fn reverb() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, wet_dry, edit: fn(&mut crate::ReverbParams)| {
        patch(EffectKind::Reverb, name, tags, description, |effect| {
            effect.wet_dry = wet_dry;
            if let EffectParams::Reverb(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Small Room", &["room", "short"], "A small, dampened room.", 0.2, |p| {
            p.size = 0.25;
            p.decay_s = 0.6;
            p.damping = 0.5;
            p.predelay_ms = 5.0;
        }),
        with("Tight Drum", &["drums", "short"], "Short and low-cut, for a kit.", 0.18, |p| {
            p.size = 0.3;
            p.decay_s = 0.9;
            p.damping = 0.45;
            p.predelay_ms = 8.0;
            p.low_cut_hz = 200.0;
        }),
        with("Hall", &["hall", "medium"], "A concert hall with a little predelay.", 0.25, |p| {
            p.size = 0.7;
            p.decay_s = 3.2;
            p.damping = 0.35;
            p.predelay_ms = 20.0;
        }),
        with("Dark Chamber", &["chamber", "dark"], "Heavily damped, bass rolled off.", 0.25, |p| {
            p.size = 0.5;
            p.decay_s = 2.0;
            p.damping = 0.75;
            p.predelay_ms = 10.0;
            p.low_cut_hz = 120.0;
        }),
        with("Cathedral", &["hall", "long"], "Eight seconds of diffuse, moving space.", 0.3, |p| {
            p.size = 1.0;
            p.decay_s = 8.0;
            p.damping = 0.25;
            p.predelay_ms = 40.0;
            p.diffusion = 0.85;
            p.modulation = 0.4;
        }),
        with("Infinite Wash", &["ambient", "pad"], "Near-endless decay, half wet: a pad from anything.", 0.5, |p| {
            p.size = 0.9;
            p.decay_s = 18.0;
            p.damping = 0.3;
            p.diffusion = 0.9;
            p.modulation = 0.5;
        }),
    ]
}

// --- Plate ------------------------------------------------------------------

fn plate() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, wet_dry, edit: fn(&mut crate::PlateParams)| {
        patch(EffectKind::Plate, name, tags, description, |effect| {
            effect.wet_dry = wet_dry;
            if let EffectParams::Plate(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Vocal Plate", &["vocal", "medium"], "The classic vocal plate with 30 ms of predelay.", 0.22, |p| {
            p.size = 0.5;
            p.decay_s = 1.8;
            p.damping = 0.45;
            p.predelay_ms = 30.0;
        }),
        with("Snare Plate", &["drums", "short"], "Short and close, for a snare.", 0.25, |p| {
            p.size = 0.35;
            p.decay_s = 1.1;
            p.damping = 0.5;
            p.predelay_ms = 10.0;
        }),
        with("Bright Plate", &["bright", "medium"], "Barely damped, so the top end rings.", 0.22, |p| {
            p.size = 0.6;
            p.decay_s = 2.5;
            p.damping = 0.15;
        }),
        with("Long Plate", &["long", "wash"], "Five seconds of plate.", 0.3, |p| {
            p.size = 0.8;
            p.decay_s = 5.0;
            p.damping = 0.3;
            p.predelay_ms = 20.0;
        }),
        with("Mono Plate", &["mono", "centre"], "Collapsed to the centre, for a source that stays put.", 0.2, |p| {
            p.size = 0.5;
            p.decay_s = 2.0;
            p.damping = 0.4;
            p.width = 0.0;
        }),
    ]
}

// --- Gate -------------------------------------------------------------------

fn gate() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::GateParams)| {
        patch(EffectKind::Gate, name, tags, description, |effect| {
            if let EffectParams::Gate(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Tight Drum Gate", &["drums", "tight"], "Fast open, short hold, closes hard.", |p| {
            p.threshold_db = -30.0;
            p.attack_ms = 0.2;
            p.hold_ms = 20.0;
            p.release_ms = 60.0;
            p.range_db = -80.0;
        }),
        with("Noise Floor", &["cleanup", "gentle"], "Trims hiss between phrases without chopping them.", |p| {
            p.threshold_db = -55.0;
            p.attack_ms = 2.0;
            p.hold_ms = 40.0;
            p.release_ms = 200.0;
            p.range_db = -40.0;
        }),
        with("Gated Tail", &["reverb", "eighties"], "Holds a reverb tail open, then cuts it dead.", |p| {
            p.threshold_db = -24.0;
            p.attack_ms = 0.5;
            p.hold_ms = 120.0;
            p.release_ms = 30.0;
            p.range_db = -80.0;
        }),
        with("Soft Expander", &["expander", "gentle"], "Twelve decibels of downward expansion, slowly.", |p| {
            p.threshold_db = -45.0;
            p.attack_ms = 5.0;
            p.hold_ms = 30.0;
            p.release_ms = 300.0;
            p.range_db = -12.0;
        }),
        with("Stutter Chop", &["rhythmic", "extreme"], "A high threshold and no hold: only the peaks pass.", |p| {
            p.threshold_db = -18.0;
            p.attack_ms = 0.1;
            p.hold_ms = 5.0;
            p.release_ms = 10.0;
            p.range_db = -80.0;
        }),
    ]
}

// --- Compressor -------------------------------------------------------------

fn compressor() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::CompressorParams)| {
        patch(EffectKind::Compressor, name, tags, description, |effect| {
            if let EffectParams::Compressor(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Gentle Glue", &["bus", "gentle"], "2:1 with a wide knee: holds a mix together.", |p| {
            p.threshold_db = -20.0;
            p.ratio = 2.0;
            p.attack_ms = 30.0;
            p.release_ms = 250.0;
            p.knee_db = 12.0;
            p.makeup_db = 2.0;
        }),
        with("Vocal Leveler", &["vocal", "medium"], "3.5:1, medium attack, rides a performance.", |p| {
            p.threshold_db = -22.0;
            p.ratio = 3.5;
            p.attack_ms = 8.0;
            p.release_ms = 150.0;
            p.knee_db = 8.0;
            p.makeup_db = 4.0;
        }),
        with("Bass Tightener", &["bass", "firm"], "5:1, slow enough to let the pick through.", |p| {
            p.threshold_db = -18.0;
            p.ratio = 5.0;
            p.attack_ms = 15.0;
            p.release_ms = 100.0;
            p.knee_db = 4.0;
            p.makeup_db = 3.0;
        }),
        with("Drum Smash", &["drums", "aggressive"], "10:1, one-millisecond attack, lots of makeup.", |p| {
            p.threshold_db = -28.0;
            p.ratio = 10.0;
            p.attack_ms = 1.0;
            p.release_ms = 80.0;
            p.knee_db = 2.0;
            p.makeup_db = 8.0;
        }),
        with("Pump", &["sidechain", "rhythmic"], "Fast release, low threshold: it breathes with the beat.", |p| {
            p.threshold_db = -30.0;
            p.ratio = 8.0;
            p.attack_ms = 5.0;
            p.release_ms = 40.0;
            p.knee_db = 3.0;
            p.makeup_db = 6.0;
        }),
        with("Peak Stop", &["limiting", "hard"], "20:1 and a hard knee: a limiter with a slower hand.", |p| {
            p.threshold_db = -12.0;
            p.ratio = 20.0;
            p.attack_ms = 0.5;
            p.release_ms = 60.0;
            p.knee_db = 0.0;
            p.makeup_db = 4.0;
        }),
    ]
}

// --- Limiter ----------------------------------------------------------------

fn limiter() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::LimiterParams)| {
        patch(EffectKind::Limiter, name, tags, description, |effect| {
            if let EffectParams::Limiter(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("Safety Ceiling", &["master", "transparent"], "No gain, a -0.1 dB ceiling, and a slow release that only catches the odd peak.", |p| {
            p.ceiling_db = -0.1;
            p.release_ms = 150.0;
            p.gain_db = 0.0;
        }),

        with("Mastering Lift", &["master", "gentle"], "3 dB louder with a slow release.", |p| {
            p.ceiling_db = -0.8;
            p.release_ms = 120.0;
            p.gain_db = 3.0;
        }),
        with("Loud", &["master", "loud"], "6 dB into the ceiling.", |p| {
            p.ceiling_db = -0.5;
            p.release_ms = 30.0;
            p.gain_db = 6.0;
        }),
        with("Brickwall Crush", &["extreme", "flat"], "14 dB of gain and a fast release: flat as a wall.", |p| {
            p.ceiling_db = -1.0;
            p.release_ms = 10.0;
            p.gain_db = 14.0;
        }),
        with("Bus Tamer", &["bus", "headroom"], "A -3 dB ceiling leaving the master some headroom.", |p| {
            p.ceiling_db = -3.0;
            p.release_ms = 80.0;
            p.gain_db = 0.0;
        }),
    ]
}

// --- Buffer -----------------------------------------------------------------

fn buffer() -> Vec<EffectFactoryPatch> {
    let with = |name, tags, description, edit: fn(&mut crate::BufferParams)| {
        patch(EffectKind::Buffer, name, tags, description, |effect| {
            if let EffectParams::Buffer(params) = &mut effect.params {
                edit(params);
            }
        })
    };
    vec![
        with("One Bar", &["short", "loop"], "One bar of memory, following the input.", |p| {
            p.bars = 1;
            p.offset_beats = 0.0;
            p.crossfade_ms = 2.5;
        }),
        with("Two Bar Stutter", &["stutter", "tight"], "Two bars with the fastest crossfade.", |p| {
            p.bars = 2;
            p.offset_beats = 0.0;
            p.crossfade_ms = 1.0;
        }),
        with("Eight Bar Recall", &["long", "recall"], "Eight bars to reach back into, with a softer fade than the default.", |p| {
            p.bars = 8;
            p.offset_beats = 0.0;
            p.crossfade_ms = 8.0;
        }),

        with("Beat Behind", &["offset", "echo"], "Reads one beat behind the writer.", |p| {
            p.bars = 4;
            p.offset_beats = 1.0;
            p.crossfade_ms = 5.0;
        }),
        with("Smooth Fades", &["soft", "ambient"], "Long crossfades, so every jump is a swell.", |p| {
            p.bars = 4;
            p.offset_beats = 0.0;
            p.crossfade_ms = 20.0;
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_kind_has_a_bank_of_its_own_kind() {
        for kind in EffectKind::ALL {
            let bank = patches(kind);
            assert!(bank.len() >= 5, "{kind:?} has only {} patches", bank.len());
            for patch in &bank {
                assert_eq!(patch.effect.kind(), kind, "{} is the wrong kind", patch.name);
                assert_eq!(patch.category, CATEGORY);
                assert!(!patch.description.is_empty(), "{} says nothing", patch.name);
                assert!(!patch.tags.is_empty(), "{} has no tags", patch.name);
            }
        }
    }

    #[test]
    fn names_are_unique_within_a_kind() {
        for kind in EffectKind::ALL {
            let mut seen = HashSet::new();
            for patch in patches(kind) {
                assert!(seen.insert(patch.name), "{kind:?} has two {}", patch.name);
            }
        }
    }

    /// A patch identical to the defaults is a demo of nothing.
    #[test]
    fn every_patch_differs_from_the_defaults() {
        for kind in EffectKind::ALL {
            for patch in patches(kind) {
                assert_ne!(
                    patch.effect,
                    EffectSlotState::of_kind(kind),
                    "{} is the default {kind:?}",
                    patch.name
                );
            }
        }
    }

    /// Every descriptor-addressed value sits inside the range its knob can
    /// reach, so a factory patch never lands somewhere the face cannot show
    /// or the integrity pass would correct.
    #[test]
    fn every_value_is_within_its_descriptor_range() {
        for kind in EffectKind::ALL {
            for patch in patches(kind) {
                for descriptor in kind.descriptors() {
                    let Some(value) = patch.effect.params.get(descriptor.id) else {
                        continue;
                    };
                    assert!(
                        value >= descriptor.min && value <= descriptor.max,
                        "{}: {} is {value}, outside {}..={}",
                        patch.name,
                        descriptor.name,
                        descriptor.min,
                        descriptor.max
                    );
                }
                assert!((0.0..=1.0).contains(&patch.effect.wet_dry), "{}", patch.name);
                assert!(!patch.effect.bypassed, "{} ships bypassed", patch.name);
            }
        }
    }
}
