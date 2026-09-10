"""The preamp surfaces reduced to the numbers a voicing is authored from.

`PreampVoicing` wants four things, and this pulls each one out of the sweep:

* `harmonics` -- h2..h5 in dB below the fundamental, which the profile spells
  at full scale. Taken at 80 Hz, below any transformer corner, so it is the
  curve's own balance rather than the curve seen through the tilt.
* `tilt_db` -- not read off directly. 06 is explicit that nothing about a
  profile may be derived, so what comes out here is the *target*: how much
  less the top of the band distorts than the bottom in the real unit, which
  is the number a fitted `tilt_db` has to reproduce when measured back.
* `tilt_hz` -- where the frequency dependence turns over, read off the
  h2-against-frequency curve as the half-way point of the total tilt.
* the voicing curve -- the linear response, measured where nothing distorts.
"""

import json
import sys

PROFILE_HZ = 80.0      # below the corner: the curve's own balance
TOP_HZ = 5000.0        # what 06's existing verification table uses


def cell(d, label, freq, level):
    return d["settings"][label]["rows"]["%d/%g" % (freq, level)]


def tilt_corner(d, label, level):
    """Frequency at which h2 has fallen half the way to its high-band value."""
    lo = cell(d, label, int(PROFILE_HZ), level).get("h2")
    hi = cell(d, label, int(TOP_HZ), level).get("h2")
    if lo is None or hi is None or abs(hi - lo) < 3.0:
        return None
    half = lo + (hi - lo) / 2.0
    prev_f, prev_v = None, None
    for f in d["freqs"]:
        v = cell(d, label, f, level).get("h2")
        if v is None:
            continue
        if v <= half and prev_v is not None and prev_v > half:
            # Log-frequency interpolation between the straddling points.
            t = (prev_v - half) / (prev_v - v)
            return round(prev_f * (f / prev_f) ** t, 1)
        prev_f, prev_v = f, v
    return None


def level_slope(d, label, freq):
    """dB of h2 per dB of input, between -30 and -18 dBFS.

    A square-law curve gives 1.0 and a cubic 2.0, so this says which term is
    doing the work before any of it saturates.
    """
    a = cell(d, label, freq, -30.0).get("h2")
    b = cell(d, label, freq, -18.0).get("h2")
    if a is None or b is None:
        return None
    return round((b - a) / 12.0, 2)


def show(slug, level=-6.0):
    d = json.load(open("out/preamp/%s.json" % slug))
    if "error" in d:
        print(slug, "ERROR", d["error"])
        return
    print("\n===== %s -- %s" % (slug, d["note"]))
    print("  at %g dBFS in. h2..h5 at %g Hz are the profile; the tilt is the "
          "h2 difference to %g Hz." % (level, PROFILE_HZ, TOP_HZ))
    print("  %-16s %7s %7s %7s %7s | %7s %7s %7s | %7s %8s %6s"
          % ("setting", "h2", "h3", "h4", "h5", "h2-h3", "h2@5k", "tilt",
             "corner", "h2/dB", "clip"))
    for label in d["settings"]:
        lo = cell(d, label, int(PROFILE_HZ), level)
        hi = cell(d, label, int(TOP_HZ), level)
        h = [lo.get("h%d" % m) for m in (2, 3, 4, 5)]
        if any(v is None for v in h):
            continue
        tilt = lo["h2"] - hi["h2"]
        print("  %-16s %7.1f %7.1f %7.1f %7.1f | %7.1f %7.1f %7.1f | %7s %8s %6s"
              % (label, h[0], h[1], h[2], h[3], h[0] - h[1], hi["h2"], tilt,
                 tilt_corner(d, label, level) or "-",
                 level_slope(d, label, int(PROFILE_HZ)) or "-",
                 "yes" if lo.get("clipped") or hi.get("clipped") else ""))

    print("  voicing curve at %g dBFS, dB relative to 1 kHz" % d["levels"][0])
    ref_f = 1250
    print("  %-16s %s" % ("setting", " ".join("%7d" % f for f in d["freqs"])))
    for label in d["settings"]:
        base = cell(d, label, ref_f, d["levels"][0])["gain_db"]
        row = [cell(d, label, f, d["levels"][0])["gain_db"] - base
               for f in d["freqs"]]
        print("  %-16s %s" % (label, " ".join("%7.2f" % v for v in row)))


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("-")]
    level = float([a for a in sys.argv[1:] if a.startswith("-l")][0][2:]) \
        if any(a.startswith("-l") for a in sys.argv[1:]) else -6.0
    for s in args:
        show(s, level)
