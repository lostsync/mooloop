"""Load every unit in the roster, prove it passes audio, write its controls.

The first pass learned this the hard way and wrote it down: an unlicensed
plugin usually loads and then outputs silence or dry signal, which measures
as a beautifully clean stage. So nothing gets measured until it has been
through here, and the parameter dump this writes is what the settings in
`voicings.py` are authored from -- a plugin's controls are not guessable and
Waves' choice strings carry a trailing space.
"""

import sys
import time
import traceback

import numpy as np

import mlab
import roster


def survey(slug):
    unit = roster.UNITS[slug]
    plugin = roster.open_unit(slug)

    n = 1 << 16
    freq, k = mlab.bin_of(1000.0, n)
    out = {"slug": slug, "name": getattr(plugin, "name", None),
           "latency_samples": mlab.latency_samples(plugin),
           "parameters": mlab.param_table(plugin)}
    out.update({key: unit[key] for key in
                ("maker", "models", "family", "group", "voice", "note")})
    out["spec"] = list(unit["spec"])

    for db in (-30.0, -18.0, -6.0):
        x = mlab.sine(freq, db, n + (1 << 14))
        y = mlab.render(plugin, x)
        mag = mlab.spectrum(y, n)
        out["at_%g_dbfs" % db] = {
            "gain_db": round(mlab.rms_db(y) - mlab.rms_db(x), 3),
            "noise_floor_db": round(mlab.noise_floor(mag, k), 2),
            **(mlab.harmonic_profile(y, k, n) or {}),
        }

    x = mlab.sine(freq, -18.0, n)
    y = mlab.render(plugin, x)
    out["silent"] = bool(np.max(np.abs(y)) < 1e-6)
    # Bit-identical output means the plugin bypassed itself, which is the
    # other thing an unauthorised one does and which measures as perfect.
    m = min(len(x), len(y))
    out["bit_identical_to_input"] = bool(
        np.max(np.abs(y[:m] - x[:m])) < 1e-9)
    return out


if __name__ == "__main__":
    slugs = sys.argv[1:] or sorted(roster.UNITS)
    ok = bad = 0
    for slug in slugs:
        t0 = time.time()
        try:
            out = survey(slug)
            out["seconds"] = round(time.time() - t0, 1)
            good = not out["silent"] and not out["bit_identical_to_input"]
            mlab.report("%-22s %-5s gain %+7.2f dB  thd %.4f%%  %d params"
                  % (slug, "OK" if good else "DEAD",
                     out["at_-18_dbfs"]["gain_db"],
                     out["at_-18_dbfs"].get("thd_pct", float("nan")),
                     len(out["parameters"])))
        except Exception as exc:
            good = False
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc),
                   "traceback": traceback.format_exc()}
            mlab.report("%-22s FAIL  %s" % (slug, str(exc)[:70]))
        ok, bad = ok + bool(good), bad + (not good)
        mlab.write_json("out/survey/%s.json" % slug, out)
    mlab.report("\n%d usable, %d not" % (ok, bad))
