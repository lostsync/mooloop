//! Synthetic, deterministic test material.
//!
//! Fixtures are generated from code rather than committed as audio: the repo
//! stays free of binaries, and anyone can reproduce the exact renders by
//! running the harness. The seeded PRNG makes every run bit-identical.

use crate::Rng;

pub const SR: u32 = 48_000;

pub struct Fixture {
    pub name: &'static str,
    pub frames: Vec<[f32; 2]>,
    /// Frame positions of the transients that were *placed* by the generator.
    /// This is ground truth, independent of any detector, so onset timing can
    /// be scored without trusting the detector under test.
    pub true_onsets: Vec<usize>,
    /// Fundamental frequency in Hz when the fixture has one, for pitch error.
    pub f0: Option<f32>,
}

fn secs(s: f32) -> usize {
    (s * SR as f32) as usize
}

fn add(dst: &mut [[f32; 2]], at: usize, sample: usize, l: f32, r: f32) {
    let i = at + sample;
    if i < dst.len() {
        dst[i][0] += l;
        dst[i][1] += r;
    }
}

/// Kick: exponential pitch drop plus a short click, so it has both a broadband
/// transient and low-frequency content that punishes short windows.
fn kick(dst: &mut [[f32; 2]], at: usize, amp: f32) {
    let n = secs(0.22);
    let mut phase = 0.0f32;
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let f = 48.0 + 82.0 * (-t * 55.0).exp();
        phase += core::f32::consts::TAU * f / SR as f32;
        let env = (-t * 22.0).exp();
        let click = if i < 40 {
            (1.0 - i as f32 / 40.0) * 0.35
        } else {
            0.0
        };
        let v = amp * (phase.sin() * env + click);
        add(dst, at, i, v, v);
    }
}

/// Snare: tonal body plus decorrelated noise, slightly wide.
fn snare(dst: &mut [[f32; 2]], at: usize, amp: f32, rng: &mut Rng) {
    let n = secs(0.26);
    let mut phase = 0.0f32;
    let mut hp_l = 0.0f32;
    let mut hp_r = 0.0f32;
    for i in 0..n {
        let t = i as f32 / SR as f32;
        phase += core::f32::consts::TAU * 187.0 / SR as f32;
        let body = phase.sin() * (-t * 42.0).exp() * 0.5;
        let nl = rng.bipolar();
        let nr = rng.bipolar();
        hp_l += 0.35 * (nl - hp_l);
        hp_r += 0.35 * (nr - hp_r);
        let noise_env = (-t * 17.0).exp();
        let l = amp * (body + (nl - hp_l) * noise_env * 0.7);
        let r = amp * (body + (nr - hp_r) * noise_env * 0.7);
        add(dst, at, i, l, r);
    }
}

/// Closed hat: bright, short, alternately panned so stereo image damage shows.
fn hat(dst: &mut [[f32; 2]], at: usize, amp: f32, pan: f32, rng: &mut Rng) {
    let n = secs(0.06);
    let mut lp = 0.0f32;
    let gl = (1.0 - pan).clamp(0.0, 1.0).sqrt();
    let gr = (1.0 + pan).clamp(0.0, 1.0).sqrt();
    for i in 0..n {
        let t = i as f32 / SR as f32;
        let x = rng.bipolar();
        lp += 0.72 * (x - lp);
        let v = amp * (x - lp) * (-t * 90.0).exp();
        add(dst, at, i, v * gl, v * gr);
    }
}

/// Band-limited sawtooth by additive synthesis: no aliasing, so measured pitch
/// error is the stretcher's and not the generator's.
fn saw(dst: &mut [[f32; 2]], at: usize, len: usize, freq: f32, amp: f32, decay: f32) {
    let harmonics = ((SR as f32 * 0.45) / freq) as usize;
    for i in 0..len {
        let t = i as f32 / SR as f32;
        let mut v = 0.0f32;
        for h in 1..=harmonics.min(80) {
            v += (core::f32::consts::TAU * freq * h as f32 * t).sin() / h as f32;
        }
        let env = (-t * decay).exp() * (1.0 - (-t * 400.0).exp());
        let v = amp * v * env * 0.55;
        add(dst, at, i, v, v);
    }
}

