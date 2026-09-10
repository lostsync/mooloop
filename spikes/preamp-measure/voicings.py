"""How each unit is put into the state it gets measured in.

One entry per unit. `base` switches out everything that is not under test --
EQ sections, dynamics, noise generators, output trims -- so what is left in
the path is the thing the chart is about. `settings` is the list of positions
that get measured, each with a depth: `full` runs every axis and `curve` runs
only response and distortion, which is a fifth of the cost and enough for the
intermediate stops on a drive knob.

Three conventions worth knowing before editing this file.

**Waves choice values carry a trailing space.** `"Off "`, not `"Off"`. That is
theirs.

**A UAD control that pedalboard types as `str` cannot be set by number.** Its
`valid_values` list is truncated at five hundred entries, so the top of the
range is unreachable by name; those are set as `("raw", x)` instead, which
`mlab.set_params` takes. Controls typed `float` carry an honest
`min_value`/`max_value` and are set directly.

**Airwindows parameters are all raw 0..1.** The consolidated build hands a
newly selected processor whatever values the previous one had rather than its
own defaults -- one of them arrived panned hard right and measured as
silence -- so every Airwindows entry sets every parameter explicitly, and
`calibrate` then trims its output to unity gain so the units are compared at
matched level rather than at matched knob positions.

The settings for units the first pass already measured are imported from
`units.py`, `eq_units.py` and `comp_units.py` rather than copied, so a
correction to one of those is a correction here too.
"""

from comp_units import COMPS
from eq_units import EQS
from units import PREAMPS

FULL, CURVE = "full", "curve"


def sweep(param, values, full=()):
    """A drive knob's stops, with the named ones measured on every axis."""
    return [("%s_%g" % (param, v), {param: v}, FULL if v in full else CURVE)
            for v in values]


def raw_sweep(param, values, full=()):
    return [("%s_%g" % (param, v), {param: ("raw", v)},
             FULL if v in full else CURVE) for v in values]


def _aw(character, unity=None, sweeps=None):
    """An Airwindows unit: every control stated, output trimmed to unity."""
    base = {k: ("raw", v) for k, v in character.items()}
    entry = {"base": base, "settings": [("mid", {}, FULL)]}
    if unity:
        entry["calibrate"] = (unity, 0.0, 1.0, 0.0, True)
    if sweeps:
        param, values, full = sweeps
        entry["settings"] = [
            ("%s_%g" % (param, v), {param: ("raw", v)},
             FULL if v in full else CURVE) for v in values]
        entry["thd_match"] = (param, 0.0, 1.0, "raw")
    return entry


VOICINGS = {}

# ======================================================== console channels

for _slug in ("nls_nevo", "nls_mike", "nls_spike"):
    VOICINGS[_slug] = {
        "base": PREAMPS[_slug]["base"],
        # 12 clips this plugin's own output and the harmonic balance
        # degenerates there; it is measured anyway and marked, because
        # "what happens if you run out of drive" is a chart too.
        "settings": sweep("drive", [0.0, 3.0, 6.0, 9.0, 12.0],
                          full=(0.0, 6.0, 12.0)),
        "thd_match": ("drive", 0.0, 12.0),
    }

for _slug, _studio in (("nlsbuss_nevo", "NEVO "), ("nlsbuss_mike", "MIKE "),
                       ("nlsbuss_spike", "SPIKE ")):
    VOICINGS[_slug] = {
        "base": {"studio": _studio, "noise": "Off ", "trim": 0.0,
                 "vca_group": "None "},
        "settings": sweep("drive", [0.0, 3.0, 6.0, 9.0, 12.0],
                          full=(0.0, 6.0, 12.0)),
        "thd_match": ("drive", 0.0, 12.0),
    }

VOICINGS["scheps73"] = {
    "base": PREAMPS["scheps73"]["base"],
    "settings": [("drive_off", {"preamp_drive": "Off "}, FULL),
                 ("drive_on", {"preamp_drive": "On "}, FULL)],
}

VOICINGS["ssl_ev2"] = {
    "base": PREAMPS["ssl_ev2"]["base"],
    # The output trim cancels the line gain, so the drive axis moves what the
    # stage sees without walking the plugin's own output into clipping.
    "settings": [("line_%g" % g, {"line": g, "output": -g},
                  FULL if g in (0.0, 10.0) else CURVE)
                 for g in (-10.0, 0.0, 10.0, 20.0)]
                + [("analog_off", {"analog": "Off ", "line": 0.0,
                                   "output": 0.0}, FULL)],
}

VOICINGS["bx_console_n"] = {
    "base": PREAMPS["bx_console_n"]["base"],
    "settings": [("thd_%g" % t, {"thd": "%.1f" % t},
                  FULL if t in (-60.0, -30.0) else CURVE)
                 for t in (-60.0, -50.0, -40.0, -35.0, -30.0)],
}

VOICINGS["ssl_native"] = {
    "base": {"eq_in": "Out", "dynamics_in": "Out", "filters_in": "Out",
             "input_trim_db": 0.0, "output_trim_db": 0.0,
             "fader_level_db": 0.0},
    "settings": [("default", {}, FULL),
                 ("trim_+10", {"input_trim_db": 10.0,
                               "output_trim_db": -10.0}, FULL)],
}

VOICINGS["api_vision"] = {
    # Everything but the line amp switched out: EQ, compressor, gate and the
    # filters all have their own On controls and all of them start off.
    "base": {"power": True, "master_bypass": False, "input_select": "Line",
             "pad": False, "phase": "Normal", "cut_filter": "Off",
             "eq_on": False, "225_on": False, "235_on": False,
             "215_on": False, "line_gain": 0.0, "level": ("raw", 1.0)},
    "settings": sweep("line_gain", [0.0, 3.0, 6.0, 9.0, 12.0],
                      full=(0.0, 6.0, 12.0)),
    "thd_match": ("line_gain", 0.0, 12.0),
}

VOICINGS["century"] = {
    "base": {"power": True, "master_bypass": False, "input_select": "Line",
             "pad": 0.0, "phase": "Normal", "gain": "Low", "eq_in": "Out",
             "gr_in": "Out", "low_cut": "Off", "level": 0.0},
    "settings": sweep("level", [0.0, 2.5, 5.0, 7.5, 10.0],
                      full=(0.0, 5.0, 10.0)),
    "thd_match": ("level", 0.0, 10.0),
    # `output` is one of the string-typed UAD controls, so the trim that
    # brings the stage back to unity has to be found rather than named.
    "calibrate": ("output", 0.0, 1.0, 0.0, True),
}

# ============================================================== preamps

VOICINGS["ua610a"] = {
    "base": {"power": True, "master_bypass": False, "input_select": "Line",
             "input_pad": "Off", "low_eq_gain": 0.0, "hi_eq_gain": 0.0,
             "polarity": 0.0, "input_gain": "Hi"},
    # `level` is the tube drive and is string-typed, so the sweep is raw.
    "settings": raw_sweep("level", [0.2, 0.4, 0.6, 0.8, 1.0],
                          full=(0.2, 0.6, 1.0)),
    "thd_match": ("level", 0.0, 1.0, "raw"),
    "calibrate": ("output", 0.0, 1.0, 0.0, True),
}

VOICINGS["ua610b"] = {
    "base": {"power": True, "master_bypass": False, "input_select": "Line",
             "input_pad": "Off", "lo_eq_gain": 0.0, "hi_eq_gain": 0.0,
             "polarity": "In 0", "input_gain": 0.0, "level": 0.0},
    "settings": sweep("level", [0.0, 2.5, 5.0, 7.5, 10.0],
                      full=(0.0, 5.0, 10.0)),
    "thd_match": ("level", 0.0, 10.0),
    "calibrate": ("output", 0.0, 1.0, 0.0, True),
}

# ========================================================= broadband colour

for _style in ("british", "american", "german", "magnetic", "clip"):
    VOICINGS["frontdaw_" + _style] = {
        "base": PREAMPS["frontdaw_" + _style]["base"],
        "settings": sweep("mojo", [0.0, 25.0, 50.0, 75.0, 100.0],
                          full=(0.0, 50.0, 100.0)),
        "thd_match": ("mojo", 0.0, 100.0),
    }

for _slug, _style in (("decap_a", "A"), ("decap_e", "E"), ("decap_n", "N"),
                      ("decap_t", "T"), ("decap_p", "P")):
    VOICINGS[_slug] = {
        # Auto-gain off on purpose: it is the whole point of the level axis
        # that the unit's own gain is visible rather than compensated.
        "base": {"style": _style, "autogain": False, "mix": 100.0,
                 "punish": False, "lowthump": False, "tone_db": 0.0,
                 "lowcut_hz": 20.0, "highcut_hz": 20000.0,
                 "outputtrim_db": 0.0, "drive": 0.0},
        "settings": sweep("drive", [0.0, 2.5, 5.0, 7.5, 10.0],
                          full=(0.0, 5.0, 10.0)),
        "thd_match": ("drive", 0.0, 10.0),
    }

VOICINGS["devil_loc"] = {
    "base": {"crunch": 1.0, "crush": 1.0},
    "settings": sweep("crush", [1.0, 3.0, 5.0, 7.0, 10.0],
                      full=(1.0, 5.0, 10.0)),
    "thd_match": ("crush", 0.0, 10.0),
}

