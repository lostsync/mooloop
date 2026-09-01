//! Control-rate modulator sources.
//!
//! A modulator produces a signed `-1..1` value and no audio. Evaluation is
//! on a fixed subdivision of the block rather than once per block or per
//! sample: once per block stair-steps audibly on a fast LFO, and per sample
//! buys nothing at these rates (`docs/MODULATION_PLAN.md`).

use mooloop_core::{
    ModEnvelopeParams, ModLfoParams, ModLfoWaveform, ModMathOp, ModMathParams, ModRandomParams,
    ModRandomTrigger, ModStepParams, ModStepTrigger, ModulatorParams, MAX_CHANNELS,
    MAX_MODULATORS_PER_CHANNEL, MOD_STEP_MAX_STEPS,
};

/// Frames between modulation updates. The plan allows 32 or 64; 32 keeps a
/// 20 Hz LFO smooth at 48 kHz while costing one evaluation per 32 frames.
pub const CONTROL_RATE_FRAMES: usize = 32;

/// A free-running LFO. Phase is kept in `0..1` so a waveform change mid-cycle
/// keeps its position rather than jumping.
#[derive(Debug, Clone, Copy)]
struct Lfo {
    params: ModLfoParams,
    phase: f32,
    fade_elapsed_seconds: f32,
    smoothed: f32,
    output_initialized: bool,
    /// Current value of the stepped random waveform, redrawn only when the
    /// phase wraps. Regenerating per evaluation would be white noise at
    /// control rate rather than sample-and-hold.
    held: f32,
    rng: u32,
}

