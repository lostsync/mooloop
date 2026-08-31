//! Measurement code shared by the harness and by the onset preparation the
//! stretchers consume.
//!
//! Everything here runs offline. The onset table it produces is exactly what a
//! production build would compute on the control thread when a sample is
//! loaded, so its cost is reported separately from per-block DSP cost.

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

use crate::hann;

pub fn mid(frames: &[[f32; 2]]) -> Vec<f32> {
    frames.iter().map(|f| 0.5 * (f[0] + f[1])).collect()
}

pub fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

pub fn db(x: f32) -> f32 {
    20.0 * (x.max(1.0e-12)).log10()
}

/// Magnitude STFT of a mono signal.
fn stft(x: &[f32], n: usize, hop: usize) -> Vec<Vec<f32>> {
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    let window = hann(n);
    let bins = n / 2 + 1;
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut buf = vec![Complex::new(0.0, 0.0); n];
    while pos + n <= x.len() {
        for i in 0..n {
            buf[i] = Complex::new(x[pos + i] * window[i], 0.0);
        }
        fft.process_with_scratch(&mut buf, &mut scratch);
        out.push(buf[..bins].iter().map(|c| c.norm()).collect());
        pos += hop;
    }
    out
}

/// Spectral-flux onset detection. Returns frame positions, ascending.
///
/// This is the control-side "prepare" step: at 48 kHz it is one 1024-point FFT
/// every 256 frames over the whole sample, done once at load.
pub fn onsets(frames: &[[f32; 2]], sample_rate: u32) -> Vec<usize> {
    let n = 1024usize;
    let hop = 256usize;
    let x = mid(frames);
    let spec = stft(&x, n, hop);
    if spec.len() < 3 {
        return Vec::new();
    }
    let mut flux = vec![0.0f32; spec.len()];
    let mut total = 0.0f32;
    for t in 1..spec.len() {
        let mut sum = 0.0f32;
        for k in 0..spec[t].len() {
            let d = spec[t][k] - spec[t - 1][k];
            if d > 0.0 {
                sum += d;
            }
        }
        flux[t] = sum;
    }
    for frame in &spec {
        total += frame.iter().sum::<f32>();
    }
    // Normalizing by the *average frame magnitude* rather than by the peak
    // flux is what keeps a steady tone from reading as a stream of onsets:
    // peak normalization rescales window-leakage wobble up to full scale as
    // soon as the signal has no real onsets in it at all.
    let norm = (total / spec.len() as f32).max(1.0e-9);
    for v in flux.iter_mut() {
        *v /= norm;
    }

    let median_span = 12usize;
    let min_gap_frames = (0.030 * sample_rate as f32 / hop as f32).round() as usize;
    let mut out: Vec<usize> = Vec::new();
    let mut scratch: Vec<f32> = Vec::with_capacity(2 * median_span + 1);
    for t in 1..flux.len() {
        let lo = t.saturating_sub(median_span);
        let hi = (t + median_span + 1).min(flux.len());
        scratch.clear();
        scratch.extend_from_slice(&flux[lo..hi]);
        scratch.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = scratch[scratch.len() / 2];
        let threshold = median * 2.0 + 0.22;
        let is_local_max = (t.saturating_sub(2)..(t + 3).min(flux.len()))
            .all(|j| flux[t] >= flux[j]);
        if flux[t] > threshold && is_local_max {
            if let Some(&last) = out.last() {
                if t - last < min_gap_frames {
                    if flux[t] > flux[last] {
                        out.pop();
                    } else {
                        continue;
                    }
                }
            }
            out.push(t);
        }
    }
    // Convert detection frames to sample positions. The flux peak sits at the
    // frame whose window first contains the attack, so bias back by half a hop.
    out.into_iter()
        .map(|t| (t * hop).saturating_sub(hop / 2))
        .collect()
}

/// Short-time energy envelope on a 0.25 ms grid, used for attack shape work.
pub fn envelope(x: &[f32], sample_rate: u32) -> (Vec<f32>, usize) {
    let bucket = (sample_rate as f32 * 0.00025).round().max(1.0) as usize;
    let mut env = Vec::with_capacity(x.len() / bucket + 1);
    let mut i = 0;
    while i < x.len() {
        let hi = (i + bucket).min(x.len());
        env.push(x[i..hi].iter().fold(0.0f32, |a, v| a.max(v.abs())));
        i += bucket;
    }
    (env, bucket)
}

