//! The master bus compressor: three measured laws, and the one detector that
//! runs whichever the voicing selects.
//!
//! `docs/plans/archive/master-bus-compressor/` is the work order (MOO-13), and
//! `docs/SCOPE.md` §2.1 is why it is not the channel strip's compressor with
//! new numbers. The reference run (`spikes/preamp-measure/RESULTS.md` §4)
//! split nine compressors cleanly on one axis: the opto and FET units hold
//! 2.4-3.6 times more reduction half a second after a long note than after a
//! short one, and every VCA unit holds exactly as much. The channel strip's
//! `process_comp` is built to have that programme dependence. **This one has
//! none**, and its three voicings differ instead in the two things the run
//! resolved best: the shape of the detector and the law of its timing.
//!
//! | Voicing | Unit | Detector | Curve | Timing |
//! | --- | --- | --- | --- | --- |
//! | Grip | SSL G bus | peak | soft knee | the SSL's marked attacks and releases, and Auto |
//! | Punch | API-2500 | **RMS** | near-hard knee | marked times with a 3.8 ms attack floor |
//! | Tube | Fairchild 670 | peak | **the 670's measured curve** | six coupled TIME positions |
//!
//! # A knob reads the marking; the law is the measurement
//!
//! Each unit's knob is printed with times the unit does not literally keep:
//! an SSL release marked 0.3 s recovers to 63% in 114 ms, which is what a
//! marking describing a fuller recovery looks like. The tables below hold
//! both -- the marking the face shows, the rig's measured time to 63%, and
//! the internal constant that makes this detector measure that time **by
//! the rig's own procedure** (a 2 kHz tone stepped from -40 to -12 dBFS at
//! about 10 dB of reduction; `measured_like_the_rig` in the tests). A marking
//! is a position on a switch, never a number in a formula, which is how a
//! knob here can read like the unit without anything on the face lying.
//!
//! # Where the ratios are honest and the units were not
//!
//! Both VCA units delivered about 3.5:1 on the static curve when set to 4:1.
//! A ratio here delivers what it reads: that difference is a calibration of
//! one plugin, not a law of the unit, and modelling it would put a knob
//! reading 4:1 over a line that is not.
//!
//! # The reduction is smoothed, not the level
//!
//! Detector, then the static curve, then attack and release on the
//! *reduction* in decibels. That order is what makes a release a clean
//! exponential in dB -- so every release constant in the tables is its own
//! measured time -- and it is also what lets Tube's two-stage positions be two
//! followers summed rather than a memory of how long the loud part lasted.

use mooloop_core::gain::{db_to_linear, db_to_linear_unfloored, linear_to_db_unfloored};
use mooloop_core::strip::{
    BusCompVoicing, MasterSectionParams, MASTER_COMP_IN, MASTER_COMP_MAKEUP_DB, MASTER_COMP_MIX,
    MASTER_COMP_THRESHOLD_DB, MASTER_COMP_VOICING, MASTER_GRIP_ATTACK, MASTER_GRIP_RATIO,
    MASTER_GRIP_RELEASE, MASTER_PUNCH_ATTACK, MASTER_PUNCH_RATIO, MASTER_PUNCH_RELEASE,
    MASTER_TUBE_TIME,
};

use crate::bus::StereoBus;
use crate::dynamics::{compressor_gain_db, time_coeff};
use crate::node::DynamicsFrame;
use crate::smooth::Smoothed;

/// Threshold, makeup and mix all move the gain directly, so a step in any of
/// them is a click. The strip's constant, for the strip's reason.
const PARAM_SMOOTH_S: f32 = 0.005;

/// A reduction, or a mean square, this close to where it is heading has
/// arrived. Far below anything audible, and far above `f32`'s subnormals,
/// which is where an asymptotic decay would otherwise spend its tail.
const SNAP_DB: f32 = 1.0e-6;
const SNAP_MEAN_SQUARE: f32 = 1.0e-20;

/// One marked position of a timing switch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    /// What the unit's panel prints, and what the face shows.
    pub marking: &'static str,
    /// Time to 63% of the change, as the rig measured the unit.
    pub measured_t63_ms: f32,
    /// This detector's own time constant, fitted so that it measures
    /// `measured_t63_ms` by the rig's procedure.
    pub tau_ms: f32,
}

/// One marked position of a ratio switch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ratio {
    pub marking: &'static str,
    pub ratio: f32,
}

/// One of the 670's six TIME positions: attack and release coupled, and on
/// positions 5 and 6 a release in two stages.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TubeTime {
    pub marking: &'static str,
    pub attack: Timing,
    /// The release as measured, with `tau_ms` the fast stage's constant.
    pub release: Timing,
    /// The slow stage's constant, and how much of the reduction it carries.
    /// A weight of 0 is a single-stage release.
    pub slow_release_ms: f32,
    pub slow_weight: f32,
}

/// How the level the curve reads is taken.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Detector {
    /// The louder channel's instantaneous magnitude.
    Peak,
    /// A running mean of the two channels' power over `window_ms`, read so
    /// that a steady sine reads its own peak -- the threshold then means the
    /// same thing on a tone under every voicing, and what differs is the
    /// transient, which is the point.
    Rms { window_ms: f32 },
}

/// The static curve: reduction against how far over the threshold the
/// detector reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    /// A ratio line with a quadratic knee `knee_db` wide, centred on the
    /// threshold (`crate::dynamics::compressor_gain_db`).
    Knee { knee_db: f32 },
    /// Points of `(dB over the threshold, dB of reduction)`, joined by
    /// straight lines and continued past the last at its slope.
    Measured(&'static [(f32, f32)]),
}

/// One voicing, as data. Deliberately a table with no behaviour attached,
/// like the strip's voicings, so a better measurement edits rows and
/// nothing else.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BusCompLaw {
    pub detector: Detector,
    pub curve: Curve,
    /// Empty where the curve is the ratio (Tube).
    pub ratios: &'static [Ratio],
    /// Empty where attack and release are coupled into `times` (Tube).
    pub attacks: &'static [Timing],
    pub releases: &'static [Timing],
    /// Empty except for Tube.
    pub times: &'static [TubeTime],
}

