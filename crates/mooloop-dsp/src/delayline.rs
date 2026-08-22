//! A stereo circular audio buffer with fractional read heads.
//!
//! This is a shared primitive, not a private part of the delay effect. The
//! retained-audio buffer device in `docs/BUFFER_ENGINE.md` needs the same
//! thing — a bounded ring, fractional reads, and clean discontinuities when a
//! head moves — so it is built once here and both use it. See
//! `docs/MODULATION_PLAN.md` ("The delay line is shared with the buffer
//! device").
//!
//! ## Realtime contract
//!
//! [`DelayLine::with_capacity_frames`] allocates and must be called off the
//! audio thread. Everything else is allocation-free and safe inside
//! `AudioNode::process`.

/// Closest a read head may come to the write head. Cubic interpolation needs
/// one sample newer than the one it lands on, so a head cannot sit on the
/// very newest frame.
pub const MIN_READ_OFFSET: f32 = 2.0;

/// A fixed-capacity stereo ring buffer.
pub struct DelayLine {
    left: Vec<f32>,
    right: Vec<f32>,
    /// Index the next written frame will occupy. The newest frame already in
    /// the buffer is at `write - 1`.
    write: usize,
}

impl DelayLine {
    /// Allocate a ring holding `frames` stereo frames. Allocates: call from
    /// the non-realtime side only.
    pub fn with_capacity_frames(frames: usize) -> Self {
        // Cubic interpolation reaches one frame either side of its landing
        // point, so the smallest useful ring is a few frames long.
        let frames = frames.max(8);
        Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            write: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.left.len()
    }

    /// Largest offset a read head may request. Beyond this the four-point
    /// interpolation window would reach past the oldest retained frame.
    pub fn max_read_offset(&self) -> f32 {
        (self.capacity() as f32 - 3.0).max(MIN_READ_OFFSET)
    }

    /// Zero the buffer and reset the write head. Allocation-free, but it
    /// touches the whole ring — do it on a reset, not per block.
    pub fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.write = 0;
    }

    /// Append one frame, advancing the write head.
    pub fn write(&mut self, l: f32, r: f32) {
        let capacity = self.capacity();
        self.left[self.write] = l;
        self.right[self.write] = r;
        self.write = (self.write + 1) % capacity;
    }

    /// Read `offset` frames behind the write head, interpolating between
    /// samples with a 4-point cubic Hermite kernel.
    ///
    /// Linear interpolation is not good enough here: the buffer device reads
    /// at varying rates and in reverse, and linear's frequency-dependent
    /// droop and aliasing are plainly audible under both. `offset` is clamped
    /// into the ring's valid window.
    pub fn read(&self, offset: f32) -> (f32, f32) {
        let offset = offset.clamp(MIN_READ_OFFSET, self.max_read_offset());
        let capacity = self.capacity();

        // Position of the newest frame is `write - 1`; walk back from there.
        let position = self.write as f32 - 1.0 - offset;
        let base = position.floor();
        let t = position - base;

        // `base` can be negative before the ring has wrapped; bias by a whole
        // number of laps before converting so the modulo stays in range.
        let bias = (capacity * 2) as f32;
        let index = (base + bias) as usize;

        let xm1 = self.frame((index + capacity - 1) % capacity);
        let x0 = self.frame(index % capacity);
        let x1 = self.frame((index + 1) % capacity);
        let x2 = self.frame((index + 2) % capacity);

        (
            hermite(xm1.0, x0.0, x1.0, x2.0, t),
            hermite(xm1.1, x0.1, x1.1, x2.1, t),
        )
    }

    fn frame(&self, index: usize) -> (f32, f32) {
        (self.left[index], self.right[index])
    }
}

/// 4-point, 3rd-order Hermite interpolation between `x0` and `x1` at `t`,
/// using `xm1` and `x2` to estimate the slopes.
fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b = w + a;
    ((a * t - b) * t + c) * t + x0
}

/// A read head positioned relative to the write head, able to move without
/// clicking.
///
/// The head does not know about playback rate or direction: the caller passes
/// how far the offset should drift each frame, which is what lets one type
/// serve a fixed delay tap, a repitching tape delay, a reverse window, and
/// the buffer device's detached heads.
///
/// Drift per frame is `1 - rate` for forward playback and `1 + rate` for
/// reverse, because the write head is also advancing: a forward head at rate
/// 1.0 holds a constant offset, and a held head falls behind at 1.0.
pub struct ReadHead {
    offset: f32,
    /// Trajectory being faded out after a jump.
    fade_offset: f32,
    fade_remaining: u32,
    fade_len: u32,
}