pub struct OnsetScore {
    pub matched: usize,
    pub missed: usize,
    /// Detected onsets in the output with no expected counterpart: flams,
    /// echoes, and duplicated attacks.
    pub spurious: usize,
    pub mean_abs_err_ms: f32,
    pub max_abs_err_ms: f32,
}

/// Compare detected output onsets against where the source onsets should have
/// landed after an exact `ratio` stretch.
pub fn score_onsets(
    expected_src: &[usize],
    detected_out: &[usize],
    ratio: f64,
    sample_rate: u32,
    tolerance_ms: f32,
) -> OnsetScore {
    let tol = (tolerance_ms * 0.001 * sample_rate as f32) as f64;
    let mut used = vec![false; detected_out.len()];
    let mut matched = 0usize;
    let mut missed = 0usize;
    let mut errs: Vec<f32> = Vec::new();
    for &e in expected_src {
        let target = e as f64 * ratio;
        let mut best: Option<(usize, f64)> = None;
        for (i, &d) in detected_out.iter().enumerate() {
            if used[i] {
                continue;
            }
            let err = (d as f64 - target).abs();
            if err <= tol && best.map(|(_, b)| err < b).unwrap_or(true) {
                best = Some((i, err));
            }
        }
        match best {
            Some((i, err)) => {
                used[i] = true;
                matched += 1;
                errs.push((err / sample_rate as f64 * 1000.0) as f32);
            }
            None => missed += 1,
        }
    }
    let spurious = used.iter().filter(|u| !**u).count();
    let mean = if errs.is_empty() {
        0.0
    } else {
        errs.iter().sum::<f32>() / errs.len() as f32
    };
    let max = errs.iter().cloned().fold(0.0f32, f32::max);
    OnsetScore {
        matched,
        missed,
        spurious,
        mean_abs_err_ms: mean,
        max_abs_err_ms: max,
    }
}

/// 10%-90% rise time of the envelope leading into the peak nearest `at`.
pub fn attack_rise_ms(x: &[f32], at: usize, sample_rate: u32) -> Option<f32> {
    let (env, bucket) = envelope(x, sample_rate);
    let start = at / bucket;
    let win = (0.020 * sample_rate as f32 / bucket as f32) as usize;
    let lo = start.saturating_sub(win / 2);
    let hi = (start + win).min(env.len());
    if lo >= hi {
        return None;
    }
    let (peak_i, peak) = env[lo..hi]
        .iter()
        .enumerate()
        .fold((0usize, 0.0f32), |acc, (i, &v)| {
            if v > acc.1 {
                (i + lo, v)
            } else {
                acc
            }
        });
    if peak <= 1.0e-6 {
        return None;
    }
    let mut i10 = peak_i;
    let mut i90 = peak_i;
    let mut j = peak_i;
    while j > lo {
        if env[j] <= 0.9 * peak {
            i90 = j;
            break;
        }
        j -= 1;
    }
    let mut j = i90;
    while j > lo {
        if env[j] <= 0.1 * peak {
            i10 = j;
            break;
        }
        j -= 1;
    }
    Some((i90.saturating_sub(i10) * bucket) as f32 / sample_rate as f32 * 1000.0)
}

/// Peak-to-RMS in a window around a hit. Falls when a transient is smeared.
pub fn crest_db(x: &[f32], at: usize, sample_rate: u32, window_ms: f32) -> Option<f32> {
    let half = (window_ms * 0.001 * sample_rate as f32) as usize / 2;
    let lo = at.saturating_sub(half);
    let hi = (at + half).min(x.len());
    if lo + 8 >= hi {
        return None;
    }
    let slice = &x[lo..hi];
    let peak = slice.iter().fold(0.0f32, |a, v| a.max(v.abs()));
    let r = rms(slice);
    if r <= 1.0e-9 {
        return None;
    }
    Some(db(peak) - db(r))
}