/// The SSL's ratio switch.
pub const GRIP_RATIOS: [Ratio; 3] = [
    Ratio { marking: "2:1", ratio: 2.0 },
    Ratio { marking: "4:1", ratio: 4.0 },
    Ratio { marking: "10:1", ratio: 10.0 },
];

/// The SSL's attack switch. Its two fastest positions resolve in
/// quarter-milliseconds on a 2 kHz tone -- a peak detector sees a new peak
/// every half cycle -- which is also why the rig calls any attack under a
/// millisecond a ceiling rather than a value.
pub const GRIP_ATTACKS: [Timing; 6] = [
    Timing { marking: "0.1 ms", measured_t63_ms: 0.31, tau_ms: 0.135 },
    Timing { marking: "0.3 ms", measured_t63_ms: 0.44, tau_ms: 0.271 },
    Timing { marking: "1 ms", measured_t63_ms: 0.94, tau_ms: 0.549 },
    Timing { marking: "3 ms", measured_t63_ms: 3.46, tau_ms: 1.985 },
    Timing { marking: "10 ms", measured_t63_ms: 8.88, tau_ms: 5.414 },
    Timing { marking: "30 ms", measured_t63_ms: 21.4, tau_ms: 14.46 },
];

/// The SSL's release switch. A one-pole on the reduction releases to 37% in
/// its own constant, so these are the measurements themselves.
pub const GRIP_RELEASES: [Timing; 5] = [
    Timing { marking: "0.1 s", measured_t63_ms: 77.2, tau_ms: 77.2 },
    Timing { marking: "0.3 s", measured_t63_ms: 114.2, tau_ms: 114.2 },
    Timing { marking: "0.6 s", measured_t63_ms: 222.2, tau_ms: 222.2 },
    Timing { marking: "1.2 s", measured_t63_ms: 410.8, tau_ms: 410.8 },
    Timing { marking: "Auto", measured_t63_ms: 3382.2, tau_ms: 3382.2 },
];

/// The API-2500's ratio switch.
pub const PUNCH_RATIOS: [Ratio; 6] = [
    Ratio { marking: "1.5:1", ratio: 1.5 },
    Ratio { marking: "2:1", ratio: 2.0 },
    Ratio { marking: "3:1", ratio: 3.0 },
    Ratio { marking: "4:1", ratio: 4.0 },
    Ratio { marking: "6:1", ratio: 6.0 },
    Ratio { marking: "10:1", ratio: 10.0 },
];

/// The API-2500's attack switch. **The floor is the unit**: the 0.03 and
/// 0.1 ms markings measure 3.81 and 3.98 ms, and here that floor is the RMS
/// window ([`PUNCH_RMS_WINDOW_MS`]) rather than a clamp on the knob.
pub const PUNCH_ATTACKS: [Timing; 7] = [
    Timing { marking: "0.03 ms", measured_t63_ms: 3.81, tau_ms: 0.0 },
    Timing { marking: "0.1 ms", measured_t63_ms: 3.98, tau_ms: 0.239 },
    Timing { marking: "0.3 ms", measured_t63_ms: 6.15, tau_ms: 1.854 },
    Timing { marking: "1 ms", measured_t63_ms: 12.98, tau_ms: 7.542 },
    Timing { marking: "3 ms", measured_t63_ms: 20.9, tau_ms: 15.07 },
    Timing { marking: "10 ms", measured_t63_ms: 29.21, tau_ms: 23.33 },
    Timing { marking: "30 ms", measured_t63_ms: 67.54, tau_ms: 62.30 },
];

/// The API-2500's release switch. The RMS window adds its own lag to the
/// reduction's, so each constant is fitted a little short of its time.
pub const PUNCH_RELEASES: [Timing; 6] = [
    Timing { marking: "0.05 s", measured_t63_ms: 44.25, tau_ms: 27.37 },
    Timing { marking: "0.1 s", measured_t63_ms: 61.23, tau_ms: 44.89 },
    Timing { marking: "0.2 s", measured_t63_ms: 96.73, tau_ms: 80.81 },
    Timing { marking: "0.5 s", measured_t63_ms: 477.75, tau_ms: 462.46 },
    Timing { marking: "1 s", measured_t63_ms: 907.73, tau_ms: 892.54 },
    Timing { marking: "2 s", measured_t63_ms: 1713.69, tau_ms: 1698.83 },
];

/// The RMS detector's window, fitted so the fastest attack measures the
/// unit's 3.8 ms floor.
pub const PUNCH_RMS_WINDOW_MS: f32 = 9.815;

/// The 670's six TIME positions. Releases reproduce the published
/// 0.3/0.8/2/5/10/25 s at about a third, as the rig found; positions 5 and 6
/// are two stages, a 495 ms one they share and a slow one at a third of the
/// published time, weighted to land the measured 63% (and, on position 5,
/// the measured 90% at 6.0 s).
pub const TUBE_TIMES: [TubeTime; 6] = [
    TubeTime {
        marking: "1",
        attack: Timing { marking: "1", measured_t63_ms: 1.52, tau_ms: 0.828 },
        release: Timing { marking: "1", measured_t63_ms: 103.6, tau_ms: 104.1 },
        slow_release_ms: 104.1,
        slow_weight: 0.0,
    },
    TubeTime {
        marking: "2",
        attack: Timing { marking: "2", measured_t63_ms: 1.94, tau_ms: 1.080 },
        release: Timing { marking: "2", measured_t63_ms: 306.1, tau_ms: 306.1 },
        slow_release_ms: 306.1,
        slow_weight: 0.0,
    },
    TubeTime {
        marking: "3",
        attack: Timing { marking: "3", measured_t63_ms: 3.54, tau_ms: 1.866 },
        release: Timing { marking: "3", measured_t63_ms: 909.1, tau_ms: 909.3 },
        slow_release_ms: 909.3,
        slow_weight: 0.0,
    },
    TubeTime {
        marking: "4",
        attack: Timing { marking: "4", measured_t63_ms: 7.10, tau_ms: 3.763 },
        release: Timing { marking: "4", measured_t63_ms: 1793.1, tau_ms: 1793.6 },
        slow_release_ms: 1793.6,
        slow_weight: 0.0,
    },
    TubeTime {
        marking: "5",
        attack: Timing { marking: "5", measured_t63_ms: 3.52, tau_ms: 1.862 },
        release: Timing { marking: "5", measured_t63_ms: 1759.6, tau_ms: 495.3 },
        slow_release_ms: 3333.3,
        slow_weight: 0.6048,
    },
    TubeTime {
        marking: "6",
        attack: Timing { marking: "6", measured_t63_ms: 1.56, tau_ms: 0.793 },
        release: Timing { marking: "6", measured_t63_ms: 1425.7, tau_ms: 495.3 },
        slow_release_ms: 8333.3,
        slow_weight: 0.3964,
    },
];