fn grid(bpm: f32, step: usize) -> usize {
    let sixteenth = 60.0 / bpm / 4.0;
    secs(sixteenth * step as f32)
}

/// Two bars of a break-style pattern at 138 BPM. Primary material.
pub fn drum_break() -> Fixture {
    let bpm = 138.0;
    let len = grid(bpm, 32) + secs(0.4);
    let mut frames = vec![[0.0f32; 2]; len];
    let mut rng = Rng::new(0x5EED_0001);
    let mut onsets = Vec::new();

    // Amen-flavoured placement: kicks on 0/10/16/26, snares on 4/12/20/28
    // with ghosts, hats on every second sixteenth.
    let kicks = [0usize, 10, 16, 26, 27];
    let snares = [4usize, 12, 20, 28];
    let ghosts = [7usize, 14, 23, 30];
    for step in 0..32 {
        let at = grid(bpm, step);
        if kicks.contains(&step) {
            kick(&mut frames, at, 0.85);
            onsets.push(at);
        }
        if snares.contains(&step) {
            snare(&mut frames, at, 0.7, &mut rng);
            onsets.push(at);
        }
        if ghosts.contains(&step) {
            snare(&mut frames, at, 0.22, &mut rng);
            onsets.push(at);
        }
        if step % 2 == 0 {
            let pan = if (step / 2) % 2 == 0 { -0.35 } else { 0.35 };
            hat(&mut frames, at, 0.32, pan, &mut rng);
            if !kicks.contains(&step) && !snares.contains(&step) {
                onsets.push(at);
            }
        }
    }
    onsets.sort_unstable();
    onsets.dedup();
    Fixture {
        name: "drum_break",
        frames,
        true_onsets: onsets,
        f0: None,
    }
}

/// A single snare. The cleanest test for "did the transient get duplicated".
pub fn percussive_oneshot() -> Fixture {
    let mut frames = vec![[0.0f32; 2]; secs(0.6)];
    let mut rng = Rng::new(0x5EED_0002);
    snare(&mut frames, secs(0.02), 0.9, &mut rng);
    Fixture {
        name: "percussive_oneshot",
        frames,
        true_onsets: vec![secs(0.02)],
        f0: None,
    }
}

/// Widely spaced short bursts. Transient timing and flam counting are
/// unambiguous here because nothing overlaps.
pub fn click_train() -> Fixture {
    let len = secs(4.0);
    let mut frames = vec![[0.0f32; 2]; len];
    let mut onsets = Vec::new();
    let mut rng = Rng::new(0x5EED_0003);
    for k in 0..16 {
        let at = secs(0.1 + 0.24 * k as f32);
        hat(&mut frames, at, 0.9, 0.0, &mut rng);
        onsets.push(at);
    }
    Fixture {
        name: "click_train",
        frames,
        true_onsets: onsets,
        f0: None,
    }
}

/// One bar of monophonic bass. Secondary stress case: low fundamentals expose
/// short-window time-domain stretching, and the pitch is measurable.
pub fn bass_line() -> Fixture {
    let bpm = 138.0;
    let len = grid(bpm, 16) + secs(0.3);
    let mut frames = vec![[0.0f32; 2]; len];
    let notes = [(0usize, 55.0f32), (4, 55.0), (7, 73.42), (10, 82.41), (12, 61.74)];
    let mut onsets = Vec::new();
    for (step, freq) in notes {
        let at = grid(bpm, step);
        saw(&mut frames, at, secs(0.42), freq, 0.8, 5.0);
        onsets.push(at);
    }
    Fixture {
        name: "bass_line",
        frames,
        true_onsets: onsets,
        // Deliberately `None`: this fixture changes note, so a whole-render
        // spectral peak does not correspond to any single nominal pitch.
        f0: None,
    }
}

