//! A simple metronome click generator.
//!
//! Throwaway scaffolding to prove the audio path works end-to-end. Phase 0
//! only; it gets removed once the Sampler lands in Phase 1. Implements `Device`
//! so it slots into the engine's graph like any other node.
//!
//! The click is a short sine burst with an exponential decay envelope. The
//! downbeat (beat 0 of each bar) uses a higher pitch and a slightly longer
//! envelope so it reads as the "1".
//!
//! The engine's transport detects beat boundaries while walking a block and
//! calls [`Metronome::trigger`] with the sample offset of each crossing; the
//! click is then rendered sample-accurately inside [`Device::process`].

use crate::device::{Device, ProcessContext};

const NORMAL_FREQ: f64 = 1500.0;
const ACCENT_FREQ: f64 = 2200.0;
const NORMAL_LEN_MS: f64 = 25.0;
const ACCENT_LEN_MS: f64 = 40.0;

/// Capacity for the pending-clicks ring. A single block at sane tempos
/// contains at most a couple of beat crossings; 8 is comfortable headroom.
const PENDING_CAP: usize = 8;

#[derive(Debug, Clone, Copy)]
struct PendingClick {
    offset: usize,
    accent: bool,
}

pub struct Metronome {
    sample_rate: u32,
    volume: f32,
    // Active voice state
    phase: f64,
    env_remaining: usize,
    env_total: usize,
    freq: f64,
    // Clicks scheduled to start during the next process() call.
    pending: [Option<PendingClick>; PENDING_CAP],
}

impl Metronome {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            volume: 0.0,
            phase: 0.0,
            env_remaining: 0,
            env_total: 1,
            freq: NORMAL_FREQ,
            pending: [None; PENDING_CAP],
        }
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    /// Schedule a click at `offset` samples into the upcoming block. Called
    /// from the realtime thread by the transport when a beat boundary falls
    /// within the block being processed.
    pub fn trigger(&mut self, offset: usize, accent: bool) {
        for slot in &mut self.pending {
            if slot.is_none() {
                *slot = Some(PendingClick { offset, accent });
                return;
            }
        }
        // Queue full: drop. In practice this never happens at sane tempos.
    }

    fn start_click(&mut self, accent: bool) {
        let len_ms = if accent { ACCENT_LEN_MS } else { NORMAL_LEN_MS };
        self.env_total = (len_ms * 0.001 * f64::from(self.sample_rate)).round() as usize;
        self.env_total = self.env_total.max(1);
        self.env_remaining = self.env_total;
        self.freq = if accent { ACCENT_FREQ } else { NORMAL_FREQ };
        self.phase = 0.0;
    }
}

impl Device for Metronome {
    fn process(&mut self, ctx: ProcessContext, out_l: &mut [f32], out_r: &mut [f32]) {
        // Take the pending queue for this block and reset our slot.
        let mut pending = self.pending;
        self.pending = [None; PENDING_CAP];

        let sr = f64::from(self.sample_rate);
        for i in 0..ctx.frames {
            // Launch any clicks whose offset matches this sample.
            for slot in pending.iter_mut() {
                if let Some(pc) = slot {
                    if pc.offset == i {
                        self.start_click(pc.accent);
                        *slot = None;
                    }
                }
            }

            if self.env_remaining > 0 {
                self.env_remaining -= 1;
                // Exponential amplitude decay over the click length.
                let progress = 1.0 - (self.env_remaining as f64 / self.env_total as f64);
                let env = (-5.0 * progress).exp();
                self.phase += self.freq / sr;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }
                let sample =
                    ((self.phase * core::f64::consts::TAU).sin() as f32) * (env as f32) * self.volume;
                out_l[i] += sample;
                out_r[i] += sample;
            }
        }
    }
}