VOICINGS["little_radiator"] = {
    "base": {"mix": 100.0, "noise": False, "bias": False, "heat_db": 0.0},
    "settings": sweep("heat_db", [-15.0, -7.5, 0.0, 7.5, 15.0],
                      full=(-15.0, 0.0, 15.0)),
    "thd_match": ("heat_db", -15.0, 15.0),
}

VOICINGS["driver"] = {
    # Filter wound fully open and every modulation route at zero, so what is
    # measured is the drive stage and not the filter in front of it.
    "base": {"filter_type": "LPF", "frequency": 35480.0, "resonance": 0.0,
             "range": "Low", "input": 0.0, "output": 0.0, "color": 5.0,
             "smooth": 0.0, "d_env_modulation": 0.0, "f_env_modulation": 0.0,
             "am_env_modulation": 0.0, "d_am_modulation": 0.0,
             "f_am_modulation": 0.0, "distortion": 0.0},
    "settings": sweep("distortion", [0.0, 2.5, 5.0, 7.5, 10.0],
                      full=(0.0, 5.0, 10.0)),
    "thd_match": ("distortion", 0.0, 10.0),
}

# ================================================================== tape

VOICINGS["studer_a800"] = {
    # Auto-cal on keeps the machine lined up as the speed changes, which is
    # what makes the three speeds comparable rather than three alignments.
    "base": {"power": True, "master_bypass": False, "auto_cal": True,
             "path_select": "Repro", "noise": False, "tape_type": "456",
             "emphasis_eq": "NAB", "cal_level": 6.0, "ips": "15 IPS",
             "input_level": 0.0, "output_level": 0.0},
    "settings": [
        # Speed is the first axis anyone asks a tape machine about: it moves
        # the head bump, the top end and the noise all at once.
        ("ips_7.5", {"ips": "7.5 IPS"}, FULL),
        ("ips_15", {"ips": "15 IPS"}, FULL),
        ("ips_30", {"ips": "30 IPS"}, FULL),
        ("ccir_15", {"ips": "15 IPS", "emphasis_eq": "CCIR"}, FULL),
        # Formulation at one speed, and then the level axis, which is what
        # "hitting the tape harder" means as a measurement.
        ("tape_250", {"ips": "15 IPS", "emphasis_eq": "NAB",
                      "tape_type": "250"}, CURVE),
        ("tape_900", {"tape_type": "900"}, CURVE),
        ("tape_gp9", {"tape_type": "GP9"}, CURVE),
        ("hot_+6", {"tape_type": "456", "input_level": 6.0,
                    "output_level": -6.0}, FULL),
        ("hot_+12", {"input_level": 12.0, "output_level": -12.0}, CURVE),
    ],
    "thd_match": ("input_level", -12.0, 24.0),
}

VOICINGS["verve"] = {
    "base": {"power": True, "master_bypass": False, "machine": "WARM",
             "param_1": 50.0, "param_2": 0.0, "output_trim": 0.0},
    # Ten machines behind two controls: the same argument for NLS applies
    # here, in that the styles are measured against each other on one rig.
    "settings": [("machine_%s" % m.lower(), {"machine": m},
                  FULL if m in ("WARM", "THICKEN", "OVERDRIVE") else CURVE)
                 for m in ("THICKEN", "VINTAGIZE", "OVERDRIVE", "EDGE",
                           "SPUTTER", "GLOW", "DISTORT", "SWEETEN", "WARM",
                           "FIRE")],
    "thd_match": ("param_1", 0.0, 100.0),
}

# ========================================================= Airwindows colour
#
# Every control stated, because a newly selected processor inherits the last
# one's values. Character controls sit at 0.5 -- the middle of the range,
# which is where Airwindows plugins are meant to be started from -- and the
# output trim is found rather than assumed, so two of them are compared at
# matched gain.

VOICINGS["aw_console7channel"] = _aw({"fader": 0.5}, unity="fader")
VOICINGS["aw_console9channel"] = _aw({"fader": 0.5, "pan": 0.5},
                                     unity="fader")
VOICINGS["aw_consolemcchannel"] = _aw(
    {"fader": 0.5, "pan": 0.5, "treble": 0.5, "mid": 0.5, "bass": 0.5},
    unity="fader")
VOICINGS["aw_consolemdchannel"] = _aw(
    {"fader": 0.5, "pan": 0.5, "treble": 0.5, "midfreq": 0.5,
     "midpeak": 0.5, "bass": 0.5}, unity="fader")
VOICINGS["aw_consolelachannel"] = _aw(
    {"fader": 0.5, "pan": 0.5, "treble": 0.5, "mid": 0.5, "bass": 0.5},
    unity="fader")
VOICINGS["aw_calibre"] = _aw(
    {"drive": 0.5, "hardns": 0.5, "persnlty": 0.5, "output": 1.0},
    unity="output", sweeps=("drive", [0.0, 0.25, 0.5, 0.75, 1.0],
                            (0.0, 0.5, 1.0)))
VOICINGS["aw_desk4"] = _aw(
    {"overdrive": 0.5, "hi_choke": 0.5, "power_sag": 0.5, "frequency": 0.5,
     "output_trim": 0.5, "dry_wet": 1.0}, unity="output_trim",
    sweeps=("overdrive", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))
VOICINGS["aw_density2"] = _aw(
    {"density": 0.5, "highpass": 0.0, "output": 0.5, "dry_wet": 1.0},
    unity="output", sweeps=("density", [0.0, 0.25, 0.5, 0.75, 1.0],
                            (0.0, 0.5, 1.0)))
VOICINGS["aw_drive"] = _aw(
    {"drive": 0.5, "highpass": 0.0, "out_level": 0.5, "dry_wet": 1.0},
    unity="out_level", sweeps=("drive", [0.0, 0.25, 0.5, 0.75, 1.0],
                               (0.0, 0.5, 1.0)))
VOICINGS["aw_purestdrive"] = _aw(
    {"drive": 0.5}, sweeps=("drive", [0.0, 0.25, 0.5, 0.75, 1.0],
                            (0.0, 0.5, 1.0)))
VOICINGS["aw_focus"] = _aw(
    {"boost": 0.5, "focus": 0.5, "mode": 0.0, "output": 1.0, "dry_wet": 1.0},
    unity="output", sweeps=("boost", [0.0, 0.25, 0.5, 0.75, 1.0],
                            (0.0, 0.5, 1.0)))
VOICINGS["aw_cojones"] = _aw(
    {"breathy": 0.5, "body": 0.5, "cojones": 0.5, "output": 1.0,
     "dry_wet": 1.0}, unity="output",
    sweeps=("cojones", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))
VOICINGS["aw_loud"] = _aw(
    {"boost": 0.5, "output_level": 0.5, "dry_wet": 1.0},
    sweeps=("boost", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))
VOICINGS["aw_hype"] = _aw({})
VOICINGS["aw_mackity"] = _aw(
    {"in_trim": 0.5, "out_pad": 0.5}, unity="out_pad",
    sweeps=("in_trim", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))

# ========================================================== Airwindows tape

VOICINGS["aw_totape7"] = _aw(
    {"encamt": 0.5, "encfreq": 0.5, "tapedrv": 0.5, "flutter": 0.5,
     "flutspd": 0.5, "bias": 0.5, "headbmp": 0.5, "headfrq": 0.5,
     "decamt": 0.5, "decfreq": 0.5},
    sweeps=("tapedrv", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))
VOICINGS["aw_tape"] = _aw(
    {"slam": 0.5, "bump": 0.5},
    sweeps=("slam", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))
VOICINGS["aw_ironoxide5"] = _aw(
    {"input_trim": 0.5, "tape_high": 0.5, "tape_low": 0.5, "flutter": 0.5,
     "noise": 0.0, "output_trim": 0.5, "inv_dry_wet": 1.0},
    unity="output_trim",
    sweeps=("input_trim", [0.25, 0.375, 0.5, 0.625, 0.75], (0.25, 0.5, 0.75)))
VOICINGS["aw_ironoxideclassic2"] = _aw(
    {"input_trim": 0.5, "tape_speed": 0.5, "output_trim": 0.5},
    unity="output_trim",
    sweeps=("input_trim", [0.25, 0.375, 0.5, 0.625, 0.75], (0.25, 0.5, 0.75)))
VOICINGS["aw_chromeoxide"] = _aw({"intense": 0.5, "bias": 0.5},
                                 sweeps=("intense", [0.0, 0.25, 0.5, 0.75, 1.0],
                                         (0.0, 0.5, 1.0)))
VOICINGS["aw_fromtape"] = _aw(
    {"softer": 0.5, "weight": 0.5, "louder": 0.5, "output": 0.5,
     "dry_wet": 1.0}, unity="output",
    sweeps=("softer", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))
VOICINGS["aw_dubly2"] = _aw(
    {"encamnt": 0.5, "encfreq": 0.5, "tapedrv": 0.5, "decamnt": 0.5,
     "decfreq": 0.5, "out_pad": 0.5, "dry_wet": 1.0}, unity="out_pad",
    sweeps=("tapedrv", [0.0, 0.25, 0.5, 0.75, 1.0], (0.0, 0.5, 1.0)))


