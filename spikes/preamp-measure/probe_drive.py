"""Which control on this unit actually drives its input stage?

A channel-strip plugin has several gains and usually only one of them reaches
the nonlinearity. Setting the wrong one produces a sweep of identical rows,
which is a result that looks like data. This asks the question directly: move
one parameter, watch the 2nd and 3rd harmonic at 80 Hz.
"""

import sys

import mlab
from units import PREAMPS

N = 1 << 16
SETTLE = 1 << 14


def sweep(slug, param, values, level=-6.0, freq=80.0, extra=None):
    unit = PREAMPS[slug]
    plugin = mlab.open_plugin(*unit["spec"])
    mlab.set_params(plugin, unit["base"])
    if extra:
        mlab.set_params(plugin, extra)
    f, k = mlab.bin_of(freq, N)
    print("== %s: %s at %g Hz, %g dBFS in%s"
          % (slug, param, freq, level, (" (%s)" % extra) if extra else ""))
    for v in values:
        try:
            mlab.set_params(plugin, {param: v})
        except Exception as exc:
            print("   %-12s SET FAILED %s" % (v, str(exc)[:90]))
            continue
        p = mlab.stable_harmonics(plugin, f, k, N, level, SETTLE, repeats=4)
        print("   %-12s h2 %8.1f  h3 %8.1f  thd %8.4f%%  out %7.2f dB  %s"
              % (v, p.get("h2", float("nan")), p.get("h3", float("nan")),
                 p.get("thd_pct", float("nan")),
                 p.get("fundamental_dbfs", float("nan")) - level,
                 "CLIPPED" if p.get("clipped") else ""))


if __name__ == "__main__":
    slug, param = sys.argv[1], sys.argv[2]
    raw = sys.argv[3:]
    values = []
    for v in raw:
        if v.startswith("raw:"):
            values.append(("raw", float(v[4:])))
            continue
        try:
            values.append(float(v))
        except ValueError:
            values.append(v.replace("_", " "))
    sweep(slug, param, values)
