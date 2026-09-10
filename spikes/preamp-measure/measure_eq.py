"""Two things about an EQ that a biquad table does not tell you.

**The curve.** Analogue EQ curves are not textbook biquads: bandwidth moves
with gain, shelves have overshoot, and a "1.5 Q" marking means whatever the
unit's designer decided. Measured at -40 dBFS, where nothing here distorts,
the fundamental's level across frequency *is* the curve.

**Where the distortion lives.** 06 says an inductor EQ's nonlinearity sits
*inside* the frequency-shaping network, so its distortion should be
band-dependent rather than broadband. That is a testable claim: sweep a sine
at a working level with one band boosted, and if the distortion tracks the
boosted band rather than the whole spectrum, the claim holds and the same
filter-sandwich structure the preamp uses covers the EQ too.
"""

import sys
import time

import mlab
from eq_units import EQS

N = 1 << 16
SETTLE = 1 << 14
REPEATS = 4

# Log grid, 20 Hz to 20 kHz, dense enough to read a bell's width off.
CURVE_FREQS = [round(20.0 * (10 ** (i / 24.0)), 1) for i in range(73)]
CURVE_LEVEL = -40.0

DIST_FREQS = [40, 60, 80, 110, 160, 220, 320, 440, 630, 880,
              1250, 1800, 2500, 3500, 5000, 7000, 10000]
DIST_LEVELS = [-18.0, -12.0, -6.0]


def curve(plugin):
    """Magnitude response, from the fundamental at a level nothing distorts."""
    out = {}
    for freq in CURVE_FREQS:
        f, k = mlab.bin_of(freq, N)
        prof = mlab.stable_harmonics(plugin, f, k, N, CURVE_LEVEL, SETTLE,
                                     REPEATS)
        out["%g" % freq] = round(prof["fundamental_dbfs"] - CURVE_LEVEL, 3)
    return out


def distortion(plugin):
    out = {}
    for freq in DIST_FREQS:
        f, k = mlab.bin_of(freq, N)
        for db in DIST_LEVELS:
            prof = mlab.stable_harmonics(plugin, f, k, N, db, SETTLE, REPEATS)
            prof["valid_orders"] = int(mlab.SR / 2 / f)
            out["%d/%g" % (freq, db)] = prof
    return out


def measure(slug):
    unit = EQS[slug]
    path, name = unit["spec"]
    plugin = mlab.open_plugin(path, name)
    result = {"slug": slug, "note": unit["note"], "path": path, "name": name,
              "curve_freqs": CURVE_FREQS, "curve_level": CURVE_LEVEL,
              "dist_freqs": DIST_FREQS, "dist_levels": DIST_LEVELS,
              "base": mlab.set_params(plugin, unit["base"]), "settings": {}}
    for label, settings in unit["settings"]:
        applied = mlab.set_params(plugin, settings)
        entry = {"applied": applied, "curve": curve(plugin)}
        if label in unit.get("distortion_settings", []):
            entry["distortion"] = distortion(plugin)
        result["settings"][label] = entry
        print("  %-16s %s" % (slug, label), flush=True)
    return result


if __name__ == "__main__":
    for slug in sys.argv[1:]:
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc)}
            print("  %-16s ERROR %s" % (slug, exc), flush=True)
        mlab.write_json("out/eq/%s.json" % slug, out)
