"""EQ settings to measure, and which of them get the distortion sweep.

Three inductor/passive designs, one modern clean one for contrast, and
SlickEQ, which has a switch for exactly the question being asked -- its EQ
saturation can be turned off while the curve stays put.

`distortion_settings` names the settings that also get the full harmonic
sweep; the rest are curve-only, which is a tenth of the cost.
"""

from candidates import VST3, WAVES

EQS = {
    # Pultec EQP-1A: passive inductor network, tube make-up amp. The boost
    # and cut sections overlap, which is why the "trick" setting below is a
    # curve rather than a cancellation.
    "puigtec_eqp1a": {
        "spec": (WAVES, "PuigTec EQP1A Stereo"),
        "note": "Waves PuigTec EQP-1A, Pultec, passive inductor + tube",
        "base": {"onoff": "On ", "mains": "Off ", "gain": 0.0,
                 "lowfrequency": 100.0, "hifrequency": 10000.0,
                 "attenselect": 10000.0, "bandwidth": 0.0},
        "settings": [
            ("flat", {"lowboost": 0.0, "lowatten": 0.0,
                      "hiboost": 0.0, "hiatten": 0.0}),
            ("low_boost_max", {"lowboost": 11.0, "lowatten": 0.0}),
            ("low_boost_half", {"lowboost": 5.5, "lowatten": 0.0}),
            ("low_cut_max", {"lowboost": 0.0, "lowatten": 11.0}),
            # Boost and cut at once, the setting the unit is famous for.
            ("low_boost_and_cut", {"lowboost": 11.0, "lowatten": 11.0}),
            ("high_boost_wide", {"lowboost": 0.0, "lowatten": 0.0,
                                 "hiboost": 11.0, "bandwidth": 0.0}),
            ("high_boost_narrow", {"hiboost": 11.0, "bandwidth": 11.0}),
            ("high_cut_max", {"hiboost": 0.0, "hiatten": 11.0}),
        ],
        "distortion_settings": ["flat", "low_boost_max", "high_boost_wide",
                                "low_boost_and_cut"],
    },

    # Neve 1081: inductor EQ, discrete amps.
    "veq4": {
        "spec": (WAVES, "VEQ4 Stereo"),
        "note": "Waves VEQ4, Neve 1081 inductor EQ",
        "base": {"eq": "In ", "analog": "In ", "output": 0.0,
                 "phase": "Out ", "hpf": "OFF ", "lpf": "OFF ",
                 "lf_bell": "Out ", "hf_bell": "Out ",
                 "lmhq": "Out ", "hmhq": "Out "},
        "settings": [
            ("flat", {"lfg": 0.0, "lff": "OFF ", "lmfg": 0.0, "lmff": "OFF ",
                      "hmfg": 0.0, "hmff": "OFF ", "hfg": 0.0, "hff": "OFF "}),
            ("lf_shelf_12", {"lfg": 12.0, "lff": "100Hz "}),
            ("lf_bell_12", {"lfg": 12.0, "lff": "100Hz ", "lf_bell": "In "}),
            ("lmf_12", {"lfg": 0.0, "lff": "OFF ", "lf_bell": "Out ",
                        "lmfg": 12.0, "lmff": "560 Hz "}),
            ("lmf_6", {"lmfg": 6.0, "lmff": "560 Hz "}),
            ("lmf_12_hiq", {"lmfg": 12.0, "lmff": "560 Hz ", "lmhq": "In "}),
            # Analogue EQs are rarely symmetric about zero; this says whether
            # this one is.
            ("lmf_minus_12", {"lmfg": -12.0, "lmff": "560 Hz ",
                              "lmhq": "Out "}),
            ("hmf_12", {"lmfg": 0.0, "lmff": "OFF ",
                        "hmfg": 12.0, "hmff": "3.3 kHz "}),
            ("hf_shelf_12", {"hmfg": 0.0, "hmff": "OFF ",
                             "hfg": 12.0, "hff": "10.0kHz "}),
            ("analog_off_flat", {"hfg": 0.0, "hff": "OFF ", "analog": "Out "}),
        ],
        "distortion_settings": ["flat", "lf_shelf_12", "lmf_12",
                                "hf_shelf_12", "analog_off_flat"],
    },

    # Manley Massive Passive: passive network, tube make-up. Gains are
    # unipolar and the direction is a three-way switch per band.
    "massive_passive": {
        "spec": (VST3 % "uaudio_manley_massive_passive", None),
        "note": "UAD Manley Massive Passive, passive inductor + tube",
        "base": {"power": True, "ctrllink": "LINKED", "ch1enable": "IN",
                 "ch1gain": 0.0, "ch1hipass": "OFF", "ch1lopass": "OFF",
                 "ch1loenable": "OUT", "ch1lomidenable": "OUT",
                 "ch1himidenable": "OUT", "ch1hienable": "OUT",
                 "ch1lofreq": 100.0, "ch1lomidfreq": 560.0,
                 "ch1himidfreq": 1500.0, "ch1hifreq": 8200.0},
        "settings": [
            ("flat", {}),
            ("lomid_boost_bw2", {"ch1lomidenable": "BOOST",
                                 "ch1lomidgain": 20.0, "ch1lomidbw": 2.0}),
            ("lomid_boost_half", {"ch1lomidgain": 10.0}),
            ("lomid_boost_bw1", {"ch1lomidgain": 20.0, "ch1lomidbw": 1.0}),
            ("lomid_boost_bw3", {"ch1lomidgain": 20.0, "ch1lomidbw": 3.0}),
            ("lomid_cut_max", {"ch1lomidenable": "CUT", "ch1lomidgain": 20.0,
                               "ch1lomidbw": 2.0}),
            ("lo_shelf_boost", {"ch1lomidenable": "OUT",
                                "ch1loenable": "BOOST", "ch1logain": 20.0,
                                "ch1loshape": "SHELF"}),
            ("lo_bell_boost", {"ch1loshape": "BELL"}),
        ],
        "distortion_settings": ["flat", "lomid_boost_bw2", "lo_shelf_boost"],
    },

    # TDR SlickEQ has the switch this whole question wants: the same curve
    # with its saturation stage in or out.
    "slickeq": {
        "spec": (VST3 % "TDR VOS SlickEQ", None),
        "note": "TDR VOS SlickEQ, saturation switchable at a fixed curve",
        "base": {"auto_gain": False, "quality": "Full", "mode": "Stereo",
                 "out_gain_db": 0.0, "hp_freq_hz": "Off",
                 "low_freq_hz": 85.0, "mid_freq_hz": 1000.0,
                 "high_freq_hz": 10000.0, "out_stage": "Linear"},
        "settings": [
            ("flat_clean", {"eq_sat": False, "low_gain_db": 0.0,
                            "mid_gain_db": 0.0, "high_gain_db": 0.0}),
            ("low_boost_clean", {"eq_sat": False, "low_gain_db": 10.0}),
            ("low_boost_sat", {"eq_sat": True, "low_gain_db": 10.0}),
            ("flat_sat", {"eq_sat": True, "low_gain_db": 0.0}),
        ],
        "distortion_settings": ["flat_clean", "low_boost_clean",
                                "low_boost_sat", "flat_sat"],
    },

    # A modern SSL channel EQ, as the clean curve reference the analogue
    # ones are being compared against.
    "ssl_native": {
        "spec": (VST3 % "SSL Native Channel Strip 2", None),
        "note": "SSL Native Channel Strip 2 EQ, modern reference",
        "base": {"eq_in": "In", "dynamics_in": "Out", "filters_in": "Out",
                 "input_trim_db": 0.0, "output_trim_db": 0.0,
                 "fader_level_db": 0.0, "eq_type": "E",
                 "lf_frequency_hz": 100.0, "lmf_frequency_khz": 0.56,
                 "hmf_freq_khz": 3.3, "hf_freq_khz": 10.0},
        "settings": [
            ("flat", {"lf_gain_db": 0.0, "lmf_gain_db": 0.0,
                      "hmf_gain_db": 0.0, "hf_gain_db": 0.0}),
            ("lmf_12_e", {"lmf_gain_db": 12.0, "lmf_q": 1.5, "eq_type": "E"}),
            ("lmf_6_e", {"lmf_gain_db": 6.0}),
            ("lmf_12_g", {"lmf_gain_db": 12.0, "eq_type": "G"}),
            ("lf_shelf_12", {"lmf_gain_db": 0.0, "lf_gain_db": 12.0,
                             "eq_type": "E"}),
        ],
        "distortion_settings": ["flat", "lmf_12_e"],
    },
}
