"""Everything a colour stage does, measured where two of them can be compared.

The first pass measured harmonics against frequency, level and drive, which
answered the question it was asked and leaves a reader who wants to *choose*
a plugin with nothing to plot one against another. This measures the same
units on shared axes:

* **response** -- magnitude, phase and group delay on one frequency grid,
  taken at three levels, because a transformer's low end and a tape machine's
  top end move with level and a single curve hides that.
* **thd against level** -- the headline chart. One line per unit, and the
  place a reader reads "where does this start to break up".
* **harmonics against frequency** -- the tilt: warm on a kick, clean on a
  hat, as a number.
* **intermodulation** -- SMPTE and CCIF, which say things single tones do not
  and which catch a plugin that does not oversample.
* **noise** -- on silence and under a tone, because half of these only hiss
  when signal is passing.
* **transients** -- crest retention and the slew index.

And the axis none of the controls offer: every unit with a drive control is
also measured at the settings where it makes **0.1%, 1% and 3% THD**. "Drive
6" compares nothing across two makers; equal distortion compares character.
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

# The response grid is `analysis.CURVE_FREQS` for every unit in the survey.
# Levels: -40 is below anything's nonlinearity and is the voicing curve, -12
# is mooloop's operating level, -3 is a unit being leant on.
RESPONSE_LEVELS = [-40.0, -12.0, -3.0]

# A sparse tone grid to check the swept sine against. A sweep is one render
# and one render is what the Waves glitch ruins, so every dense curve here
# has nine points measured the slow, four-times-repeated way beside it.
CHECK_FREQS = [40, 100, 250, 630, 1000, 2500, 5000, 10000, 16000]

# THD against level: the chart. 1 kHz is the reference tone and 100 Hz is
# where a transformer-fed stage does its work.
LEVEL_FREQS = [100, 1000]
LEVELS = [-60, -54, -48, -42, -36, -30, -27, -24, -21, -18,
          -15, -12, -9, -6, -3, 0, 3]

# Harmonics against frequency: third-octave, 31.5 Hz to 10 kHz. Above 10 kHz
# only the 2nd is below Nyquist and the row stops being a spectrum.
TILT_FREQS = [f for f in analysis.THIRD_OCTAVE if 31.5 <= f <= 10000]
TILT_LEVELS = [-12.0, -3.0]

IMD_LEVELS = [-30.0, -18.0, -12.0, -6.0, -3.0]
TRANSIENT_LEVELS = [-24.0, -12.0, -6.0, -3.0]

THD_TARGETS = [0.1, 1.0, 3.0]


def full_suite(plugin, tape=False):
    """Every axis, for a setting worth plotting on all of them."""
    out = {}
    out["response"], out["response_check"], out["response_agreement"] = {}, {}, {}
    for db in RESPONSE_LEVELS:
        key = "%g" % db
        out["response"][key] = analysis.sweep_response(plugin, db=db)
        out["response_check"][key] = analysis.tone_response(
            plugin, CHECK_FREQS, db, n=N, settle=SETTLE, repeats=3)
        out["response_agreement"][key] = analysis.sweep_agreement(
            out["response"][key], out["response_check"][key])
    out["thd_vs_level"] = analysis.harmonics_grid(
        plugin, LEVEL_FREQS, LEVELS, n=N, settle=SETTLE, repeats=REPEATS)
    out["harmonics_vs_freq"] = analysis.harmonics_grid(
        plugin, TILT_FREQS, TILT_LEVELS, n=N, settle=SETTLE, repeats=REPEATS)
    out["imd"] = {
        "smpte_60_7k": [analysis.imd_smpte(plugin, db) for db in IMD_LEVELS],
        "ccif_19_20k": [analysis.imd_ccif(plugin, db) for db in IMD_LEVELS],
    }
    out["noise"] = {
        "silence": analysis.noise_report(plugin),
        "under_tone_-12": analysis.noise_report(plugin, tone_db=-12.0),
    }
    out["transients"] = {"%g" % db: analysis.transient_row(plugin, db)
                         for db in TRANSIENT_LEVELS}
    if tape:
        # The one thing a tape machine has that no other unit here does, and
        # the thing two machines at the same THD still differ on.
        out["wow_flutter"] = analysis.wow_flutter(plugin)
    return out


def curve_suite(plugin):
    """The two cheap axes, for the intermediate positions of a drive knob."""
    return {
        "response": {"-40": analysis.sweep_response(plugin, db=-40.0),
                     "-12": analysis.sweep_response(plugin, db=-12.0)},
        "response_check": {"-40": analysis.tone_response(
            plugin, CHECK_FREQS, -40.0, n=N, settle=SETTLE, repeats=2)},
        "thd_vs_level": analysis.harmonics_grid(
            plugin, [1000], LEVELS, n=N, settle=SETTLE, repeats=REPEATS),
        "harmonics_vs_freq": analysis.harmonics_grid(
            plugin, TILT_FREQS, [-12.0], n=N, settle=SETTLE, repeats=REPEATS),
    }


def measure(slug):
    unit = roster.UNITS[slug]
    voicing = voicing_of(slug)
    plugin = roster.open_unit(slug)
    tape = unit["family"] == "tape"

    result = {
        "slug": slug, "measured": "colour",
        "sample_rate": mlab.SR, "analysis_samples": N,
        "operating_level_dbfs": -12.0,
        "grids": {"response_hz": analysis.CURVE_FREQS,
                  "check_hz": CHECK_FREQS,
                  "level_hz": LEVEL_FREQS, "levels_dbfs": LEVELS,
                  "tilt_hz": TILT_FREQS, "tilt_levels_dbfs": TILT_LEVELS,
                  "imd_levels_dbfs": IMD_LEVELS,
                  "transient_levels_dbfs": TRANSIENT_LEVELS},
        "latency_samples": mlab.latency_samples(plugin),
        "base": mlab.set_params(plugin, voicing["base"]),
        "settings": {},
    }
    result.update({k: unit[k] for k in
                   ("maker", "models", "family", "group", "note")})

    # A unit left 20 dB hot clips its own output and measures as a fuzzbox,
    # and Airwindows units in particular arrive at whatever trim the last
    # processor had. So the output trim is found before anything is measured.
    if voicing.get("calibrate"):
        ctl, lo, hi, target, raw = voicing["calibrate"]
        result["calibration"] = analysis.match_gain(
            plugin, ctl, lo, hi, target_db=target, raw=raw)
        mlab.report("  %-22s trimmed %s to %+.2f dB"
              % (slug, ctl, result["calibration"]["measured_gain_db"]))

    # Nothing gets measured that is not passing audio. An unlicensed or
    # mis-set plugin outputs silence, and silence measures as a perfectly
    # clean stage rather than as a failure.
    probe = analysis.tone_response(plugin, [1000.0], -18.0, repeats=2)["1000"]
    result["gain_at_1k_db"] = probe
    if probe < -80.0:
        raise RuntimeError("silent after base: gain %.1f dB at 1 kHz" % probe)

    for label, params, depth in voicing["settings"]:
        applied = mlab.set_params(plugin, params)
        entry = {"applied": applied, "depth": depth}
        entry.update(full_suite(plugin, tape) if depth == "full"
                     else curve_suite(plugin))
        result["settings"][label] = entry
        mlab.report("  %-22s %-16s %s" % (slug, label, depth))

    # The equal-distortion axis. Only reachable on a unit whose drive is a
    # continuum -- a switch cannot be bisected, and a unit that cannot make
    # 1% THD at all is recorded as not reaching it rather than as its
    # nearest miss.
    voice = voicing.get("thd_match", unit["voice"])
    if voice:
        control, lo, hi = voice[:3]
        raw = len(voice) > 3 and voice[3] == "raw"
        mlab.set_params(plugin, voicing["base"])
        matched = analysis.match_thd(plugin, control, lo, hi, THD_TARGETS,
                                     raw=raw)
        result["thd_matched"] = matched
        for key, found in matched.items():
            if not found["reached"]:
                continue
            value = ("raw", found["value"]) if raw else found["value"]
            mlab.set_params(plugin, {control: value})
            entry = {"applied": {control: found["value"]},
                     "depth": "full", "matched": key}
            entry.update(full_suite(plugin, tape))
            result["settings"]["matched_" + key] = entry
            mlab.report("  %-22s %-16s full (%s=%.4g)"
                  % (slug, "matched_" + key, control, found["value"]))
    return result


if __name__ == "__main__":
    slugs = sys.argv[1:] or [s for s, u in roster.UNITS.items()
                             if u["family"] in ("colour", "tape")]
    for slug in slugs:
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc),
                   "traceback": traceback.format_exc()}
            mlab.report("  %-22s ERROR %s" % (slug, str(exc)[:70]))
        mlab.write_json("out/chart/colour/%s.json" % slug, out)
