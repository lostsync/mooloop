"""Gain-trajectory measurement for compressors.

06-preamp-modelling.md's ruling about compressors is that "the timing is most
of the sound... detector shape, attack and release curves and programme
dependence do more than the gain element's transfer curve does". So this
measures the trajectory, not a time constant: gain reduction sampled on a log
time grid, which is what tells an exponential from a two-stage opto recovery.

The gain trace is the analytic-signal envelope of the output over that of the
input. A sine's Hilbert envelope is exact and instantaneous, where a sliding
RMS would smear the fast attacks flat -- an 1176's fastest is about 20 us, and
one cycle of the 2 kHz carrier is 500 us. The carrier sets the floor on
resolution: about 0.1 ms here, so a "20 us" marking is reported as
"at or below the resolution limit" rather than invented.
"""

import numpy as np
from scipy.signal import hilbert

import mlab

CARRIER = 2000.0
# Log grid, milliseconds. Wide enough for an 1176 and a Fairchild alike.
TIME_GRID_MS = [0.1, 0.2, 0.5, 1, 2, 5, 10, 20, 50, 100, 200, 500,
                1000, 2000, 5000]


def envelope(x):
    return np.abs(hilbert(x))


def gain_trace_db(y, x):
    """Output gain over input, in dB, per sample."""
    ex, ey = envelope(x), envelope(y)
    good = ex > (np.max(ex) * 1e-3)
    g = np.full(len(x), np.nan)
    g[good] = 20.0 * np.log10(np.maximum(ey[good], 1e-12) / ex[good])
    return g


def step_signal(low_db, high_db, pre_s, hold_s, post_s, sr=mlab.SR,
                freq=CARRIER):
    """A carrier that steps up for `hold_s`, then back down."""
    f, _ = mlab.bin_of(freq, int(sr))
    n = int(sr * (pre_s + hold_s + post_s))
    t = np.arange(n) / sr
    carrier = np.sin(2 * np.pi * f * t)
    amp = np.full(n, mlab.db_to_lin(low_db))
    a, b = int(sr * pre_s), int(sr * (pre_s + hold_s))
    amp[a:b] = mlab.db_to_lin(high_db)
    return carrier * amp, a, b


def sample_curve(g, start, sr=mlab.SR, grid_ms=TIME_GRID_MS):
    """Gain at each grid time after `start`, averaged over a short window."""
    out = {}
    for ms in grid_ms:
        i = start + int(sr * ms / 1000.0)
        w = max(1, int(sr * min(ms, 5.0) / 1000.0 / 8))
        lo, hi = max(0, i - w), min(len(g), i + w + 1)
        if lo >= len(g):
            continue
        seg = g[lo:hi]
        seg = seg[~np.isnan(seg)]
        if len(seg):
            out["%g" % ms] = float(np.mean(seg))
    return out


def crossing_ms(g, start, final, fraction, sr=mlab.SR, skip_ms=0.0):
    """When the trace first reaches `fraction` of its final excursion.

    `skip_ms` ignores the moments right after the step. The analytic
    envelope rings at a sudden amplitude discontinuity, and on the release
    edge that ring crosses the target immediately -- which is how every
    Fairchild time constant first measured a release of 0.729 ms.
    """
    base = g[start]
    target = base + fraction * (final - base)
    skip = int(sr * skip_ms / 1000.0)
    start = start + skip
    seg = g[start:]
    if final < base:
        hit = np.where(seg <= target)[0]
    else:
        hit = np.where(seg >= target)[0]
    if not len(hit):
        return None
    return float(hit[0] * 1000.0 / sr)


# A gain trace may legitimately dive by the gain reduction on offer. It may
# not dive by this much: past here the plugin has dropped out, which is the
# same Waves glitch mlab.stable_harmonics rejects, and it lands in a release
# curve as a hundred dB of gain reduction that never happened.
DROPOUT_DB = 40.0


def step_response(plugin, low_db, high_db, hold_s, post_s, pre_s=1.0,
                  sr=mlab.SR, attempts=4):
    """Attack and release curves from one up-then-down step.

    Re-rendered if the trace shows a dropout rather than a compressor, so
    the glitch costs a repeat on the runs that hit it rather than four
    renders on every run that does not.
    """
    x, a, b = step_signal(low_db, high_db, pre_s, hold_s, post_s, sr)
    rejected = 0
    for attempt in range(attempts):
        y = mlab.render(plugin, x, sr)
        n = min(len(x), len(y))
        g = gain_trace_db(y[:n], x[:n])
        quiet_ref = float(np.nanmedian(g[int(a * 0.5):a - 100]))
        if np.nanmin(g[a:]) > quiet_ref - DROPOUT_DB:
            break
        rejected += 1

    # Everything is relative to the gain in force just before the step, so a
    # unit's make-up gain drops out and only the gain *reduction* is left.
    quiet = float(np.nanmedian(g[int(a * 0.5):a - 100]))
    settled = float(np.nanmedian(g[b - int(sr * 0.02):b - 100]))
    end = float(np.nanmedian(g[n - int(sr * 0.05):n - 100]))

    return {
        "rejected_runs": rejected,
        "quiet_gain_db": quiet,
        "gr_at_end_of_hold_db": quiet - settled,
        "gr_at_end_of_release_db": quiet - end,
        "attack_curve_db": {k: quiet - v
                            for k, v in sample_curve(g, a, sr).items()},
        "release_curve_db": {k: quiet - v
                             for k, v in sample_curve(g, b, sr).items()},
        "attack_t63_ms": crossing_ms(g, a, settled, 0.63, sr),
        "attack_t90_ms": crossing_ms(g, a, settled, 0.90, sr),
        # Nothing analogue releases in under a millisecond, so skipping that
        # long costs no real measurement and removes the edge artifact.
        "release_t63_ms": crossing_ms(g, b, quiet, 0.63, sr, skip_ms=1.0),
        "release_t90_ms": crossing_ms(g, b, quiet, 0.90, sr, skip_ms=1.0),
    }


def calibrate(plugin, control, values, target_gr_db, level_db, sr=mlab.SR,
              mirror=None):
    """Pick the setting whose steady-state gain reduction is nearest target.

    Units disagree about what the compression control even is -- an LA-2A has
    Peak Reduction, an 1176 has Input, a dbx 160 has Threshold -- so the only
    way to put them on a comparable footing is to drive each to the same gain
    reduction and report which marking that took.
    """
    f, _ = mlab.bin_of(CARRIER, int(sr))
    n = int(sr * 1.5)
    quiet = mlab.sine(f, level_db - 40.0, n)
    loud = mlab.sine(f, level_db, n)
    tail = slice(int(sr * 1.2), n - 100)

    def apply(v):
        # A unit with per-channel controls has to have both moved together,
        # or a linked sidechain measures the channel that was left behind.
        settings = {control: v}
        if mirror and control in mirror:
            settings[mirror[control]] = v
        mlab.set_params(plugin, settings)

    best = None
    for v in values:
        apply(v)
        ref = mlab.rms_db(mlab.render(plugin, quiet, sr)[tail]) - (level_db - 40.0)
        out = mlab.rms_db(mlab.render(plugin, loud, sr)[tail]) - level_db
        gr = ref - out
        if best is None or abs(gr - target_gr_db) < abs(best[1] - target_gr_db):
            best = (v, gr)
    apply(best[0])
    return {"control": control, "value": mlab._plain(best[0]),
            "measured_gr_db": round(best[1], 2), "target_gr_db": target_gr_db}