impl ReadHead {
    pub fn new(offset: f32) -> Self {
        Self {
            offset,
            fade_offset: offset,
            fade_remaining: 0,
            fade_len: 0,
        }
    }

    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// Move with no crossfade. For continuous motion (a tape delay gliding to
    /// a new time), where a fade would smear rather than hide anything.
    pub fn set_offset(&mut self, offset: f32) {
        self.offset = offset;
    }

    /// Whether a crossfade is currently running.
    pub fn is_fading(&self) -> bool {
        self.fade_remaining > 0
    }

    /// Jump to a new offset, crossfading from the old trajectory over
    /// `fade_frames`.
    ///
    /// A `fade_frames` of 0 is legal and jumps hard: abrupt edits are a
    /// musical option here, so this deliberately does not enforce a minimum.
    pub fn jump_to(&mut self, offset: f32, fade_frames: u32) {
        if fade_frames == 0 {
            self.offset = offset;
            self.fade_remaining = 0;
            self.fade_len = 0;
            return;
        }
        // A jump during a jump restarts the fade from wherever the outgoing
        // trajectory currently is, rather than stacking fades.
        self.fade_offset = self.offset;
        self.offset = offset;
        self.fade_remaining = fade_frames;
        self.fade_len = fade_frames;
    }

    /// Drift both trajectories by `delta` frames and age any running fade.
    pub fn advance(&mut self, delta: f32) {
        self.offset += delta;
        if self.fade_remaining > 0 {
            self.fade_offset += delta;
            self.fade_remaining -= 1;
        }
    }