/// Dominant frequency by parabolic interpolation on a long Hann-windowed FFT.
pub fn dominant_hz(x: &[f32], sample_rate: u32, min_hz: f32) -> Option<f32> {
    let n = 65536usize;
    if x.len() < n {
        return None;
    }
    let offset = (x.len() - n) / 2;
    let window = hann(n);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| Complex::new(x[offset + i] * window[i], 0.0))
        .collect();
    fft.process_with_scratch(&mut buf, &mut scratch);
    let bins = n / 2;
    let min_bin = (min_hz * n as f32 / sample_rate as f32).ceil() as usize;
    let mut best = min_bin.max(1);
    let mut best_mag = 0.0f32;
    for k in min_bin.max(1)..bins {
        let m = buf[k].norm();
        if m > best_mag {
            best_mag = m;
            best = k;
        }
    }
    if best_mag <= 1.0e-9 || best == 0 || best + 1 >= bins {
        return None;
    }
    let a = buf[best - 1].norm().max(1.0e-12).ln();
    let b = buf[best].norm().max(1.0e-12).ln();
    let c = buf[best + 1].norm().max(1.0e-12).ln();
    let delta = 0.5 * (a - c) / (a - 2.0 * b + c);
    Some((best as f32 + delta) * sample_rate as f32 / n as f32)
}

pub fn cents(measured: f32, reference: f32) -> f32 {
    1200.0 * (measured / reference).log2()
}

/// Level-normalized long-term average spectrum in 30 log-spaced bands.
pub fn ltas(x: &[f32], sample_rate: u32) -> Vec<f32> {
    let n = 2048usize;
    let spec = stft(x, n, 1024);
    let bins = n / 2 + 1;
    let mut avg = vec![0.0f32; bins];
    if spec.is_empty() {
        return vec![0.0; 30];
    }
    for frame in &spec {
        for (k, v) in frame.iter().enumerate() {
            avg[k] += v * v;
        }
    }
    for v in avg.iter_mut() {
        *v /= spec.len() as f32;
    }
    let bands = 30usize;
    let lo_hz = 50.0f32;
    let hi_hz = 16_000.0f32.min(sample_rate as f32 * 0.45);
    let mut out = vec![0.0f32; bands];
    for (b, slot) in out.iter_mut().enumerate() {
        let f0 = lo_hz * (hi_hz / lo_hz).powf(b as f32 / bands as f32);
        let f1 = lo_hz * (hi_hz / lo_hz).powf((b + 1) as f32 / bands as f32);
        let k0 = (f0 * n as f32 / sample_rate as f32).round() as usize;
        let k1 = ((f1 * n as f32 / sample_rate as f32).round() as usize).min(bins - 1);
        let mut sum = 0.0f32;
        for k in k0..=k1.max(k0) {
            if k < bins {
                sum += avg[k];
            }
        }
        *slot = 10.0 * (sum.max(1.0e-15)).log10();
    }
    let mean = out.iter().sum::<f32>() / bands as f32;
    for v in out.iter_mut() {
        *v -= mean;
    }
    out
}

/// RMS difference between two level-normalized LTAS curves, in dB.
pub fn ltas_distance_db(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    (a.iter()
        .zip(b.iter())
        .take(n)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        / n as f32)
        .sqrt()
}

pub fn stereo_correlation(frames: &[[f32; 2]]) -> f32 {
    let n = frames.len();
    if n == 0 {
        return 1.0;
    }
    let (mut sl, mut sr) = (0.0f64, 0.0f64);
    for f in frames {
        sl += f[0] as f64;
        sr += f[1] as f64;
    }
    let (ml, mr) = (sl / n as f64, sr / n as f64);
    let (mut num, mut dl, mut dr) = (0.0f64, 0.0f64, 0.0f64);
    for f in frames {
        let a = f[0] as f64 - ml;
        let b = f[1] as f64 - mr;
        num += a * b;
        dl += a * a;
        dr += b * b;
    }
    if dl <= 1.0e-18 || dr <= 1.0e-18 {
        return 1.0;
    }
    (num / (dl.sqrt() * dr.sqrt())) as f32
}

pub fn side_mid_db(frames: &[[f32; 2]]) -> f32 {
    let mut m = 0.0f64;
    let mut s = 0.0f64;
    for f in frames {
        let mid = 0.5 * (f[0] + f[1]) as f64;
        let side = 0.5 * (f[0] - f[1]) as f64;
        m += mid * mid;
        s += side * side;
    }
    (10.0 * ((s.max(1.0e-18)) / (m.max(1.0e-18))).log10()) as f32
}

