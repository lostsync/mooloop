"""Load one candidate, dump its parameters, and prove it passes real audio.

A demo-limited or unlicensed plugin usually still loads and then outputs
silence, noise bursts, or the dry signal. Every number downstream depends on
this check, so it runs first and separately for every unit.
"""

import sys

import numpy as np

import mlab
from candidates import CANDIDATES


def probe(slug):
    path, name, role, note = CANDIDATES[slug]
    plugin = mlab.open_plugin(path, name)

    n = 1 << 16
    freq, k = mlab.bin_of(1000.0, n)
    result = {
        "slug": slug, "path": path, "name": name, "role": role, "note": note,
        "latency_samples": mlab.latency_samples(plugin),
        "parameters": mlab.param_table(plugin),
    }

    for db in (-30.0, -18.0, -6.0):
        x = mlab.sine(freq, db, n + (1 << 14))
        y = mlab.render(plugin, x)
        mag = mlab.spectrum(y, n)
        result["at_%g_dbfs" % db] = {
            "in_rms_db": mlab.rms_db(x),
            "out_rms_db": mlab.rms_db(y),
            "gain_db": mlab.rms_db(y) - mlab.rms_db(x),
            "noise_floor_db": mlab.noise_floor(mag, k),
            **(mlab.harmonic_profile(y, k, n) or {}),
        }

    # Bit-identical to the input means the plugin bypassed itself, which is
    # what an unauthorised one often does.
    x = mlab.sine(freq, -18.0, n)
    y = mlab.render(plugin, x)
    result["silent"] = bool(np.max(np.abs(y)) < 1e-6)
    return result


if __name__ == "__main__":
    slug = sys.argv[1]
    try:
        out = probe(slug)
        ok = not out["silent"]
    except Exception as exc:  # a plugin that will not load is a result too
        out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc)}
        ok = False
    mlab.write_json("out/probe/%s.json" % slug, out)
    print("%-16s %s" % (slug, "OK" if ok else "FAILED"))
