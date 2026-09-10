"""The reference units, and which of Adam's voicings each one speaks for."""

VST3 = "/Library/Audio/Plug-Ins/VST3/%s.vst3"
AU = "/Library/Audio/Plug-Ins/Components/%s.component"
WAVES = "/Library/Audio/Plug-Ins/VST3/WaveShell1-VST3 16.7.vst3"

# slug -> (path, plugin name inside a shell or None, role, note)
CANDIDATES = {
    # ---- preamp / console input stages
    "scheps73":   (WAVES, "Scheps 73 Stereo", "preamp", "Neve 1073 -> Iron"),
    "nls_nevo":   (WAVES, "NLS Channel Stereo", "preamp", "Neve 5116 -> Iron"),
    "ua610b":     (VST3 % "uaudio_610_b", None, "preamp", "UA 610-B tube -> Iron"),
    "century":    (VST3 % "uaudio_century_channel_strip", None, "preamp", "tube strip"),
    "ssl_ev2":    (WAVES, "SSL EV2 Channel Stereo", "preamp", "SSL 4000 E -> Grip"),
    "ssl_native": (VST3 % "SSL Native Channel Strip 2", None, "preamp", "SSL -> Grip"),
    "bx_console_n": (VST3 % "bx_console N", None, "preamp", "Neve VXS"),
    "api_vision": (VST3 % "uaudio_api_vision_channel_strip", None, "preamp", "API -> Punch"),
    "api550a":    (WAVES, "API-550A Stereo", "preamp", "API 550A -> Punch"),

    # ---- EQ
    "massive_passive": (VST3 % "uaudio_manley_massive_passive", None, "eq", "passive/inductor"),
    "puigtec_eqp1a":   (WAVES, "PuigTec EQP1A Stereo", "eq", "Pultec, inductor"),
    "veq4":            (WAVES, "VEQ4 Stereo", "eq", "Neve 1081 inductor"),
    "solid_eq":        (VST3 % "Solid EQ", None, "eq", "SSL 4000, clean reference"),
    "slickeq":         (VST3 % "TDR VOS SlickEQ", None, "eq", "with saturation stage"),

    # ---- compressors, one per gain element
    "la2a":     (VST3 % "uaudio_teletronix_la-2a_tc", None, "comp", "opto"),
    "cla3a":    (WAVES, "CLA-3A Stereo", "comp", "opto"),
    "1176ln":   (VST3 % "uaudio_ua_1176ln_rev_e", None, "comp", "FET"),
    "1176a":    (VST3 % "uaudio_ua_1176_rev_a", None, "comp", "FET, bluestripe"),
    "dbx160":   (VST3 % "uaudio_dbx_160", None, "comp", "VCA"),
    "fairchild670": (VST3 % "uaudio_fairchild_670", None, "comp", "vari-mu"),
    "distressor":   (VST3 % "uaudio_distressor", None, "comp", "FET, programme dependent"),
    "ssl_gbus": (VST3 % "uaudio_ssl_g_bus_compressor", None, "comp", "VCA bus"),
    "api2500":  (WAVES, "API-2500 Stereo", "comp", "VCA, thrust/feedback"),
    "kotelnikov": (VST3 % "TDR Kotelnikov", None, "comp", "clean digital reference"),

    # ---- tape, for the transformer/hysteresis comparison
    "studer_a800": (VST3 % "uaudio_studer_a800", None, "tape", "tape, for contrast"),
}
