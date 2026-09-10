"""How to put each unit into the state we want to measure.

A "setting" is a label plus the parameters that produce it. Everything not
under test is switched out: EQ sections bypassed, dynamics bypassed, noise
off, output trims at unity. What is left in the path is the input stage,
which is the thing 06-preamp-modelling.md is asking about.

Waves choice parameters carry a trailing space in their value strings. That
is theirs, not a typo.
"""

from candidates import AU, VST3, WAVES

PREAMPS = {
    # One plugin, three consoles, one drive control and no EQ in the path:
    # the only way to get three voicings measured on an identical rig.
    "nls_nevo": {
        "spec": (WAVES, "NLS Channel Stereo"),
        "note": "Waves NLS Channel, Neve 5116 -> Iron",
        "base": {"studio": "NEVO ", "noise": "Off ", "link": "Dual Mono ",
                 "output": 0.0, "mic": "Off "},
        "settings": [("drive_%g" % d, {"drive": d}) for d in (0.0, 3.0, 6.0, 9.0, 12.0)],
    },
    "nls_mike": {
        "spec": (WAVES, "NLS Channel Stereo"),
        "note": "Waves NLS Channel, SSL 4000 G+ -> Grip",
        "base": {"studio": "MIKE ", "noise": "Off ", "link": "Dual Mono ",
                 "output": 0.0, "mic": "Off "},
        "settings": [("drive_%g" % d, {"drive": d}) for d in (0.0, 3.0, 6.0, 9.0, 12.0)],
    },
    "nls_spike": {
        "spec": (WAVES, "NLS Channel Stereo"),
        "note": "Waves NLS Channel, EMI TG12345 -> Punch",
        "base": {"studio": "SPIKE ", "noise": "Off ", "link": "Dual Mono ",
                 "output": 0.0, "mic": "Off "},
        "settings": [("drive_%g" % d, {"drive": d}) for d in (0.0, 3.0, 6.0, 9.0, 12.0)],
    },
    # The 1073 itself, EQ switched out so only the preamp is in the path.
    #
    # Measured first with the gain selector as the drive axis, which produced
    # five identical rows: on this plugin the selector sets level and nothing
    # else, and the input trim moves level without moving distortion either.
    # Preamp Drive is a switch, not a continuum, so that is what gets swept
    # and the sweep's own level axis carries the rest.
    "scheps73": {
        "spec": (WAVES, "Scheps 73 Stereo"),
        "note": "Waves Scheps 73, Neve 1073 preamp, EQ out",
        "base": {"eq": "Out ", "preamp_on_off": "On ", "link_i_o": "Unlinked ",
                 "input_l_m": 0.0, "output_l_m": 0.0,
                 "preamp_l_m": "0L ", "preamp_r_s": "0L "},
        "settings": [("drive_off", {"preamp_drive": "Off "}),
                     ("drive_on", {"preamp_drive": "On "})],
    },
    # SSL 4000 E channel, analogue stage on, EQ and dynamics out.
    "ssl_ev2": {
        "spec": (WAVES, "SSL EV2 Channel Stereo"),
        "note": "Waves SSL EV2 channel, analogue stage only",
        "base": {"analog": "On ", "eq_bypass": "On ", "dynamics_bypass": "On ",
                 "mic": 0.0, "output": 0.0, "20db_pad": "Off "},
        # The output trim cancels the line gain, so the drive axis moves what
        # the stage sees without walking the measurement into the plugin's
        # own output clipping, which is what the first pass measured at +10
        # and +20.
        "settings": [("line_%g" % g, {"line": g, "output": -g})
                     for g in (-10.0, 0.0, 10.0, 20.0)]
                    + [("analog_off", {"analog": "Off ", "line": 0.0,
                                       "output": 0.0})],
    },
    # bx_console N's THD control is calibrated in dB, which makes it the one
    # unit where the drive axis is already a number rather than a knob.
    #
    # Its distortion lives *inside the EQ section*: with every section
    # bypassed the channel measures 0.0001% however the THD control is set,
    # and switching the EQ in at a flat curve brings the whole 0.95% back.
    # So the EQ stays in, flat, which is also how a console channel sits.
    "bx_console_n": {
        "spec": (VST3 % "bx_console N", None),
        "note": "Brainworx bx_console N, Neve VXS channel, EQ in and flat",
        "base": {"eq_on_off": True, "lc_on_off": False, "ge_on_off": False,
                 "hpf_on_off": False, "lpf_on_off": False, "console_channel": 1.0,
                 "in_gain_db": 0.0, "out_gain_db": 0.0},
        "settings": [("thd_%g" % t, {"thd": "%.1f" % t})
                     for t in (-60.0, -50.0, -40.0, -35.0, -30.0)],  # -30 is the knob's top
    },
    # UAD units, added by probe_uad once their VST3 parameter names are known.
    "ua610b": {
        "spec": (VST3 % "uaudio_610_b", None),
        "note": "UAD 610-B tube preamp",
        "base": {},
        "settings": [],
    },
    "api_vision": {
        "spec": (VST3 % "uaudio_api_vision_channel_strip", None),
        "note": "UAD API Vision channel strip -> Punch",
        "base": {},
        "settings": [],
    },
}

# FrontDAW is what REFERENCE_MEASUREMENTS.md says to start with, and the
# reason it gives is the right one: five voicings behind one Mojo control
# means the styles are measured *relative to each other* under identical
# conditions, which is worth more than five plugins measured five ways.
#
# Its high-pass cannot be switched out, only wound down to 20 Hz, so the
# 40 Hz row of every FrontDAW curve is sitting on that filter's skirt.
for _style in ("British", "American", "German", "Magnetic", "Clip"):
    PREAMPS["frontdaw_%s" % _style.lower()] = {
        "spec": (VST3 % "FrontDAW", None),
        "note": "FrontDAW, %s style" % _style,
        "base": {"style": _style, "high_pass": 20.0, "link_io": 0.0,
                 "input": 0.0, "output": 0.0},
        "settings": [("mojo_%d" % m, {"mojo": float(m)})
                     for m in (0, 25, 50, 75, 100)],
    }
