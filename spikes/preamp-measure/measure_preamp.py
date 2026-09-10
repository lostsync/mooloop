"""Harmonic amplitude against frequency, level and drive, for one unit.

This is the surface 06-preamp-modelling.md says it wants and cannot get from
a spec sheet: not total THD but where it goes -- which harmonic, at which
frequency, at which level. A transformer-fed stage saturates from the bottom
up, so the 2nd harmonic at 80 Hz sitting well above the 2nd at 5 kHz is the
measurement that says "warm on a kick, clean on a hat" as a number.

The lowest level in the grid doubles as the linear reference: at -40 dBFS
nothing here distorts measurably, so the fundamental's level there is the
unit's frequency response, which is the voicing curve.
"""

import sys
import time

import numpy as np

import mlab
from units import PREAMPS

N = 1 << 16            # 1.37 s of analysis at 48 kHz, ~0.73 Hz bins
SETTLE = 1 << 14       # discarded before analysis, so filters and any
                       # programme-dependent stage have reached steady state
REPEATS = 4            # see mlab.stable_harmonics: these plugins glitch

FREQS = [40, 60, 80, 110, 160, 220, 320, 440, 630, 880,
         1250, 1800, 2500, 3500, 5000, 7000, 10000]
LEVELS = [-40.0, -30.0, -24.0, -18.0, -12.0, -6.0, -3.0]


def silence_floor(plugin):
    """What the unit puts out given nothing at all.

    REFERENCE_MEASUREMENTS.md asks for this first, and the reason is that
    Grip-class harmonics live near -90 dB while plenty of "analog" plugins
    hum or hiss above that, and a profile fitted to hiss is worse than a
    picked one.
    """
    y = mlab.render(plugin, np.zeros(1 << 16))
    return {"rms_db": mlab.rms_db(y),
            "peak_db": float(mlab.lin_to_db(np.max(np.abs(y))))}


def measure(slug):
    unit = PREAMPS[slug]
    path, name = unit["spec"]
    plugin = mlab.open_plugin(path, name)

    result = {"slug": slug, "note": unit["note"], "path": path, "name": name,
              "sample_rate": mlab.SR, "analysis_samples": N,
              "freqs": FREQS, "levels": LEVELS,
              "latency_samples": mlab.latency_samples(plugin),
              "silence": silence_floor(plugin),
              "base": mlab.set_params(plugin, unit["base"]),
              "settings": {}}

    for label, settings in unit["settings"]:
        applied = mlab.set_params(plugin, settings)
        rows = {}
        for freq in FREQS:
            f, k = mlab.bin_of(freq, N)
            for db in LEVELS:
                prof = mlab.stable_harmonics(plugin, f, k, N, db, SETTLE,
                                             repeats=REPEATS)
                prof["gain_db"] = prof.get("fundamental_dbfs", -200.0) - db
                # Harmonics above Nyquist alias; say which orders are real so
                # a reader never quotes a folded product as a measurement.
                prof["valid_orders"] = int(mlab.SR / 2 / f)
                rows["%d/%g" % (freq, db)] = prof
        result["settings"][label] = {"applied": applied, "rows": rows}
        print("  %-12s %s" % (slug, label), flush=True)
    return result


if __name__ == "__main__":
    for slug in sys.argv[1:]:
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc)}
            print("  %-12s ERROR %s" % (slug, exc), flush=True)
        mlab.write_json("out/preamp/%s.json" % slug, out)