/// The 670's static curve as the rig measured it, from a threshold at the
/// middle of its onset: about 2:1 as it starts and 8:1 thirty decibels over.
/// No knee formula has that shape, so the measurement is the curve.
pub const TUBE_CURVE: [(f32, f32); 11] = [
    (-3.0, 0.0),
    (0.0, 0.658),
    (3.0, 2.003),
    (6.0, 3.784),
    (9.0, 5.838),
    (12.0, 8.076),
    (15.0, 10.439),
    (18.0, 12.877),
    (21.0, 15.367),
    (24.0, 17.912),
    (27.0, 20.538),
];

pub const GRIP_LAW: BusCompLaw = BusCompLaw {
    detector: Detector::Peak,
    curve: Curve::Knee { knee_db: 8.4 },
    ratios: &GRIP_RATIOS,
    attacks: &GRIP_ATTACKS,
    releases: &GRIP_RELEASES,
    times: &[],
};

pub const PUNCH_LAW: BusCompLaw = BusCompLaw {
    detector: Detector::Rms { window_ms: PUNCH_RMS_WINDOW_MS },
    curve: Curve::Knee { knee_db: 3.4 },
    ratios: &PUNCH_RATIOS,
    attacks: &PUNCH_ATTACKS,
    releases: &PUNCH_RELEASES,
    times: &[],
};

pub const TUBE_LAW: BusCompLaw = BusCompLaw {
    detector: Detector::Peak,
    curve: Curve::Measured(&TUBE_CURVE),
    ratios: &[],
    attacks: &[],
    releases: &[],
    times: &TUBE_TIMES,
};

/// What `voicing` is made of.
pub const fn bus_comp_law(voicing: BusCompVoicing) -> &'static BusCompLaw {
    match voicing {
        BusCompVoicing::Grip => &GRIP_LAW,
        BusCompVoicing::Punch => &PUNCH_LAW,
        BusCompVoicing::Tube => &TUBE_LAW,
    }
}

/// A position into a table, clamped onto its nearest end rather than
/// wrapped, as a strip band's is: a hand-edited file lands on the last
/// position, which is audible and sane.
fn at<T: Copy>(table: &[T], position: u8) -> T {
    table[(position as usize).min(table.len() - 1)]
}

/// The settings a section's positions select under its voicing, resolved to
/// numbers: what the detector runs on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub detector: Detector,
    pub curve: Curve,
    /// The ratio line's ratio; unused by a measured curve.
    pub ratio: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub slow_release_ms: f32,
    pub slow_weight: f32,
}

pub fn resolve(params: &MasterSectionParams) -> Resolved {
    let law = bus_comp_law(params.voicing);
    let (ratio, attack, release, slow_release_ms, slow_weight) = match params.voicing {
        BusCompVoicing::Grip => {
            let release = at(law.releases, params.grip_release).tau_ms;
            (
                at(law.ratios, params.grip_ratio).ratio,
                at(law.attacks, params.grip_attack).tau_ms,
                release,
                release,
                0.0,
            )
        }
        BusCompVoicing::Punch => {
            let release = at(law.releases, params.punch_release).tau_ms;
            (
                at(law.ratios, params.punch_ratio).ratio,
                at(law.attacks, params.punch_attack).tau_ms,
                release,
                release,
                0.0,
            )
        }
        BusCompVoicing::Tube => {
            let time = at(law.times, params.tube_time);
            (
                1.0,
                time.attack.tau_ms,
                time.release.tau_ms,
                time.slow_release_ms,
                time.slow_weight,
            )
        }
    };
    Resolved {
        detector: law.detector,
        curve: law.curve,
        ratio,
        attack_ms: attack,
        release_ms: release,
        slow_release_ms,
        slow_weight,
    }
}

/// The reduction, in dB as a positive depth, that `curve` asks for at
/// `level_db` against `threshold_db`.
pub fn static_reduction_db(curve: Curve, ratio: f32, threshold_db: f32, level_db: f32) -> f32 {
    match curve {
        Curve::Knee { knee_db } => -compressor_gain_db(level_db, threshold_db, ratio, knee_db),
        Curve::Measured(points) => {
            let over = level_db - threshold_db;
            let (first, last) = (points[0], points[points.len() - 1]);
            if over <= first.0 {
                return first.1;
            }
            for pair in points.windows(2) {
                let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
                if over <= x1 {
                    return y0 + (y1 - y0) * (over - x0) / (x1 - x0);
                }
            }
            let before = points[points.len() - 2];
            last.1 + (over - last.0) * (last.1 - before.1) / (last.0 - before.0)
        }
    }
}

/// One follower step on a reduction: towards `target` at the attack's rate
/// when it is deeper, at the release's when it is shallower, and onto it
/// once the gap is inaudible.
fn follow(current: f32, target: f32, attack: f32, release: f32) -> f32 {
    let coeff = if target > current { attack } else { release };
    let next = target + coeff * (current - target);
    if (next - target).abs() < SNAP_DB {
        target
    } else {
        next
    }
}

/// The master bus compressor, at one sample rate. See the module docs.
#[derive(Debug, Clone)]
pub struct BusComp {
    params: MasterSectionParams,
    sample_rate: u32,
    resolved: Resolved,
    attack: f32,
    release: f32,
    slow_release: f32,
    rms: f32,
    /// The running power the RMS detector reads. Tracked under every voicing,
    /// so switching to Punch mid-mix starts from what the mix has been doing
    /// rather than from silence.
    mean_square: f32,
    /// The two followers, in dB of reduction. They attack together, so a
    /// short hit and a long one leave them in the same place -- the reason
    /// Tube's two-stage release is not programme dependence.
    fast: f32,
    slow: f32,
    threshold: Smoothed,
    makeup: Smoothed,
    mix: Smoothed,
    /// Block extremes, for the meter: the deepest reduction and the loudest
    /// detector reading since the block began.
    block_reduction_db: f32,
    block_detector: f32,
}

