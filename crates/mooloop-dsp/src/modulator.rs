//! Control-rate modulator sources.
//!
//! A modulator produces a signed `-1..1` value and no audio. Evaluation is
//! on a fixed subdivision of the block rather than once per block or per
//! sample: once per block stair-steps audibly on a fast LFO, and per sample
//! buys nothing at these rates (`docs/MODULATION.md`).

use mooloop_core::{
    ModEnvelopeParams, ModLfoParams, ModLfoWaveform, ModMathOp, ModMathParams, ModRandomParams,
    ModRandomTrigger, ModStepParams, ModStepTrigger, ModulatorParams, MAX_CHANNELS,
    MOD_STEP_MAX_STEPS,
};

/// Frames between modulation updates. The plan allows 32 or 64; 32 keeps a
/// 20 Hz LFO smooth at 48 kHz while costing one evaluation per 32 frames.
pub const CONTROL_RATE_FRAMES: usize = 32;

/// An LFO. Phase is kept in `0..1` so a waveform change mid-cycle keeps its
/// position rather than jumping.
///
/// **A tempo-synced LFO follows the song position** while the transport runs
/// (MOO-127): its phase is the position in beats over its division, plus its
/// phase offset, re-derived every control tick, so Play and Seek land it
/// where the position implies and an export, which starts fresh modulators
/// at the top, hears the same phase playback did. Stopped, it free-runs from
/// wherever it was. One that retriggers on notes follows the notes instead,
/// and an unsynced one always free-runs.
#[derive(Debug, Clone, Copy)]
struct Lfo {
    params: ModLfoParams,
    phase: f32,
    /// The song-position cycle `held` was drawn for, while following the
    /// song: the random waveform redraws when this changes, from the cycle
    /// number rather than the running generator, so its steps are a
    /// function of the position too.
    song_cycle: Option<i64>,
    fade_elapsed_seconds: f32,
    smoothed: f32,
    output_initialized: bool,
    /// Current value of the stepped random waveform, redrawn only when the
    /// phase wraps. Regenerating per evaluation would be white noise at
    /// control rate rather than sample-and-hold.
    held: f32,
    rng: u32,
}