/// Second difference of the signal: flat for a smooth waveform, spiky at any
/// splice discontinuity. Used instead of a first difference because a first
/// difference is large wherever the signal is simply loud.
fn curvature(x: &[f32]) -> Vec<f32> {
    if x.len() < 3 {
        return vec![0.0; x.len()];
    }
    let mut d = vec![0.0f32; x.len()];
    for i in 1..x.len() - 1 {
        d[i] = (x[i + 1] - 2.0 * x[i] + x[i - 1]).abs();
    }
    d
}

/// Worst curvature inside `+/- radius` of `at`, in dB above the signal's own
/// 99th-percentile curvature.
///
/// The radius has to be wide: WSOLA's similarity search moves the actual join
/// by up to `search` frames either side of the nominal position, so a narrow
/// probe measures whatever happens to sit at the nominal index instead of the
/// join. Callers subtract the same statistic taken at a control position so
/// that material which is naturally spiky does not read as a glitch.
pub fn glitch_db(x: &[f32], at: usize, radius: usize) -> f32 {
    if x.len() < 32 {
        return 0.0;
    }
    let d = curvature(x);
    let lo = at.saturating_sub(radius);
    let hi = (at + radius).min(d.len());
    if lo >= hi {
        return 0.0;
    }
    let local = d[lo..hi].iter().fold(0.0f32, |a, v| a.max(*v));
    let mut sorted = d.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p99 = sorted[((sorted.len() as f32 * 0.99) as usize).min(sorted.len() - 1)];
    db(local) - db(p99.max(1.0e-9))
}

/// Fraction of total energy still sitting within `+/- 20 Hz` of `f0`, in dB.
///
/// On a pure input tone this is the single most direct read on the artifacts
/// people describe as phasiness, roughness, or warble: every one of them moves
/// energy out of the fundamental and into sidebands or splice noise. A perfect
/// stretcher scores 0 dB.
pub fn tonal_purity_db(x: &[f32], sample_rate: u32, f0: f32) -> Option<f32> {
    let n = 32768usize;
    if x.len() < n {
        return None;
    }
    let offset = (x.len() - n) / 2;
    let window = hann(n);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut scratch = vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()];
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| Complex::new(x[offset + i] * window[i], 0.0))
        .collect();
    fft.process_with_scratch(&mut buf, &mut scratch);
    let bins = n / 2;
    let hz_per_bin = sample_rate as f32 / n as f32;
    let k0 = ((f0 - 20.0) / hz_per_bin).floor().max(1.0) as usize;
    let k1 = (((f0 + 20.0) / hz_per_bin).ceil() as usize).min(bins - 1);
    let mut band = 0.0f64;
    let mut total = 0.0f64;
    for k in 1..bins {
        let e = buf[k].norm_sqr() as f64;
        total += e;
        if k >= k0 && k <= k1 {
            band += e;
        }
    }
    if total <= 1.0e-20 {
        return None;
    }
    Some((10.0 * (band / total).log10()) as f32)
}

/// Time until a steady signal's short-time level first reaches 90% of its
/// settled level. Non-zero means the algorithm fades in at note-on.
pub fn start_ramp_ms(x: &[f32], sample_rate: u32) -> f32 {
    let win = (sample_rate as f32 * 0.002) as usize;
    if x.len() < win * 20 {
        return 0.0;
    }
    let settled = rms(&x[x.len() / 2..x.len() / 2 + win * 20]);
    if settled <= 1.0e-9 {
        return 0.0;
    }
    let mut i = 0usize;
    while i + win < x.len() {
        if rms(&x[i..i + win]) >= 0.9 * settled {
            return i as f32 / sample_rate as f32 * 1000.0;
        }
        i += win / 4;
    }
    0.0
}

pub fn write_wav(path: &std::path::Path, frames: &[[f32; 2]], sample_rate: u32) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).expect("wav create");
    for f in frames {
        writer.write_sample(f[0]).unwrap();
        writer.write_sample(f[1]).unwrap();
    }
    writer.finalize().unwrap();
}