impl Lfo {
    fn new(params: ModLfoParams) -> Self {
        let mut lfo = Self {
            params,
            phase: params.phase.fract(),
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
#[derive(Debug, Clone, Copy)]
struct StepSequencer {
    params: ModStepParams,
    step: usize,
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
}

/// Sample-and-hold with room to be musical: a due draw can be skipped by
/// chance, snapped to a grid, or made to walk from the held value.
#[derive(Debug, Clone, Copy)]
struct RandomSource {
    params: ModRandomParams,
    /// The held value in the source's own range: `-1..1` when bipolar,
    /// `0..1` when not.
    held: f32,
    phase: f32,
    rng: u32,
}

impl RandomSource {
    /// Seeded from the slot, so two random modules on one channel are
    /// uncorrelated rather than identical, and still deterministic: an
    /// offline render draws the same sequence a realtime one did.
    fn new(params: ModRandomParams, slot: usize) -> Self {
        let mut source = Self {
            params,
            held: 0.0,
            phase: 0.0,
            // Odd, so the xorshift state can never be zero and stick there.
            rng: 0x9E37_79B9 ^ ((slot as u32).wrapping_add(1).wrapping_mul(0x85EB_CA6B) | 1),
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
}

/// The smallest divisor a math module will use. Division clamps its operand
/// away from zero rather than emitting an infinity a route would then
/// multiply into a destination.
const MATH_MIN_DIVISOR: f32 = 1.0e-3;

/// Arithmetic over another slot's output. Stateless: the whole module is its
/// params, and the slot-order rule lives in `ModulatorRack::tick`.
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

/// One channel's modulator slots and their current outputs.
///
/// Deliberately inline rather than boxed nodes: no modulator kind allocates,
/// so the install/reclaim machinery the effect chain needs buys nothing here
/// and would put a `Box` drop on the path of every rack edit.
#[derive(Debug, Clone, Copy)]
pub struct ModulatorRack {
    slots: [Option<Source>; MAX_MODULATORS_PER_CHANNEL],
    outputs: [f32; MAX_MODULATORS_PER_CHANNEL],
}

impl Default for ModulatorRack {
    fn default() -> Self {
        Self::new()
    }
}

impl ModulatorRack {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_MODULATORS_PER_CHANNEL],
            outputs: [0.0; MAX_MODULATORS_PER_CHANNEL],
        }
    }

    /// Install or clear one slot. Reconfiguring a slot that already holds the
    /// same kind keeps its phase, so retuning an LFO's rate does not restart
    /// it mid-performance.
    pub fn set_slot(&mut self, slot: usize, params: Option<ModulatorParams>) {
        let Some(existing) = self.slots.get_mut(slot) else {
            return;
        };
        match (params, existing.as_mut()) {
            (Some(ModulatorParams::Lfo(next)), Some(Source::Lfo(lfo))) => {
                let fade_changed = lfo.params.fade_in_seconds != next.fade_in_seconds
                    || lfo.params.fade_in_tempo_sync != next.fade_in_tempo_sync
                    || lfo.params.fade_in_division != next.fade_in_division;
                lfo.params = next;
                if fade_changed {
                    lfo.fade_elapsed_seconds = 0.0;
                }
            }
            (Some(ModulatorParams::Lfo(next)), _) => *existing = Some(Source::Lfo(Lfo::new(next))),
            (Some(ModulatorParams::Envelope(next)), Some(Source::Envelope(envelope))) => {
                if envelope.params.input_channel != next.input_channel {
                    envelope.held_notes = 0;
                    envelope.begin(EnvelopeStage::Release);
                }
                envelope.params = next;
            }
            (Some(ModulatorParams::Envelope(next)), _) => {
                *existing = Some(Source::Envelope(Envelope::new(next)))
            }
            // Retuning a running pattern keeps its position; a shortened
            // length folds the cursor back inside rather than stalling it
            // on a step that no longer plays.
            (Some(ModulatorParams::Step(next)), Some(Source::Step(sequencer))) => {
                sequencer.params = next;
                let length = sequencer.length();
                if sequencer.step >= length {
                    sequencer.step %= length;
                }
            }
            (Some(ModulatorParams::Step(next)), _) => {
                *existing = Some(Source::Step(StepSequencer::new(next)))
            }
            (Some(ModulatorParams::Random(next)), Some(Source::Random(random))) => {
                random.params = next;
            }
            (Some(ModulatorParams::Random(next)), _) => {
                *existing = Some(Source::Random(RandomSource::new(next, slot)))
            }
            (Some(ModulatorParams::Math(next)), Some(Source::Math(math))) => math.params = next,
            (Some(ModulatorParams::Math(next)), _) => {
                *existing = Some(Source::Math(MathSource { params: next }))
            }
            (None, _) => {
                *existing = None;
                self.outputs[slot] = 0.0;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Current `-1..1` output of every slot. Empty slots read zero, so an
    /// unassigned route contributes nothing rather than needing a guard.
    pub fn outputs(&self) -> &[f32; MAX_MODULATORS_PER_CHANNEL] {
        &self.outputs
    }

    /// Evaluate every slot for the coming `frames` and advance its phase.
    ///
    /// Modules evaluate in slot order within a control tick, so a module
    /// reading a lower slot sees this tick's value and one reading itself or
    /// a higher slot sees the previous tick's. That single rule is what makes
    /// a chain of modules deterministic, identical realtime and offline, and
    /// bounded without any cycle machinery: `outputs` simply still holds last
    /// tick's value everywhere this pass has not reached yet.
    pub fn tick(&mut self, sample_rate: u32, frames: usize, bpm: f64) {
        for (slot, source) in self.slots.iter_mut().enumerate() {
            let Some(source) = source else { continue };
            match source {
                Source::Lfo(lfo) => {
                    self.outputs[slot] = lfo.value(sample_rate, frames, bpm);
                    lfo.advance(sample_rate, frames, bpm);
                }
                Source::Envelope(envelope) => {
                    self.outputs[slot] = envelope.value();
                    envelope.advance(sample_rate, frames, bpm);
                }
                Source::Step(sequencer) => {
                    self.outputs[slot] = sequencer.value(bpm);
                    sequencer.advance(sample_rate, frames, bpm);
                }
                Source::Random(random) => {
                    self.outputs[slot] = random.value();
                    random.advance(sample_rate, frames, bpm);
                }
                Source::Math(math) => {
                    let input = self
                        .outputs
                        .get(math.params.input_slot as usize)
                        .copied()
                        .unwrap_or(0.0);
                    self.outputs[slot] = math.value(input);
                }
            };
        }
    }

    /// Deliver the current control tick's channel-note adapters, then
    /// evaluate. The owning channel remains the LFO's legacy Note On input;
    /// envelopes name their input channel explicitly.
    pub fn tick_with_note_gates(
        &mut self,
        sample_rate: u32,
        frames: usize,
        bpm: f64,
        owning_channel: usize,
        gates: &[NoteGateEvents; MAX_CHANNELS],
    ) {
        let owning_notes = gates
            .get(owning_channel)
            .map_or(0, |events| events.note_ons);
        for source in self.slots.iter_mut().flatten() {
            match source {
                Source::Lfo(lfo) => {
                    if owning_notes > 0 {
                        lfo.retrigger();
                    }
                }
                Source::Envelope(envelope) => {
                    let input = usize::from(envelope.params.input_channel);
                    if let Some(events) = gates.get(input).copied() {
                        envelope.note_events(events);
                    }
                }
                // Step and random modules take the owning channel's notes,
                // the same legacy input the LFO's retrigger uses. Their own
                // input jack arrives with the grid's explicit jacks.
                Source::Step(sequencer) => {
                    if owning_notes > 0 {
                        sequencer.note_advance();
                    }
                }
                Source::Random(random) => {
                    if owning_notes > 0 {
                        random.note_trigger();
                    }
                }
                Source::Math(_) => {}
            }
        }
        self.tick(sample_rate, frames, bpm);
    }

    /// Move every slot that follows notes: an LFO restarts its phase, a step
    /// pattern takes one step, a note-triggered random draws.
    pub fn retrigger(&mut self) {
        for source in self.slots.iter_mut().flatten() {
            match source {
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

    fn lfo(params: ModLfoParams) -> ModulatorRack {
        let mut rack = ModulatorRack::new();
        rack.set_slot(0, Some(ModulatorParams::Lfo(params)));
        rack
    }

    #[test]
    fn a_sine_lfo_completes_one_cycle_per_period() {
        let mut rack = lfo(ModLfoParams {
            rate_hz: 1.0,
            ..ModLfoParams::default()
        });
        // `tick` reports the value for the frames it is about to cover, then
        // advances, so each reading is the phase *before* that step.
        rack.tick(48_000, 12_000, 120.0);
        assert!(rack.outputs()[0].abs() < 1e-6, "starts at zero");
        // A quarter second at 1 Hz is a quarter cycle: the sine peak.
        rack.tick(48_000, 12_000, 120.0);
        assert!(
            (rack.outputs()[0] - 1.0).abs() < 1e-3,
            "{}",
            rack.outputs()[0]
        );
        rack.tick(48_000, 12_000, 120.0);
        assert!(rack.outputs()[0].abs() < 1e-3, "back through zero");
    }

    #[test]
    fn depth_scales_the_output_and_an_empty_slot_reads_zero() {
        let mut rack = lfo(ModLfoParams {
            rate_hz: 1.0,
            depth: 0.5,
            waveform: ModLfoWaveform::Square,
            ..ModLfoParams::default()
        });
        rack.tick(48_000, 0, 120.0);
        assert_eq!(rack.outputs()[0], 0.5);
        assert_eq!(rack.outputs()[1], 0.0);

        rack.set_slot(0, None);
        rack.tick(48_000, 0, 120.0);
        assert_eq!(rack.outputs()[0], 0.0);
        assert!(rack.is_empty());
    }

    /// Retuning a running LFO must not restart it: an automated rate change
    /// mid-performance should bend the motion, not reset the phase.
    #[test]
    fn reconfiguring_a_slot_keeps_its_phase() {
        let mut rack = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Saw,
            ..ModLfoParams::default()
        });
        rack.tick(48_000, 12_000, 120.0);
        let advanced = rack.outputs()[0];
        rack.set_slot(
            0,
            Some(ModulatorParams::Lfo(ModLfoParams {
                rate_hz: 4.0,
                waveform: ModLfoWaveform::Saw,
                ..ModLfoParams::default()
            })),
        );
        rack.tick(48_000, 0, 120.0);
        assert!(
            rack.outputs()[0] > advanced,
            "phase restarted on reconfigure"
        );
    }

    /// Sample-and-hold must hold. Regenerating every evaluation would be
    /// white noise at control rate rather than a stepped modulator.
    #[test]
    fn random_holds_its_value_across_a_cycle() {
        let mut rack = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Random,
            ..ModLfoParams::default()
        });
        rack.tick(48_000, 1_000, 120.0);
        let held = rack.outputs()[0];
        assert!((-1.0..=1.0).contains(&held), "out of range: {held}");
        // Ten more evaluations well inside the same cycle.
        for _ in 0..10 {
            rack.tick(48_000, 1_000, 120.0);
            assert_eq!(rack.outputs()[0], held, "value changed mid-cycle");
        }
        // Crossing the wrap draws a new one.
        rack.tick(48_000, 48_000, 120.0);
        rack.tick(48_000, 0, 120.0);
        assert_ne!(rack.outputs()[0], held);
    }

    #[test]
    fn retrigger_only_resets_slots_that_asked_for_it() {
        let mut free = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Saw,
            retrigger: false,
            ..ModLfoParams::default()
        });
        free.tick(48_000, 12_000, 120.0);
        assert_eq!(free.outputs()[0], -1.0, "a saw starts at its floor");
        free.retrigger();
        free.tick(48_000, 0, 120.0);
        assert_eq!(
            free.outputs()[0],
            -0.5,
            "a free-running LFO must ignore retrigger"
        );

        let mut played = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Saw,
            retrigger: true,
            ..ModLfoParams::default()
        });
        played.tick(48_000, 12_000, 120.0);
        played.retrigger();
        played.tick(48_000, 0, 120.0);
        assert_eq!(played.outputs()[0], -1.0, "saw must restart at its floor");
    }