# ==================================================================== EQ
#
# Each band names the nearest frequency the unit can actually reach and the
# gain markings nearest the shared list, and the measurement records what it
# set. Where a unit's gain control is not calibrated in dB at all -- a
# Pultec's Boost runs 0 to 11, a Massive Passive's runs 0 to 20 -- the
# markings are its own and the dB it delivers is read off the measured curve
# afterwards, which is the only way round that does not invent a number.

_API_GAINS = [-12.0, -9.0, -6.0, -4.0, 4.0, 6.0, 9.0, 12.0]


def _api_band(band, hz, freq_param, gain_param, extra=None,
              gains=_API_GAINS, marking="dB"):
    rows = []
    for g in gains:
        params = {freq_param: hz, gain_param: g}
        params.update(extra or {})
        rows.append(("%+g" % g, params))
    top = dict(extra or {})
    top.update({freq_param: hz, gain_param: max(gains)})
    return {"band": band, "hz": hz, "marking": marking, "gains": rows,
            "distortion_at": ("%+g" % max(gains), top)}


VOICINGS["api550a"] = {
    "base": {"in": "In ", "analog": "On ", "filter": "Off ",
             "polarity": "Out ", "output": 0.0,
             "low_gain": 0.0, "mid_gain": 0.0, "high_gain": 0.0,
             "lf_type": "Shelf ", "hf_type": "Shelf "},
    "eq": {
        "flat": {"low_gain": 0.0, "mid_gain": 0.0, "high_gain": 0.0},
        "bands": [
            _api_band("low_shelf", 100.0, "low_freq", "low_gain",
                      {"lf_type": "Shelf "}),
            _api_band("low_bell", 100.0, "low_freq", "low_gain",
                      {"lf_type": "Bell "}),
            # 400 Hz is this unit's nearest stop to the shared 500, and 800
            # its nearest to 1000; both are recorded rather than rounded.
            _api_band("lomid_bell", 400.0, "mid_freq", "mid_gain"),
            _api_band("mid_bell", 800.0, "mid_freq", "mid_gain"),
            _api_band("himid_bell", 3000.0, "mid_freq", "mid_gain"),
            _api_band("high_bell", 10000.0, "high_freq", "high_gain",
                      {"hf_type": "Bell "}),
            _api_band("high_shelf", 10000.0, "high_freq", "high_gain",
                      {"hf_type": "Shelf "}),
        ],
        "extra": [("analog_off", {"analog": "Off ", "mid_freq": 800.0,
                                  "mid_gain": 12.0})],
    },
}

VOICINGS["api550b"] = {
    "base": {"in": "In ", "analog": "On ", "polarity": "Out ", "output": 0.0,
             "low_gain": 0.0, "low_mid_gain": 0.0, "high_mid_gain": 0.0,
             "high_gain": 0.0, "lf_type": "Shelf ", "hf_type": "Shelf "},
    "eq": {
        "flat": {"low_gain": 0.0, "low_mid_gain": 0.0,
                 "high_mid_gain": 0.0, "high_gain": 0.0},
        "bands": [
            _api_band("low_shelf", 100.0, "low_freq", "low_gain",
                      {"lf_type": "Shelf "}),
            _api_band("low_bell", 100.0, "low_freq", "low_gain",
                      {"lf_type": "Bell "}),
            _api_band("lomid_bell", 500.0, "low_mid_freq", "low_mid_gain"),
            _api_band("mid_bell", 1000.0, "low_mid_freq", "low_mid_gain"),
            _api_band("himid_bell", 3000.0, "high_mid_freq", "high_mid_gain"),
            _api_band("high_bell", 10000.0, "high_freq", "high_gain",
                      {"hf_type": "Bell "}),
            _api_band("high_shelf", 10000.0, "high_freq", "high_gain",
                      {"hf_type": "Shelf "}),
        ],
        "extra": [("analog_off", {"analog": "Off ", "low_mid_freq": 1000.0,
                                  "low_mid_gain": 12.0})],
    },
}

VOICINGS["api_vision_eq"] = {
    "base": {"power": True, "master_bypass": False, "input_select": "Line",
             "pad": False, "phase": "Normal", "cut_filter": "Off",
             "line_gain": 0.0, "level": ("raw", 1.0),
             "eq_on": True, "eq_type": "550L", "eq_predyn": False,
             "eq_dyn_sc": False, "225_on": False, "235_on": False,
             "215_on": False,
             "550_lf_gain": 0.0, "550_lmf_gain": 0.0, "550_hmf_gain": 0.0,
             "550_hf_gain": 0.0, "550_lf_filter": "Shelf",
             "550_hf_filter": "Shelf"},
    "eq": {
        "flat": {"550_lf_gain": 0.0, "550_lmf_gain": 0.0,
                 "550_hmf_gain": 0.0, "550_hf_gain": 0.0},
        "bands": [
            _api_band("low_shelf", 100.0, "550_lf_freq", "550_lf_gain",
                      {"550_lf_filter": "Shelf"}),
            _api_band("low_bell", 100.0, "550_lf_freq", "550_lf_gain",
                      {"550_lf_filter": "Peak"}),
            _api_band("lomid_bell", 500.0, "550_lmf_freq", "550_lmf_gain"),
            _api_band("mid_bell", 1000.0, "550_lmf_freq", "550_lmf_gain"),
            _api_band("himid_bell", 3000.0, "550_hmf_freq", "550_hmf_gain"),
            _api_band("high_bell", 10000.0, "550_hf_freq", "550_hf_gain",
                      {"550_hf_filter": "Peak"}),
            _api_band("high_shelf", 10000.0, "550_hf_freq", "550_hf_gain",
                      {"550_hf_filter": "Shelf"}),
        ],
        # The 560L graphic is the same channel's other EQ and is worth a
        # curve of its own, since ten fixed bands is a different instrument.
        "extra": [("560_1k_+12", {"eq_type": "560L", "560_1khz": 12.0}),
                  ("560_1k_-12", {"eq_type": "560L", "560_1khz": -12.0})],
    },
}

VOICINGS["sieq"] = {
    "base": {"drive_db": 0.0, "low_gain_db": 0.0, "mid_gain_db": 0.0,
             "high_gain_db": 0.0, "mid_frequency": 1000.0},
    "eq": {
        "flat": {"low_gain_db": 0.0, "mid_gain_db": 0.0,
                 "high_gain_db": 0.0},
        "bands": [
            # Its low and high bands are fixed-frequency shelves; the
            # measurement reads their corners off the curve.
            {"band": "low_shelf", "hz": None, "marking": "dB", "gains":
                [("%+g" % g, {"low_gain_db": g})
                 for g in (-12.0, -9.0, -6.0, -3.0, 3.0, 6.0, 9.0, 12.0)],
             "distortion_at": ("+12", {"low_gain_db": 12.0})},
            {"band": "mid_bell", "hz": 1000.0, "marking": "dB", "gains":
                [("%+g" % g, {"mid_frequency": 1000.0, "mid_gain_db": g})
                 for g in (-8.0, -6.0, -3.0, 3.0, 6.0, 8.0)],
             "distortion_at": ("+8", {"mid_frequency": 1000.0,
                                      "mid_gain_db": 8.0})},
            {"band": "himid_bell", "hz": 3500.0, "marking": "dB", "gains":
                [("%+g" % g, {"mid_frequency": 3500.0, "mid_gain_db": g})
                 for g in (-8.0, -6.0, -3.0, 3.0, 6.0, 8.0)],
             "distortion_at": ("+8", {"mid_frequency": 3500.0,
                                      "mid_gain_db": 8.0})},
            {"band": "high_shelf", "hz": None, "marking": "dB", "gains":
                [("%+g" % g, {"high_gain_db": g})
                 for g in (-12.0, -9.0, -6.0, -3.0, 3.0, 6.0, 9.0, 12.0)],
             "distortion_at": ("+12", {"high_gain_db": 12.0})},
        ],
        "extra": [("drive_+15", {"drive_db": 15.0}),
                  ("drive_-15", {"drive_db": -15.0})],
    },
}

# ---------------------------------------------------------------- Pultec

# A Pultec's Boost and Atten knobs run 0 to 11 and are not calibrated in dB,
# so the markings below are the unit's own and the dB each delivers comes off
# the measured curve. The pair also overlap, which is why the boost-and-cut
# setting is a curve rather than a cancellation and gets an entry of its own.
_PULTEC_STOPS = [2.0, 4.0, 6.0, 8.0, 11.0]