/// The random waveform's value for song-position cycle `cycle`: a hash of the
/// cycle number (splitmix64), so an LFO following the song steps through the
/// same values wherever playback or an export starts.
fn cycle_random(cycle: i64) -> f32 {
    let mut z = (cycle as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    const SCALE: f32 = (1u32 << 24) as f32;
    ((z >> 40) as f32 / SCALE) * 2.0 - 1.0
}

impl Lfo {
    fn new(params: ModLfoParams) -> Self {
        let mut lfo = Self {
            params,
            phase: params.phase.fract(),
            song_cycle: None,
            fade_elapsed_seconds: 0.0,
            smoothed: 0.0,
            output_initialized: false,
            held: 0.0,
            // Any odd constant; the sequence only has to be uncorrelated
            // between slots, not cryptographic.
            rng: 0x2545_F491,
        };
        lfo.held = lfo.next_random();
        lfo
    }

    fn next_random(&mut self) -> f32 {
        // xorshift32: no allocation, no division, deterministic across runs
        // so an offline render matches a realtime one.
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        // Top 24 bits to `0..1`, then to the signed `-1..1` every source uses.
        const SCALE: f32 = (1u32 << 24) as f32;
        ((self.rng >> 8) as f32 / SCALE) * 2.0 - 1.0
    }

    /// Whether this LFO's phase is the song position's. See the type.
    fn follows_song(&self) -> bool {
        self.params.tempo_sync && !self.params.retrigger
    }

    /// Put the phase where `beats` into the song implies.
    fn follow_song(&mut self, beats: f64) {
        let division = f64::from(self.params.rate_division.beats()).max(f64::from(f32::EPSILON));
        let cycles = beats / division + f64::from(self.params.phase.fract());
        let cycle = cycles.floor();
        self.phase = ((cycles - cycle) as f32).clamp(0.0, 1.0 - f32::EPSILON);
        let cycle = cycle as i64;
        if self.song_cycle != Some(cycle) {
            self.song_cycle = Some(cycle);
            self.held = cycle_random(cycle);
        }
    }

    fn rate_hz(&self, bpm: f64) -> f32 {
        if self.params.tempo_sync {
            self.params.rate_division.rate_hz(bpm)
        } else {
            self.params.rate_hz
        }
    }

    fn fade_seconds(&self, bpm: f64) -> f32 {
        if self.params.fade_in_tempo_sync {
            self.params.fade_in_division.seconds(bpm)
        } else {
            self.params.fade_in_seconds
        }
    }

    fn value(&mut self, sample_rate: u32, frames: usize, bpm: f64) -> f32 {
        let phase = self.phase;
        let raw = match self.params.waveform {
            ModLfoWaveform::Sine => (phase * core::f32::consts::TAU).sin(),
            ModLfoWaveform::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            ModLfoWaveform::Saw => phase * 2.0 - 1.0,
            ModLfoWaveform::Square => {
                if phase < self.params.pulse_width.clamp(0.01, 0.99) {
                    1.0
                } else {
                    -1.0
                }
            }
            ModLfoWaveform::Random => self.held,
        };
        let fade_seconds = self.fade_seconds(bpm).max(0.0);
        let fade = if fade_seconds <= f32::EPSILON {
            1.0
        } else {
            (self.fade_elapsed_seconds / fade_seconds).clamp(0.0, 1.0)
        };
        let target = raw * self.params.depth.clamp(0.0, 1.0) * fade;
        let smoothing = self.params.smoothing_seconds.clamp(0.0, 2.0);
        if smoothing <= f32::EPSILON || !self.output_initialized {
            self.smoothed = target;
            self.output_initialized = true;
        } else {
            let elapsed = frames as f32 / sample_rate.max(1) as f32;
            let coefficient = 1.0 - (-elapsed / smoothing).exp();
            self.smoothed += (target - self.smoothed) * coefficient;
        }
        self.smoothed
    }

    fn advance(&mut self, sample_rate: u32, frames: usize, bpm: f64) {
        let elapsed = frames as f32 / sample_rate.max(1) as f32;
        self.fade_elapsed_seconds += elapsed;
        if self.song_cycle.take().is_some() {
            // Following the song: the next tick's position sets the phase
            // again, and the random step is the cycle's own. If the transport
            // has stopped, the LFO free-runs on from here, drawing afresh at
            // its next wrap.
            let increment = self.rate_hz(bpm).max(0.0) * elapsed;
            self.phase = (self.phase + increment).fract();
            return;
        }
        let increment = self.rate_hz(bpm).max(0.0) * elapsed;
        let advanced = self.phase + increment;
        self.phase = advanced.fract();
        if self.phase < 0.0 {
            self.phase += 1.0;
        }
        // One new random value per completed cycle, drawn at the wrap.
        if advanced >= 1.0 {
            self.held = self.next_random();
        }
    }

    fn retrigger(&mut self) {
        if self.params.retrigger {
            self.phase = self.params.phase.fract();
            self.fade_elapsed_seconds = 0.0;
            self.held = self.next_random();
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoteGateEvents {
    pub note_ons: u8,
    pub note_offs: u8,
    pub choke: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvelopeStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone, Copy)]
struct Envelope {
    params: ModEnvelopeParams,
    stage: EnvelopeStage,
    level: f32,
    stage_start: f32,
    elapsed: f32,
    held_notes: u16,
}

impl Envelope {
    fn new(params: ModEnvelopeParams) -> Self {
        Self {
            params,
            stage: EnvelopeStage::Idle,
            level: 0.0,
            stage_start: 0.0,
            elapsed: 0.0,
            held_notes: 0,
        }
    }

    fn seconds(free: f32, synced: bool, division: mooloop_core::ModTimeDivision, bpm: f64) -> f32 {
        if synced {
            division.seconds(bpm)
        } else {
            free.max(0.0)
        }
    }

    fn note_events(&mut self, events: NoteGateEvents) {
        if events.choke {
            self.held_notes = 0;
            self.begin(EnvelopeStage::Release);
        } else {
            self.held_notes = self.held_notes.saturating_sub(u16::from(events.note_offs));
        }
        if events.note_ons > 0 {
            self.held_notes = self.held_notes.saturating_add(u16::from(events.note_ons));
            // Every new gate edge retriggers from the current value. This is
            // click-safe for overlapping notes while still making drum gates
            // articulate repeated contours.
            self.begin(EnvelopeStage::Attack);
            if !self.params.attack_tempo_sync && self.params.attack_seconds <= f32::EPSILON {
                self.level = 1.0;
                self.begin(EnvelopeStage::Decay);
            }
        } else if self.held_notes == 0 && events.note_offs > 0 {
            self.begin(EnvelopeStage::Release);
            if !self.params.release_tempo_sync && self.params.release_seconds <= f32::EPSILON {
                self.level = 0.0;
                self.begin(EnvelopeStage::Idle);
            }
        }
    }

    fn begin(&mut self, stage: EnvelopeStage) {
        self.stage = stage;
        self.stage_start = self.level;
        self.elapsed = 0.0;
    }

    fn value(&self) -> f32 {
        // The rack's realtime convention remains signed. A normal unipolar
        // route lifts this back to 0..1, so idle contributes zero offset.
        self.level.clamp(0.0, 1.0) * self.params.amount.clamp(0.0, 1.0) * 2.0 - 1.0
    }

    fn advance(&mut self, sample_rate: u32, frames: usize, bpm: f64) {
        let delta = frames as f32 / sample_rate.max(1) as f32;
        self.elapsed += delta;
        match self.stage {
            EnvelopeStage::Idle => self.level = 0.0,
            EnvelopeStage::Attack => {
                let duration = Self::seconds(
                    self.params.attack_seconds,
                    self.params.attack_tempo_sync,
                    self.params.attack_division,
                    bpm,
                );
                if duration <= f32::EPSILON || self.elapsed >= duration {
                    self.level = 1.0;
                    self.begin(EnvelopeStage::Decay);
                } else {
                    self.level =
                        self.stage_start + (1.0 - self.stage_start) * self.elapsed / duration;
                }
            }
            EnvelopeStage::Decay => {
                let sustain = self.params.sustain.clamp(0.0, 1.0);
                let duration = Self::seconds(
                    self.params.decay_seconds,
                    self.params.decay_tempo_sync,
                    self.params.decay_division,
                    bpm,
                );
                if duration <= f32::EPSILON || self.elapsed >= duration {
                    self.level = sustain;
                    self.begin(EnvelopeStage::Sustain);
                } else {
                    self.level = 1.0 + (sustain - 1.0) * self.elapsed / duration;
                }
            }
            EnvelopeStage::Sustain => self.level = self.params.sustain.clamp(0.0, 1.0),
            EnvelopeStage::Release => {
                let duration = Self::seconds(
                    self.params.release_seconds,
                    self.params.release_tempo_sync,
                    self.params.release_division,
                    bpm,
                );
                if duration <= f32::EPSILON || self.elapsed >= duration {
                    self.level = 0.0;
                    self.begin(EnvelopeStage::Idle);
                } else {
                    self.level = self.stage_start * (1.0 - self.elapsed / duration);
                }
            }
        }
    }
}

/// A clocked pattern of control values. The step array is always sixteen
/// wide and `length` decides how much of it plays, so shortening a pattern
/// while it runs never loses the tail.
///
/// **A clocked pattern follows the song position** while the transport runs
/// (MOO-373), by the LFO's rule (MOO-127): the step is the position over the
/// division, wrapped to the length, so the first step lands on the downbeat
/// and an export hears the steps playback did. Stopped, it runs on its own
/// clock from wherever it was. One that advances on notes follows the notes.
#[derive(Debug, Clone, Copy)]
struct StepSequencer {
    params: ModStepParams,
    step: usize,
    /// Whether the last tick took its step from the song position. A tick
    /// that starts following lands where continuous playback would have,
    /// rather than gliding in from wherever the free clock had got to.
    following: bool,
    /// Seconds into the current step. Glide reads it even in note-advance
    /// mode, where nothing else does.
    elapsed: f32,
    /// Output at the moment the current step began, so a glide slides from
    /// wherever the last one actually got to.
    from: f32,
    output: f32,
}

impl StepSequencer {
    fn new(params: ModStepParams) -> Self {
        let mut sequencer = Self {
            params,
            step: 0,
            following: false,
            elapsed: 0.0,
            from: 0.0,
            output: 0.0,
        };
        sequencer.output = sequencer.target();
        sequencer.from = sequencer.output;
        sequencer
    }

    fn length(&self) -> usize {
        (self.params.length as usize).clamp(1, MOD_STEP_MAX_STEPS)
    }

    fn target(&self) -> f32 {
        self.params
            .steps
            .get(self.step)
            .copied()
            .unwrap_or(0.0)
            .clamp(-1.0, 1.0)
    }

    fn step_seconds(&self, bpm: f64) -> f32 {
        self.params.division.seconds(bpm).max(f32::EPSILON)
    }

    fn value(&mut self, bpm: f64) -> f32 {
        let target = self.target();
        let glide_seconds = self.params.glide.clamp(0.0, 1.0) * self.step_seconds(bpm);
        self.output = if glide_seconds <= f32::EPSILON {
            target
        } else {
            let travelled = (self.elapsed / glide_seconds).clamp(0.0, 1.0);
            self.from + (target - self.from) * travelled
        };
        self.output
    }

    /// Move to the next step, sliding from wherever the output currently is
    /// rather than from the step that just ended.
    fn advance_step(&mut self) {
        self.from = self.output;
        self.elapsed = 0.0;
        let length = self.length();
        self.step = if self.step + 1 >= length {
            0
        } else {
            self.step + 1
        };
    }

    fn advance(&mut self, sample_rate: u32, frames: usize, bpm: f64) {
        self.elapsed += frames as f32 / sample_rate.max(1) as f32;
        if self.params.trigger != ModStepTrigger::Clock {
            return;
        }
        let step_seconds = self.step_seconds(bpm);
        // A bounded catch-up: a long block or a very fast division must not
        // spin here, and a pattern that has lapped itself is in the same
        // place either way.
        let mut guard = 0;
        while self.elapsed >= step_seconds && guard < MOD_STEP_MAX_STEPS {
            self.elapsed -= step_seconds;
            self.advance_step();
            guard += 1;
        }
        if self.elapsed >= step_seconds {
            self.elapsed = 0.0;
        }
    }

    fn note_advance(&mut self) {
        if self.params.trigger == ModStepTrigger::NoteAdvance {
            self.advance_step();
        }
    }

    /// Whether this pattern's step is the song position's. See the type.
    fn follows_song(&self) -> bool {
        self.params.trigger == ModStepTrigger::Clock
    }

    /// Put the step, and how far into it, where `beats` into the song
    /// implies.
    fn follow_song(&mut self, beats: f64, bpm: f64) {
        let division = f64::from(self.params.division.beats()).max(f64::from(f32::EPSILON));
        let steps = beats / division;
        let count = steps.floor();
        let step = (count as i64).rem_euclid(self.length() as i64) as usize;
        self.elapsed = (steps - count) as f32 * self.step_seconds(bpm);
        if !self.following {
            // Where continuous playback would be: gliding in from the step
            // before, whose own glide, never longer than a step, has landed.
            // The downbeat has no step before it.
            self.step = if count >= 1.0 {
                (step + self.length() - 1) % self.length()
            } else {
                step
            };
            self.from = self.target();
            self.output = self.from;
            self.step = step;
        } else if step != self.step {
            self.from = self.output;
            self.step = step;
        }
        self.following = true;
    }
}

/// Sample-and-hold with room to be musical: a due draw can be skipped by
/// chance, snapped to a grid, or made to walk from the held value.
///
/// **A clocked, synced one follows the song position** while the transport
/// runs (MOO-373), by the LFO's rule (MOO-127): it draws as the position
/// crosses each division, from a generator seeded by its seed and the
/// division's number, so the draw on the downbeat is the same in playback
/// and in an export. Stopped, or unsynced, it runs on its own clock.
#[derive(Debug, Clone, Copy)]
struct RandomSource {
    params: ModRandomParams,
    seed: u32,
    /// The song-position division the held value was drawn for, while
    /// following the song.
    song_cycle: Option<i64>,
    /// The held value in the source's own range: `-1..1` when bipolar,
    /// `0..1` when not.
    held: f32,
    phase: f32,
    rng: u32,
}

impl RandomSource {
    /// Seeded from the module's seed ([`ModuleSpec::seed`]), so two random
    /// modules are uncorrelated rather than identical, and still
    /// deterministic: an offline render draws the same sequence a realtime
    /// one did. A module converted from a 0.1.6 rack is seeded with its old
    /// slot, which is what that rack seeded it with.
    fn new(params: ModRandomParams, seed: u32) -> Self {
        let mut source = Self {
            params,
            seed,
            song_cycle: None,
            held: 0.0,
            phase: 0.0,
            // Odd, so the xorshift state can never be zero and stick there.
            rng: 0x9E37_79B9 ^ (seed.wrapping_add(1).wrapping_mul(0x85EB_CA6B) | 1),
        };
        source.held = source.fresh();
        source
    }

    /// Uniform `0..1`. xorshift32, allocation-free and reproducible.
    fn next_unit(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        const SCALE: f32 = (1u32 << 24) as f32;
        (self.rng >> 8) as f32 / SCALE
    }

    /// Low end of the source's own range; the high end is always 1.
    fn floor(&self) -> f32 {
        if self.params.bipolar {
            -1.0
        } else {
            0.0
        }
    }

    fn quantize(&self, value: f32) -> f32 {
        let levels = self.params.quantize;
        if levels < 2 {
            return value;
        }
        let floor = self.floor();
        let span = 1.0 - floor;
        let last = f32::from(levels - 1);
        floor + span * ((value - floor) / span * last).round() / last
    }

    fn fresh(&mut self) -> f32 {
        let unit = self.next_unit();
        let floor = self.floor();
        self.quantize(floor + (1.0 - floor) * unit)
    }

    /// One drunk step: a bounded walk from the held value, reflected off the
    /// ends of the range so a long walk cannot park against a rail.
    fn walked(&mut self) -> f32 {
        let unit = self.next_unit();
        let floor = self.floor();
        let span = 1.0 - floor;
        let distance = (unit * 2.0 - 1.0) * self.params.walk.clamp(0.0, 1.0) * span * 0.5;
        let mut next = self.held + distance;
        if next > 1.0 {
            next = 2.0 - next;
        }
        if next < floor {
            next = 2.0 * floor - next;
        }
        self.quantize(next.clamp(floor, 1.0))
    }

    /// Redraw, unless chance says to keep what is held. Probability at zero
    /// freezes the source; at one it draws on every clock.
    fn draw(&mut self) {
        if self.next_unit() >= self.params.probability.clamp(0.0, 1.0) {
            return;
        }
        self.held = if self.params.drunk {
            self.walked()
        } else {
            self.fresh()
        };
    }

    fn rate_hz(&self, bpm: f64) -> f32 {
        if self.params.tempo_sync {
            self.params.rate_division.rate_hz(bpm)
        } else {
            self.params.rate_hz
        }
    }

    /// The wire value. Unipolar lifts to the signed convention exactly as
    /// the envelope does, so a unipolar route folds it back to `0..1`.
    fn value(&self) -> f32 {
        if self.params.bipolar {
            self.held
        } else {
            self.held * 2.0 - 1.0
        }
    }

    fn advance(&mut self, sample_rate: u32, frames: usize, bpm: f64) {
        if self.params.trigger != ModRandomTrigger::Clock {
            return;
        }
        let elapsed = frames as f32 / sample_rate.max(1) as f32;
        let advanced = self.phase + self.rate_hz(bpm).max(0.0) * elapsed;
        self.phase = advanced.fract();
        if advanced >= 1.0 {
            self.draw();
        }
    }

    fn note_trigger(&mut self) {
        if self.params.trigger == ModRandomTrigger::NoteTrigger {
            self.draw();
        }
    }

    /// Whether this source draws on the song position. See the type.
    fn follows_song(&self) -> bool {
        self.params.trigger == ModRandomTrigger::Clock && self.params.tempo_sync
    }

    /// Draw for the division `beats` into the song falls in, if it has not
    /// drawn for it already.
    fn follow_song(&mut self, beats: f64) {
        let division = f64::from(self.params.rate_division.beats()).max(f64::from(f32::EPSILON));
        let cycles = beats / division;
        let cycle = cycles.floor();
        self.phase = ((cycles - cycle) as f32).clamp(0.0, 1.0 - f32::EPSILON);
        let cycle = cycle as i64;
        if self.song_cycle != Some(cycle) {
            let entering = self.song_cycle.is_none();
            self.song_cycle = Some(cycle);
            self.rng = cycle_seed(self.seed, cycle);
            if entering {
                // Play starting here hears what an export starting here
                // does: a fresh draw, which neither chance nor a walk from
                // whatever the free clock left held can keep out. The
                // chance is still rolled, so a draw that would have landed
                // anyway is the same draw.
                self.next_unit();
                self.held = self.fresh();
            } else {
                self.draw();
            }
        }
    }
}

/// A random generator's state for division `cycle` of a module seeded with
/// `seed`: splitmix64 over both, odd so xorshift cannot stick at zero.
fn cycle_seed(seed: u32, cycle: i64) -> u32 {
    let mut z = (u64::from(seed) << 32 ^ cycle as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 32) as u32 | 1
}

/// The smallest divisor a math module will use. Division clamps its operand
/// away from zero rather than emitting an infinity a route would then
/// multiply into a destination.
const MATH_MIN_DIVISOR: f32 = 1.0e-3;

/// Arithmetic over another slot's output. Stateless: the whole module is its
/// params, and the list-order rule lives in `ModulatorSet::tick`.
#[derive(Debug, Clone, Copy)]
struct MathSource {
    params: ModMathParams,
}

impl MathSource {
    fn value(&self, input: f32) -> f32 {
        let operand = self.params.operand;
        let raw = match self.params.op {
            ModMathOp::Add => input + operand,
            ModMathOp::Subtract => input - operand,
            ModMathOp::Multiply => input * operand,
            ModMathOp::Divide => {
                let divisor = if operand.abs() < MATH_MIN_DIVISOR {
                    MATH_MIN_DIVISOR.copysign(operand)
                } else {
                    operand
                };
                input / divisor
            }
            ModMathOp::Min => input.min(operand),
            ModMathOp::Max => input.max(operand),
            ModMathOp::Clamp => {
                // Dragging the low bound past the high one must reorder,
                // not panic: `f32::clamp` refuses an inverted range.
                let low = self.params.clamp_low.min(self.params.clamp_high);
                let high = self.params.clamp_low.max(self.params.clamp_high);
                input.clamp(low, high)
            }
        };
        // Everything clamps at the module edge, so a route never sees a
        // value outside the rack's convention.
        if raw.is_finite() {
            raw.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Source {
    Lfo(Lfo),
    Envelope(Envelope),
    Step(StepSequencer),
    Random(RandomSource),
    Math(MathSource),
}

/// What one module of a [`ModulatorSet`] is built from: the song's module,
/// resolved to positions off the audio thread
/// (`mooloop_core::CompiledModule`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModuleSpec {
    pub params: ModulatorParams,
    /// What a Random module's generator starts from.
    pub seed: u32,
    /// The seat of the channel whose notes it hears: the Envelope's gate, the
    /// LFO's retrigger, the Step's advance, the Random's trigger.
    pub gate: Option<u8>,
    /// The list position a Math module reads.
    pub reads: Option<u16>,
}

impl ModuleSpec {
    fn build(&self) -> Source {
        match self.params {
            ModulatorParams::Lfo(params) => Source::Lfo(Lfo::new(params)),
            ModulatorParams::Envelope(params) => Source::Envelope(Envelope::new(params)),
            ModulatorParams::Step(params) => Source::Step(StepSequencer::new(params)),
            ModulatorParams::Random(params) => Source::Random(RandomSource::new(params, self.seed)),
            ModulatorParams::Math(params) => Source::Math(MathSource { params }),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Module {
    spec: ModuleSpec,
    source: Source,
}

/// The song's modulators and their current outputs, in list order
/// (`docs/plans/archive/song-modulation/02-the-engine-runs-one-set.md`).
///
/// Built off the audio thread at the size of the song's set and never resized
/// on it: a module added or removed arrives as a new set, which takes each
/// surviving module's running state from the one it replaces
/// ([`Self::carry`]). What a retune can change happens in place
/// ([`Self::retune`]), and nothing here allocates once built: a module is
/// `Copy`.
#[derive(Debug, Clone, Default)]
pub struct ModulatorSet {
    modules: Vec<Module>,
    outputs: Vec<f32>,
}

impl ModulatorSet {
    /// Every module fresh, outputs at zero. Allocates.
    pub fn new(specs: impl IntoIterator<Item = ModuleSpec>) -> Self {
        let modules: Vec<Module> = specs
            .into_iter()
            .map(|spec| Module {
                spec,
                source: spec.build(),
            })
            .collect();
        let outputs = vec![0.0; modules.len()];
        Self { modules, outputs }
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Give the module at `at` new params. One that keeps its kind keeps its
    /// running state, so retuning an LFO's rate does not restart it
    /// mid-performance; a kind change rebuilds it.
    pub fn retune(&mut self, at: usize, params: ModulatorParams) {
        let Some(module) = self.modules.get_mut(at) else {
            return;
        };
        module.spec.params = params;
        match (params, &mut module.source) {
            (ModulatorParams::Lfo(next), Source::Lfo(lfo)) => {
                let fade_changed = lfo.params.fade_in_seconds != next.fade_in_seconds
                    || lfo.params.fade_in_tempo_sync != next.fade_in_tempo_sync
                    || lfo.params.fade_in_division != next.fade_in_division;
                lfo.params = next;
                if fade_changed {
                    lfo.fade_elapsed_seconds = 0.0;
                }
            }
            (ModulatorParams::Envelope(next), Source::Envelope(envelope)) => {
                envelope.params = next;
            }
            // Retuning a running pattern keeps its position; a shortened
            // length folds the cursor back inside rather than stalling it
            // on a step that no longer plays.
            (ModulatorParams::Step(next), Source::Step(sequencer)) => {
                sequencer.params = next;
                let length = sequencer.length();
                if sequencer.step >= length {
                    sequencer.step %= length;
                }
            }
            (ModulatorParams::Random(next), Source::Random(random)) => {
                // `held` is kept in the source's *own* range, so the Bipolar
                // switch changes what the stored number means. Without the
                // re-fold, a held -0.8 read back through the unipolar arm of
                // `value` is -2.6 -- outside the -1..1 the set is entitled
                // to assume, and nothing downstream clamps a source value.
                // It would persist until the next draw, which never comes if
                // the trigger is Note and no notes arrive, or if Chance is 0.
                let refolded = random.params.bipolar != next.bipolar;
                random.params = next;
                if refolded {
                    let floor = random.floor();
                    random.held = random.held.clamp(floor, 1.0);
                }
            }
            (ModulatorParams::Math(next), Source::Math(math)) => math.params = next,
            _ => module.source = module.spec.build(),
        }
    }

    /// Take the running state of `from`'s module at `from_at` into this set's
    /// module at `at`, as the same module carried across a new set: its
    /// phase, cursor, envelope stage and held draw, and its last output,
    /// which a Math module listed before it reads this tick. Then this set's
    /// params are retuned onto it by [`Self::retune`]'s rules. A module whose
    /// kind changed is not carried, and an envelope whose input changed
    /// releases, as one does when its gate is repointed.
    pub fn carry(&mut self, at: usize, from: &ModulatorSet, from_at: usize, input_changed: bool) {
        let (Some(module), Some(previous)) = (self.modules.get(at), from.modules.get(from_at))
        else {
            return;
        };
        if module.spec.params.kind() != previous.spec.params.kind() {
            return;
        }
        let params = module.spec.params;
        self.modules[at].source = previous.source;
        self.outputs[at] = from.outputs[from_at];
        if input_changed {
            if let Source::Envelope(envelope) = &mut self.modules[at].source {
                envelope.held_notes = 0;
                envelope.begin(EnvelopeStage::Release);
            }
        }
        self.retune(at, params);
    }

    /// Current `-1..1` output of every module, in list order.
    pub fn outputs(&self) -> &[f32] {
        &self.outputs
    }

    /// Deliver this control tick's note gates to every module that hears a
    /// channel, then evaluate every module for the coming `frames` and
    /// advance it. `song_beats` is how far into the song this tick starts,
    /// in quarter-note beats, `None` while the transport is stopped: a
    /// tempo-synced LFO takes its phase from it (MOO-127), and a clocked
    /// Step and a clocked, synced Random their steps and draws (MOO-373).
    ///
    /// Modules evaluate in list order within a control tick, so a Math
    /// module reading one listed before it sees this tick's value and one
    /// reading itself or a module listed after it sees the previous tick's.
    /// That single rule is what makes a chain of modules deterministic,
    /// identical realtime and offline, and bounded without any cycle
    /// machinery: `outputs` simply still holds last tick's value everywhere
    /// this pass has not reached yet. It is the rule a channel's rack had
    /// over its slots, and a converted song lists each rack's modules in
    /// slot order.
    pub fn tick(
        &mut self,
        sample_rate: u32,
        frames: usize,
        bpm: f64,
        song_beats: Option<f64>,
        gates: &[NoteGateEvents; MAX_CHANNELS],
    ) {
        for module in &mut self.modules {
            let Some(events) = module.spec.gate.and_then(|seat| gates.get(usize::from(seat))) else {
                continue;
            };
            let notes = events.note_ons > 0;
            match &mut module.source {
                Source::Lfo(lfo) => {
                    if notes {
                        lfo.retrigger();
                    }
                }
                Source::Envelope(envelope) => envelope.note_events(*events),
                Source::Step(sequencer) => {
                    if notes {
                        sequencer.note_advance();
                    }
                }
                Source::Random(random) => {
                    if notes {
                        random.note_trigger();
                    }
                }
                Source::Math(_) => {}
            }
        }
        for at in 0..self.modules.len() {
            let module = &mut self.modules[at];
            self.outputs[at] = match &mut module.source {
                Source::Lfo(lfo) => {
                    if let Some(beats) = song_beats.filter(|_| lfo.follows_song()) {
                        lfo.follow_song(beats);
                    }
                    let value = lfo.value(sample_rate, frames, bpm);
                    lfo.advance(sample_rate, frames, bpm);
                    value
                }
                Source::Envelope(envelope) => {
                    let value = envelope.value();
                    envelope.advance(sample_rate, frames, bpm);
                    value
                }
                Source::Step(sequencer) => {
                    match song_beats.filter(|_| sequencer.follows_song()) {
                        Some(beats) => sequencer.follow_song(beats, bpm),
                        None => sequencer.following = false,
                    }
                    let value = sequencer.value(bpm);
                    if !sequencer.following {
                        sequencer.advance(sample_rate, frames, bpm);
                    }
                    value
                }
                Source::Random(random) => {
                    match song_beats.filter(|_| random.follows_song()) {
                        Some(beats) => random.follow_song(beats),
                        None => random.song_cycle = None,
                    }
                    let value = random.value();
                    if random.song_cycle.is_none() {
                        random.advance(sample_rate, frames, bpm);
                    }
                    value
                }
                Source::Math(math) => {
                    let input = module
                        .spec
                        .reads
                        .and_then(|read| self.outputs.get(usize::from(read)))
                        .copied()
                        .unwrap_or(0.0);
                    math.value(input)
                }
            };
        }
    }

    /// Move every module that follows notes: an LFO restarts its phase, a
    /// step pattern takes one step, a note-triggered random draws.
    pub fn retrigger(&mut self) {
        for module in &mut self.modules {
            match &mut module.source {
                Source::Lfo(lfo) => lfo.retrigger(),
                Source::Step(sequencer) => sequencer.note_advance(),
                Source::Random(random) => random.note_trigger(),
                Source::Envelope(_) | Source::Math(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::ModulatorParams;

    const NO_GATES: [NoteGateEvents; MAX_CHANNELS] = [NoteGateEvents {
        note_ons: 0,
        note_offs: 0,
        choke: false,
    }; MAX_CHANNELS];

    /// A module as a converted rack's slot `at` would be: seeded with its
    /// position, hearing channel 0, and a Math module reading the position
    /// its `input_slot` names.
    fn spec(at: usize, params: ModulatorParams) -> ModuleSpec {
        ModuleSpec {
            params,
            seed: at as u32,
            gate: Some(0),
            reads: match params {
                ModulatorParams::Math(math) => Some(u16::from(math.input_slot)),
                _ => None,
            },
        }
    }

    fn set_of(modules: &[ModulatorParams]) -> ModulatorSet {
        ModulatorSet::new(modules.iter().enumerate().map(|(at, params)| spec(at, *params)))
    }

    fn lfo(params: ModLfoParams) -> ModulatorSet {
        set_of(&[ModulatorParams::Lfo(params)])
    }

    impl ModulatorSet {
        fn run(&mut self, sample_rate: u32, frames: usize, bpm: f64) {
            self.tick(sample_rate, frames, bpm, None, &NO_GATES);
        }

        fn run_at(&mut self, sample_rate: u32, frames: usize, bpm: f64, beats: Option<f64>) {
            self.tick(sample_rate, frames, bpm, beats, &NO_GATES);
        }
    }

    /// A Random module keeps its held value in the range its own Bipolar
    /// flag declares, so turning the flag off has to re-fold it. Without
    /// that, a held -0.8 read back through the unipolar arm of `value` is
    /// -2.6 -- outside the -1..1 the set is entitled to assume, and nothing
    /// downstream clamps a source value: a route multiplies it by depth and
    /// only the summed result meets a clamp. A depth-1.0 route would pin its
    /// destination to the bottom of its range, and stay there until the next
    /// draw, which never comes with Chance at 0.
    #[test]
    fn a_random_module_refolds_its_held_value_when_bipolar_changes() {
        use mooloop_core::{ModRandomParams, ModRandomTrigger};

        // Chance 0 freezes the held value, so the only thing that can move
        // the reading is the re-fold under test.
        let frozen = |bipolar: bool| ModRandomParams {
            bipolar,
            probability: 0.0,
            trigger: ModRandomTrigger::Clock,
            ..ModRandomParams::default()
        };

        let mut set = set_of(&[ModulatorParams::Random(frozen(true))]);
        set.run(48_000, 64, 120.0);
        let bipolar_value = set.outputs()[0];
        // Stated rather than assumed: the re-fold only moves a *negative*
        // held value, so a seed that draws positive would make everything
        // below pass whether the fix is present or not. The seed is
        // deterministic, so this cannot fail today -- it is here to fail
        // loudly on the day someone changes it, instead of going quiet.
        assert!(
            bipolar_value < 0.0,
            "this test needs a negative draw to mean anything; the seed now \
             gives {bipolar_value}, so re-pick the seed or the params"
        );

        set.retune(0, ModulatorParams::Random(frozen(false)));
        set.run(48_000, 64, 120.0);
        let after = set.outputs()[0];
        assert!(
            (-1.0..=1.0).contains(&after),
            "the held value must be re-folded into the new range, got {after}"
        );
    }

    #[test]
    fn a_sine_lfo_completes_one_cycle_per_period() {
        let mut set = lfo(ModLfoParams {
            rate_hz: 1.0,
            ..ModLfoParams::default()
        });
        // `tick` reports the value for the frames it is about to cover, then
        // advances, so each reading is the phase *before* that step.
        set.run(48_000, 12_000, 120.0);
        assert!(set.outputs()[0].abs() < 1e-6, "starts at zero");
        // A quarter second at 1 Hz is a quarter cycle: the sine peak.
        set.run(48_000, 12_000, 120.0);
        assert!((set.outputs()[0] - 1.0).abs() < 1e-3, "{}", set.outputs()[0]);
        set.run(48_000, 12_000, 120.0);
        assert!(set.outputs()[0].abs() < 1e-3, "back through zero");
    }

    #[test]
    fn depth_scales_the_output() {
        let mut set = lfo(ModLfoParams {
            rate_hz: 1.0,
            depth: 0.5,
            waveform: ModLfoWaveform::Square,
            ..ModLfoParams::default()
        });
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], 0.5);
        assert!(ModulatorSet::new([]).is_empty());
    }

    /// Retuning a running LFO must not restart it: an automated rate change
    /// mid-performance should bend the motion, not reset the phase.
    #[test]
    fn retuning_a_module_keeps_its_phase() {
        let mut set = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Saw,
            ..ModLfoParams::default()
        });
        set.run(48_000, 12_000, 120.0);
        let advanced = set.outputs()[0];
        set.retune(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 4.0,
                waveform: ModLfoWaveform::Saw,
                ..ModLfoParams::default()
            }),
        );
        set.run(48_000, 0, 120.0);
        assert!(set.outputs()[0] > advanced, "phase restarted on retune");
    }

    /// Sample-and-hold must hold. Regenerating every evaluation would be
    /// white noise at control rate rather than a stepped modulator.
    #[test]
    fn random_holds_its_value_across_a_cycle() {
        let mut set = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Random,
            ..ModLfoParams::default()
        });
        set.run(48_000, 1_000, 120.0);
        let held = set.outputs()[0];
        assert!((-1.0..=1.0).contains(&held), "out of range: {held}");
        // Ten more evaluations well inside the same cycle.
        for _ in 0..10 {
            set.run(48_000, 1_000, 120.0);
            assert_eq!(set.outputs()[0], held, "value changed mid-cycle");
        }
        // Crossing the wrap draws a new one.
        set.run(48_000, 48_000, 120.0);
        set.run(48_000, 0, 120.0);
        assert_ne!(set.outputs()[0], held);
    }

    #[test]
    fn retrigger_only_resets_modules_that_asked_for_it() {
        let mut free = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Saw,
            retrigger: false,
            ..ModLfoParams::default()
        });
        free.run(48_000, 12_000, 120.0);
        assert_eq!(free.outputs()[0], -1.0, "a saw starts at its floor");
        free.retrigger();
        free.run(48_000, 0, 120.0);
        assert_eq!(free.outputs()[0], -0.5, "a free-running LFO must ignore retrigger");

        let mut played = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Saw,
            retrigger: true,
            ..ModLfoParams::default()
        });
        played.run(48_000, 12_000, 120.0);
        played.retrigger();
        played.run(48_000, 0, 120.0);
        assert_eq!(played.outputs()[0], -1.0, "saw must restart at its floor");
    }

    #[test]
    fn tempo_synced_rate_follows_the_current_bpm() {
        let mut set = lfo(ModLfoParams {
            tempo_sync: true,
            rate_division: mooloop_core::ModTimeDivision::Quarter,
            ..ModLfoParams::default()
        });
        // At 120 BPM a quarter-note cycle is 0.5 seconds. One eighth of a
        // second advances to the sine peak.
        set.run(48_000, 6_000, 120.0);
        set.run(48_000, 0, 120.0);
        assert!((set.outputs()[0] - 1.0).abs() < 1e-3);
    }

    fn synced(waveform: ModLfoWaveform) -> ModLfoParams {
        ModLfoParams {
            tempo_sync: true,
            rate_division: mooloop_core::ModTimeDivision::Whole,
            waveform,
            ..ModLfoParams::default()
        }
    }

    /// **A synced LFO's phase is the song position's** (MOO-127).
    #[test]
    fn a_synced_lfo_lands_on_the_phase_the_song_position_implies() {
        for waveform in [ModLfoWaveform::Sine, ModLfoWaveform::Saw, ModLfoWaveform::Random] {
            let mut set = lfo(synced(waveform));
            set.run_at(48_000, 32, 120.0, Some(0.0));
            let downbeat = set.outputs()[0];
            // Stopped for a while: it free-runs somewhere else.
            for _ in 0..777 {
                set.run_at(48_000, 32, 120.0, None);
            }
            // Play from the top again: the same value on the downbeat.
            set.run_at(48_000, 32, 120.0, Some(0.0));
            assert_eq!(set.outputs()[0], downbeat, "{waveform:?}");

            // A seek to bar 3 reads what continuous playback from bar 1 read
            // there, and so does a fresh set, which is what an export builds.
            let mut continuous = lfo(synced(waveform));
            let mut beats = 0.0;
            while beats < 8.0 {
                continuous.run_at(48_000, 32, 120.0, Some(beats));
                beats += 32.0 / 24_000.0;
            }
            continuous.run_at(48_000, 32, 120.0, Some(8.0));
            set.run_at(48_000, 32, 120.0, Some(8.0));
            let mut fresh = lfo(synced(waveform));
            fresh.run_at(48_000, 32, 120.0, Some(8.0));
            assert_eq!(set.outputs()[0], continuous.outputs()[0], "{waveform:?}");
            assert_eq!(fresh.outputs()[0], continuous.outputs()[0], "{waveform:?}");
        }
    }

    /// **A clocked Step and a clocked, synced Random follow the song
    /// position** (MOO-373), by the synced LFO's rule: the downbeat reads the
    /// first step and the same draw however long the module free-ran before
    /// Play, and a seek, or a fresh set as an export builds, reads what
    /// continuous playback read there.
    #[test]
    fn a_clocked_step_and_a_synced_random_land_where_the_song_position_implies() {
        let step = ModulatorParams::Step(ModStepParams {
            trigger: ModStepTrigger::Clock,
            glide: 0.4,
            length: 5,
            steps: core::array::from_fn(|step| step as f32 / 8.0 - 0.5),
            ..ModStepParams::default()
        });
        let random = |drunk| {
            ModulatorParams::Random(ModRandomParams {
                trigger: ModRandomTrigger::Clock,
                tempo_sync: true,
                drunk,
                ..ModRandomParams::default()
            })
        };
        for params in [step, random(false), random(true)] {
            let mut set = set_of(&[params]);
            set.run_at(48_000, 32, 120.0, Some(0.0));
            let downbeat = set.outputs()[0];
            if let ModulatorParams::Step(step) = params {
                assert_eq!(downbeat, step.steps[0], "the first step on the downbeat");
            }
            for _ in 0..7_777 {
                set.run_at(48_000, 32, 120.0, None);
            }
            set.run_at(48_000, 32, 120.0, Some(0.0));
            assert_eq!(set.outputs()[0], downbeat, "{params:?}");

            let mut continuous = set_of(&[params]);
            let mut played = Vec::new();
            let mut beats = 0.0;
            while beats < 8.0 {
                continuous.run_at(48_000, 32, 120.0, Some(beats));
                played.push(continuous.outputs()[0]);
                beats += 32.0 / 24_000.0;
            }
            continuous.run_at(48_000, 32, 120.0, Some(8.0));
            let mut fresh = set_of(&[params]);
            fresh.run_at(48_000, 32, 120.0, Some(8.0));
            if !matches!(params, ModulatorParams::Random(ModRandomParams { drunk: true, .. })) {
                // A drunk walk starts from what it held, so only playback
                // from the same place can promise the same walk.
                assert_eq!(fresh.outputs()[0], continuous.outputs()[0], "{params:?}");
            }

            // Playback from the top again, after a stop, reads every value
            // the first pass did: a bounce matches playback.
            for _ in 0..333 {
                continuous.run_at(48_000, 32, 120.0, None);
            }
            let mut beats = 0.0;
            for (tick, value) in played.iter().enumerate() {
                continuous.run_at(48_000, 32, 120.0, Some(beats));
                assert_eq!(continuous.outputs()[0], *value, "{params:?} at tick {tick}");
                beats += 32.0 / 24_000.0;
            }
        }
    }

    /// A quarter of the way through a whole-note cycle is the sine's peak,
    /// wherever the free run had got to.
    #[test]
    fn a_synced_sine_follows_the_beat_it_is_told() {
        let mut set = lfo(synced(ModLfoWaveform::Sine));
        set.run_at(48_000, 5_000, 90.0, None);
        set.run_at(48_000, 32, 90.0, Some(1.0));
        assert!((set.outputs()[0] - 1.0).abs() < 1e-5, "{}", set.outputs()[0]);
    }

    /// Unsynced, or retriggered by notes, it runs as it always did.
    #[test]
    fn an_unsynced_or_retriggered_lfo_ignores_the_song_position() {
        for params in [
            ModLfoParams::default(),
            ModLfoParams {
                retrigger: true,
                ..synced(ModLfoWaveform::Sine)
            },
        ] {
            let mut told = lfo(params);
            let mut free = lfo(params);
            for step in 0..100 {
                told.run_at(48_000, 32, 120.0, Some(f64::from(step) * 3.7));
                free.run(48_000, 32, 120.0);
                assert_eq!(told.outputs()[0], free.outputs()[0]);
            }
        }
    }

    #[test]
    fn fade_in_scales_output_and_restarts_with_a_note_trigger() {
        let mut set = lfo(ModLfoParams {
            waveform: ModLfoWaveform::Square,
            retrigger: true,
            fade_in_seconds: 1.0,
            ..ModLfoParams::default()
        });
        set.run(48_000, 24_000, 120.0);
        assert_eq!(set.outputs()[0], 0.0);
        set.run(48_000, 0, 120.0);
        assert!((set.outputs()[0].abs() - 0.5).abs() < 1e-6);
        set.retrigger();
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], 0.0);

        set.run(48_000, 48_000, 120.0);
        set.retune(
            0,
            ModulatorParams::Lfo(ModLfoParams {
                waveform: ModLfoWaveform::Square,
                retrigger: true,
                fade_in_seconds: 2.0,
                ..ModLfoParams::default()
            }),
        );
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], 0.0, "editing fade must audition a new ramp");
    }

    #[test]
    fn pulse_width_moves_the_square_transition() {
        let mut set = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Square,
            pulse_width: 0.2,
            ..ModLfoParams::default()
        });
        set.run(48_000, 12_000, 120.0);
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], -1.0, "25% phase is past a 20% pulse");
    }

    #[test]
    fn smoothing_slews_instead_of_stepping_between_levels() {
        let mut set = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Square,
            pulse_width: 0.2,
            smoothing_seconds: 0.5,
            ..ModLfoParams::default()
        });
        set.run(48_000, 12_000, 120.0);
        assert_eq!(set.outputs()[0], 1.0);
        set.run(48_000, 4_800, 120.0);
        assert!(
            (-1.0..1.0).contains(&set.outputs()[0]),
            "smoothed transition jumped to {}",
            set.outputs()[0]
        );
    }

    fn envelope_on(seat: u8, params: ModEnvelopeParams) -> ModulatorSet {
        ModulatorSet::new([ModuleSpec {
            gate: Some(seat),
            ..spec(0, ModulatorParams::Envelope(params))
        }])
    }

    #[test]
    fn envelope_follows_its_input_channels_gate_through_release() {
        let mut set = envelope_on(
            2,
            ModEnvelopeParams {
                attack_seconds: 0.1,
                decay_seconds: 0.0,
                sustain: 1.0,
                release_seconds: 0.1,
                ..ModEnvelopeParams::default()
            },
        );
        let mut gates = NO_GATES;
        gates[2].note_ons = 1;
        set.tick(48_000, 4_800, 120.0, None, &gates);
        assert_eq!(set.outputs()[0], -1.0, "attack starts at the floor");
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], 1.0, "attack reaches the ceiling");

        gates = NO_GATES;
        gates[2].note_offs = 1;
        set.tick(48_000, 4_800, 120.0, None, &gates);
        assert_eq!(set.outputs()[0], 1.0, "release begins from the held level");
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], -1.0, "release returns to the floor");
    }

    #[test]
    fn envelope_ignores_other_channels_notes() {
        let mut set = envelope_on(
            3,
            ModEnvelopeParams {
                attack_seconds: 0.0,
                ..ModEnvelopeParams::default()
            },
        );
        let mut gates = NO_GATES;
        gates[1].note_ons = 1;
        set.tick(48_000, 32, 120.0, None, &gates);
        set.run(48_000, 0, 120.0);
        assert_eq!(set.outputs()[0], -1.0);
    }

    /// A pattern whose every step is the same value, for wiring a
    /// deterministic constant into a math module's input.
    fn constant_step(value: f32) -> ModulatorParams {
        ModulatorParams::Step(ModStepParams {
            steps: [value; MOD_STEP_MAX_STEPS],
            length: 1,
            ..ModStepParams::default()
        })
    }

    /// One sixteenth at 120 BPM, in frames at 48 kHz.
    const STEP_FRAMES: usize = 6_000;

    #[test]
    fn a_step_pattern_walks_its_length_and_wraps() {
        let mut steps = [0.0; MOD_STEP_MAX_STEPS];
        steps[0] = 1.0;
        steps[1] = -1.0;
        steps[2] = 0.5;
        // The tail is inside the array but outside `length`, so it must not
        // play: shortening a pattern hides steps rather than deleting them.
        steps[3] = 0.25;
        let mut set = set_of(&[ModulatorParams::Step(ModStepParams {
            steps,
            length: 3,
            division: mooloop_core::ModTimeDivision::Sixteenth,
            ..ModStepParams::default()
        })]);
        for expected in [1.0, -1.0, 0.5, 1.0, -1.0] {
            set.run(48_000, STEP_FRAMES, 120.0);
            assert_eq!(set.outputs()[0], expected);
        }
    }

    /// Glide spends its fraction of the step sliding from wherever the last
    /// step actually left the output, and zero glide is the hard staircase a
    /// stepped source is expected to make.
    #[test]
    fn glide_slides_across_its_share_of_the_step() {
        let mut steps = [0.0; MOD_STEP_MAX_STEPS];
        steps[0] = 1.0;
        steps[1] = -1.0;
        let params = ModStepParams {
            steps,
            length: 2,
            division: mooloop_core::ModTimeDivision::Sixteenth,
            glide: 1.0,
            ..ModStepParams::default()
        };
        let mut set = set_of(&[ModulatorParams::Step(params)]);
        set.run(48_000, STEP_FRAMES, 120.0);
        assert_eq!(set.outputs()[0], 1.0, "the first step starts at its value");
        set.run(48_000, STEP_FRAMES / 2, 120.0);
        assert_eq!(set.outputs()[0], 1.0, "a full glide leaves from the old value");
        set.run(48_000, 0, 120.0);
        assert!(
            set.outputs()[0].abs() < 1e-6,
            "half a glide should be halfway: {}",
            set.outputs()[0]
        );

        let mut hard = set_of(&[ModulatorParams::Step(ModStepParams {
            glide: 0.0,
            ..params
        })]);
        hard.run(48_000, STEP_FRAMES, 120.0);
        hard.run(48_000, STEP_FRAMES / 2, 120.0);
        assert_eq!(hard.outputs()[0], -1.0, "no glide must step, not slide");
    }

    #[test]
    fn a_note_advance_pattern_ignores_the_clock_and_moves_on_its_inputs_notes() {
        let mut steps = [0.0; MOD_STEP_MAX_STEPS];
        steps[0] = 1.0;
        steps[1] = -1.0;
        let mut set = ModulatorSet::new([ModuleSpec {
            gate: Some(1),
            ..spec(
                0,
                ModulatorParams::Step(ModStepParams {
                    steps,
                    length: 2,
                    trigger: ModStepTrigger::NoteAdvance,
                    ..ModStepParams::default()
                }),
            )
        }]);
        for _ in 0..8 {
            set.run(48_000, STEP_FRAMES, 120.0);
            assert_eq!(set.outputs()[0], 1.0, "the clock must not advance it");
        }
        let mut gates = NO_GATES;
        gates[0].note_ons = 1;
        set.tick(48_000, 0, 120.0, None, &gates);
        assert_eq!(set.outputs()[0], 1.0, "another channel's notes must not");
        gates = NO_GATES;
        gates[1].note_ons = 1;
        set.tick(48_000, 0, 120.0, None, &gates);
        assert_eq!(set.outputs()[0], -1.0);
    }

    /// Probability is the whole musical point of the random module: at zero
    /// it freezes what it holds, at one it draws on every clock.
    #[test]
    fn probability_gates_whether_a_due_draw_lands() {
        let mut frozen = set_of(&[ModulatorParams::Random(ModRandomParams {
            probability: 0.0,
            ..ModRandomParams::default()
        })]);
        frozen.run(48_000, 0, 120.0);
        let held = frozen.outputs()[0];
        for _ in 0..32 {
            frozen.run(48_000, 48_000, 120.0);
            assert_eq!(frozen.outputs()[0], held, "chance zero must freeze");
        }

        let mut always = set_of(&[ModulatorParams::Random(ModRandomParams::default())]);
        let mut seen = Vec::new();
        for _ in 0..32 {
            always.run(48_000, 48_000, 120.0);
            let value = always.outputs()[0];
            assert!((-1.0..=1.0).contains(&value), "out of range: {value}");
            if !seen.contains(&value) {
                seen.push(value);
            }
        }
        assert!(seen.len() > 4, "chance one should keep drawing: {seen:?}");
    }

    /// A drunk walk stays inside the range and never jumps further than its
    /// declared step, which is the only thing that distinguishes it from
    /// plain sample-and-hold.
    #[test]
    fn a_drunk_walk_stays_bounded_and_takes_small_steps() {
        let walk = 0.1;
        let mut set = set_of(&[ModulatorParams::Random(ModRandomParams {
            drunk: true,
            walk,
            ..ModRandomParams::default()
        })]);
        set.run(48_000, 0, 120.0);
        let mut previous = set.outputs()[0];
        for _ in 0..256 {
            set.run(48_000, 48_000, 120.0);
            let value = set.outputs()[0];
            assert!((-1.0..=1.0).contains(&value), "escaped the range: {value}");
            assert!(
                (value - previous).abs() <= walk + 1e-5,
                "jumped {} from {previous} to {value}",
                (value - previous).abs()
            );
            previous = value;
        }
    }

    /// Two random modules must not be the same random module. Seeding each
    /// from its own seed decorrelates them without giving up the determinism
    /// an offline render depends on.
    #[test]
    fn random_modules_draw_independent_sequences() {
        let random = ModulatorParams::Random(ModRandomParams::default());
        let mut set = set_of(&[random, random]);
        let mut differed = false;
        for _ in 0..16 {
            set.run(48_000, 48_000, 120.0);
            differed |= set.outputs()[0] != set.outputs()[1];
        }
        assert!(differed, "both modules drew the same sequence");

        // Same seed, same ticks, same values: the sequence is reproducible.
        let mut replay = set_of(&[random]);
        let mut first = Vec::new();
        for _ in 0..16 {
            replay.run(48_000, 48_000, 120.0);
            first.push(replay.outputs()[0]);
        }
        let mut again = set_of(&[random]);
        for expected in first {
            again.run(48_000, 48_000, 120.0);
            assert_eq!(again.outputs()[0], expected);
        }
    }

    #[test]
    fn quantized_draws_land_on_the_grid_and_unipolar_lifts_to_the_wire() {
        let mut set = set_of(&[ModulatorParams::Random(ModRandomParams {
            bipolar: false,
            quantize: 3,
            ..ModRandomParams::default()
        })]);
        // Three levels across 0..1 are 0, 0.5 and 1, carried on the signed
        // wire as -1, 0 and 1 exactly as the envelope carries its unipolar
        // contour.
        for _ in 0..32 {
            set.run(48_000, 48_000, 120.0);
            let value = set.outputs()[0];
            assert!(
                [-1.0, 0.0, 1.0].iter().any(|level| (value - level).abs() < 1e-5),
                "off the grid: {value}"
            );
        }
    }

    fn math(input_slot: u8, op: ModMathOp, operand: f32) -> ModulatorParams {
        ModulatorParams::Math(ModMathParams {
            input_slot,
            op,
            operand,
            ..ModMathParams::default()
        })
    }

    /// The list-order rule, stated in both directions: a module reading one
    /// listed before it sees this tick's value, and one reading a module
    /// listed after it sees the previous tick's.
    #[test]
    fn math_reads_lower_slots_now_and_higher_slots_one_tick_late() {
        let mut forward = set_of(&[constant_step(0.25), math(0, ModMathOp::Multiply, 2.0)]);
        forward.run(48_000, 0, 120.0);
        assert_eq!(forward.outputs()[0], 0.25);
        assert_eq!(forward.outputs()[1], 0.5, "an earlier module resolves this tick");

        let mut backward = set_of(&[math(1, ModMathOp::Multiply, 2.0), constant_step(0.25)]);
        backward.run(48_000, 0, 120.0);
        assert_eq!(backward.outputs()[0], 0.0, "a later module must still read last tick");
        backward.run(48_000, 0, 120.0);
        assert_eq!(backward.outputs()[0], 0.5);
    }

    /// Self-reference needs no cycle machinery: it simply reads last tick,
    /// and the module's own output clamp keeps the feedback bounded.
    #[test]
    fn a_math_module_reading_itself_is_bounded_by_its_output_clamp() {
        let mut set = set_of(&[math(0, ModMathOp::Add, 0.25)]);
        for expected in [0.25, 0.5, 0.75, 1.0, 1.0, 1.0] {
            set.run(48_000, 0, 120.0);
            assert_eq!(set.outputs()[0], expected);
        }
    }

    #[test]
    fn math_refuses_to_divide_by_zero_or_to_invert_a_clamp() {
        let mut divide = set_of(&[constant_step(0.5), math(0, ModMathOp::Divide, 0.0)]);
        divide.run(48_000, 0, 120.0);
        assert!(divide.outputs()[1].is_finite());
        assert_eq!(divide.outputs()[1], 1.0, "a tiny divisor still clamps");

        let mut inverted = set_of(&[
            constant_step(0.5),
            ModulatorParams::Math(ModMathParams {
                input_slot: 0,
                op: ModMathOp::Clamp,
                clamp_low: 1.0,
                clamp_high: -1.0,
                ..ModMathParams::default()
            }),
        ]);
        inverted.run(48_000, 0, 120.0);
        assert_eq!(inverted.outputs()[1], 0.5);
    }

    /// **A module carried into a new set keeps running as itself**, wherever
    /// it lands in the list: a module added before it moves its position and
    /// must not restart its phase, take its neighbour's, or reseed a Random.
    #[test]
    fn a_carried_module_keeps_its_running_state_at_its_new_position() {
        let slow = ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 1.0,
            ..ModLfoParams::default()
        });
        let fast = ModulatorParams::Lfo(ModLfoParams {
            rate_hz: 7.0,
            phase: 0.25,
            ..ModLfoParams::default()
        });
        let mut before = set_of(&[slow, fast]);
        for _ in 0..40 {
            before.run(48_000, 32, 120.0);
        }
        let mut continued = before.clone();
        continued.run(48_000, 32, 120.0);

        // The two swapped, with a new module in front of them.
        let mut after = set_of(&[ModulatorParams::Random(ModRandomParams::default()), fast, slow]);
        after.carry(1, &before, 1, false);
        after.carry(2, &before, 0, false);
        assert_eq!(after.outputs()[1], before.outputs()[1]);
        after.run(48_000, 32, 120.0);
        assert_eq!(after.outputs()[1], continued.outputs()[1]);
        assert_eq!(after.outputs()[2], continued.outputs()[0]);

        // A kind change is not a carry.
        let mut changed = set_of(&[constant_step(0.5)]);
        changed.carry(0, &before, 0, false);
        changed.run(48_000, 0, 120.0);
        assert_eq!(changed.outputs()[0], 0.5);
    }
}