impl BusComp {
    pub fn new(params: MasterSectionParams, sample_rate: u32) -> Self {
        let smoothed = |value| Smoothed::new(value, PARAM_SMOOTH_S, sample_rate);
        let mut comp = Self {
            params,
            sample_rate,
            resolved: resolve(&params),
            attack: 0.0,
            release: 0.0,
            slow_release: 0.0,
            rms: 0.0,
            mean_square: 0.0,
            fast: 0.0,
            slow: 0.0,
            threshold: smoothed(params.threshold_db),
            makeup: smoothed(db_to_linear(params.makeup_db)),
            mix: smoothed(params.mix),
            block_reduction_db: 0.0,
            block_detector: 0.0,
        };
        comp.retime();
        comp
    }

    pub fn params(&self) -> &MasterSectionParams {
        &self.params
    }

    /// Replace the settings wholesale (a document arriving): jump to them,
    /// since there is nothing sounding to be continuous with.
    pub fn set_params(&mut self, params: MasterSectionParams) {
        self.params = params;
        self.retime();
        self.snap();
    }

    fn snap(&mut self) {
        self.threshold.reset_to(self.params.threshold_db);
        self.makeup.reset_to(db_to_linear(self.params.makeup_db));
        self.mix.reset_to(self.params.mix);
    }

    fn retime(&mut self) {
        self.resolved = resolve(&self.params);
        let rate = self.sample_rate;
        self.attack = time_coeff(self.resolved.attack_ms, rate);
        self.release = time_coeff(self.resolved.release_ms, rate);
        self.slow_release = time_coeff(self.resolved.slow_release_ms, rate);
        self.rms = time_coeff(
            match self.resolved.detector {
                Detector::Rms { window_ms } => window_ms,
                Detector::Peak => PUNCH_RMS_WINDOW_MS,
            },
            rate,
        );
    }

