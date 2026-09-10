"""EQ curves taken at settings that mean the same thing on every unit.

The comparison an EQ chart has to survive is that no two of these units agree
about what their controls are. A Pultec has separate boost and attenuation
knobs that overlap; a Massive Passive has a unipolar gain and a three-way
direction switch; an API 550A's frequencies are stepped and its gain is
stepped too. So a "+6 dB at 500 Hz" that is set the same way everywhere does
not exist.

What does exist is a *nearest* setting, chosen per unit and written down. Each
unit's entry in `voicings.py` names, for each of seven target bands, the
frequency it can actually reach and the gain markings nearest the shared
list -- and the measurement records the setting it used, so a chart can say
"API 550A at 500 Hz" and a footnote can say the knob was on 400.

Two things per band, then:

* **the curve**, from one swept sine at -40 dBFS where nothing is
  distorting, on the same frequency grid as every other unit here;
* **the distortion**, at the band's maximum boost and at mooloop's operating
  level, which is where the first pass found the effect that matters -- an
  inductor EQ's nonlinearity is *inside* the filter, so its distortion moves
  with the band rather than sitting across the spectrum.
"""

import sys
import time
import traceback

import analysis
import mlab
import roster
from voicings import voicing_of

N = 1 << 16
SETTLE = 1 << 14
REPEATS = 4

# The seven bands every unit is asked for. A unit that has no such band is
# absent from its own list rather than faked with a neighbour.
TARGET_BANDS = {
    "low_shelf": 100.0,
    "low_bell": 100.0,
    "lomid_bell": 500.0,
    "mid_bell": 1000.0,
    "himid_bell": 3300.0,
    "high_bell": 10000.0,
    "high_shelf": 10000.0,
}

# The shared gain list. A stepped unit takes the nearest marking it has and
# the measurement writes down which.
TARGET_GAINS = [-12.0, -9.0, -6.0, -3.0, 3.0, 6.0, 9.0, 12.0]

CHECK_FREQS = [40, 100, 250, 630, 1000, 2500, 5000, 10000, 16000]
DIST_FREQS = [f for f in analysis.THIRD_OCTAVE if 31.5 <= f <= 10000]
DIST_LEVELS = [-18.0, -12.0, -6.0]


def curve(plugin, check=False):
    """One swept sine, and optionally the nine tones that check it."""
    out = {"sweep": analysis.sweep_response(plugin, db=-40.0)}
    if check:
        out["check"] = analysis.tone_response(plugin, CHECK_FREQS, -40.0,
                                              n=N, settle=SETTLE, repeats=3)
        out["agreement"] = analysis.sweep_agreement(out["sweep"], out["check"])
    return out


def measure(slug):
    unit = roster.UNITS[slug]
    voicing = voicing_of(slug)
    spec = voicing.get("eq")
    if not spec:
        raise KeyError("%s has no eq band map in voicings.py" % slug)
    plugin = roster.open_unit(slug)

    result = {
        "slug": slug, "measured": "eq",
        "sample_rate": mlab.SR, "analysis_samples": N,
        "target_bands_hz": TARGET_BANDS, "target_gains_db": TARGET_GAINS,
        "grids": {"response_hz": analysis.CURVE_FREQS,
                  "check_hz": CHECK_FREQS,
                  "dist_hz": DIST_FREQS, "dist_levels_dbfs": DIST_LEVELS},
        "curve_level_dbfs": -40.0,
        "latency_samples": mlab.latency_samples(plugin),
        "base": mlab.set_params(plugin, voicing["base"]),
        "bands": {},
    }
    result.update({k: unit[k] for k in
                   ("maker", "models", "family", "group", "note")})

    # Flat, with the full check: the reference every band curve is read
    # against, and the row that says whether the unit colours at all when it
    # is doing nothing.
    flat = spec.get("flat", {})
    result["flat"] = {"applied": mlab.set_params(plugin, flat)}
    result["flat"].update(curve(plugin, check=True))
    result["flat"]["distortion"] = analysis.harmonics_grid(
        plugin, DIST_FREQS, DIST_LEVELS, n=N, settle=SETTLE, repeats=REPEATS)
    mlab.report("  %-22s flat" % slug)

    for band in spec["bands"]:
        key = band["band"]
        rows = {}
        first = True
        for label, params in band["gains"]:
            mlab.set_params(plugin, flat)
            applied = mlab.set_params(plugin, params)
            entry = {"applied": applied}
            entry.update(curve(plugin, check=first))
            rows[label] = entry
            first = False
        result["bands"][key] = {
            "target_hz": TARGET_BANDS.get(key),
            "actual_hz": band.get("hz"),
            "marking": band.get("marking"),
            "gains": rows,
        }
        # Distortion at the band's biggest boost: where an inductor EQ puts
        # its colour, and the row that separates a nonlinearity inside the
        # filter from a shaper bolted on after it.
        if band.get("distortion_at"):
            mlab.set_params(plugin, flat)
            applied = mlab.set_params(plugin, dict(band["distortion_at"][1]))
            result["bands"][key]["distortion"] = {
                "applied": applied, "label": band["distortion_at"][0],
                "rows": analysis.harmonics_grid(
                    plugin, DIST_FREQS, DIST_LEVELS, n=N, settle=SETTLE,
                    repeats=REPEATS),
            }
        mlab.report("  %-22s %-12s %d gains%s"
              % (slug, key, len(rows),
                 " + distortion" if band.get("distortion_at") else ""))

    # Some of these units have a saturation stage that switches independently
    # of the curve, which is the controlled version of the same question.
    for label, params in spec.get("extra", []):
        mlab.set_params(plugin, flat)
        applied = mlab.set_params(plugin, params)
        entry = {"applied": applied}
        entry.update(curve(plugin, check=True))
        entry["distortion"] = analysis.harmonics_grid(
            plugin, DIST_FREQS, DIST_LEVELS, n=N, settle=SETTLE,
            repeats=REPEATS)
        result.setdefault("extra", {})[label] = entry
        mlab.report("  %-22s extra %s" % (slug, label))
    return result


if __name__ == "__main__":
    slugs = sys.argv[1:] or roster.by_family("eq")
    for slug in slugs:
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc),
                   "traceback": traceback.format_exc()}
            mlab.report("  %-22s ERROR %s" % (slug, str(exc)[:70]))
        mlab.write_json("out/chart/eq/%s.json" % slug, out)