VOICINGS["puigtec_eqp1a"] = {
    "base": {"onoff": "On ", "mains": "Off ", "gain": 0.0, "bandwidth": 0.0,
             "lowfrequency": 100.0, "hifrequency": 10000.0,
             "attenselect": 10000.0, "lowboost": 0.0, "lowatten": 0.0,
             "hiboost": 0.0, "hiatten": 0.0},
    "eq": {
        "flat": {"lowboost": 0.0, "lowatten": 0.0, "hiboost": 0.0,
                 "hiatten": 0.0},
        "bands": [
            {"band": "low_shelf", "hz": 100.0, "marking": "0-11 boost",
             "gains": [("boost_%g" % g, {"lowfrequency": 100.0,
                                         "lowboost": g})
                       for g in _PULTEC_STOPS]
                      + [("atten_%g" % g, {"lowfrequency": 100.0,
                                           "lowatten": g})
                         for g in _PULTEC_STOPS],
             "distortion_at": ("boost_11", {"lowfrequency": 100.0,
                                            "lowboost": 11.0})},
            {"band": "high_shelf", "hz": 10000.0, "marking": "0-11 boost",
             "gains": [("boost_%g" % g, {"hifrequency": 10000.0,
                                         "hiboost": g, "bandwidth": 0.0})
                       for g in _PULTEC_STOPS]
                      + [("boost_%g_narrow" % g,
                          {"hifrequency": 10000.0, "hiboost": g,
                           "bandwidth": 11.0}) for g in _PULTEC_STOPS]
                      + [("atten_%g" % g, {"attenselect": 10000.0,
                                           "hiatten": g})
                         for g in _PULTEC_STOPS],
             "distortion_at": ("boost_11", {"hifrequency": 10000.0,
                                            "hiboost": 11.0})},
        ],
        "extra": [
            # The setting the unit is famous for, and the first pass measured
            # it as a real curve: +16.6 dB at 39 Hz and -5.7 dB at 1 kHz.
            ("low_boost_and_cut", {"lowfrequency": 100.0, "lowboost": 11.0,
                                   "lowatten": 11.0}),
            ("low_20hz_boost_11", {"lowfrequency": 20.0, "lowboost": 11.0}),
            ("high_3k_boost_11", {"hifrequency": 3000.0, "hiboost": 11.0}),
        ],
    },
}

VOICINGS["puigtec_meq5"] = {
    "base": {"onoff": "On ", "mains": "Off ", "gain": 0.0,
             "lm_frequency": 500.0, "mid_frequency": 1500.0,
             "hm_frequency": 3000.0, "lm_peak": 0.0, "mid_dip": 0.0,
             "hm_peak": 0.0},
    "eq": {
        "flat": {"lm_peak": 0.0, "mid_dip": 0.0, "hm_peak": 0.0},
        "bands": [
            {"band": "lomid_bell", "hz": 500.0, "marking": "0-11 peak",
             "gains": [("peak_%g" % g, {"lm_frequency": 500.0, "lm_peak": g})
                       for g in _PULTEC_STOPS],
             "distortion_at": ("peak_11", {"lm_frequency": 500.0,
                                           "lm_peak": 11.0})},
            {"band": "mid_bell", "hz": 1000.0, "marking": "0-11 dip",
             "gains": [("dip_%g" % g, {"mid_frequency": 1000.0,
                                       "mid_dip": g})
                       for g in _PULTEC_STOPS],
             "distortion_at": ("dip_11", {"mid_frequency": 1000.0,
                                          "mid_dip": 11.0})},
            {"band": "himid_bell", "hz": 3000.0, "marking": "0-11 peak",
             "gains": [("peak_%g" % g, {"hm_frequency": 3000.0, "hm_peak": g})
                       for g in _PULTEC_STOPS],
             "distortion_at": ("peak_11", {"hm_frequency": 3000.0,
                                           "hm_peak": 11.0})},
        ],
    },
}

# ------------------------------------------------------- Massive Passive
#
# Two emulations of the same box, so the two entries below are set the same
# way on purpose: same frequency stops, same gain markings, same bandwidth,
# and the direction is a switch rather than a sign.

_MP_STOPS = [4.0, 8.0, 12.0, 20.0]


def _mp_band(band, hz, freq_p, gain_p, enable_p, shape_p, shape, bw_p,
             boost="BOOST", cut="CUT", bw=2.0, stops=None):
    # The mastering version's knob runs 0 to 11 where the mixing version's
    # runs 0 to 20, so the stops are per unit and the marking says which.
    stops = stops or _MP_STOPS
    top = max(stops)
    gains = [("boost_%g" % g, {enable_p: boost, freq_p: hz, gain_p: g,
                               shape_p: shape, bw_p: bw})
             for g in stops]
    gains += [("cut_%g" % g, {enable_p: cut, freq_p: hz, gain_p: g,
                              shape_p: shape, bw_p: bw})
              for g in stops]
    # Bandwidth is the other half of this unit's gain: the first pass found
    # the same "maximum boost" spanning 16.5 dB across the bandwidth range.
    gains += [("boost_%g_bw%g" % (top, w), {enable_p: boost, freq_p: hz,
                                            gain_p: top, shape_p: shape,
                                            bw_p: w})
              for w in (1.0, 3.0)]
    return {"band": band, "hz": hz, "marking": "0-%g knob" % top,
            "gains": gains,
            "distortion_at": ("boost_%g" % top,
                              {enable_p: boost, freq_p: hz, gain_p: top,
                               shape_p: shape, bw_p: bw})}


_MP_OFF = {"ch1loenable": "OUT", "ch1lomidenable": "OUT",
           "ch1himidenable": "OUT", "ch1hienable": "OUT"}

# The two Massive Passives are set from one list of bands so the mixing and
# mastering versions differ only where the hardware does: the frequency stops
# are the same and the gain knob is not.
_MP_BANDS = [
    ("low_shelf", 100.0, "ch1lofreq", "ch1logain", "ch1loenable",
     "ch1loshape", "SHELF", "ch1lobw"),
    ("low_bell", 100.0, "ch1lofreq", "ch1logain", "ch1loenable",
     "ch1loshape", "BELL", "ch1lobw"),
    ("lomid_bell", 560.0, "ch1lomidfreq", "ch1lomidgain", "ch1lomidenable",
     "ch1lomidshape", "BELL", "ch1lomidbw"),
    ("mid_bell", 1000.0, "ch1himidfreq", "ch1himidgain", "ch1himidenable",
     "ch1himidshape", "BELL", "ch1himidbw"),
    ("himid_bell", 3300.0, "ch1himidfreq", "ch1himidgain", "ch1himidenable",
     "ch1himidshape", "BELL", "ch1himidbw"),
    ("high_bell", 10000.0, "ch1himidfreq", "ch1himidgain", "ch1himidenable",
     "ch1himidshape", "BELL", "ch1himidbw"),
    # 12 kHz is the high band's nearest stop to the shared 10 kHz.
    ("high_shelf", 12000.0, "ch1hifreq", "ch1higain", "ch1hienable",
     "ch1hishape", "SHELF", "ch1hibw"),
]


def _massive_passive(stops):
    return {
        "base": dict(_MP_OFF, power=True, master_bypass=False,
                     ctrllink="LINKED", ch1enable="IN", ch1gain=0.0,
                     ch1hipass="OFF", ch1lopass="OFF", ch1lobw=2.0,
                     ch1lomidbw=2.0, ch1himidbw=2.0, ch1hibw=2.0),
        "eq": {"flat": dict(_MP_OFF),
               "bands": [_mp_band(*args, stops=stops) for args in _MP_BANDS]},
    }


VOICINGS["massive_passive"] = _massive_passive(_MP_STOPS)
# The mastering version's boost knob runs 0 to 11, not 0 to 20.
VOICINGS["massive_passive_m"] = _massive_passive([2.0, 4.0, 7.0, 11.0])

_PEQ_OFF = {"low_1_on_off": False, "low_mid_1_on_off": False,
            "high_mid_1_on_off": False, "high_1_on_off": False,
            "low_2_on_off": False, "low_mid_2_on_off": False,
            "high_mid_2_on_off": False, "high_2_on_off": False}


def _peq_band(band, hz, n, freq_p, gain_p, mode_p, shape_p, shape, bw_p,
              on_p):
    def row(mode, g, bw=5.0):
        return {on_p: True, freq_p: hz, gain_p: g, mode_p: mode,
                shape_p: shape, bw_p: bw}
    gains = [("boost_%g" % g, row("Boost", g)) for g in _MP_STOPS]
    gains += [("cut_%g" % g, row("Cut", g)) for g in _MP_STOPS]
    gains += [("boost_20_bw%g" % w, row("Boost", 20.0, w)) for w in (2.0, 8.0)]
    return {"band": band, "hz": hz, "marking": "0-20 knob", "gains": gains,
            "distortion_at": ("boost_20", row("Boost", 20.0))}


VOICINGS["passive_eq"] = {
    "base": dict(_PEQ_OFF, ms_mode="Dual Mono", link_1=True, link_2=True,
                 gain_1=0.0, gain_2=0.0, hp_1="OFF", hp_2="OFF",
                 lp_1="OFF", lp_2="OFF"),
    "eq": {
        "flat": dict(_PEQ_OFF),
        "bands": [
            _peq_band("low_shelf", 100.0, 1, "l1_freq", "l1_gain",
                      "l1_g_mode", "l1_bw_mode", "Shelf", "l1_bw",
                      "low_1_on_off"),
            _peq_band("low_bell", 100.0, 1, "l1_freq", "l1_gain",
                      "l1_g_mode", "l1_bw_mode", "Bell", "l1_bw",
                      "low_1_on_off"),
            _peq_band("lomid_bell", 560.0, 1, "lm1_freq", "lm1_gain",
                      "lm1_g_mode", "lm1_bw_mode", "Bell", "lm1_bw",
                      "low_mid_1_on_off"),
            _peq_band("mid_bell", 1000.0, 1, "hm1_freq", "hm1_gain",
                      "hm1_g_mode", "hm1_bw_mode", "Bell", "hm1_bw",
                      "high_mid_1_on_off"),
            _peq_band("himid_bell", 3300.0, 1, "hm1_freq", "hm1_gain",
                      "hm1_g_mode", "hm1_bw_mode", "Bell", "hm1_bw",
                      "high_mid_1_on_off"),
            _peq_band("high_bell", 10000.0, 1, "hm1_freq", "hm1_gain",
                      "hm1_g_mode", "hm1_bw_mode", "Bell", "hm1_bw",
                      "high_mid_1_on_off"),
            _peq_band("high_shelf", 12000.0, 1, "h1_freq", "h1_gain",
                      "h1_g_mode", "h1_bw_mode", "Shelf", "h1_bw",
                      "high_1_on_off"),
        ],
    },
}

