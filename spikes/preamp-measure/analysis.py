"""Measurement primitives the first pass did not need, all chart-shaped.

`mlab` answers "what harmonics does this unit make at this one tone". Every
function here answers a question a reader wants plotted against something:
response against frequency, distortion against level, intermodulation against
level, noise against band, speed against time. It imports `mlab` and adds
nothing that is already there.

Two conventions carry over unchanged and matter for reading any of it:
amplitudes are dBFS peak, and -12 dBFS is mooloop's operating level while
-18 dBFS is the studio one the plugins are calibrated to.
"""

import numpy as np

import mlab

SR = mlab.SR

# One frequency grid, used by every curve in the collection so two units are
# always sampled at the same places and a chart can subtract them. It is the
# grid the first pass used for EQ curves: 1/24 decade, 20 Hz to 20 kHz.
CURVE_FREQS = [round(20.0 * (10 ** (i / 24.0)), 1) for i in range(73)]

# A third-octave grid for anything read as a spectrum rather than a curve.
# ISO 266 preferred centres rather than exact powers of two, because a
# third-octave chart labelled 31.2 / 39.4 / 49.6 reads as broken to anyone who
# has looked at an analyser.
THIRD_OCTAVE = [20, 25, 31.5, 40, 50, 63, 80, 100, 125, 160, 200, 250, 315,
                400, 500, 630, 800, 1000, 1250, 1600, 2000, 2500, 3150, 4000,
                5000, 6300, 8000, 10000, 12500, 16000, 20000]


def complex_spectrum(y, n):
    """Rectangular-window complex spectrum of the last n samples.

    Rectangular on purpose: every tone in this rig is placed on an exact FFT
    bin, so there is nothing to leak and a window would only widen the skirts
    that the sideband measurements read.
    """
    seg = y[-n:]
    if len(seg) < n:
        seg = np.pad(seg, (n - len(seg), 0))
    return np.fft.rfft(seg) * (2.0 / n)


# ------------------------------------------------------- swept sine (Farina)


def log_sweep(f1, f2, seconds, db, sr=SR):
    """An exponential sine sweep and the inverse filter that deconvolves it.

    One render of this gives the whole magnitude *and phase* response, where
    the tone-by-tone method needs one render per frequency. The exponential
    rate is what separates the harmonics: a unit's Nth harmonic response
    arrives at a known time before the linear one, so the same render also
    carries distortion against frequency.

    Farina 2000. The inverse's 1/f envelope is what makes the deconvolved
    linear response flat rather than pink.
    """
    n = int(round(seconds * sr))
    t = np.arange(n) / sr
    w1, w2 = 2 * np.pi * f1, 2 * np.pi * f2
    k = seconds * w1 / np.log(w2 / w1)
    sweep = np.sin(k * (np.exp(t / seconds * np.log(w2 / w1)) - 1.0))
    # Half-cosine fades: a sweep that starts or stops on a discontinuity puts
    # a click through the unit, and a click measures as broadband distortion.
    fade_in, fade_out = int(sr * 0.02), int(sr * 0.05)
    sweep[:fade_in] *= 0.5 * (1 - np.cos(np.pi * np.arange(fade_in) / fade_in))
    sweep[-fade_out:] *= 0.5 * (1 + np.cos(np.pi * np.arange(fade_out) / fade_out))
    sweep *= mlab.db_to_lin(db)

    # The time-reversed sweep needs a +6 dB/octave emphasis to undo the
    # sweep's own pink energy distribution. In the *reversed* sweep the high
    # frequencies are at the start, so the envelope is largest at t=0 and
    # decays with t -- indexing this the other way round tilts every response
    # by twelve dB per octave and looks entirely plausible while doing it.
    inverse = sweep[::-1] * np.exp(-t / seconds * np.log(w2 / w1))
    inverse *= 1.0 / mlab.db_to_lin(db)
    return sweep, inverse, k