    /// Move one parameter, and report whether it was one of this section's.
    ///
    /// Switching the section in starts it empty and at its knobs, for the
    /// strip's reason: followers last fed audio minutes ago would put that
    /// into the first block back. Switching voicing keeps the followers --
    /// a reduction in dB means the same thing under every law -- so a voicing
    /// change mid-mix moves the character rather than the level.
    pub fn apply_param(&mut self, id: u32, value: f32) -> bool {
        if !self.params.set(id, value) {
            return false;
        }
        match id {
            MASTER_COMP_IN => {
                if self.params.comp_in {
                    self.reset();
                    self.snap();
                }
            }
            MASTER_COMP_THRESHOLD_DB => self.threshold.set_target(self.params.threshold_db),
            MASTER_COMP_MAKEUP_DB => self.makeup.set_target(db_to_linear(self.params.makeup_db)),
            MASTER_COMP_MIX => self.mix.set_target(self.params.mix),
            MASTER_COMP_VOICING | MASTER_GRIP_RATIO | MASTER_GRIP_ATTACK | MASTER_GRIP_RELEASE
            | MASTER_PUNCH_RATIO | MASTER_PUNCH_ATTACK | MASTER_PUNCH_RELEASE
            | MASTER_TUBE_TIME => self.retime(),
            _ => {}
        }
        true
    }

    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        if sample_rate == self.sample_rate {
            return;
        }
        self.sample_rate = sample_rate;
        self.retime();
        for smoothed in [&mut self.threshold, &mut self.makeup, &mut self.mix] {
            smoothed.set_time(PARAM_SMOOTH_S, sample_rate);
        }
    }

    pub fn reset(&mut self) {
        self.mean_square = 0.0;
        self.fast = 0.0;
        self.slow = 0.0;
        self.block_reduction_db = 0.0;
        self.block_detector = 0.0;
    }

    /// The reduction being applied now, in dB as a positive depth.
    pub fn reduction_db(&self) -> f32 {
        let weight = self.resolved.slow_weight;
        if weight == 0.0 {
            self.fast
        } else {
            (1.0 - weight) * self.fast + weight * self.slow
        }
    }

    /// What the compressor did in the block just rendered, or `None` while
    /// it is out.
    pub fn dynamics_frame(&self) -> Option<DynamicsFrame> {
        self.params.comp_in.then_some(DynamicsFrame {
            detector_db: linear_to_db_unfloored(self.block_detector),
            reduction_db: -self.block_reduction_db,
        })
    }

    /// Whether a block skipped would leave it where a block of silence
    /// would: out, or settled with nothing held.
    pub fn is_at_rest(&self) -> bool {
        !self.params.comp_in
            || (self.fast == 0.0
                && self.slow == 0.0
                && self.mean_square == 0.0
                && self.threshold.is_settled()
                && self.makeup.is_settled()
                && self.mix.is_settled())
    }

    /// Run the section over the block, in place. Out is out: nothing is
    /// read or written.
    pub fn process_block(&mut self, bus: &mut StereoBus, frames: usize) {
        self.begin_block();
        self.process_range(bus, 0, frames);
    }

    /// Start a block: forget the last one's extremes, so
    /// [`Self::dynamics_frame`] reports only what the next ranges do.
    pub fn begin_block(&mut self) {
        self.block_reduction_db = 0.0;
        self.block_detector = 0.0;
    }

    /// Run the section over `start..end` of the bus, in place, carrying the
    /// block's extremes on from any range before it. [`Self::process_block`]
    /// is one of these after [`Self::begin_block`]; the Bus Comp insert
    /// (MOO-216) runs several, cut at its parameter events. Out is out.
    pub fn process_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        if !self.params.comp_in {
            return;
        }
        let end = end.min(bus.capacity());
        for frame in start..end {
            let (l, r) = self.process_frame(bus.l[frame], bus.r[frame]);
            bus.l[frame] = l;
            bus.r[frame] = r;
        }
    }

    /// One stereo frame through the section. Public for the measurements,
    /// which read [`Self::reduction_db`] after every frame the way the rig
    /// read a gain trace.
    pub fn process_frame(&mut self, left: f32, right: f32) -> (f32, f32) {
        let threshold_db = self.threshold.advance();
        let makeup = self.makeup.advance();
        let mix = self.mix.advance();

        let power = 0.5 * (left * left + right * right);
        self.mean_square = power + self.rms * (self.mean_square - power);
        if power == 0.0 && self.mean_square < SNAP_MEAN_SQUARE {
            self.mean_square = 0.0;
        }
        let level = match self.resolved.detector {
            Detector::Peak => left.abs().max(right.abs()),
            // `sqrt(2 * mean square)` is a steady sine's peak.
            Detector::Rms { .. } => (2.0 * self.mean_square).sqrt(),
        };
        self.block_detector = self.block_detector.max(level);
        let target = if level > 0.0 {
            static_reduction_db(
                self.resolved.curve,
                self.resolved.ratio,
                threshold_db,
                linear_to_db_unfloored(level),
            )
            .max(0.0)
        } else {
            0.0
        };
        self.fast = follow(self.fast, target, self.attack, self.release);
        if self.resolved.slow_weight != 0.0 {
            self.slow = follow(self.slow, target, self.attack, self.slow_release);
        } else {
            self.slow = 0.0;
        }
        let depth = self.reduction_db();
        self.block_reduction_db = self.block_reduction_db.max(depth);
        // Exactly one while nothing is reduced, so a section that is in and
        // idle, at no makeup and a full mix, is the identity.
        let reduced = if depth == 0.0 {
            1.0
        } else {
            db_to_linear_unfloored(-depth)
        };
        let gain = (1.0 - mix) + mix * reduced * makeup;
        (left * gain, right * gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::strip::{
        GRIP_ATTACK_POSITIONS, GRIP_RATIO_POSITIONS, GRIP_RELEASE_POSITIONS,
        PUNCH_ATTACK_POSITIONS, PUNCH_RATIO_POSITIONS, PUNCH_RELEASE_POSITIONS,
        TUBE_TIME_POSITIONS,
    };

    const RATE: u32 = 48_000;
    /// The rig's carrier, reference level and resting level
    /// (`spikes/preamp-measure/measure_comp.py`).
    const CARRIER_HZ: f32 = 2_000.0;
    const LOUD_DB: f32 = -12.0;
    const QUIET_DB: f32 = -40.0;
    /// Thresholds that put each voicing at about the rig's 10 dB of
    /// reduction on the loud tone.
    const GRIP_THRESHOLD: f32 = -25.5;
    const PUNCH_THRESHOLD: f32 = -25.5;
    const TUBE_THRESHOLD: f32 = -27.0;

    fn section(voicing: BusCompVoicing, edit: impl FnOnce(&mut MasterSectionParams)) -> MasterSectionParams {
        let mut params = MasterSectionParams {
            comp_in: true,
            voicing,
            threshold_db: match voicing {
                BusCompVoicing::Grip => GRIP_THRESHOLD,
                BusCompVoicing::Punch => PUNCH_THRESHOLD,
                BusCompVoicing::Tube => TUBE_THRESHOLD,
            },
            ..MasterSectionParams::default()
        };
        // 4:1 on both ratio switches, as the rig set them.
        params.grip_ratio = 1;
        params.punch_ratio = 3;
        edit(&mut params);
        params
    }

    fn sine(db: f32, n: usize) -> f32 {
        sine_at(RATE, db, n)
    }

    fn sine_at(rate: u32, db: f32, n: usize) -> f32 {
        db_to_linear_unfloored(db)
            * (2.0 * core::f32::consts::PI * CARRIER_HZ * n as f32 / rate as f32).sin()
    }

    /// The reduction after every frame of a tone at `level(n)` dB, gated by
    /// `gate(n)`.
    fn trace(
        params: MasterSectionParams,
        frames: usize,
        level: impl Fn(usize) -> f32,
    ) -> Vec<f32> {
        trace_at(RATE, params, frames, level)
    }

    fn trace_at(
        rate: u32,
        params: MasterSectionParams,
        frames: usize,
        level: impl Fn(usize) -> f32,
    ) -> Vec<f32> {
        let mut comp = BusComp::new(params, rate);
        (0..frames)
            .map(|n| {
                let x = sine_at(rate, level(n), n);
                comp.process_frame(x, x);
                comp.reduction_db()
            })
            .collect()
    }

    /// The rig's step response: quiet, then the loud tone for `hold_s`, then
    /// quiet for `release_s`. Returns the attack's and the release's time to
    /// 63%, in milliseconds, and the reduction the hold settled at.
    fn measured_like_the_rig(
        params: MasterSectionParams,
        hold_s: f32,
        release_s: f32,
    ) -> (f32, f32, f32) {
        measured_like_the_rig_at(RATE, params, hold_s, release_s)
    }

    fn measured_like_the_rig_at(
        rate: u32,
        params: MasterSectionParams,
        hold_s: f32,
        release_s: f32,
    ) -> (f32, f32, f32) {
        let pre = (0.05 * rate as f32) as usize;
        let hold = (hold_s * rate as f32) as usize;
        let release = (release_s * rate as f32) as usize;
        let gr = trace_at(rate, params, pre + hold + release, |n| {
            if n >= pre && n < pre + hold {
                LOUD_DB
            } else {
                QUIET_DB
            }
        });
        let end = pre + hold;
        let window = (0.02 * rate as f32) as usize;
        let settled = gr[end - window..end].iter().sum::<f32>() / window as f32;
        let ms = |frames: usize| frames as f32 * 1000.0 / rate as f32;
        let attack = gr[pre..end]
            .iter()
            .position(|&g| g >= 0.632 * settled)
            .map(ms)
            .unwrap_or(f32::INFINITY);
        let start = gr[end - 1];
        let release = gr[end..]
            .iter()
            .position(|&g| g <= 0.368 * start)
            .map(ms)
            .unwrap_or(f32::INFINITY);
        (attack, release, settled)
    }

    /// Within 10% of the rig, or within half a cycle of its 2 kHz carrier:
    /// a peak detector sees a new peak only every 0.25 ms, which is the
    /// resolution both this measurement and the rig's have.
    fn assert_near(what: &str, got: f32, want: f32) {
        let tolerance = (0.10 * want).max(0.26);
        assert!(
            (got - want).abs() <= tolerance,
            "{what}: measured {got:.3} ms against the unit's {want:.3} ms"
        );
    }

    /// The constants were fitted at 48 kHz; the laws are times, and hold at
    /// the other rates the measurement kit runs at (MOO-117), within the same
    /// tolerance.
    #[test]
    fn the_laws_hold_at_other_sample_rates() {
        for rate in [44_100, 96_000] {
            for (name, params, want) in [
                ("Grip 3 ms", section(BusCompVoicing::Grip, |p| p.grip_attack = 3), 3.46),
                ("Grip 30 ms", section(BusCompVoicing::Grip, |p| p.grip_attack = 5), 21.4),
                ("Punch floor", section(BusCompVoicing::Punch, |p| p.punch_attack = 0), 3.81),
                ("Punch 1 ms", section(BusCompVoicing::Punch, |p| p.punch_attack = 3), 12.98),
                ("Tube 1", section(BusCompVoicing::Tube, |p| p.tube_time = 0), 1.52),
                ("Tube 4", section(BusCompVoicing::Tube, |p| p.tube_time = 3), 7.10),
            ] {
                let (attack, _, _) = measured_like_the_rig_at(rate, params, 0.25, 0.01);
                assert_near(&format!("{name} at {rate} Hz"), attack, want);
            }
            let params = section(BusCompVoicing::Grip, |p| p.grip_release = 2);
            let (_, release, _) = measured_like_the_rig_at(rate, params, 0.2, 0.7);
            assert_near(&format!("Grip 0.6 s at {rate} Hz"), release, 222.2);
        }
    }

    #[test]
    fn every_table_has_a_row_for_every_position() {
        assert_eq!(GRIP_RATIOS.len(), GRIP_RATIO_POSITIONS as usize);
        assert_eq!(GRIP_ATTACKS.len(), GRIP_ATTACK_POSITIONS as usize);
        assert_eq!(GRIP_RELEASES.len(), GRIP_RELEASE_POSITIONS as usize);
        assert_eq!(PUNCH_RATIOS.len(), PUNCH_RATIO_POSITIONS as usize);
        assert_eq!(PUNCH_ATTACKS.len(), PUNCH_ATTACK_POSITIONS as usize);
        assert_eq!(PUNCH_RELEASES.len(), PUNCH_RELEASE_POSITIONS as usize);
        assert_eq!(TUBE_TIMES.len(), TUBE_TIME_POSITIONS as usize);
    }

    #[test]
    fn the_calibration_lands_near_the_rigs_ten_decibels() {
        for voicing in BusCompVoicing::ALL {
            let (_, _, settled) = measured_like_the_rig(section(voicing, |_| {}), 0.3, 0.05);
            assert!(
                (8.5..=11.5).contains(&settled),
                "{voicing:?} settled at {settled:.2} dB, not about 10"
            );
        }
    }

    #[test]
    fn grips_attacks_measure_like_the_ssl() {
        for (position, timing) in GRIP_ATTACKS.iter().enumerate() {
            let params = section(BusCompVoicing::Grip, |p| p.grip_attack = position as u8);
            let (attack, _, _) = measured_like_the_rig(params, 0.25, 0.01);
            assert_near(&format!("Grip attack {}", timing.marking), attack, timing.measured_t63_ms);
        }
    }

    #[test]
    fn grips_releases_measure_like_the_ssl() {
        for (position, timing) in GRIP_RELEASES.iter().enumerate() {
            let params = section(BusCompVoicing::Grip, |p| p.grip_release = position as u8);
            let (_, release, _) =
                measured_like_the_rig(params, 0.2, 3.0 * timing.measured_t63_ms / 1000.0);
            assert_near(&format!("Grip release {}", timing.marking), release, timing.measured_t63_ms);
        }
    }

    #[test]
    fn punchs_attacks_measure_like_the_api_including_its_floor() {
        let mut fastest = Vec::new();
        for (position, timing) in PUNCH_ATTACKS.iter().enumerate() {
            let params = section(BusCompVoicing::Punch, |p| p.punch_attack = position as u8);
            let (attack, _, _) = measured_like_the_rig(params, 0.4, 0.01);
            assert_near(&format!("Punch attack {}", timing.marking), attack, timing.measured_t63_ms);
            if position < 2 {
                fastest.push(attack);
            }
        }
        // The two fastest markings are one attack: the floor is the unit.
        assert!(
            (fastest[0] - fastest[1]).abs() <= 0.05 * fastest[1],
            "Punch's floor moved: {fastest:?}"
        );
        assert!(fastest[0] > 3.0, "Punch attacked faster than its floor: {}", fastest[0]);
    }

    #[test]
    fn punchs_releases_measure_like_the_api() {
        for (position, timing) in PUNCH_RELEASES.iter().enumerate() {
            let params = section(BusCompVoicing::Punch, |p| p.punch_release = position as u8);
            let (_, release, _) =
                measured_like_the_rig(params, 0.3, 3.0 * timing.measured_t63_ms / 1000.0);
            assert_near(&format!("Punch release {}", timing.marking), release, timing.measured_t63_ms);
        }
    }

    #[test]
    fn tubes_time_positions_measure_like_the_670() {
        for (position, time) in TUBE_TIMES.iter().enumerate() {
            let params = section(BusCompVoicing::Tube, |p| p.tube_time = position as u8);
            let (attack, release, _) =
                measured_like_the_rig(params, 0.3, 3.0 * time.release.measured_t63_ms / 1000.0);
            assert_near(&format!("Tube {} attack", time.marking), attack, time.attack.measured_t63_ms);
            assert_near(&format!("Tube {} release", time.marking), release, time.release.measured_t63_ms);
        }
    }

    /// Position 5's second stage: the rig measured its 90% recovery at 6.0 s,
    /// three times its 63% where one exponential would take 2.3 times.
    #[test]
    fn tubes_fifth_position_releases_in_two_stages() {
        let params = section(BusCompVoicing::Tube, |p| p.tube_time = 4);
        let pre = (0.05 * RATE as f32) as usize;
        let hold = (0.3 * RATE as f32) as usize;
        let gr = trace(params, pre + hold + 8 * RATE as usize, |n| {
            if n >= pre && n < pre + hold {
                LOUD_DB
            } else {
                QUIET_DB
            }
        });
        let start = gr[pre + hold - 1];
        let t90 = gr[pre + hold..].iter().position(|&g| g <= 0.1 * start).unwrap() as f32
            * 1000.0
            / RATE as f32;
        assert!((t90 - 5999.0).abs() < 600.0, "t90 is {t90:.0} ms, not about 6.0 s");
    }

    /// The whole difference from the channel strip, as the rig measured it:
    /// the reduction left 500 ms after a 30 ms hit and after a 3 s passage
    /// is the same fraction of where each started. Every voicing, and every
    /// one of Tube's positions including the two-stage ones.
    #[test]
    fn no_voicing_remembers_how_long_the_loud_part_lasted() {
        let mut cases: Vec<(String, MasterSectionParams)> = vec![
            ("Grip".into(), section(BusCompVoicing::Grip, |_| {})),
            ("Punch".into(), section(BusCompVoicing::Punch, |_| {})),
        ];
        for position in 0..TUBE_TIME_POSITIONS {
            cases.push((
                format!("Tube {}", position + 1),
                section(BusCompVoicing::Tube, |p| p.tube_time = position),
            ));
        }
        for (name, params) in cases {
            let left_after = |hold_s: f32| {
                let pre = (0.05 * RATE as f32) as usize;
                let hold = (hold_s * RATE as f32) as usize;
                let after = (0.5 * RATE as f32) as usize;
                let gr = trace(params, pre + hold + after + 1, |n| {
                    if n >= pre && n < pre + hold {
                        LOUD_DB
                    } else {
                        QUIET_DB
                    }
                });
                gr[pre + hold + after] / gr[pre + hold - 1]
            };
            let short = left_after(0.03);
            let long = left_after(3.0);
            // Within 5% of each other, or within half a percentage point of
            // the reduction where both have all but finished: the rig's
            // opto units differ by 12 points at this moment, and its VCAs
            // by a tenth of one.
            assert!(
                (short - long).abs() <= (0.05 * long).max(0.005),
                "{name}: {short:.4} of the reduction left after a short hit, {long:.4} after a long one"
            );
        }
    }

    /// The rig's detector question: a tone, and 10 ms bursts every 50 ms at
    /// the same peak, which are 7 dB lower in RMS. A peak detector reduces
    /// both alike; an RMS detector treats the bursts as the quieter signal
    /// they are. The rig: SSL 11.1 / 10.7, Fairchild 10.4 / 9.8, API 8.3 / 4.4.
    #[test]
    fn grip_and_tube_read_peaks_and_punch_reads_power() {
        let median = |params: MasterSectionParams, bursting: bool| {
            let frames = 4 * RATE as usize;
            let on = |n: usize| !bursting || (n % (RATE as usize / 20)) < RATE as usize / 100;
            let gr = trace(params, frames, |n| if on(n) { LOUD_DB } else { -300.0 });
            let mut held: Vec<f32> = (2 * RATE as usize..frames)
                .filter(|&n| on(n))
                .map(|n| gr[n])
                .collect();
            held.sort_by(f32::total_cmp);
            held[held.len() / 2]
        };
        let rig = |voicing| {
            section(voicing, |p| {
                // The rig's base settings: SSL 1 ms / 0.3 s, API 1 ms / 0.2 s,
                // Fairchild position 1.
                p.grip_attack = 2;
                p.grip_release = 1;
                p.punch_attack = 3;
                p.punch_release = 2;
                p.tube_time = 0;
            })
        };
        for voicing in [BusCompVoicing::Grip, BusCompVoicing::Tube] {
            let (tone, bursts) = (median(rig(voicing), false), median(rig(voicing), true));
            assert!(
                (tone - bursts).abs() < 1.0,
                "{voicing:?} is not reading peaks: {tone:.2} dB on the tone, {bursts:.2} on bursts"
            );
        }
        let (tone, bursts) = (
            median(rig(BusCompVoicing::Punch), false),
            median(rig(BusCompVoicing::Punch), true),
        );
        assert!(
            tone - bursts >= 3.0,
            "Punch is not reading power: {tone:.2} dB on the tone, {bursts:.2} on bursts"
        );
    }

    /// Past the knee a steady tone sits on the ratio line the knob reads, and
    /// Tube sits on the curve the rig measured.
    ///
    /// At the fastest attack. A peak detector with a slower one only charges
    /// on the crest of each cycle and lets go between them, so it settles a
    /// fraction of a decibel short of the line -- which a real unit does too,
    /// and is the ripple the SSL's third harmonic at 60 Hz comes from, but is
    /// not what this test is about.
    #[test]
    fn a_steady_tone_sits_on_the_static_curve() {
        let curve = |voicing, ratio: f32, over: f32| {
            static_reduction_db(bus_comp_law(voicing).curve, ratio, 0.0, over)
        };
        for voicing in [BusCompVoicing::Grip, BusCompVoicing::Punch] {
            for ratio in [2.0f32, 4.0, 10.0] {
                let want = (1.0 - 1.0 / ratio) * 20.0;
                assert!((curve(voicing, ratio, 20.0) - want).abs() < 1e-4, "{voicing:?} {ratio}:1");
            }
        }
        let steady = |params: MasterSectionParams, level: f32| {
            let frames = RATE as usize;
            let gr = trace(params, frames, |_| level);
            let tail = &gr[frames - RATE as usize / 10..];
            tail.iter().sum::<f32>() / tail.len() as f32
        };
        for voicing in [BusCompVoicing::Grip, BusCompVoicing::Punch] {
            for position in 0..3u8 {
                let params = section(voicing, |p| {
                    p.grip_ratio = position;
                    p.punch_ratio = position;
                    p.grip_attack = 0;
                    p.punch_attack = 0;
                    p.grip_release = 2;
                    p.punch_release = 3;
                });
                let ratio = resolve(&params).ratio;
                let level = params.threshold_db + 20.0;
                let want = (1.0 - 1.0 / ratio) * 20.0;
                let got = steady(params, level);
                assert!(
                    (got - want).abs() < 0.35,
                    "{voicing:?} at {ratio}:1: {got:.2} dB, the line says {want:.2}"
                );
            }
        }
        for (input, rig) in [(-24.0, 2.003), (-18.0, 5.838), (-12.0, 10.439), (-6.0, 15.367)] {
            // Position 1, as the rig measured the curve.
            let params = section(BusCompVoicing::Tube, |p| p.tube_time = 0);
            let got = steady(params, input);
            assert!(
                (got - rig).abs() < 0.5,
                "Tube at {input} dBFS: {got:.2} dB where the 670 measured {rig}"
            );
        }
    }

    #[test]
    fn tubes_curve_is_monotone_and_steepens() {
        let mut previous_slope = 0.0f32;
        for pair in TUBE_CURVE.windows(2) {
            let slope = (pair[1].1 - pair[0].1) / (pair[1].0 - pair[0].0);
            assert!(slope > 0.0 && slope < 1.0, "a slope of {slope} is not a compressor");
            assert!(slope >= previous_slope - 0.02, "the 670's ratio fell at {:?}", pair[1]);
            previous_slope = slope;
        }
        // Continued past the last point at its own slope, never flat.
        let far = static_reduction_db(Curve::Measured(&TUBE_CURVE), 1.0, 0.0, 40.0);
        assert!(far > TUBE_CURVE[10].1);
    }

    /// Out is out, and in-but-idle is the identity: nothing reduced, no
    /// makeup and a full mix pass every sample bit for bit.
    #[test]
    fn out_and_idle_are_bit_transparent() {
        for params in [
            MasterSectionParams::default(),
            MasterSectionParams {
                comp_in: true,
                threshold_db: 0.0,
                ..MasterSectionParams::default()
            },
        ] {
            let mut comp = BusComp::new(params, RATE);
            let mut bus = StereoBus::with_capacity(512);
            for n in 0..512 {
                bus.l[n] = sine(-20.0, n);
                bus.r[n] = 0.5 * sine(-26.0, n + 7);
            }
            let (want_l, want_r) = (bus.l.clone(), bus.r.clone());
            comp.process_block(&mut bus, 512);
            for n in 0..512 {
                assert_eq!(bus.l[n].to_bits(), want_l[n].to_bits(), "left moved at {n}");
                assert_eq!(bus.r[n].to_bits(), want_r[n].to_bits(), "right moved at {n}");
            }
        }
    }

    #[test]
    fn a_mix_of_zero_is_the_dry_signal_exactly() {
        let params = section(BusCompVoicing::Punch, |p| {
            p.mix = 0.0;
            p.makeup_db = 6.0;
        });
        let mut comp = BusComp::new(params, RATE);
        for n in 0..4_800 {
            let x = sine(LOUD_DB, n);
            let (l, r) = comp.process_frame(x, x);
            assert_eq!((l.to_bits(), r.to_bits()), (x.to_bits(), x.to_bits()));
        }
        assert!(comp.reduction_db() > 5.0, "the detector should still be working");
    }

    /// Both sides take one gain, so a limited image does not lean.
    #[test]
    fn both_sides_are_turned_down_together() {
        for voicing in BusCompVoicing::ALL {
            let mut comp = BusComp::new(section(voicing, |_| {}), RATE);
            let mut last = (0.0, 0.0);
            for n in 0..9_600 {
                let x = sine(LOUD_DB, n);
                last = comp.process_frame(x, 0.25 * x);
            }
            let x = sine(LOUD_DB, 9_600);
            let (l, r) = comp.process_frame(x, 0.25 * x);
            assert!((r - 0.25 * l).abs() < 1e-6, "{voicing:?} leaned: {l} {r} ({last:?})");
        }
    }

    #[test]
    fn switching_in_starts_empty_and_silence_comes_to_rest() {
        let mut comp = BusComp::new(section(BusCompVoicing::Grip, |_| {}), RATE);
        for n in 0..4_800 {
            let x = sine(LOUD_DB, n);
            comp.process_frame(x, x);
        }
        assert!(comp.reduction_db() > 5.0);
        comp.apply_param(MASTER_COMP_IN, 0.0);
        comp.apply_param(MASTER_COMP_IN, 1.0);
        assert_eq!(comp.reduction_db(), 0.0, "switched in holding old reduction");

        let mut comp = BusComp::new(section(BusCompVoicing::Punch, |_| {}), RATE);
        for n in 0..4_800 {
            let x = sine(LOUD_DB, n);
            comp.process_frame(x, x);
        }
        let mut frames = 0;
        while !comp.is_at_rest() {
            comp.process_frame(0.0, 0.0);
            frames += 1;
            assert!(frames < 30 * RATE as usize, "never came to rest");
        }
    }

    /// A voicing change keeps the reduction it has, so the level does not
    /// jump: the character changes, not the gain.
    #[test]
    fn a_voicing_change_mid_mix_keeps_the_reduction() {
        let mut comp = BusComp::new(section(BusCompVoicing::Grip, |_| {}), RATE);
        for n in 0..9_600 {
            let x = sine(LOUD_DB, n);
            comp.process_frame(x, x);
        }
        let before = comp.reduction_db();
        comp.apply_param(MASTER_COMP_VOICING, 2.0);
        assert_eq!(comp.reduction_db(), before);
    }

    #[test]
    fn every_position_resolves_inside_its_table_and_strangers_are_refused() {
        let mut params = section(BusCompVoicing::Grip, |p| p.grip_attack = 200);
        assert_eq!(resolve(&params).attack_ms, GRIP_ATTACKS[5].tau_ms);
        params.voicing = BusCompVoicing::Tube;
        params.tube_time = 99;
        assert_eq!(resolve(&params).slow_weight, TUBE_TIMES[5].slow_weight);
        let mut comp = BusComp::new(params, RATE);
        assert!(!comp.apply_param(mooloop_core::strip::STRIP_COMP_IN, 1.0));
    }

    #[test]
    fn a_bus_comp_is_small_and_copies_nothing_it_does_not_own() {
        assert!(
            core::mem::size_of::<BusComp>() <= 320,
            "{} bytes on every track's strip",
            core::mem::size_of::<BusComp>()
        );
    }
}