# ------------------------------------------------------------------ Neve

_NEVE_GAINS = [-12.0, -9.0, -6.0, -3.0, 3.0, 6.0, 9.0, 12.0]


def _neve_band(band, hz, freq_p, gain_p, freq_value, extra=None,
               gains=_NEVE_GAINS):
    rows = []
    for g in gains:
        params = {freq_p: freq_value, gain_p: g}
        params.update(extra or {})
        rows.append(("%+g" % g, params))
    top = dict(extra or {})
    top.update({freq_p: freq_value, gain_p: 12.0})
    return {"band": band, "hz": hz, "marking": "dB", "gains": rows,
            "distortion_at": ("+12", top)}


VOICINGS["veq3"] = {
    # A 1073's switches are Off/On where a 1081's are Out/In; the two Waves
    # plugins do not agree about this and neither is wrong.
    "base": {"eqin": "On ", "analog": "On ", "output": 0.0, "phase": "Off ",
             "hpf": "OFF ", "lff": "OFF ", "mff": "OFF ", "hff": "OFF ",
             "lfg": 0.0, "mfg": 0.0, "hfg": 0.0},
    "eq": {
        "flat": {"lff": "OFF ", "mff": "OFF ", "hff": "OFF ",
                 "lfg": 0.0, "mfg": 0.0, "hfg": 0.0},
        "bands": [
            _neve_band("low_shelf", 100.0, "lff", "lfg", "100Hz "),
            # A 1073's mid starts at 360 Hz, which is its nearest stop to the
            # shared 500 and is 1.4x away from it.
            _neve_band("lomid_bell", 360.0, "mff", "mfg", "0.36 kHz "),
            _neve_band("mid_bell", 1200.0, "mff", "mfg", "1.2 kHz "),
            _neve_band("himid_bell", 3200.0, "mff", "mfg", "3.2 kHz "),
            _neve_band("high_shelf", 10000.0, "hff", "hfg", "10 kHz "),
        ],
        "extra": [("analog_off_flat", {"analog": "Off "}),
                  ("hpf_50", {"hpf": "50 Hz "}),
                  ("low_35hz_+12", {"lff": "35 Hz ", "lfg": 12.0})],
    },
}

VOICINGS["veq4"] = {
    "base": EQS["veq4"]["base"],
    "eq": {
        "flat": {"lfg": 0.0, "lff": "OFF ", "lmfg": 0.0, "lmff": "OFF ",
                 "hmfg": 0.0, "hmff": "OFF ", "hfg": 0.0, "hff": "OFF ",
                 "lf_bell": "Out ", "hf_bell": "Out ", "lmhq": "Out ",
                 "hmhq": "Out "},
        "bands": [
            _neve_band("low_shelf", 100.0, "lff", "lfg", "100Hz ",
                       {"lf_bell": "Out "}),
            _neve_band("low_bell", 100.0, "lff", "lfg", "100Hz ",
                       {"lf_bell": "In "}),
            _neve_band("lomid_bell", 470.0, "lmff", "lmfg", "470 Hz ",
                       {"lmhq": "Out "}),
            _neve_band("mid_bell", 1000.0, "lmff", "lmfg", "1000 Hz ",
                       {"lmhq": "Out "}),
            _neve_band("himid_bell", 3300.0, "hmff", "hmfg", "3.3 kHz ",
                       {"hmhq": "Out "}),
            _neve_band("high_shelf", 10000.0, "hff", "hfg", "10.0kHz ",
                       {"hf_bell": "Out "}),
            _neve_band("high_bell", 10000.0, "hff", "hfg", "10.0kHz ",
                       {"hf_bell": "In "}),
        ],
        "extra": [
            ("analog_off_flat", {"analog": "Out "}),
            # The high-Q switch raises the peak as well as narrowing it,
            # which is the sort of thing a biquad table gets wrong.
            ("lmf_1k_+12_hiq", {"lmff": "1000 Hz ", "lmfg": 12.0,
                                "lmhq": "In "}),
            ("hmf_3k3_+12_hiq", {"hmff": "3.3 kHz ", "hmfg": 12.0,
                                 "hmhq": "In "}),
        ],
    },
}

# ------------------------------------------------------------------- SSL

_SSL_OFF = {"lf_gain_db": 0.0, "lmf_gain_db": 0.0, "hmf_gain_db": 0.0,
            "hf_gain_db": 0.0}


def _ssl_band(band, hz, freq_p, gain_p, freq_value, extra=None):
    rows = []
    for g in _NEVE_GAINS:
        params = dict(_SSL_OFF)
        params.update({freq_p: freq_value, gain_p: g})
        params.update(extra or {})
        rows.append(("%+g" % g, params))
    top = dict(_SSL_OFF)
    top.update({freq_p: freq_value, gain_p: 12.0})
    top.update(extra or {})
    return {"band": band, "hz": hz, "marking": "dB", "gains": rows,
            "distortion_at": ("+12", top)}


def _ssl_eq(mode, q=1.0):
    return {
        "base": {"eq_in": "In", "dynamics_in": "Out", "filters_in": "Out",
                 "input_trim_db": 0.0, "output_trim_db": 0.0,
                 "fader_level_db": 0.0, "eq_type": mode,
                 "lmf_q": q, "hmf_q": q, **_SSL_OFF},
        "eq": {
            "flat": dict(_SSL_OFF),
            "bands": [
                _ssl_band("low_shelf", 100.0, "lf_frequency_hz",
                          "lf_gain_db", 100.0),
                _ssl_band("lomid_bell", 500.0, "lmf_frequency_khz",
                          "lmf_gain_db", 0.5),
                _ssl_band("mid_bell", 1000.0, "lmf_frequency_khz",
                          "lmf_gain_db", 1.0),
                _ssl_band("himid_bell", 3300.0, "hmf_freq_khz",
                          "hmf_gain_db", 3.3),
                _ssl_band("high_shelf", 10000.0, "hf_freq_khz",
                          "hf_gain_db", 10.0),
            ],
            # Q against gain is the proportional-Q question, and the E and G
            # curves are the reason this unit is here twice.
            "extra": [("lmf_1k_+12_q0.7", dict(_SSL_OFF, lmf_frequency_khz=1.0,
                                               lmf_gain_db=12.0, lmf_q=0.7)),
                      ("lmf_1k_+12_q2.5", dict(_SSL_OFF, lmf_frequency_khz=1.0,
                                               lmf_gain_db=12.0, lmf_q=2.5)),
                      ("lmf_1k_+3_q1", dict(_SSL_OFF, lmf_frequency_khz=1.0,
                                            lmf_gain_db=3.0, lmf_q=1.0))],
        },
    }


VOICINGS["ssl_native_eq"] = _ssl_eq("E")

# ------------------------------------------------------------- digital EQ


def _clean_band(band, hz, params_for, gains=_NEVE_GAINS):
    rows = [("%+g" % g, params_for(g)) for g in gains]
    return {"band": band, "hz": hz, "marking": "dB", "gains": rows,
            "distortion_at": ("+12", params_for(12.0))}


_Q10_OFF = {"band_%d_on_off" % i: "Out " for i in range(1, 11)}

VOICINGS["q10"] = {
    "base": dict(_Q10_OFF, in_gain_left=0.0, in_gain_right=0.0, inv_l="+ "),
    "eq": {
        "flat": dict(_Q10_OFF),
        "bands": [
            _clean_band("low_shelf", 100.0, lambda g: dict(
                _Q10_OFF, band_1_on_off="In ", band_1_type="Low-Shelf ",
                band_1_frq=100.0, band_1_gain=g, band_1_q=0.71)),
            _clean_band("low_bell", 100.0, lambda g: dict(
                _Q10_OFF, band_1_on_off="In ", band_1_type="Bell ",
                band_1_frq=100.0, band_1_gain=g, band_1_q=1.0)),
            _clean_band("lomid_bell", 500.0, lambda g: dict(
                _Q10_OFF, band_5_on_off="In ", band_5_type="Bell ",
                band_5_frq=500.0, band_5_gain=g, band_5_q=1.0)),
            _clean_band("mid_bell", 1000.0, lambda g: dict(
                _Q10_OFF, band_6_on_off="In ", band_6_type="Bell ",
                band_6_frq=1000.0, band_6_gain=g, band_6_q=1.0)),
            _clean_band("himid_bell", 3300.0, lambda g: dict(
                _Q10_OFF, band_7_on_off="In ", band_7_type="Bell ",
                band_7_frq=3300.0, band_7_gain=g, band_7_q=1.0)),
            _clean_band("high_bell", 10000.0, lambda g: dict(
                _Q10_OFF, band_9_on_off="In ", band_9_type="Bell ",
                band_9_frq=10000.0, band_9_gain=g, band_9_q=1.0)),
            _clean_band("high_shelf", 10000.0, lambda g: dict(
                _Q10_OFF, band_9_on_off="In ", band_9_type="Hi-Shelf ",
                band_9_frq=10000.0, band_9_gain=g, band_9_q=0.71)),
        ],
    },
}