    #[test]
    fn tempo_synced_rate_follows_the_current_bpm() {
        let mut rack = lfo(ModLfoParams {
            tempo_sync: true,
            rate_division: mooloop_core::ModTimeDivision::Quarter,
            ..ModLfoParams::default()
        });
        // At 120 BPM a quarter-note cycle is 0.5 seconds. One eighth of a
        // second advances to the sine peak.
        rack.tick(48_000, 6_000, 120.0);
        rack.tick(48_000, 0, 120.0);
        assert!((rack.outputs()[0] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn fade_in_scales_output_and_restarts_with_a_note_trigger() {
        let mut rack = lfo(ModLfoParams {
            waveform: ModLfoWaveform::Square,
            retrigger: true,
            fade_in_seconds: 1.0,
            ..ModLfoParams::default()
        });
        rack.tick(48_000, 24_000, 120.0);
        assert_eq!(rack.outputs()[0], 0.0);
        rack.tick(48_000, 0, 120.0);
        assert!((rack.outputs()[0].abs() - 0.5).abs() < 1e-6);
        rack.retrigger();
        rack.tick(48_000, 0, 120.0);
        assert_eq!(rack.outputs()[0], 0.0);

        rack.tick(48_000, 48_000, 120.0);
        rack.set_slot(
            0,
            Some(ModulatorParams::Lfo(ModLfoParams {
                waveform: ModLfoWaveform::Square,
                retrigger: true,
                fade_in_seconds: 2.0,
                ..ModLfoParams::default()
            })),
        );
        rack.tick(48_000, 0, 120.0);
        assert_eq!(
            rack.outputs()[0],
            0.0,
            "editing fade must audition a new ramp"
        );
    }

    #[test]
    fn pulse_width_moves_the_square_transition() {
        let mut rack = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Square,
            pulse_width: 0.2,
            ..ModLfoParams::default()
        });
        rack.tick(48_000, 12_000, 120.0);
        rack.tick(48_000, 0, 120.0);
        assert_eq!(rack.outputs()[0], -1.0, "25% phase is past a 20% pulse");
    }

    #[test]
    fn smoothing_slews_instead_of_stepping_between_levels() {
        let mut rack = lfo(ModLfoParams {
            rate_hz: 1.0,
            waveform: ModLfoWaveform::Square,
            pulse_width: 0.2,
            smoothing_seconds: 0.5,
            ..ModLfoParams::default()
        });
        rack.tick(48_000, 12_000, 120.0);
        assert_eq!(rack.outputs()[0], 1.0);
        rack.tick(48_000, 4_800, 120.0);
        assert!(
            (-1.0..1.0).contains(&rack.outputs()[0]),
            "smoothed transition jumped to {}",
            rack.outputs()[0]
        );
    }

    #[test]
    fn envelope_follows_its_selected_channel_gate_through_release() {
        let mut rack = ModulatorRack::new();
        rack.set_slot(
            0,
            Some(ModulatorParams::Envelope(ModEnvelopeParams {
                input_channel: 2,
                attack_seconds: 0.1,
                decay_seconds: 0.0,
                sustain: 1.0,
                release_seconds: 0.1,
                ..ModEnvelopeParams::default()
            })),
        );
        let mut gates = [NoteGateEvents::default(); MAX_CHANNELS];
        gates[2].note_ons = 1;
        rack.tick_with_note_gates(48_000, 4_800, 120.0, 0, &gates);
        assert_eq!(rack.outputs()[0], -1.0, "attack starts at the floor");
        rack.tick_with_note_gates(
            48_000,
            0,
            120.0,
            0,
            &[NoteGateEvents::default(); MAX_CHANNELS],
        );
        assert_eq!(rack.outputs()[0], 1.0, "attack reaches the ceiling");

        gates = [NoteGateEvents::default(); MAX_CHANNELS];
        gates[2].note_offs = 1;
        rack.tick_with_note_gates(48_000, 4_800, 120.0, 0, &gates);
        assert_eq!(rack.outputs()[0], 1.0, "release begins from the held level");
        rack.tick_with_note_gates(
            48_000,
            0,
            120.0,
            0,
            &[NoteGateEvents::default(); MAX_CHANNELS],
        );
        assert_eq!(rack.outputs()[0], -1.0, "release returns to the floor");
    }

    #[test]
    fn envelope_ignores_unselected_channel_notes() {
        let mut rack = ModulatorRack::new();
        rack.set_slot(
            0,
            Some(ModulatorParams::Envelope(ModEnvelopeParams {
                input_channel: 3,
                attack_seconds: 0.0,
                ..ModEnvelopeParams::default()
            })),
        );
        let mut gates = [NoteGateEvents::default(); MAX_CHANNELS];
        gates[1].note_ons = 1;
        rack.tick_with_note_gates(48_000, 32, 120.0, 0, &gates);
        rack.tick(48_000, 0, 120.0);
        assert_eq!(rack.outputs()[0], -1.0);
    }

    fn rack_with(slots: &[(usize, ModulatorParams)]) -> ModulatorRack {
        let mut rack = ModulatorRack::new();
        for (slot, params) in slots {
            rack.set_slot(*slot, Some(*params));
        }
        rack
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
        let mut rack = rack_with(&[(
            0,
            ModulatorParams::Step(ModStepParams {
                steps,
                length: 3,
                division: mooloop_core::ModTimeDivision::Sixteenth,
                ..ModStepParams::default()
            }),
        )]);
        for expected in [1.0, -1.0, 0.5, 1.0, -1.0] {
            rack.tick(48_000, STEP_FRAMES, 120.0);
            assert_eq!(rack.outputs()[0], expected);
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
        let mut rack = rack_with(&[(0, ModulatorParams::Step(params))]);
        rack.tick(48_000, STEP_FRAMES, 120.0);
        assert_eq!(rack.outputs()[0], 1.0, "the first step starts at its value");
        rack.tick(48_000, STEP_FRAMES / 2, 120.0);
        assert_eq!(rack.outputs()[0], 1.0, "a full glide leaves from the old value");
        rack.tick(48_000, 0, 120.0);
        assert!(
            rack.outputs()[0].abs() < 1e-6,
            "half a glide should be halfway: {}",
            rack.outputs()[0]
        );

        let mut hard = rack_with(&[(
            0,
            ModulatorParams::Step(ModStepParams {
                glide: 0.0,
                ..params
            }),
        )]);
        hard.tick(48_000, STEP_FRAMES, 120.0);
        hard.tick(48_000, STEP_FRAMES / 2, 120.0);
        assert_eq!(hard.outputs()[0], -1.0, "no glide must step, not slide");
    }

    #[test]
    fn a_note_advance_pattern_ignores_the_clock_and_moves_on_notes() {
        let mut steps = [0.0; MOD_STEP_MAX_STEPS];
        steps[0] = 1.0;
        steps[1] = -1.0;
        let mut rack = rack_with(&[(
            0,
            ModulatorParams::Step(ModStepParams {
                steps,
                length: 2,
                trigger: ModStepTrigger::NoteAdvance,
                ..ModStepParams::default()
            }),
        )]);
        for _ in 0..8 {
            rack.tick(48_000, STEP_FRAMES, 120.0);
            assert_eq!(rack.outputs()[0], 1.0, "the clock must not advance it");
        }
        let mut gates = [NoteGateEvents::default(); MAX_CHANNELS];
        gates[1].note_ons = 1;
        rack.tick_with_note_gates(48_000, 0, 120.0, 1, &gates);
        assert_eq!(rack.outputs()[0], -1.0);
    }

    /// Probability is the whole musical point of the random module: at zero
    /// it freezes what it holds, at one it draws on every clock.
    #[test]
    fn probability_gates_whether_a_due_draw_lands() {
        let mut frozen = rack_with(&[(
            0,
            ModulatorParams::Random(ModRandomParams {
                probability: 0.0,
                ..ModRandomParams::default()
            }),
        )]);
        frozen.tick(48_000, 0, 120.0);
        let held = frozen.outputs()[0];
        for _ in 0..32 {
            frozen.tick(48_000, 48_000, 120.0);
            assert_eq!(frozen.outputs()[0], held, "chance zero must freeze");
        }

        let mut always = rack_with(&[(
            0,
            ModulatorParams::Random(ModRandomParams::default()),
        )]);
        let mut seen = Vec::new();
        for _ in 0..32 {
            always.tick(48_000, 48_000, 120.0);
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
        let mut rack = rack_with(&[(
            0,
            ModulatorParams::Random(ModRandomParams {
                drunk: true,
                walk,
                ..ModRandomParams::default()
            }),
        )]);
        rack.tick(48_000, 0, 120.0);
        let mut previous = rack.outputs()[0];
        for _ in 0..256 {
            rack.tick(48_000, 48_000, 120.0);
            let value = rack.outputs()[0];
            assert!((-1.0..=1.0).contains(&value), "escaped the range: {value}");
            assert!(
                (value - previous).abs() <= walk + 1e-5,
                "jumped {} from {previous} to {value}",
                (value - previous).abs()
            );
            previous = value;
        }
    }

    /// Two random modules on one channel must not be the same random
    /// module. Seeding from the slot decorrelates them without giving up
    /// the determinism an offline render depends on.
    #[test]
    fn random_slots_draw_independent_sequences() {
        let mut rack = rack_with(&[
            (0, ModulatorParams::Random(ModRandomParams::default())),
            (1, ModulatorParams::Random(ModRandomParams::default())),
        ]);
        let mut differed = false;
        for _ in 0..16 {
            rack.tick(48_000, 48_000, 120.0);
            differed |= rack.outputs()[0] != rack.outputs()[1];
        }
        assert!(differed, "both slots drew the same sequence");

        // Same rack, same ticks, same values: the sequence is reproducible.
        let mut replay = rack_with(&[(0, ModulatorParams::Random(ModRandomParams::default()))]);
        let mut first = Vec::new();
        for _ in 0..16 {
            replay.tick(48_000, 48_000, 120.0);
            first.push(replay.outputs()[0]);
        }
        let mut again = rack_with(&[(0, ModulatorParams::Random(ModRandomParams::default()))]);
        for expected in first {
            again.tick(48_000, 48_000, 120.0);
            assert_eq!(again.outputs()[0], expected);
        }
    }

    #[test]
    fn quantized_draws_land_on_the_grid_and_unipolar_lifts_to_the_wire() {
        let mut rack = rack_with(&[(
            0,
            ModulatorParams::Random(ModRandomParams {
                bipolar: false,
                quantize: 3,
                ..ModRandomParams::default()
            }),
        )]);
        // Three levels across 0..1 are 0, 0.5 and 1, carried on the signed
        // wire as -1, 0 and 1 exactly as the envelope carries its unipolar
        // contour.
        for _ in 0..32 {
            rack.tick(48_000, 48_000, 120.0);
            let value = rack.outputs()[0];
            assert!(
                [-1.0, 0.0, 1.0].iter().any(|level| (value - level).abs() < 1e-5),
                "off the grid: {value}"
            );
        }
    }

    /// The slot-order rule, stated in both directions: a module reading a
    /// lower slot sees this tick's value, and one reading a higher slot sees
    /// the previous tick's.
    #[test]
    fn math_reads_lower_slots_now_and_higher_slots_one_tick_late() {
        let doubler = ModulatorParams::Math(ModMathParams {
            input_slot: 0,
            op: ModMathOp::Multiply,
            operand: 2.0,
            ..ModMathParams::default()
        });
        let mut forward = rack_with(&[(0, constant_step(0.25)), (1, doubler)]);
        forward.tick(48_000, 0, 120.0);
        assert_eq!(forward.outputs()[0], 0.25);
        assert_eq!(forward.outputs()[1], 0.5, "a lower slot resolves this tick");

        let mut backward = rack_with(&[
            (
                0,
                ModulatorParams::Math(ModMathParams {
                    input_slot: 2,
                    op: ModMathOp::Multiply,
                    operand: 2.0,
                    ..ModMathParams::default()
                }),
            ),
            (2, constant_step(0.25)),
        ]);
        backward.tick(48_000, 0, 120.0);
        assert_eq!(
            backward.outputs()[0],
            0.0,
            "a higher slot must still read last tick"
        );
        backward.tick(48_000, 0, 120.0);
        assert_eq!(backward.outputs()[0], 0.5);
    }

    /// Self-reference needs no cycle machinery: it simply reads last tick,
    /// and the module's own output clamp keeps the feedback bounded.
    #[test]
    fn a_math_module_reading_itself_is_bounded_by_its_output_clamp() {
        let mut rack = rack_with(&[(
            0,
            ModulatorParams::Math(ModMathParams {
                input_slot: 0,
                op: ModMathOp::Add,
                operand: 0.25,
                ..ModMathParams::default()
            }),
        )]);
        for expected in [0.25, 0.5, 0.75, 1.0, 1.0, 1.0] {
            rack.tick(48_000, 0, 120.0);
            assert_eq!(rack.outputs()[0], expected);
        }
    }

    #[test]
    fn math_refuses_to_divide_by_zero_or_to_invert_a_clamp() {
        let mut divide = rack_with(&[
            (0, constant_step(0.5)),
            (
                1,
                ModulatorParams::Math(ModMathParams {
                    input_slot: 0,
                    op: ModMathOp::Divide,
                    operand: 0.0,
                    ..ModMathParams::default()
                }),
            ),
        ]);
        divide.tick(48_000, 0, 120.0);
        assert!(divide.outputs()[1].is_finite());
        assert_eq!(divide.outputs()[1], 1.0, "a tiny divisor still clamps");

        let mut inverted = rack_with(&[
            (0, constant_step(0.5)),
            (
                1,
                ModulatorParams::Math(ModMathParams {
                    input_slot: 0,
                    op: ModMathOp::Clamp,
                    clamp_low: 1.0,
                    clamp_high: -1.0,
                    ..ModMathParams::default()
                }),
            ),
        ]);
        inverted.tick(48_000, 0, 120.0);
        assert_eq!(inverted.outputs()[1], 0.5);
    }
}
