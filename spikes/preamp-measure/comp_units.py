"""Compressors to measure, one per gain element, and how to drive each.

`amount` is whatever control this unit uses to decide how hard it works --
Peak Reduction, Input, Threshold -- searched to land on a common gain
reduction so the timings are comparable. `attacks` and `releases` are the
markings to measure in milliseconds.

Half of these units carry marked times and half do not, which is the point.
Kotelnikov states 6.0 ms attack and knows it exactly, so measuring it back
says whether the rig is telling the truth before any of the 1176's unmarked
knob positions are believed.
"""

from candidates import VST3, WAVES

UAD = {"power": True, "mix": 100.0, "headroom": 16.0}

COMPS = {
    # ---- opto
    "la2a": {
        "spec": (VST3 % "uaudio_teletronix_la-2a_tc", None),
        "note": "UAD Teletronix LA-2A, opto, no timing controls at all",
        "element": "opto",
        # No mix and no headroom control on this one, unlike the other UAD
        # units, so it cannot take the shared base.
        "base": {"power": True, "comp_limit": "Comp", "gain": "40"},
        "amount": ("peak_reduct", ["10", "20", "30", "40", "50", "60", "70", "80"]),
        "attacks": None,
        "releases": None,
    },
    "cla3a": {
        "spec": (WAVES, "CLA-3A Stereo"),
        "note": "Waves CLA-3A, opto",
        "element": "opto",
        "base": {"comp_limiter": "Comp ", "analog": "Off ",
                 "auto_makeup": "Off ", "mix": 100.0, "trim": 0.0,
                 "gain": 0.0, "hi_freq": 50.0},
        "amount": ("peak_reduction", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]),
        "attacks": None,
        "releases": None,
    },

    # ---- FET
    "1176ln": {
        "spec": (VST3 % "uaudio_ua_1176ln_rev_e", None),
        "note": "UAD 1176LN Rev E, FET, knob positions rather than times",
        "element": "fet",
        "base": dict(UAD, ratio="4:1", sc_filter=False, output=-20.0),
        "amount": ("input", [-40.0, -35.0, -30.0, -25.0, -20.0, -15.0, -10.0, -5.0]),
        "attacks": ("attack", ["1.00", "2.00", "3.00", "4.00", "5.00", "6.00", "7.00"]),
        "releases": ("release", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
        "fixed": {"attack": "4.00", "release": 5.0},
    },
    "distressor": {
        "spec": (VST3 % "uaudio_distressor", None),
        "note": "UAD Distressor, FET, programme-dependent by design",
        "element": "fet",
        "base": dict(UAD, ratio="4:1", detector="Norm", audio="Norm",
                     output=5.0),
        "amount": ("input", [2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]),
        "attacks": ("attack", [0.0, 2.0, 4.0, 6.0, 8.0, 10.5]),
        "releases": ("release", [0.0, 2.0, 4.0, 6.0, 8.0, 10.5]),
        "fixed": {"attack": 5.0, "release": 5.0},
    },

    # ---- VCA
    "dbx160": {
        "spec": (VST3 % "uaudio_dbx_160", None),
        "note": "UAD dbx 160, VCA, over-easy knee, no timing controls",
        "element": "vca",
        # No headroom control on this one either.
        "base": {"power": True, "mix": 100.0, "compress": " 4.0:1",
                 "sc_filter": False, "gain": 0.0},
        "amount": ("thresh", [-40.0, -35.0, -30.0, -25.0, -20.0, -15.0, -10.0]),
        "attacks": None,
        "releases": None,
    },
    "ssl_gbus": {
        "spec": (VST3 % "uaudio_ssl_g_bus_compressor", None),
        "note": "UAD SSL G bus compressor, VCA, marked times",
        "element": "vca",
        "base": dict(UAD, ratio="4:1", sc_filter="Off", make_up=0.0,
                     attack=1.0, release="0.3 s"),
        "amount": ("thresh", [-15.0, -12.0, -9.0, -6.0, -3.0, 0.0, 3.0, 6.0]),
        "attacks": ("attack", [0.1, 0.3, 1.0, 3.0, 10.0, 30.0]),
        "releases": ("release", ["0.1 s", "0.3 s", "0.6 s", "1.2 s", "Auto"]),
        "fixed": {"attack": 1.0, "release": "0.3 s"},
    },
    "api2500": {
        "spec": (WAVES, "API-2500 Stereo"),
        "note": "Waves API-2500, VCA, marked times, thrust filter",
        "element": "vca",
        "base": {"in": "In ", "ratio": "4:1 ", "knee": "Hard ",
                 "thrust": "Norm ", "type": "New ", "makeup": "Manual ",
                 "trim": 0.0, "output": 0.0, "analog": "Off ", "mix": 100.0,
                 "attack": 1.0, "release": "0.2 s"},
        "amount": ("thresh", [10.0, 5.0, 0.0, -5.0, -10.0, -15.0, -20.0]),
        "attacks": ("attack", [0.03, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0]),
        "releases": ("release", ["0.05 s", "0.1 s", "0.2 s", "0.5 s", "1 s", "2 s"]),
        "fixed": {"attack": 1.0, "release": "0.2 s"},
    },

    # ---- vari-mu
    "fairchild670": {
        "spec": (VST3 % "uaudio_fairchild_670", None),
        "note": "UAD Fairchild 670, vari-mu, six-position time constant",
        "element": "vari-mu",
        "base": dict(UAD, agc_matrix="L-R", ctrl_link="Link", sc_link="Link",
                     l_sc_filt="Off", r_sc_filt="Off", l_output=0.0,
                     r_output=0.0, l_input=0.0, r_input=0.0,
                     l_dc_thr=8.6, r_dc_thr=8.6,
                     l_timecnst=1.0, r_timecnst=1.0),
        "amount": ("l_thresh", [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0]),
        # The six time constants are the unit's whole timing story, so they
        # are swept as attacks and the release axis rides along with them.
        "attacks": ("l_timecnst", [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        "releases": None,
        "fixed": {"l_timecnst": 1.0},
        "mirror": {"l_thresh": "r_thresh", "l_timecnst": "r_timecnst"},
    },

    # ---- the digital control, whose times are known exactly
    "kotelnikov": {
        "spec": (VST3 % "TDR Kotelnikov", None),
        "note": "TDR Kotelnikov, digital, states its times: the rig's control",
        "element": "digital",
        "base": {"ratio": 4.0, "soft_knee_db": 0.0, "makeup_db": 0.0,
                 "out_gain_db": 0.0, "dry_wet": 0.0, "peak_crest_db": 0.0,
                 "sc_hp_freq_hz": 25.0, "quality": "Precise",
                 "release_peak_ms": 100.0, "release_rms_ms": 100.0,
                 "attack_ms": 6.0},
        "amount": ("threshold_db", [-40.0, -35.0, -30.0, -25.0, -20.0, -15.0, -10.0]),
        "attacks": ("attack_ms", [0.5, 1.0, 3.0, 6.0, 20.0, 60.0]),
        "releases": ("release_peak_ms", [20.0, 50.0, 100.0, 300.0, 1000.0]),
        "fixed": {"attack_ms": 6.0, "release_peak_ms": 100.0},
        # Kotelnikov runs a peak and an RMS release together, so the RMS one
        # has to move with the peak one or it masks every short setting.
        "mirror": {"release_peak_ms": "release_rms_ms"},
    },
}