_SLICK_OFF = {"low_gain_db": 0.0, "mid_gain_db": 0.0, "high_gain_db": 0.0}

VOICINGS["slickeq"] = {
    "base": dict(_SLICK_OFF, auto_gain=False, quality="Full", mode="Stereo",
                 out_gain_db=0.0, out_drive_db=0.0, hp_freq_hz="Off",
                 out_stage="Linear", eq_model="American", eq_sat=False,
                 low_shape="Shelf", high_shape="Shelf",
                 low_freq_hz=100.0, mid_freq_hz=1000.0,
                 high_freq_hz=10000.0),
    "eq": {
        "flat": dict(_SLICK_OFF),
        "bands": [
            _clean_band("low_shelf", 100.0, lambda g: dict(
                _SLICK_OFF, low_shape="Shelf", low_freq_hz=100.0,
                low_gain_db=g)),
            _clean_band("low_bell", 100.0, lambda g: dict(
                _SLICK_OFF, low_shape="Bell", low_freq_hz=100.0,
                low_gain_db=g)),
            _clean_band("lomid_bell", 500.0, lambda g: dict(
                _SLICK_OFF, mid_freq_hz=500.0, mid_gain_db=g)),
            _clean_band("mid_bell", 1000.0, lambda g: dict(
                _SLICK_OFF, mid_freq_hz=1000.0, mid_gain_db=g)),
            _clean_band("himid_bell", 3300.0, lambda g: dict(
                _SLICK_OFF, mid_freq_hz=3300.0, mid_gain_db=g)),
            _clean_band("high_bell", 10000.0, lambda g: dict(
                _SLICK_OFF, high_shape="Bell", high_freq_hz=10000.0,
                high_gain_db=g)),
            _clean_band("high_shelf", 10000.0, lambda g: dict(
                _SLICK_OFF, high_shape="Shelf", high_freq_hz=10000.0,
                high_gain_db=g)),
        ],
        "extra": [
            # The switch this whole question wants: the same curve with the
            # saturation stage in and out, and four voicings of the curve.
            ("flat_sat", dict(_SLICK_OFF, eq_sat=True)),
            ("low_+10_clean", dict(_SLICK_OFF, low_freq_hz=100.0,
                                   low_gain_db=10.0, eq_sat=False)),
            ("low_+10_sat", dict(_SLICK_OFF, low_freq_hz=100.0,
                                 low_gain_db=10.0, eq_sat=True)),
            ("out_drive_12", dict(_SLICK_OFF, out_drive_db=12.0)),
            ("model_british", dict(_SLICK_OFF, eq_model="British",
                                   low_freq_hz=100.0, low_gain_db=10.0)),
            ("model_german", dict(_SLICK_OFF, eq_model="German",
                                  low_freq_hz=100.0, low_gain_db=10.0)),
            ("model_soviet", dict(_SLICK_OFF, eq_model="Soviet",
                                  low_freq_hz=100.0, low_gain_db=10.0)),
        ],
    },
}

_NOVA_OFF = {"band_%d_active" % i: False for i in range(1, 5)}
_NOVA_OFF.update({"band_%d_dyn" % i: "Off" for i in range(1, 5)})

VOICINGS["tdr_nova"] = {
    # Auto-gain off: it is exactly the thing that would hide a shelf's own
    # level shift, and the level shift is part of the curve.
    "base": dict(_NOVA_OFF, master_bypass=False, eq_auto_gain=False,
                 dry_mix=0.0, output_gain_db=0.0, hp_active=False,
                 lp_active=False, quality="Precise", channels="Stereo",
                 sidechain_mode="Int SC"),
    "eq": {
        "flat": dict(_NOVA_OFF),
        "bands": [
            _clean_band("low_shelf", 100.0, lambda g: dict(
                _NOVA_OFF, band_1_active=True, band_1_type="Low S",
                band_1_frequency_hz=100.0, band_1_gain_db=g, band_1_q=0.71)),
            _clean_band("low_bell", 100.0, lambda g: dict(
                _NOVA_OFF, band_1_active=True, band_1_type="Bell",
                band_1_frequency_hz=100.0, band_1_gain_db=g, band_1_q=1.0)),
            _clean_band("lomid_bell", 500.0, lambda g: dict(
                _NOVA_OFF, band_2_active=True, band_2_type="Bell",
                band_2_frequency_hz=500.0, band_2_gain_db=g, band_2_q=1.0)),
            _clean_band("mid_bell", 1000.0, lambda g: dict(
                _NOVA_OFF, band_2_active=True, band_2_type="Bell",
                band_2_frequency_hz=1000.0, band_2_gain_db=g, band_2_q=1.0)),
            _clean_band("himid_bell", 3300.0, lambda g: dict(
                _NOVA_OFF, band_3_active=True, band_3_type="Bell",
                band_3_frequency_hz=3300.0, band_3_gain_db=g, band_3_q=1.0)),
            _clean_band("high_bell", 10000.0, lambda g: dict(
                _NOVA_OFF, band_4_active=True, band_4_type="Bell",
                band_4_frequency_hz=10000.0, band_4_gain_db=g,
                band_4_q=1.0)),
            _clean_band("high_shelf", 10000.0, lambda g: dict(
                _NOVA_OFF, band_4_active=True, band_4_type="High S",
                band_4_frequency_hz=10000.0, band_4_gain_db=g,
                band_4_q=0.71)),
        ],
        # A textbook bell at four Q settings, which is the reference every
        # analogue curve in this collection is read against.
        "extra": [("bell_1k_+12_q%g" % q, dict(
            _NOVA_OFF, band_2_active=True, band_2_type="Bell",
            band_2_frequency_hz=1000.0, band_2_gain_db=12.0, band_2_q=q))
            for q in (0.3, 0.7, 2.0, 6.0)],
    },
}

# kHs 3-Band and the niveau filter are tone controls rather than parametrics:
# fixed crossovers, one knob each, and gain controls that report a string. So
# they are swept on the raw 0..1 axis and the dB each position delivers comes
# off the curve, same as the Pultec.
_KH_FLAT = {"low": ("raw", 0.5), "mid": ("raw", 0.5), "high": ("raw", 0.5)}
_KH_RAW = [0.0, 0.15, 0.3, 0.7, 0.85, 1.0]

VOICINGS["kh_3band"] = {
    "base": dict(_KH_FLAT, low_split=100.0, high_split=10000.0),
    "eq": {
        "flat": dict(_KH_FLAT),
        "bands": [
            {"band": "low_shelf", "hz": 100.0, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, dict(_KH_FLAT, low=("raw", r)))
                       for r in _KH_RAW],
             "distortion_at": ("raw_1", dict(_KH_FLAT, low=("raw", 1.0)))},
            {"band": "mid_bell", "hz": 1000.0, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, dict(_KH_FLAT, mid=("raw", r)))
                       for r in _KH_RAW],
             "distortion_at": ("raw_1", dict(_KH_FLAT, mid=("raw", 1.0)))},
            {"band": "high_shelf", "hz": 10000.0, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, dict(_KH_FLAT, high=("raw", r)))
                       for r in _KH_RAW],
             "distortion_at": ("raw_1", dict(_KH_FLAT, high=("raw", 1.0)))},
        ],
    },
}

VOICINGS["elysia_niveau"] = {
    "base": {"active": True, "eq_freq_x10": False, "eq_freq_hz": 230.0,
             "eq_gain_db": ("raw", 0.5)},
    "eq": {
        "flat": {"eq_gain_db": ("raw", 0.5)},
        "bands": [
            {"band": "tilt_230", "hz": 230.0, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, {"eq_freq_hz": 230.0,
                                       "eq_gain_db": ("raw", r)})
                       for r in (0.0, 0.25, 0.5, 0.75, 1.0)],
             "distortion_at": ("raw_1", {"eq_freq_hz": 230.0,
                                         "eq_gain_db": ("raw", 1.0)})},
            {"band": "tilt_1k", "hz": 1000.0, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, {"eq_freq_hz": 1000.0,
                                       "eq_gain_db": ("raw", r)})
                       for r in (0.0, 0.25, 0.5, 0.75, 1.0)],
             "distortion_at": ("raw_1", {"eq_freq_hz": 1000.0,
                                         "eq_gain_db": ("raw", 1.0)})},
        ],
    },
}

VOICINGS["aw_baxandall2"] = {
    "base": {"bass": ("raw", 0.5), "treble": ("raw", 0.5)},
    "eq": {
        "flat": {"bass": ("raw", 0.5), "treble": ("raw", 0.5)},
        "bands": [
            {"band": "low_shelf", "hz": None, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, {"bass": ("raw", r),
                                       "treble": ("raw", 0.5)})
                       for r in (0.0, 0.25, 0.5, 0.75, 1.0)],
             "distortion_at": ("raw_1", {"bass": ("raw", 1.0)})},
            {"band": "high_shelf", "hz": None, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, {"bass": ("raw", 0.5),
                                       "treble": ("raw", r)})
                       for r in (0.0, 0.25, 0.5, 0.75, 1.0)],
             "distortion_at": ("raw_1", {"treble": ("raw", 1.0)})},
        ],
    },
}

