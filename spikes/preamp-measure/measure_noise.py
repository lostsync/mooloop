"""Self-noise, with each unit's noise generator switched on rather than off.

Every other measurement here switches the noise out, because a hiss model
sitting on top of a harmonic sweep is a nuisance. That leaves a gap: measured
on silence with the noise off, all hundred-odd of these plugins read -200
dBFS, which is true and useless.

This is the other half. It finds the controls that make a unit hiss, hum or
switch its "analog" model in -- from the survey's parameter dump, so nothing
has to be listed by hand and nothing is missed -- turns each on, and measures
what comes out on silence and underneath a tone. The output is the noise
floor chart: how much a modelled console channel costs you when you put forty
of them in a session, and which units put mains hum in rather than hiss.

Both states are measured in the same run so the pair can be subtracted.
"""

import re
import sys
import time
import traceback

import analysis
import mlab
import roster
from voicings import voicing_of

# The controls that make a unit noisy, in the order they are worth trying.
NOISY = re.compile(r"^(noise|hiss.*|hum.*|mains|analog|analogue|noise_on"
                   r"|hiss_noise|hum_noise)$")

# Some units carry the noise inside an "analog" switch that also changes the
# audio path; those are measured and labelled, not excluded, because the
# switch is what a user actually has.
LEVELS = [None, -12.0]          # silence, and underneath a -12 dBFS tone


def _on_value(entry):
    """The setting of this control that turns the noise on.

    Choice lists are the awkward case: `Off/On` is obvious, `Off/60Hz/50Hz`
    is a mains frequency and either is "on", and a Waves list carries a
    trailing space in every value.
    """
    values = [str(v) for v in (entry.get("valid_values") or [])]
    if values:
        live = [v for v in values
                if v.strip().lower() not in ("off", "out", "false", "0.0")]
        if not live:
            return None
        # Prefer a plain On, else the first live setting.
        for v in live:
            if v.strip().lower() in ("on", "in", "true"):
                return v if entry.get("type") != "bool" else True
        return live[0]
    if entry.get("type") == "bool":
        return True
    top = entry.get("max_value")
    return float(top) if top is not None else None


def _off_value(entry):
    """The setting that turns it off, so the pair is a real before and after.

    Needed because several units carry their analogue model switched *in* by
    default and the base states here leave it that way -- measuring "off"
    as whatever the base happened to be would report no added noise for
    exactly the units that have most of it.
    """
    values = [str(v) for v in (entry.get("valid_values") or [])]
    for v in values:
        if v.strip().lower() in ("off", "out", "false"):
            return False if entry.get("type") == "bool" else v
    if entry.get("type") == "bool":
        return False
    bottom = entry.get("min_value")
    return float(bottom) if bottom is not None else None


def noise_controls(slug, off=False):
    import json
    import os
    path = "out/survey/%s.json" % slug
    if not os.path.exists(path):
        return {}
    d = json.load(open(path))
    if "error" in d:
        return {}
    out = {}
    for key, entry in d.get("parameters", {}).items():
        if not NOISY.match(key):
            continue
        value = _off_value(entry) if off else _on_value(entry)
        if value is not None:
            out[key] = value
    return out


def measure(slug):
    meta = roster.UNITS[slug]
    voicing = voicing_of(slug)
    controls = noise_controls(slug)
    if not controls:
        raise KeyError("%s has no noise, hum or analog control" % slug)

    plugin = roster.open_unit(slug)
    result = {"slug": slug, "measured": "noise", "sample_rate": mlab.SR,
              "third_octave_hz": analysis.THIRD_OCTAVE,
              "base": mlab.set_params(plugin, voicing["base"]),
              "controls_found": controls, "states": {}}
    result.update({k: meta[k] for k in
                   ("maker", "models", "family", "group", "note")})

    def take(label):
        return {"silence": analysis.noise_report(plugin, seconds=6.0),
                "under_tone_-12": analysis.noise_report(plugin, seconds=6.0,
                                                        tone_db=-12.0)}

    result["states"]["off"] = {
        "applied": mlab.set_params(plugin, noise_controls(slug, off=True)),
        **take("off")}
    result["states"]["on"] = {
        "applied": mlab.set_params(plugin, controls), **take("on")}

    off = result["states"]["off"]["silence"]["rms_dbfs"]
    on = result["states"]["on"]["silence"]["rms_dbfs"]
    result["noise_added_db"] = round(on - off, 2)
    mlab.report("  %-22s off %7.1f  on %7.1f dBFS  (%s)"
                % (slug, off, on, ", ".join("%s=%s" % kv
                                            for kv in controls.items())))
    return result


if __name__ == "__main__":
    slugs = sys.argv[1:] or sorted(roster.UNITS)
    for slug in slugs:
        if not noise_controls(slug):
            continue
        t0 = time.time()
        try:
            out = measure(slug)
            out["seconds"] = round(time.time() - t0, 1)
        except Exception as exc:
            out = {"slug": slug, "error": "%s: %s" % (type(exc).__name__, exc),
                   "traceback": traceback.format_exc()}
            mlab.report("  %-22s ERROR %s" % (slug, str(exc)[:70]))
        mlab.write_json("out/chart/noise/%s.json" % slug, out)
