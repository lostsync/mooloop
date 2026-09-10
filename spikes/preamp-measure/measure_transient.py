"""The transient half: what a stage does to an edge rather than to a level.

`PreampVoicing::slew` is the one field the harmonic sweep cannot speak to. A
static curve cannot tell a fast transient from a loud steady tone; a slew
limit can, and Punch is specified as *not* having much of one.

Two readings, because one of them has a confound worth stating. Maximum
output slope against input amplitude is the textbook slew test -- a limited
stage's slope stops rising while its input keeps getting louder -- but any
anti-alias or bandwidth limiting also caps slope, so a low number is not
proof of a slew limiter. Crest-factor retention has no such ambiguity: it
asks what happened to the peak-to-RMS ratio of a percussive signal, which is
the thing "grabbed rather than smeared" actually means.
"""

import sys
import time

import numpy as np

import mlab
from units import PREAMPS

SR = mlab.SR
AMPS = [-24.0, -18.0, -12.0, -6.0, -3.0, 0.0]


def max_slope(y):
    """Largest sample-to-sample change, as full scale per millisecond."""
    return float(np.max(np.abs(np.diff(y))) * SR / 1000.0)


def best_render(plugin, x, tries=2):
    """Render more than once and keep the loudest, rejecting a dropout.

    Same glitch as everywhere else, and it matters more here: a dropout in
    the click train removes the very peak the measurement is about.
    """
    best = None
    for _ in range(tries):
        y = mlab.render(plugin, x)
        if best is None or np.max(np.abs(y)) > np.max(np.abs(best)):
            best = y
    return best


def crest_db(x):
    rms = np.sqrt(np.mean(np.square(x)))
    return float(mlab.lin_to_db(np.max(np.abs(x)) / max(rms, 1e-12)))


def click_train(amp_db, n, period_s=0.25, width=8):
    """Short shaped bursts: high crest factor, no DC, nothing to ring on."""
    x = np.zeros(n)
    env = np.hanning(2 * width)[width:]
    t = np.arange(width) / SR
    burst = env * np.sin(2 * np.pi * 3000.0 * t)
    for start in range(int(SR * 0.05), n - width, int(SR * period_s)):
        x[start:start + width] = burst
    return x * (mlab.db_to_lin(amp_db) / np.max(np.abs(x)))


def square(amp_db, n, freq=500.0):
    t = np.arange(n) / SR
    return mlab.db_to_lin(amp_db) * np.sign(np.sin(2 * np.pi * freq * t))


def measure(slug):
    unit = PREAMPS[slug]
    plugin = mlab.open_plugin(*unit["spec"])
    result = {"slug": slug, "note": unit["note"],
              "base": mlab.set_params(plugin, unit["base"]),
              "amps_dbfs": AMPS, "settings": {}}
    n = int(SR * 1.5)

    for label, settings in unit["settings"]:
        applied = mlab.set_params(plugin, settings)
        rows = {}
        tone_hz, _ = mlab.bin_of(1000.0, n)
        for amp in AMPS:
            sq = square(amp, n)
            sine = mlab.sine(tone_hz, amp, n)
            clicks = click_train(amp, n)
            y_sq = best_render(plugin, sq)
            y_sine = best_render(plugin, sine)
            y_cl = best_render(plugin, clicks)
            rows["%g" % amp] = {
                "square_slope_in": max_slope(sq),
                "square_slope_out": max_slope(y_sq),
                "sine_slope_in": max_slope(sine),
                "sine_slope_out": max_slope(y_sine),
                "crest_in_db": crest_db(clicks),
                "crest_out_db": crest_db(y_cl),
                "crest_change_db": crest_db(y_cl) - crest_db(clicks),
                # The square's slope ratio on its own mostly measures the
                # stage's gain. Divided by the sine's, the gain cancels and
                # what is left is whether fast edges are held back relative
                # to a slow tone -- which is what a slew limit does and a
                # static curve cannot.
                "slew_index": float(
                    (max_slope(y_sq) / max(max_slope(sq), 1e-12)) /
                    max(max_slope(y_sine) / max(max_slope(sine), 1e-12), 1e-12)),
                "click_peak_gain_db": float(
                    mlab.lin_to_db(np.max(np.abs(y_cl))) -
                    mlab.lin_to_db(np.max(np.abs(clicks)))),
            }
        result["settings"][label] = {"applied": applied, "rows": rows}
        print("  %-14s %s" % (slug, label), flush=True)
    return result


if __name__ == "__main__":
    for slug in sys.argv[1:]:
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            import traceback
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc),
                   "traceback": traceback.format_exc()}
            print("  %-14s ERROR %s" % (slug, exc), flush=True)
        mlab.write_json("out/transient/%s.json" % slug, out)