VOICINGS["aw_capacitor2"] = {
    "base": {"lowpass": ("raw", 1.0), "highpass": ("raw", 0.0),
             "nonlin": ("raw", 0.5), "dry_wet": ("raw", 1.0)},
    "eq": {
        "flat": {"lowpass": ("raw", 1.0), "highpass": ("raw", 0.0)},
        "bands": [
            {"band": "lowpass", "hz": None, "marking": "raw 0..1",
             "gains": [("lp_%g" % r, {"lowpass": ("raw", r),
                                      "highpass": ("raw", 0.0)})
                       for r in (0.25, 0.5, 0.75, 1.0)],
             "distortion_at": ("lp_0.5", {"lowpass": ("raw", 0.5)})},
            {"band": "highpass", "hz": None, "marking": "raw 0..1",
             "gains": [("hp_%g" % r, {"lowpass": ("raw", 1.0),
                                      "highpass": ("raw", r)})
                       for r in (0.0, 0.25, 0.5, 0.75)],
             "distortion_at": ("hp_0.5", {"highpass": ("raw", 0.5)})},
        ],
        # Capacitor2's nonlinearity is the point of it: the same filter with
        # the modelled dielectric in and out.
        "extra": [("nonlin_0", {"nonlin": ("raw", 0.0),
                                "lowpass": ("raw", 0.5)}),
                  ("nonlin_1", {"nonlin": ("raw", 1.0),
                                "lowpass": ("raw", 0.5)})],
    },
}

VOICINGS["aw_air3"] = {
    "base": {"air": ("raw", 0.5), "gnd": ("raw", 0.5)},
    "eq": {
        "flat": {"air": ("raw", 0.5)},
        "bands": [
            {"band": "high_shelf", "hz": None, "marking": "raw 0..1",
             "gains": [("raw_%g" % r, {"air": ("raw", r)})
                       for r in (0.0, 0.25, 0.5, 0.75, 1.0)],
             "distortion_at": ("raw_1", {"air": ("raw", 1.0)})},
        ],
    },
}

# ============================================================ compressors
#
# `amount` is whatever control this unit uses to decide how hard it works --
# Peak Reduction, Input, Threshold -- and it is searched rather than set, so
# every unit can be driven to the same gain reduction and its timings
# compared. `attacks` and `releases` are the markings the unit actually has.
#
# Where a Softube plugin reports a frozen value for a control it nonetheless
# obeys, the amount is given as raw positions; the measurement records the
# reduction each produced, which is the number that matters anyway.

for _slug, _spec in COMPS.items():
    VOICINGS[_slug] = {
        "base": _spec["base"],
        "settings": [],
        "comp": {"element": _spec["element"], "amount": _spec["amount"],
                 "attacks": _spec["attacks"], "releases": _spec["releases"],
                 "fixed": _spec.get("fixed"), "mirror": _spec.get("mirror")},
    }

_UAD = {"power": True, "master_bypass": False, "mix": 100.0, "headroom": 16.0}

# ---- the four 1176s. Same box, four makers' readings of it, so they are set
# alike: 4:1, sidechain filter out, attack and release at position 4 and 5.