/// Steady tone. Pitch error and phasiness read directly off this.
pub fn sine_tone() -> Fixture {
    let len = secs(2.0);
    let mut frames = vec![[0.0f32; 2]; len];
    for (i, f) in frames.iter_mut().enumerate() {
        let v = 0.5 * (core::f32::consts::TAU * 440.0 * i as f32 / SR as f32).sin();
        *f = [v, v];
    }
    Fixture {
        name: "sine_440",
        frames,
        true_onsets: vec![],
        f0: Some(440.0),
    }
}

/// Break plus bass plus a sustained chord: the realistic worst case, where a
/// transient-first algorithm has to not wreck the sustained part.
pub fn mixed_loop() -> Fixture {
    let drums = drum_break();
    let bass = bass_line();
    let len = drums.frames.len();
    let mut frames = drums.frames.clone();
    for (i, f) in bass.frames.iter().enumerate() {
        if i < len {
            frames[i][0] += f[0] * 0.8;
            frames[i][1] += f[1] * 0.8;
        }
        let j = i + bass.frames.len();
        if j < len {
            frames[j][0] += f[0] * 0.8;
            frames[j][1] += f[1] * 0.8;
        }
    }
    // Sustained triad, slightly detuned per channel so the pad has real width.
    let mut pad = vec![[0.0f32; 2]; len];
    for (k, freq) in [220.0f32, 261.63, 329.63].iter().enumerate() {
        let det = 1.0 + 0.0015 * (k as f32 - 1.0);
        for (i, f) in pad.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            f[0] += (core::f32::consts::TAU * freq * t).sin() * 0.09;
            f[1] += (core::f32::consts::TAU * freq * det * t).sin() * 0.09;
        }
    }
    for (i, f) in frames.iter_mut().enumerate() {
        f[0] += pad[i][0];
        f[1] += pad[i][1];
    }
    for f in frames.iter_mut() {
        f[0] = f[0].clamp(-1.0, 1.0);
        f[1] = f[1].clamp(-1.0, 1.0);
    }
    Fixture {
        name: "mixed_loop",
        frames,
        true_onsets: drums.true_onsets,
        f0: None,
    }
}

/// Fully decorrelated stereo noise bed with panned hits. Any algorithm that
/// treats the two channels independently collapses or scrambles this.
pub fn stereo_wide() -> Fixture {
    let len = secs(2.0);
    let mut frames = vec![[0.0f32; 2]; len];
    let mut rng = Rng::new(0x5EED_0004);
    let mut lp = [0.0f32; 2];
    for f in frames.iter_mut() {
        let a = rng.bipolar();
        let b = rng.bipolar();
        lp[0] += 0.08 * (a - lp[0]);
        lp[1] += 0.08 * (b - lp[1]);
        *f = [lp[0] * 2.4, lp[1] * 2.4];
    }
    let mut onsets = Vec::new();
    for k in 0..8 {
        let at = secs(0.15 + 0.24 * k as f32);
        let pan = if k % 2 == 0 { -0.7 } else { 0.7 };
        hat(&mut frames, at, 0.8, pan, &mut rng);
        onsets.push(at);
    }
    Fixture {
        name: "stereo_wide",
        frames,
        true_onsets: onsets,
        f0: None,
    }
}

/// One sustained bass note. `bass_line` cannot be used for pitch error because
/// it contains several different notes, so the strongest partial over the whole
/// render belongs to whichever note is longest, not to the nominal root.
pub fn bass_note() -> Fixture {
    let len = secs(2.5);
    let mut frames = vec![[0.0f32; 2]; len];
    saw(&mut frames, 0, len, 55.0, 0.8, 0.15);
    Fixture {
        name: "bass_note",
        frames,
        true_onsets: vec![0],
        f0: Some(55.0),
    }
}

pub fn all() -> Vec<Fixture> {
    vec![
        drum_break(),
        percussive_oneshot(),
        click_train(),
        bass_line(),
        bass_note(),
        sine_tone(),
        mixed_loop(),
        stereo_wide(),
    ]
}
