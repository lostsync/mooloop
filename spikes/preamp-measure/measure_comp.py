"""What a compressor does, in the order 06-preamp-modelling.md ranks it.

That document's ruling is that timing beats the gain element: "detector
shape, attack and release curves and programme dependence do more than the
gain element's transfer curve does. Get those right first." So the timing
comes first here and the harmonics come last.

Every unit is first driven to the same gain reduction, because the units do
not agree on what the control even is -- Peak Reduction, Input, Threshold --
and a timing measured at 2 dB of reduction is not comparable with one taken
at 12.
"""

import sys
import time

import numpy as np

import ctiming
import mlab
from comp_units import COMPS

REF_DB = -12.0          # the level the unit is calibrated at
QUIET_DB = -42.0        # far enough below threshold to read the resting gain
TARGET_GR_DB = 10.0

STATIC_LEVELS = [-54, -48, -42, -36, -30, -27, -24, -21, -18,
                 -15, -12, -9, -6, -3, 0]
HOLD_S = 2.0
POST_S = 6.0            # an LA-2A's release runs to several seconds


def mirrored(unit, settings):
    """Some units have per-channel duplicates of every control."""
    out = dict(settings)
    for src, dst in (unit.get("mirror") or {}).items():
        if src in settings:
            out[dst] = settings[src]
    return out


def static_curve(plugin, sr=mlab.SR):
    """Output against input at rest: threshold, knee and the real ratio."""
    f, _ = mlab.bin_of(ctiming.CARRIER, int(sr))
    n = int(sr * 3.0)
    tail = slice(int(sr * 2.6), n - 100)
    rows = {}
    for db in STATIC_LEVELS:
        # Twice, keeping the louder: a dropout can only lose energy, and one
        # landing here would read as a compressor that suddenly got keener.
        out = max(mlab.rms_db(mlab.render(plugin, mlab.sine(f, db, n), sr)[tail])
                  for _ in range(2))
        rows["%g" % db] = {"out_db": round(out, 3), "gain_db": round(out - db, 3)}
    # Gain in force where nothing is compressing, so the rest can be read as
    # reduction rather than as level.
    ref = rows["%g" % STATIC_LEVELS[0]]["gain_db"]
    for row in rows.values():
        row["gr_db"] = round(ref - row["gain_db"], 3)
    return rows


def detector_shape(plugin, expected_gr_db=None, sr=mlab.SR, attempts=3):
    """Peak or RMS, with a cross-check that the run was not glitched.

    The continuous-tone reduction measured here has to agree with the static
    curve's reading at the same level; they are two ways of asking one
    question. When a glitch landed in this measurement it disagreed by six
    dB and looked perfectly plausible on its own, which is why the check is
    here rather than in a reader's eye.
    """
    for _ in range(attempts):
        out = _detector_once(plugin, sr)
        out["expected_gr_db"] = expected_gr_db
        if expected_gr_db is None:
            break
        out["agrees_with_static"] = bool(
            abs(out["gr_continuous_tone_db"] - expected_gr_db) < 1.0)
        if out["agrees_with_static"]:
            break
    return out


def _detector_once(plugin, sr=mlab.SR):
    """Peak-sensing or RMS-sensing, asked as a question about duty cycle.

    A tone and a bursting tone of the same peak have very different RMS. A
    peak detector reduces both about equally; an RMS detector barely notices
    the bursts. The gap between them is the answer.
    """
    f, _ = mlab.bin_of(ctiming.CARRIER, int(sr))
    n = int(sr * 4.0)
    t = np.arange(n) / sr
    carrier = np.sin(2 * np.pi * f * t)

    duty, period = 0.010, 0.050
    gate = ((t % period) < duty).astype(float)
    burst_rms_offset = 10.0 * np.log10(duty / period)

    def gr_of(x, mask):
        y = mlab.render(plugin, x, sr)
        n2 = min(len(x), len(y))
        g = ctiming.gain_trace_db(y[:n2], x[:n2])
        sel = mask[:n2] & ~np.isnan(g)
        sel[:int(sr * 2.0)] = False
        return float(np.nanmedian(g[sel]))

    whole = np.ones(n, dtype=bool)
    rest = gr_of(carrier * mlab.db_to_lin(QUIET_DB), whole)
    tone = gr_of(carrier * mlab.db_to_lin(REF_DB), whole)
    bursts = gr_of(carrier * mlab.db_to_lin(REF_DB) * gate, gate > 0.5)
    quiet_tone = gr_of(carrier * mlab.db_to_lin(REF_DB + burst_rms_offset), whole)

    return {
        "burst_duty": duty / period,
        "burst_rms_below_peak_db": round(burst_rms_offset, 2),
        "gr_continuous_tone_db": round(rest - tone, 2),
        "gr_bursts_same_peak_db": round(rest - bursts, 2),
        "gr_tone_at_burst_rms_db": round(rest - quiet_tone, 2),
    }