def _halfwin(n, fade):
    """Rectangular with a faded tail: keeps the onset, kills the wrap."""
    w = np.ones(n)
    f = max(1, min(fade, n // 4))
    w[-f:] = 0.5 * (1 + np.cos(np.pi * np.arange(f) / f))
    return w


def sweep_response(plugin, f1=10.0, f2=22000.0, seconds=6.0, db=-40.0,
                   sr=SR, freqs=None):
    """Magnitude, phase and group delay against frequency, from one render.

    Normalised to 1 kHz, so what comes back is a response rather than a level
    and two units can be laid over each other.

    The same render also carries the harmonics, at known offsets before the
    linear impulse -- and they are deliberately not read here. Getting their
    *amplitudes* right needs a normalisation this does not do, the tone grid
    measures the same thing correctly for a few seconds more, and a number
    that is wrong by an unknown factor is worse in a data set than no number.
    Every response below is checked against that grid; see `sweep_agreement`.
    """
    freqs = freqs or CURVE_FREQS
    sweep, inverse, _ = log_sweep(f1, f2, seconds, db, sr)
    pre = int(sr * 0.1)
    x = np.concatenate([np.zeros(pre), sweep, np.zeros(int(sr * 1.0))])
    y = mlab.render(plugin, x, sr)
    y = y[:len(x)] if len(y) >= len(x) else np.pad(y, (0, len(x) - len(y)))
    if np.max(np.abs(y)) < 1e-9:
        return None

    nfft = 1 << int(np.ceil(np.log2(len(x) + len(inverse))))
    ir = np.fft.irfft(np.fft.rfft(y, nfft) * np.fft.rfft(inverse, nfft), nfft)

    # The linear impulse sits at the sweep's own length; the Nth harmonic sits
    # earlier by dt_N = T*ln(N)/ln(f2/f1).
    def dt(m):
        return seconds * np.log(m) / np.log(f2 / f1)

    t_lin = pre + int(sr * seconds)
    lo = max(0, t_lin - 256)
    peak = lo + int(np.argmax(np.abs(ir[lo:t_lin + 8192])))
    half = min(int(sr * dt(2) * 0.5), 1 << 15)
    lin = ir[max(0, peak - 512):peak + half]
    spec = np.fft.rfft(lin * _halfwin(len(lin), 1024), 1 << 17)
    df = sr / float(1 << 17)
    mag = np.abs(spec)
    ph = np.unwrap(np.angle(spec))
    gd = -np.gradient(ph, 2 * np.pi * df) * 1000.0

    i1k = int(round(1000.0 / df))
    ref_db = float(mlab.lin_to_db(mag[i1k]))

    out = {"ref_1k_db": round(ref_db, 3), "magnitude_db": {},
           "phase_deg": {}, "group_delay_ms": {},
           "sweep": {"f1": f1, "f2": f2, "seconds": seconds,
                     "level_dbfs": db, "normalised_at_hz": 1000.0}}
    for f in freqs:
        i = int(round(f / df))
        if i >= len(mag):
            continue
        key = "%g" % f
        out["magnitude_db"][key] = round(float(mlab.lin_to_db(mag[i])) - ref_db, 3)
        out["phase_deg"][key] = round(float((np.degrees(ph[i]) + 180.0) % 360.0 - 180.0), 2)
        out["group_delay_ms"][key] = round(float(gd[i]), 4)

    return out


# ------------------------------------------------------------------ tone grids


def tone_response(plugin, freqs, db, n=1 << 16, settle=1 << 14, repeats=3,
                  sr=SR):
    """Gain at each frequency, tone by tone: the swept sine's check.

    Slow, and the reason it is here anyway is that a sweep is one render and
    one render is exactly what the Waves glitch ruins. Run on a sparse grid,
    it says whether the dense sweep can be believed for this unit.
    """
    out = {}
    for f in freqs:
        fx, k = mlab.bin_of(f, n, sr)
        prof = mlab.stable_harmonics(plugin, fx, k, n, db, settle,
                                     repeats=repeats, sr=sr)
        out["%g" % f] = round(prof["fundamental_dbfs"] - db, 3)
    return out


def sweep_agreement(swept, tones, ref_hz=1000.0):
    """How far the dense sweep is from the tones, in dB, at their own points.

    Recorded beside every response in the collection. Both are normalised to
    1 kHz first, so this compares shapes and not levels; anything much above
    a few tenths of a dB means the sweep's one render was the one that
    glitched and the tone grid is the curve to use.
    """
    if not swept or not tones:
        return None
    if "%g" % ref_hz not in tones:
        return None
    ref = tones["%g" % ref_hz]
    mag = swept["magnitude_db"]
    keys = sorted(float(k) for k in mag)
    worst, at = 0.0, None
    rows = {}
    for key, value in tones.items():
        f = float(key)
        near = min(keys, key=lambda k: abs(k - f))
        if abs(near - f) > f * 0.06:
            continue
        diff = mag["%g" % near] - (value - ref)
        rows[key] = round(diff, 3)
        if abs(diff) > abs(worst):
            worst, at = diff, key
    return {"worst_db": round(worst, 3), "worst_at_hz": at, "per_point": rows}


def harmonics_grid(plugin, freqs, levels, n=1 << 16, settle=1 << 14,
                   repeats=4, sr=SR):
    """The harmonic surface: every harmonic at every frequency and level."""
    rows = {}
    for f in freqs:
        fx, k = mlab.bin_of(f, n, sr)
        for db in levels:
            prof = mlab.stable_harmonics(plugin, fx, k, n, db, settle,
                                         repeats=repeats, sr=sr)
            prof["gain_db"] = round(prof.get("fundamental_dbfs", -200.0) - db, 3)
            prof["valid_orders"] = int(sr / 2 / fx)
            rows["%g/%g" % (f, db)] = prof
    return rows


# ------------------------------------------------------------ intermodulation


def _two_tone(f1, f2, ratio_db, peak_db, n, sr=SR):
    """Two tones whose *sum* peaks at `peak_db`, f1 louder by `ratio_db`."""
    t = np.arange(n) / sr
    x = (np.sin(2 * np.pi * f1 * t)
         + mlab.db_to_lin(-ratio_db) * np.sin(2 * np.pi * f2 * t))
    return x * (mlab.db_to_lin(peak_db) / np.max(np.abs(x)))


def imd_smpte(plugin, peak_db, n=1 << 17, settle=1 << 14, sr=SR, repeats=2):
    """SMPTE/DIN intermodulation: 60 Hz and 7 kHz mixed 4:1.

    The number every analogue spec sheet quotes, and the one that catches a
    nonlinearity a harmonic sweep can miss -- a stage whose gain moves with
    the low end puts sidebands on the high tone, which is audible as
    intermodulation long before its own harmonics are.
    """
    f1, _ = mlab.bin_of(60.0, n, sr)
    f2, k2 = mlab.bin_of(7000.0, n, sr)
    k1 = int(round(f1 * n / sr))
    best = None
    for _ in range(repeats):
        x = _two_tone(f1, f2, 12.04, peak_db, n + settle, sr)
        y = mlab.render(plugin, x, sr)
        mag = np.abs(complex_spectrum(y, n))
        carrier = float(mag[k2])
        if carrier <= 0:
            continue
        side, pairs = 0.0, {}
        for order in (1, 2, 3, 4, 5):
            lo, hi = k2 - order * k1, k2 + order * k1
            if lo < 0 or hi >= len(mag):
                continue
            e = float(mag[lo]) ** 2 + float(mag[hi]) ** 2
            side += e
            pairs["order_%d" % order] = round(
                float(mlab.lin_to_db(np.sqrt(e) / carrier)), 2)
        row = {"peak_in_dbfs": peak_db,
               "carrier_dbfs": round(float(mlab.lin_to_db(carrier)), 2),
               "imd_pct": round(100.0 * float(np.sqrt(side)) / carrier, 5),
               "imd_db": round(float(mlab.lin_to_db(np.sqrt(side) / carrier)), 2),
               "sidebands_db": pairs}
        if best is None or row["carrier_dbfs"] > best["carrier_dbfs"]:
            best = row
    return best


def imd_ccif(plugin, peak_db, n=1 << 17, settle=1 << 14, sr=SR, repeats=2):
    """CCIF/ITU-R twin-tone: 19 kHz and 20 kHz at equal amplitude.

    Reads what SMPTE cannot. Both tones and every harmonic of either are
    above the band, so the 1 kHz difference tone that appears there was made
    by the unit and nothing else -- and a plugin that does not oversample
    shows its aliasing here first.
    """
    f1, k1 = mlab.bin_of(19000.0, n, sr)
    f2, k2 = mlab.bin_of(20000.0, n, sr)
    d = k2 - k1
    best = None
    for _ in range(repeats):
        x = _two_tone(f1, f2, 0.0, peak_db, n + settle, sr)
        y = mlab.render(plugin, x, sr)
        mag = np.abs(complex_spectrum(y, n))
        ref = float(np.sqrt(float(mag[k1]) ** 2 + float(mag[k2]) ** 2))
        if ref <= 0:
            continue
        out, power = {}, 0.0
        for label, idx in (("d2_1k", d), ("d3_18k", k1 - d), ("d3_21k", k2 + d)):
            if idx <= 0 or idx >= len(mag):
                continue
            out[label] = round(float(mlab.lin_to_db(mag[idx] / ref)), 2)
            power += float(mag[idx]) ** 2
        row = {"peak_in_dbfs": peak_db,
               "tones_dbfs": round(float(mlab.lin_to_db(ref)), 2),
               "imd_pct": round(100.0 * float(np.sqrt(power)) / ref, 5),
               "imd_db": round(float(mlab.lin_to_db(np.sqrt(power) / ref)), 2),
               "products_db": out}
        if best is None or row["tones_dbfs"] > best["tones_dbfs"]:
            best = row
    return best


# --------------------------------------------------------------------- noise


def a_weight_db(f):
    """IEC 61672 A-weighting. Quoted because everyone quotes it; the
    unweighted number is reported beside it because hiss is not speech."""
    f = np.asarray(f, dtype=float)
    f2 = f * f
    num = (12194.0 ** 2) * f2 * f2
    den = ((f2 + 20.6 ** 2)
           * np.sqrt((f2 + 107.7 ** 2) * (f2 + 737.9 ** 2))
           * (f2 + 12194.0 ** 2))
    with np.errstate(divide="ignore", invalid="ignore"):
        return 20.0 * np.log10(np.where(den > 0, num / den, 1e-30)) + 2.0


def noise_report(plugin, seconds=4.0, sr=SR, tone_db=None):
    """What the unit puts out on its own, in the shape a chart wants.

    With `tone_db` set it measures the floor *under* a tone instead, because
    plenty of analogue emulations only make their noise when signal is
    passing and measure as silent on silence.
    """
    n = int(sr * seconds)
    if tone_db is None:
        x = np.zeros(n)
    else:
        f, _ = mlab.bin_of(1000.0, n, sr)
        x = mlab.sine(f, tone_db, n, sr)
    y = mlab.render(plugin, x, sr)
    y = y[int(sr * 0.5):]
    if len(y) < 4096:
        return None
    if tone_db is not None:
        # Notch the tone and its harmonics out rather than measuring them.
        spec = np.fft.rfft(y)
        bins = np.fft.rfftfreq(len(y), 1.0 / sr)
        for m in range(1, 21):
            spec[np.abs(bins - m * 1000.0) < 8.0] = 0.0
        y = np.fft.irfft(spec, len(y))

    nfft = 1 << int(np.floor(np.log2(len(y))))
    seg = y[:nfft] * np.blackman(nfft)
    mag = np.abs(np.fft.rfft(seg)) * (2.0 / (nfft * 0.42))
    bins = np.fft.rfftfreq(nfft, 1.0 / sr)
    aw = mlab.db_to_lin(a_weight_db(np.maximum(bins, 1.0)))

    out = {
        "under_tone_dbfs": tone_db,
        "rms_dbfs": round(mlab.rms_db(y), 2),
        "peak_dbfs": round(float(mlab.lin_to_db(np.max(np.abs(y)))), 2),
        "a_weighted_dbfs": round(float(mlab.lin_to_db(
            np.sqrt(np.sum((mag * aw) ** 2) / 2.0))), 2),
        "third_octave_dbfs": {},
        "tones": [],
    }
    for fc in THIRD_OCTAVE:
        sel = (bins >= fc / 2 ** (1 / 6.0)) & (bins < fc * 2 ** (1 / 6.0))
        if np.any(sel):
            out["third_octave_dbfs"]["%g" % fc] = round(
                float(mlab.lin_to_db(np.sqrt(np.sum(mag[sel] ** 2) / 2.0))), 2)

    # Discrete tones -- hum, whine, a stuck oscillator -- named rather than
    # smeared into a band, since one of them is a defect and hiss is not.
    audible = bins > 20.0
    if np.any(audible) and out["rms_dbfs"] > -180.0:
        med = float(np.median(mag[audible]))
        seen = []
        for i in sorted(np.argsort(mag)[::-1][:400]):
            if bins[i] < 20.0 or mag[i] < med * 30.0:
                continue
            if seen and abs(bins[i] - seen[-1][0]) < 5.0:
                continue
            seen.append((float(bins[i]), float(mlab.lin_to_db(mag[i]))))
        out["tones"] = [{"hz": round(f, 1), "dbfs": round(d, 2)}
                        for f, d in sorted(seen, key=lambda p: -p[1])[:8]]
    return out


# --------------------------------------------------------------- wow & flutter


def wow_flutter(plugin, freq=3150.0, seconds=12.0, db=-12.0, sr=SR):
    """Speed stability, the way a tape machine is specified.

    3150 Hz is the standard tone. The instantaneous frequency comes from the
    derivative of the analytic phase; what is left after the mean is removed
    is wow (below ~4 Hz), flutter (4-100 Hz) and scrape (above). Reported
    unweighted RMS and peak in percent, plus the modulation spectrum, which
    is the chart -- two machines with the same 0.05% number can wobble at
    completely different rates.
    """
    from scipy.signal import hilbert

    f, _ = mlab.bin_of(freq, int(sr), sr)
    n = int(sr * seconds)
    y = mlab.render(plugin, mlab.sine(f, db, n, sr), sr)
    y = y[int(sr * 1.0):int(sr * (seconds - 0.5))]
    if len(y) < sr or np.max(np.abs(y)) < 1e-6:
        return None
    inst = np.gradient(np.unwrap(np.angle(hilbert(y)))) * sr / (2 * np.pi)
    inst = inst[sr // 10:-(sr // 10)]
    mean = float(np.mean(inst))
    if not np.isfinite(mean) or mean <= 0:
        return None
    dev = (inst - mean) / mean            # fractional speed error

    nfft = 1 << int(np.floor(np.log2(len(dev))))
    seg = dev[:nfft] * np.blackman(nfft)
    mag = np.abs(np.fft.rfft(seg)) * (2.0 / (nfft * 0.42))
    bins = np.fft.rfftfreq(nfft, 1.0 / sr)

    def band(lo, hi):
        sel = (bins >= lo) & (bins < hi)
        return round(100.0 * float(np.sqrt(np.sum(mag[sel] ** 2) / 2.0)), 5)

    spectrum = {}
    for i in range(-10, 24):             # 0.1 Hz to ~200 Hz, third-octave
        fc = 2 ** (i / 3.0)
        sel = (bins >= fc / 2 ** (1 / 6.0)) & (bins < fc * 2 ** (1 / 6.0))
        if np.any(sel):
            spectrum["%g" % round(fc, 3)] = round(
                100.0 * float(np.sqrt(np.sum(mag[sel] ** 2) / 2.0)), 6)

    return {
        "tone_hz": freq,
        "measured_hz": round(mean, 3),
        "speed_error_pct": round(100.0 * (mean - f) / f, 4),
        "total_rms_pct": round(100.0 * float(np.std(dev)), 5),
        "peak_pct": round(100.0 * float(np.max(np.abs(dev))), 5),
        "wow_pct_0p1_4hz": band(0.1, 4.0),
        "flutter_pct_4_100hz": band(4.0, 100.0),
        "scrape_pct_100_1000hz": band(100.0, 1000.0),
        "modulation_spectrum_pct": spectrum,
    }


# --------------------------------------------------------------- drive matching


def thd_at(plugin, db=-12.0, freq=1000.0, n=1 << 16, settle=1 << 14,
           repeats=3, sr=SR):
    f, k = mlab.bin_of(freq, n, sr)
    return mlab.stable_harmonics(plugin, f, k, n, db, settle, repeats=repeats,
                                 sr=sr)


def match_thd(plugin, control, lo, hi, targets, db=-12.0, freq=1000.0,
              steps=9, sr=SR, apply=None, raw=False):
    """Find the drive setting at which this unit makes a given THD.

    The comparison every one of these charts wants and none of the controls
    offer: "Drive 6" means nothing across two plugins and "1% THD" means the
    same thing everywhere. Bisection on the control, which is monotonic in
    drive over any range worth measuring.
    """
    setter = apply or (lambda v: mlab.set_params(
        plugin, {control: ("raw", v) if raw else v}))
    out = {}
    for target in targets:
        a, b, best = lo, hi, None
        for _ in range(steps):
            mid = 0.5 * (a + b)
            setter(mid)
            prof = thd_at(plugin, db, freq, sr=sr)
            thd = prof.get("thd_pct", 0.0)
            if best is None or abs(thd - target) < abs(best[1] - target):
                best = (mid, thd, prof)
            if thd < target:
                a = mid
            else:
                b = mid
        setter(best[0])
        out["thd_%g_pct" % target] = {
            "control": control, "raw": raw, "value": round(best[0], 5),
            "measured_thd_pct": round(best[1], 5),
            "reached": bool(abs(best[1] - target) < target * 0.25),
            "profile": best[2],
        }
    return out


# ------------------------------------------------------------------ transients

# These four moved here from `measure_transient.py`, which still imports them:
# they are stimulus and metric rather than measurement, and the chart suite
# needs the same click train the first pass used or the two cannot be read
# side by side.


def max_slope(y, sr=SR):
    """Largest sample-to-sample change, as full scale per millisecond."""
    return float(np.max(np.abs(np.diff(y))) * sr / 1000.0)


def crest_db(x):
    rms = np.sqrt(np.mean(np.square(x)))
    return float(mlab.lin_to_db(np.max(np.abs(x)) / max(rms, 1e-12)))


def click_train(amp_db, n, period_s=0.25, width=8, sr=SR):
    """Short shaped bursts: high crest factor, no DC, nothing to ring on."""
    x = np.zeros(n)
    env = np.hanning(2 * width)[width:]
    burst = env * np.sin(2 * np.pi * 3000.0 * (np.arange(width) / sr))
    for start in range(int(sr * 0.05), n - width, int(sr * period_s)):
        x[start:start + width] = burst
    return x * (mlab.db_to_lin(amp_db) / np.max(np.abs(x)))


def square(amp_db, n, freq=500.0, sr=SR):
    t = np.arange(n) / sr
    return mlab.db_to_lin(amp_db) * np.sign(np.sin(2 * np.pi * freq * t))


def _best_render(plugin, x, tries=2, sr=SR):
    """Render more than once and keep the loudest, rejecting a dropout.

    Same glitch as everywhere else, and it matters more here: a dropout in
    the click train removes the very peak the measurement is about.
    """
    best = None
    for _ in range(tries):
        y = mlab.render(plugin, x, sr)
        if best is None or np.max(np.abs(y)) > np.max(np.abs(best)):
            best = y
    return best


def transient_row(plugin, amp_db, n=None, sr=SR):
    """What a stage does to an edge rather than to a level, at one amplitude.

    The `slew_index` is the square's output-to-input slope ratio over the
    sine's, so the stage's gain cancels and what is left is whether fast
    edges are held back relative to a slow tone. Read it with the caveat the
    first pass established: a static saturating curve produces an index below
    1 all by itself, so a low number is distortion until something else says
    otherwise. `crest_change_db` has no such confound -- it asks what
    happened to a percussive peak, which is the question.
    """
    n = n or int(sr * 1.5)
    tone_hz, _ = mlab.bin_of(1000.0, n, sr)
    sq, sn = square(amp_db, n, sr=sr), mlab.sine(tone_hz, amp_db, n, sr)
    cl = click_train(amp_db, n, sr=sr)
    y_sq, y_sn, y_cl = (_best_render(plugin, s, sr=sr) for s in (sq, sn, cl))
    return {
        "square_slope_ratio": round(max_slope(y_sq, sr) / max(max_slope(sq, sr), 1e-12), 5),
        "sine_slope_ratio": round(max_slope(y_sn, sr) / max(max_slope(sn, sr), 1e-12), 5),
        "slew_index": round(
            (max_slope(y_sq, sr) / max(max_slope(sq, sr), 1e-12))
            / max(max_slope(y_sn, sr) / max(max_slope(sn, sr), 1e-12), 1e-12), 5),
        "crest_in_db": round(crest_db(cl), 3),
        "crest_out_db": round(crest_db(y_cl), 3),
        "crest_change_db": round(crest_db(y_cl) - crest_db(cl), 3),
        "click_peak_gain_db": round(float(
            mlab.lin_to_db(np.max(np.abs(y_cl)))
            - mlab.lin_to_db(np.max(np.abs(cl)))), 3),
        "tone_gain_db": round(float(
            mlab.lin_to_db(np.max(np.abs(y_sn)))
            - mlab.lin_to_db(np.max(np.abs(sn)))), 3),
    }


def match_gain(plugin, control, lo, hi, target_db=0.0, level_db=-40.0,
               freq=1000.0, steps=12, raw=False, sr=SR):
    """Set a level trim so the unit passes signal at unity, and say what it took.

    Measured at -40 dBFS where nothing here is distorting, so this is a gain
    match and not a loudness match. Two reasons it earns a step of its own:
    a unit left 20 dB hot clips its own output and measures as a fuzzbox,
    and a chart that compares distortion between two units is only honest if
    they were fed and read at the same level.
    """
    def setter(v):
        mlab.set_params(plugin, {control: ("raw", v) if raw else v})

    a, b, best = lo, hi, None
    for _ in range(steps):
        mid = 0.5 * (a + b)
        setter(mid)
        got = tone_response(plugin, [freq], level_db, repeats=2)["%g" % freq]
        if best is None or abs(got - target_db) < abs(best[1] - target_db):
            best = (mid, got)
        if got < target_db:
            a = mid
        else:
            b = mid
    setter(best[0])
    return {"control": control, "raw": raw, "value": round(best[0], 5),
            "measured_gain_db": round(best[1], 3),
            "target_gain_db": target_db,
            "reached": bool(abs(best[1] - target_db) < 0.5)}