    /// Read from `line`, blending the outgoing trajectory while a fade runs.
    ///
    /// The two taps are uncorrelated after a jump, so the blend is
    /// equal-power; a linear blend would dip audibly through the middle.
    pub fn read(&self, line: &DelayLine) -> (f32, f32) {
        let current = line.read(self.offset);
        if self.fade_remaining == 0 || self.fade_len == 0 {
            return current;
        }
        let previous = line.read(self.fade_offset);
        // Progress runs 0 -> 1 across the fade.
        let t = 1.0 - self.fade_remaining as f32 / self.fade_len as f32;
        let angle = t * core::f32::consts::FRAC_PI_2;
        let (gain_new, gain_old) = (angle.sin(), angle.cos());
        (
            current.0 * gain_new + previous.0 * gain_old,
            current.1 * gain_new + previous.1 * gain_old,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(frames: usize, mut f: impl FnMut(usize) -> f32) -> DelayLine {
        let mut line = DelayLine::with_capacity_frames(frames);
        for i in 0..frames {
            let v = f(i);
            line.write(v, -v);
        }
        line
    }

    #[test]
    fn integer_offsets_return_the_exact_written_frame() {
        // Ramp so every frame is distinguishable.
        let line = filled(64, |i| i as f32);
        for offset in 2..=60 {
            let (l, r) = line.read(offset as f32);
            let expected = (63 - offset) as f32;
            assert!(
                (l - expected).abs() < 1e-3,
                "offset {offset}: got {l}, expected {expected}"
            );
            assert!((r + expected).abs() < 1e-3, "right channel diverged");
        }
    }

    #[test]
    fn zero_offset_reads_near_the_newest_frame() {
        let line = filled(64, |i| i as f32);
        // Clamped to MIN_READ_OFFSET rather than reading past the write head.
        let (l, _) = line.read(0.0);
        let expected = 63.0 - MIN_READ_OFFSET;
        assert!((l - expected).abs() < 1e-3, "got {l}, expected {expected}");
    }

    #[test]
    fn fractional_offsets_interpolate_a_ramp_exactly() {
        // Hermite reproduces a linear signal exactly, so a ramp is a precise
        // check of the fractional path rather than an approximate one.
        let line = filled(128, |i| i as f32 * 0.25);
        for step in 0..40 {
            let offset = 4.0 + step as f32 * 0.25;
            let (l, _) = line.read(offset);
            let expected = (127.0 - offset) * 0.25;
            assert!(
                (l - expected).abs() < 1e-3,
                "offset {offset}: got {l}, expected {expected}"
            );
        }
    }

    #[test]
    fn interpolation_tracks_a_sine_more_closely_than_linear() {
        let capacity = 1_024;
        let period = 37.0; // deliberately not a whole number of frames
        let line = filled(capacity, |i| {
            (i as f32 / period * core::f32::consts::TAU).sin()
        });

        let mut cubic_error = 0.0f32;
        let mut linear_error = 0.0f32;
        for step in 0..200 {
            let offset = 8.0 + step as f32 * 0.37;
            let position = (capacity - 1) as f32 - offset;
            let expected = (position / period * core::f32::consts::TAU).sin();

            let (cubic, _) = line.read(offset);
            cubic_error += (cubic - expected).abs();

            // Linear reference over the same two frames.
            let base = position.floor() as usize;
            let t = position - base as f32;
            let linear = (1.0 - t) * (base as f32 / period * core::f32::consts::TAU).sin()
                + t * ((base + 1) as f32 / period * core::f32::consts::TAU).sin();
            linear_error += (linear - expected).abs();
        }
        assert!(
            cubic_error < linear_error * 0.25,
            "cubic error {cubic_error} should be far below linear {linear_error}"
        );
    }

    #[test]
    fn writes_wrap_and_keep_only_the_most_recent_frames() {
        let mut line = DelayLine::with_capacity_frames(16);
        for i in 0..100 {
            line.write(i as f32, 0.0);
        }
        // The newest frame is 99; offset 2 lands on 97.
        let (l, _) = line.read(2.0);
        assert!((l - 97.0).abs() < 1e-3, "got {l}");
        // Offsets past the ring clamp instead of reading stale audio.
        let (oldest, _) = line.read(1_000.0);
        assert!(oldest.is_finite());
    }

    #[test]
    fn a_head_at_rate_one_holds_its_offset() {
        let mut head = ReadHead::new(48.0);
        for _ in 0..1_000 {
            head.advance(1.0 - 1.0);
        }
        assert!((head.offset() - 48.0).abs() < 1e-3);
    }

    #[test]
    fn a_held_head_falls_behind_at_one_frame_per_frame() {
        let mut head = ReadHead::new(0.0);
        for _ in 0..100 {
            head.advance(1.0);
        }
        assert!((head.offset() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_reverse_head_at_rate_one_falls_behind_twice_as_fast() {
        let mut head = ReadHead::new(0.0);
        for _ in 0..100 {
            head.advance(1.0 + 1.0);
        }
        assert!((head.offset() - 200.0).abs() < 1e-3);
    }

    #[test]
    fn a_zero_length_fade_jumps_hard() {
        let line = filled(256, |i| i as f32);
        let mut head = ReadHead::new(10.0);
        head.jump_to(100.0, 0);
        assert!(!head.is_fading());
        let (l, _) = head.read(&line);
        let (direct, _) = line.read(100.0);
        assert!((l - direct).abs() < 1e-4);
    }

    /// The reason the crossfade exists: jumping between two uncorrelated
    /// points in a signal should not produce a step discontinuity.
    #[test]
    fn crossfading_a_jump_removes_the_step_a_hard_jump_leaves() {
        let capacity = 4_096;
        let line = filled(capacity, |i| {
            (i as f32 / 11.0 * core::f32::consts::TAU).sin()
        });

        let biggest_step = |fade: u32| {
            let mut head = ReadHead::new(64.0);
            let mut previous = head.read(&line).0;
            let mut worst = 0.0f32;
            for i in 0..256 {
                if i == 32 {
                    head.jump_to(1_500.0, fade);
                }
                let value = head.read(&line).0;
                worst = worst.max((value - previous).abs());
                previous = value;
                head.advance(0.0);
            }
            worst
        };

        let hard = biggest_step(0);
        let faded = biggest_step(128);
        assert!(
            faded < hard * 0.5,
            "crossfaded jump step {faded} should be well under hard jump {hard}"
        );
    }

    #[test]
    fn a_fade_completes_and_lands_on_the_new_trajectory() {
        let line = filled(1_024, |i| i as f32);
        let mut head = ReadHead::new(10.0);
        head.jump_to(500.0, 64);
        for _ in 0..64 {
            assert!(head.is_fading());
            head.advance(0.0);
        }
        assert!(!head.is_fading());
        let (l, _) = head.read(&line);
        let (direct, _) = line.read(500.0);
        assert!((l - direct).abs() < 1e-4, "got {l}, expected {direct}");
    }
}