def harmonics_at_gr(plugin, sr=mlab.SR):
    """The gain element's own colour, at the working gain reduction.

    Ranked last on purpose. It is also where a low-frequency reading stops
    being about the gain element: at 60 Hz the detector can follow the
    waveform itself, and the distortion that produces is the envelope's, not
    the element's.
    """
    n = 1 << 16
    out = {}
    for freq in (60, 220, 1000, 5000):
        f, k = mlab.bin_of(freq, n)
        out["%d" % freq] = mlab.stable_harmonics(plugin, f, k, n, REF_DB,
                                                 1 << 15, repeats=3, sr=sr)
    return out


def measure(slug):
    unit = COMPS[slug]
    path, name = unit["spec"]
    plugin = mlab.open_plugin(path, name)
    result = {"slug": slug, "note": unit["note"], "element": unit["element"],
              "path": path, "name": name, "ref_level_dbfs": REF_DB,
              "target_gr_db": TARGET_GR_DB,
              "base": mlab.set_params(plugin, unit["base"])}

    if "fixed" in unit:
        mlab.set_params(plugin, mirrored(unit, unit["fixed"]))

    control, values = unit["amount"]
    result["calibration"] = ctiming.calibrate(plugin, control, values,
                                              TARGET_GR_DB, REF_DB,
                                              mirror=unit.get("mirror"))
    print("  %-14s calibrated %s=%s -> %.1f dB GR"
          % (slug, control, result["calibration"]["value"],
             result["calibration"]["measured_gr_db"]), flush=True)

    result["static_curve"] = static_curve(plugin)
    print("  %-14s static curve" % slug, flush=True)

    result["timing"] = {}
    # Always measure the unit as configured, so the units with no timing
    # controls at all -- which is most of the interesting ones -- still get
    # an attack and a release measured rather than only a shape.
    result["timing"]["as_set"] = ctiming.step_response(
        plugin, QUIET_DB, REF_DB, HOLD_S, POST_S)
    for axis in ("attacks", "releases"):
        spec = unit.get(axis)
        if not spec:
            continue
        ctl, marks = spec
        if "fixed" in unit:
            mlab.set_params(plugin, mirrored(unit, unit["fixed"]))
        for mark in marks:
            mlab.set_params(plugin, mirrored(unit, {ctl: mark}))
            key = "%s=%s" % (ctl, mark)
            result["timing"][key] = ctiming.step_response(
                plugin, QUIET_DB, REF_DB, HOLD_S, POST_S)
        print("  %-14s %s swept (%d)" % (slug, ctl, len(marks)), flush=True)
    if "fixed" in unit:
        mlab.set_params(plugin, mirrored(unit, unit["fixed"]))

    # Programme dependence: the same release, after being held down for
    # very different lengths of time.
    result["programme_dependence"] = {}
    for hold in (0.03, 0.3, 3.0):
        result["programme_dependence"]["hold_%gs" % hold] = \
            ctiming.step_response(plugin, QUIET_DB, REF_DB, hold, POST_S)
    print("  %-14s programme dependence" % slug, flush=True)

    static_at_ref = result["static_curve"].get("%g" % REF_DB, {}).get("gr_db")
    result["detector"] = detector_shape(plugin, static_at_ref)
    result["harmonics_at_gr"] = harmonics_at_gr(plugin)
    print("  %-14s detector and harmonics" % slug, flush=True)
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
        mlab.write_json("out/comp/%s.json" % slug, out)