VOICINGS["1176a"] = {
    "base": dict(_UAD, ratio="4:1", sc_filter=False, meter="GR",
                 output=-20.0),
    "comp": {"element": "fet",
             "amount": ("input", [-40.0, -35.0, -30.0, -25.0, -20.0, -15.0,
                                  -10.0, -5.0]),
             "attacks": ("attack", ["1.00", "2.00", "3.00", "4.00", "5.00",
                                    "6.00", "7.00"]),
             "releases": ("release", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
             "fixed": {"attack": "4.00", "release": 5.0}},
}

VOICINGS["1176ae"] = {
    "base": dict(_UAD, ratio="4:1", sc_filter=False, meter="GR",
                 output=-20.0),
    "comp": {"element": "fet",
             "amount": ("input", [-40.0, -35.0, -30.0, -25.0, -20.0, -15.0,
                                  -10.0, -5.0]),
             "attacks": ("attack", ["1.00", "2.00", "3.00", "4.00", "5.00",
                                    "6.00", "7.00"]),
             "releases": ("release", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
             "fixed": {"attack": "4.00", "release": 5.0}},
}

VOICINGS["vc76"] = {
    "base": {"ratio": "4/1", "sc_mode": False, "sc_gain": 0.0,
             "dry": ("raw", 0.0), "vu_mode": "Gain Reduction",
             "output": ("raw", 0.3)},
    "comp": {"element": "fet",
             "amount": ("input", [("raw", r) for r in
                                  (0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9)]),
             "attacks": ("attack", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
             "releases": ("release", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
             "fixed": {"attack": 4.0, "release": 5.0}},
}

# ---- the three LA-2A/LA-3A readings

VOICINGS["vc2a"] = {
    "base": {"mode": "Compress", "sc_mode": False, "sc_gain": 0.0,
             "detector_hp": "off", "dry": ("raw", 0.0),
             "vu_mode": "Gain Reduction", "gain": 0.0},
    "comp": {"element": "opto",
             "amount": ("peak_reduction",
                        [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0]),
             "attacks": None, "releases": None},
}

VOICINGS["bx_opto"] = {
    "base": {"power": True, "mix": 100.0, "output_gain_db": 0.0,
             "sidechain_off_on_solo": "Off", "speed": 50.0},
    "comp": {"element": "opto",
             "amount": ("peak_reduction",
                        [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 85.0]),
             # Its one timing control moves attack and release together,
             # which is what an opto pedal has instead of two.
             "attacks": ("speed", [0.0, 25.0, 50.0, 75.0, 100.0]),
             "releases": None,
             "fixed": {"speed": 50.0}},
}

VOICINGS["supercharger_gt"] = {
    "base": {"character_mode": "Warm", "character": 1.0, "detector_hp": "Off",
             "mix": ("raw", 0.0), "compression": 1.0, "attack": 4.3},
    "comp": {"element": "tube",
             "amount": ("compression", [1.0, 2.0, 3.0, 4.0, 5.0, 6.5, 8.0,
                                        10.0]),
             "attacks": ("attack", [1.0, 3.0, 5.0, 7.0, 10.0]),
             "releases": None,
             "fixed": {"attack": 4.3}},
}

# ---- the two dbx 160s

VOICINGS["vc160"] = {
    "base": {"compression": 4.0, "sc_mode": False, "sc_gain": 0.0,
             "detector_hp": "off", "dry": ("raw", 0.0),
             "vu_mode": "Gain Reduction", "output": 0.0},
    "comp": {"element": "vca",
             "amount": ("threshold", [("raw", r) for r in
                                      (0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7,
                                       0.8)]),
             "attacks": None, "releases": None},
}

# ---- the three SSL bus compressors

VOICINGS["solid_bus_comp"] = {
    # Threshold and make-up report a frozen value on this build but obey a
    # write, so the amount axis is raw and the reduction each position
    # produced is what gets recorded.
    "base": {"ratio": 4.0, "attack": 1.0, "release": "0.1 s",
             "link_l_r": True, "sc_mode": False, "sc_gain": 0.0,
             "dry": ("raw", 0.0), "output": ("raw", 1.0)},
    "comp": {"element": "vca",
             "amount": ("threshold", [("raw", r) for r in
                                      (0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3,
                                       0.2)]),
             "attacks": ("attack", [0.1, 0.3, 1.0, 3.0, 10.0, 30.0]),
             "releases": ("release", ["0.1 s", "0.2 s", "0.4 s", "0.8 s",
                                      "1.6 s", "Auto"]),
             "fixed": {"attack": 1.0, "release": "0.1 s"}},
}

VOICINGS["ssl_native_bus"] = {
    "base": {"comp_bypass": False, "ratio": "4:1", "attack_ms": 1.0,
             "release_s": "0.3", "dry_wet_mix": 100.0,
             "makeup_gain_db": 0.0, "external_s_c": False,
             "sidechain_hpf_hz": "OFF", "oversampling": "OFF"},
    "comp": {"element": "vca",
             "amount": ("threshold_db", [15.0, 10.0, 5.0, 0.0, -5.0, -10.0,
                                         -15.0, -20.0]),
             "attacks": ("attack_ms", [0.1, 0.3, 1.0, 3.0, 10.0, 20.0, 30.0]),
             "releases": ("release_s", ["0.1", "0.3", "0.4", "0.6", "0.8",
                                        "1.2", "AUTO"]),
             "fixed": {"attack_ms": 1.0, "release_s": "0.3"}},
}

VOICINGS["solid_dynamics"] = {
    "base": {"link_l_r": True, "knee_type": "Soft", "ratio": 4.0,
             "boost": False, "mode": "Gate", "range": ("raw", 0.0),
             "threshold_g_e": ("raw", 0.0), "sc_mode": False, "sc_gain": 0.0,
             "dry": ("raw", 0.0), "atk_mode_comp": False, "rel_mode": False},
    "comp": {"element": "vca",
             "amount": ("threshold_comp", [("raw", r) for r in
                                           (0.9, 0.8, 0.7, 0.6, 0.5, 0.4,
                                            0.3, 0.2)]),
             # Its attack and release are two-position switches, which is
             # what an SSL channel gives you.
             "attacks": ("atk_mode_comp", [False, True]),
             "releases": ("rel_mode", [False, True]),
             "fixed": {"atk_mode_comp": False, "rel_mode": False}},
}

# ---- API, vari-mu and the rest

VOICINGS["api_vision_comp"] = {
    "base": {"power": True, "master_bypass": False, "input_select": "Line",
             "pad": False, "phase": "Normal", "cut_filter": "Off",
             "line_gain": 0.0, "level": ("raw", 1.0), "eq_on": False,
             "235_on": False, "215_on": False, "225_on": True,
             "225_ratio": 4.0, "225_knee": "Hard", "225_type": "Old (FB)",
             "225_attack": "Medium", "225_release": "0.50 s",
             "sc_link": True},
    "comp": {"element": "vca",
             "amount": ("225_thresh", [10.0, 5.0, 0.0, -5.0, -10.0, -15.0,
                                       -20.0, -25.0]),
             "attacks": ("225_attack", ["Fast", "Medium", "Slow"]),
             "releases": ("225_release", ["50 ms", "0.10 s", "0.20 s",
                                          "0.50 s", "1.00 s"]),
             "fixed": {"225_attack": "Medium", "225_release": "0.50 s"}},
}

VOICINGS["fairchild660"] = {
    "base": dict(_UAD, sc_filter="Off", output=0.0, input=0.0, dc_thr=8.6,
                 timecnst=1.0),
    "comp": {"element": "vari-mu",
             "amount": ("thresh", [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0,
                                   10.0]),
             "attacks": ("timecnst", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
             "releases": None,
             "fixed": {"timecnst": 1.0}},
}

VOICINGS["vari_comp"] = {
    "base": {"mode": "Compress", "detector_hp": False, "sc_mode": False,
             "sc_gain": 0.0, "dry": ("raw", 0.0), "input": 0.0,
             "output": 0.0, "attack": 70.0, "recovery": 400.0},
    "comp": {"element": "vari-mu",
             "amount": ("thresh", [10.0, 8.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0,
                                   0.0]),
             "attacks": ("attack", [25.0, 40.0, 55.0, 70.0]),
             "releases": ("recovery", [200.0, 400.0, 600.0, 4000.0, 8000.0]),
             "fixed": {"attack": 70.0, "recovery": 400.0}},
}

VOICINGS["vcomp"] = {
    "base": {"in": "In ", "ratio": "4:1 ", "analog": "Off ", "mix": 100.0,
             "output": 0.0, "attack": 3.0, "release": 400.0},
    "comp": {"element": "diode-bridge",
             "amount": ("thresh", [0.0, -3.0, -6.0, -9.0, -12.0, -15.0,
                                   -18.0, -21.0]),
             "attacks": ("attack", [0.5, 1.0, 3.0, 10.0, 30.0]),
             "releases": ("release", [100.0, 200.0, 400.0, 800.0, 1500.0]),
             "fixed": {"attack": 3.0, "release": 400.0}},
}

VOICINGS["molotok"] = {
    "base": {"ratio": 4.0, "attack_ms": 10.0, "release_ms": 100.0,
             "dry_mix": 0.0, "output_gain_db": 0.0},
    "comp": {"element": "digital",
             "amount": ("threshold_db", [-5.0, -10.0, -15.0, -20.0, -25.0,
                                         -30.0, -35.0, -40.0]),
             "attacks": ("attack_ms", [0.5, 1.0, 3.0, 10.0, 30.0, 100.0]),
             "releases": ("release_ms", [20.0, 50.0, 100.0, 300.0, 1000.0]),
             "fixed": {"attack_ms": 10.0, "release_ms": 100.0}},
}

VOICINGS["kh_comp"] = {
    "base": {},
    "comp": {"element": "digital",
             "amount": ("threshold", [("raw", r) for r in
                                      (0.9, 0.8, 0.7, 0.6, 0.5, 0.4, 0.3,
                                       0.2)]),
             "attacks": None, "releases": None},
}

VOICINGS["rcomp"] = {
    "base": {"ratio": 4.0, "attack": 5.0, "release": 200.0, "gain": 0.0,
             "arc_manual": "Manual ", "electro_opto": "Electro "},
    "comp": {"element": "digital",
             "amount": ("thresh", [0.0, -3.0, -6.0, -9.0, -12.0, -15.0,
                                   -18.0, -21.0]),
             "attacks": ("attack", [0.5, 1.0, 5.0, 20.0, 100.0]),
             "releases": ("release", [50.0, 100.0, 200.0, 500.0, 1000.0]),
             "fixed": {"attack": 5.0, "release": 200.0}},
}

VOICINGS["h_comp"] = {
    "base": {"ratio": 4.0, "attack": 5.0, "release": 200.0, "analog": "Off ",
             "mix": 100.0, "punch": 0.0, "output": 0.0},
    "comp": {"element": "hybrid",
             "amount": ("thresh", [0.0, -3.0, -6.0, -9.0, -12.0, -15.0,
                                   -18.0, -21.0]),
             "attacks": ("attack", [0.5, 1.0, 5.0, 20.0, 100.0]),
             "releases": ("release", [50.0, 100.0, 200.0, 500.0, 1000.0]),
             "fixed": {"attack": 5.0, "release": 200.0}},
}

VOICINGS["c1_comp"] = {
    "base": {"ratio": 4.0, "attack": 5.0, "release": 200.0, "gain": 0.0,
             "knee": "Soft "},
    "comp": {"element": "digital",
             "amount": ("thresh", [0.0, -6.0, -12.0, -18.0, -24.0, -30.0,
                                   -36.0, -42.0]),
             "attacks": ("attack", [0.5, 1.0, 5.0, 20.0, 100.0]),
             "releases": ("release", [50.0, 100.0, 200.0, 500.0, 1000.0]),
             "fixed": {"attack": 5.0, "release": 200.0}},
}

# ---- Airwindows compressors, every control stated as raw

def _aw_comp(character, amount, element="digital", attacks=None,
             releases=None, fixed=None):
    return {
        "base": {k: ("raw", v) for k, v in character.items()},
        "comp": {"element": element,
                 "amount": (amount, [("raw", r) for r in
                                     (0.1, 0.2, 0.3, 0.4, 0.5, 0.65, 0.8,
                                      0.95)]),
                 "attacks": attacks, "releases": releases,
                 "fixed": fixed},
    }


VOICINGS["aw_buttercomp2"] = _aw_comp(
    {"compress": 0.5, "output": 1.0, "dry_wet": 1.0}, "compress")
VOICINGS["aw_pressure5"] = _aw_comp(
    {"pressre": 0.5, "speed": 0.5, "mewines": 0.5, "pawclaw": 0.5,
     "output": 1.0, "dry_wet": 1.0}, "pressre",
    attacks=("speed", [("raw", r) for r in (0.0, 0.25, 0.5, 0.75, 1.0)]),
    fixed={"speed": ("raw", 0.5)})
VOICINGS["aw_blockparty"] = _aw_comp(
    {"pound": 0.5, "dry_wet": 1.0}, "pound")
VOICINGS["aw_pop2"] = _aw_comp(
    {"intense": 0.5, "attack": 0.5, "output": 1.0, "dry_wet": 1.0},
    "intense",
    attacks=("attack", [("raw", r) for r in (0.0, 0.25, 0.5, 0.75, 1.0)]),
    fixed={"attack": ("raw", 0.5)})
VOICINGS["aw_logical4"] = _aw_comp(
    {"threshold": 0.5, "ratio": 0.5, "speed": 0.5, "makeupgn": 0.5,
     "dry_wet": 1.0}, "threshold",
    attacks=("speed", [("raw", r) for r in (0.0, 0.25, 0.5, 0.75, 1.0)]),
    fixed={"speed": ("raw", 0.5)})
VOICINGS["aw_purestsquish"] = _aw_comp(
    {"squish": 0.5, "bassblm": 0.5, "output": 0.5, "dry_wet": 1.0}, "squish")
VOICINGS["aw_compresaturator"] = _aw_comp(
    {"drive": 0.5, "clamp": 0.5, "expand": 0.5, "output": 1.0,
     "dry_wet": 1.0}, "drive")


DEFAULT = {"base": {}, "settings": [("default", {}, FULL)]}


def voicing_of(slug):
    """This unit's entry, with the fields every measurement script expects."""
    entry = dict(DEFAULT)
    entry.update(VOICINGS.get(slug, {}))
    entry.setdefault("base", {})
    entry.setdefault("settings", [("default", {}, FULL)])
    return entry


if __name__ == "__main__":
    import roster
    missing = [s for s in roster.UNITS if s not in VOICINGS]
    extra = [s for s in VOICINGS if s not in roster.UNITS]
    print("%d voicings for %d units" % (len(VOICINGS), len(roster.UNITS)))
    for family in roster.FAMILIES:
        rows = roster.by_family(family)
        need = "eq" if family == "eq" else ("comp" if family == "comp" else None)
        ok = [s for s in rows
              if s in VOICINGS and (need is None or need in VOICINGS[s])]
        print("  %-7s %d of %d ready" % (family, len(ok), len(rows)))
    if missing:
        print("no voicing:", " ".join(sorted(missing)))
    if extra:
        print("not in roster:", " ".join(sorted(extra)))
