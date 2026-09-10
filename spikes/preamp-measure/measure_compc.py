"""A compressor measured at several depths, so the axes can be shared.

The first pass drove every unit to about 10 dB of gain reduction and measured
it there, which is the right way to compare nine units once. It is not enough
to chart them: an opto at 3 dB and the same opto at 15 dB are different
compressors, and the marked attack of an 1176 does something different at
each. So the calibration is repeated at **3, 6, 10 and 15 dB** of reduction
and the depth is an axis rather than a constant.

What is measured, in the order `06-preamp-modelling.md` ranks it:

* **timing** -- attack and release, as a t63 and t90 pair and as the whole
  log-spaced trajectory, at every marking the unit has;
* **programme dependence** -- the same release after a 30 ms note and a 3 s
  one, which is the cleanest split in the first pass's results and falls
  exactly along the gain element;
* **detector shape** -- peak or RMS, asked as a question about duty cycle;
* **the static curve** -- threshold, knee and the ratio actually delivered;
* **sidechain frequency response** -- how much reduction the same level
  produces at each frequency, which is the chart that says whether a unit
  hears bass. Nothing in the first pass asked this and it decides whether a
  compressor pumps on a kick;
* **the gain element's own distortion**, last, at each depth.

`measure_comp.py`'s helpers are imported rather than repeated; anything below
that looks like it should live there probably should.
"""

import sys
import time
import traceback

import analysis
import ctiming
import mlab
import roster
from measure_comp import (HOLD_S, POST_S, QUIET_DB, REF_DB, detector_shape,
                          mirrored, static_curve)
from voicings import voicing_of

GR_TARGETS = [3.0, 6.0, 10.0, 15.0]

# Where the timing and the character get measured. 10 dB is the first pass's
# depth, so this collection can be read against it.
PRIMARY_GR = 10.0

# One tone at one level, swept in frequency: a compressor with a sidechain
# filter, an RMS detector or a modelled transformer does not reduce a 50 Hz
# note as much as a 1 kHz one, and the difference is why some units pump.
SC_FREQS = [f for f in analysis.THIRD_OCTAVE if 31.5 <= f <= 12500]

HARMONIC_FREQS = [60, 220, 1000, 5000]


def sidechain_response(plugin, level_db=REF_DB, sr=mlab.SR):
    """Gain reduction against tone frequency at one level."""
    n = int(sr * 2.5)
    tail = slice(int(sr * 2.0), n - 100)
    out = {}
    for freq in SC_FREQS:
        f, _ = mlab.bin_of(freq, int(sr), sr)
        quiet = mlab.rms_db(
            mlab.render(plugin, mlab.sine(f, level_db - 40.0, n), sr)[tail])
        loud = mlab.rms_db(
            mlab.render(plugin, mlab.sine(f, level_db, n), sr)[tail])
        out["%g" % freq] = round((quiet - (level_db - 40.0)) - (loud - level_db), 3)
    return out


def harmonics_at(plugin, level_db=REF_DB, sr=mlab.SR):
    n = 1 << 16
    out = {}
    for freq in HARMONIC_FREQS:
        f, k = mlab.bin_of(freq, n, sr)
        out["%d" % freq] = mlab.stable_harmonics(plugin, f, k, n, level_db,
                                                 1 << 15, repeats=3, sr=sr)
    return out


def at_depth(plugin, unit, timings=False):
    """Everything worth measuring once the unit is at a stated depth."""
    out = {
        "static_curve": static_curve(plugin),
        "sidechain_response_db": sidechain_response(plugin),
        "harmonics": harmonics_at(plugin),
        "as_set": ctiming.step_response(plugin, QUIET_DB, REF_DB, HOLD_S,
                                        POST_S),
        "programme_dependence": {
            "hold_%gs" % hold: ctiming.step_response(
                plugin, QUIET_DB, REF_DB, hold, POST_S)
            for hold in (0.03, 0.3, 3.0)},
    }
    static_at_ref = out["static_curve"].get("%g" % REF_DB, {}).get("gr_db")
    out["detector"] = detector_shape(plugin, static_at_ref)

    if timings:
        out["timing"] = {}
        for axis in ("attacks", "releases"):
            spec = unit.get(axis)
            if not spec:
                continue
            control, marks = spec
            if unit.get("fixed"):
                mlab.set_params(plugin, mirrored(unit, unit["fixed"]))
            for mark in marks:
                mlab.set_params(plugin, mirrored(unit, {control: mark}))
                out["timing"]["%s=%s" % (control, mark)] = \
                    ctiming.step_response(plugin, QUIET_DB, REF_DB, HOLD_S,
                                          POST_S)
        if unit.get("fixed"):
            mlab.set_params(plugin, mirrored(unit, unit["fixed"]))
    return out


def measure(slug):
    meta = roster.UNITS[slug]
    voicing = voicing_of(slug)
    unit = voicing.get("comp")
    if not unit:
        raise KeyError("%s has no comp entry in voicings.py" % slug)
    plugin = roster.open_unit(slug)

    result = {
        "slug": slug, "measured": "comp",
        "element": unit.get("element", meta["group"].split("-")[0]),
        "sample_rate": mlab.SR, "ref_level_dbfs": REF_DB,
        "gr_targets_db": GR_TARGETS, "primary_gr_db": PRIMARY_GR,
        "grids": {"static_levels_dbfs": [-54, -48, -42, -36, -30, -27, -24,
                                         -21, -18, -15, -12, -9, -6, -3, 0],
                  "sidechain_hz": SC_FREQS,
                  "harmonic_hz": HARMONIC_FREQS,
                  "time_grid_ms": ctiming.TIME_GRID_MS},
        "latency_samples": mlab.latency_samples(plugin),
        "base": mlab.set_params(plugin, voicing["base"]),
        "depths": {},
    }
    result.update({k: meta[k] for k in
                   ("maker", "models", "family", "group", "note")})
    if unit.get("fixed"):
        mlab.set_params(plugin, mirrored(unit, unit["fixed"]))

    control, values = unit["amount"]
    for target in GR_TARGETS:
        cal = ctiming.calibrate(plugin, control, values, target, REF_DB,
                               mirror=unit.get("mirror"))
        entry = {"calibration": cal}
        # A unit that cannot reach the depth is recorded as not reaching it,
        # rather than as its nearest miss dressed up as a measurement.
        entry["reached"] = bool(abs(cal["measured_gr_db"] - target) < 1.5)
        entry.update(at_depth(plugin, unit, timings=(target == PRIMARY_GR)))
        result["depths"]["gr_%g" % target] = entry
        mlab.report("  %-20s gr %-4g  %s=%s -> %.1f dB%s"
              % (slug, target, control, cal["value"], cal["measured_gr_db"],
                 "" if entry["reached"] else "  (not reached)"))
    return result


if __name__ == "__main__":
    slugs = sys.argv[1:] or roster.by_family("comp")
    for slug in slugs:
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc),
                   "traceback": traceback.format_exc()}
            mlab.report("  %-20s ERROR %s" % (slug, str(exc)[:70]))
        mlab.write_json("out/chart/comp/%s.json" % slug, out)
