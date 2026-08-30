//! Control-rate modulator sources.
//!
//! A modulator produces a signed `-1..1` value and no audio. Evaluation is
//! on a fixed subdivision of the block rather than once per block or per
//! sample: once per block stair-steps audibly on a fast LFO, and per sample
//! buys nothing at these rates (`docs/MODULATION_PLAN.md`).

use mooloop_core::{
    ModEnvelopeParams, ModLfoParams, ModLfoWaveform, ModulatorParams, MAX_CHANNELS,
    MAX_MODULATORS_PER_CHANNEL,
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

#[derive(Debug, Clone, Copy)]
enum Source {
    Lfo(Lfo),
    Envelope(Envelope),
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
        for source in self.slots.iter_mut().flatten() {
            match source {
                Source::Lfo(lfo) => {
                    if gates
                        .get(owning_channel)
                        .is_some_and(|events| events.note_ons > 0)
                    {
                        lfo.retrigger();
                    }
                }
                Source::Envelope(envelope) => {
                    let input = usize::from(envelope.params.input_channel);
                    if let Some(events) = gates.get(input).copied() {
                        envelope.note_events(events);
                    }
                }
            }
        }
        self.tick(sample_rate, frames, bpm);
    }

    /// Restart phase on every slot configured to follow notes.
    pub fn retrigger(&mut self) {
        for source in self.slots.iter_mut().flatten() {
            if let Source::Lfo(lfo) = source {
                lfo.retrigger();
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
}
