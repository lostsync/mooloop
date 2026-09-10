"""Every unit in the comparison, and which comparison it is in.

The first pass picked units to answer questions about mooloop's own modules,
so it measured whatever settled the argument and stopped. This one is a
survey: the reader is a studio person deciding what to reach for, the output
is charts, and a chart needs every unit on it measured at the same places.

Two fields do the work the first roster had no need for:

`group` is what a chart puts on one set of axes. Four emulations of an 1176
share `fet-1176`, so "do these agree?" is a query rather than a judgement.

`voice` is the drive/character control, as (parameter, low, high). It is what
lets a unit be driven to a *stated distortion* rather than to a stated knob
position, which is the only way two makers' controls can be put on one axis.
`None` means the unit has no such control and is measured as it comes.

Paths, and the two rules about them from the first pass that still hold:
UAD's Audio Unit builds do not scan and their VST3 builds do, and Waves
choice values carry a trailing space that is theirs, not a typo.
"""

import os

VST3 = "/Library/Audio/Plug-Ins/VST3/%s.vst3"
WAVES = "/Library/Audio/Plug-Ins/VST3/WaveShell1-VST3 16.7.vst3"
AW = "aw"          # handled by aw.py, not a path

SOUNDTOYS = "/Library/Audio/Plug-Ins/VST3/Soundtoys/%s.vst3"
KILOHERTZ = "/Library/Audio/Plug-Ins/VST3/Kilohearts/%s.vst3"


def _u(slug, spec, maker, models, family, group, voice=None, note=""):
    return (slug, {"spec": spec, "maker": maker, "models": models,
                   "family": family, "group": group, "voice": voice,
                   "note": note})


_ROWS = [
    # ================================================== console channels
    _u("nls_nevo", (WAVES, "NLS Channel Stereo"), "Waves",
       "Neve 5116 console channel", "colour", "console-neve",
       ("drive", 0.0, 12.0)),
    _u("nls_mike", (WAVES, "NLS Channel Stereo"), "Waves",
       "SSL 4000 G+ console channel", "colour", "console-ssl",
       ("drive", 0.0, 12.0)),
    _u("nls_spike", (WAVES, "NLS Channel Stereo"), "Waves",
       "EMI TG12345 console channel", "colour", "console-emi",
       ("drive", 0.0, 12.0)),
    _u("nlsbuss_nevo", (WAVES, "NLS Buss Stereo"), "Waves",
       "Neve 5116 console buss", "colour", "console-neve",
       ("drive", 0.0, 12.0), "the summing side of the same three consoles"),
    _u("nlsbuss_mike", (WAVES, "NLS Buss Stereo"), "Waves",
       "SSL 4000 G+ console buss", "colour", "console-ssl",
       ("drive", 0.0, 12.0)),
    _u("nlsbuss_spike", (WAVES, "NLS Buss Stereo"), "Waves",
       "EMI TG12345 console buss", "colour", "console-emi",
       ("drive", 0.0, 12.0)),
    _u("bx_console_n", (VST3 % "bx_console N", None), "Brainworx",
       "Neve VXS console channel", "colour", "console-neve", None,
       "THD control is calibrated in dB and lives inside the EQ section"),
    _u("ssl_ev2", (WAVES, "SSL EV2 Channel Stereo"), "Waves",
       "SSL 4000 E console channel", "colour", "console-ssl", None),
    _u("ssl_native", (VST3 % "SSL Native Channel Strip 2", None), "SSL",
       "SSL 9000/4000 channel strip", "colour", "console-ssl", None,
       "modern SSL, the clean reference"),
    _u("api_vision", (VST3 % "uaudio_api_vision_channel_strip", None), "UAD",
       "API 1608 console channel", "colour", "console-api", None,
       "preamp section; its EQ and compressor are measured separately"),
    _u("century", (VST3 % "uaudio_century_channel_strip", None), "UAD",
       "Universal Audio Century tube strip", "colour", "preamp-tube", None),

    # ================================================== discrete preamps
    _u("scheps73", (WAVES, "Scheps 73 Stereo"), "Waves",
       "Neve 1073 mic preamp", "colour", "preamp-neve", None,
       "preamp drive is a switch, not a continuum"),
    _u("ua610a", (VST3 % "uaudio_610_a", None), "UAD",
       "UA 610-A tube preamp", "colour", "preamp-tube", None),
    _u("ua610b", (VST3 % "uaudio_610_b", None), "UAD",
       "UA 610-B tube preamp", "colour", "preamp-tube", None),

    # ================================================== broadband colour
    _u("frontdaw_british", (VST3 % "FrontDAW", None), "Sonimus",
       "British console style", "colour", "style-frontdaw",
       ("mojo", 0.0, 100.0)),
    _u("frontdaw_american", (VST3 % "FrontDAW", None), "Sonimus",
       "American console style", "colour", "style-frontdaw",
       ("mojo", 0.0, 100.0)),
    _u("frontdaw_german", (VST3 % "FrontDAW", None), "Sonimus",
       "German console style", "colour", "style-frontdaw",
       ("mojo", 0.0, 100.0)),
    _u("frontdaw_magnetic", (VST3 % "FrontDAW", None), "Sonimus",
       "tape style", "colour", "style-frontdaw", ("mojo", 0.0, 100.0)),
    _u("frontdaw_clip", (VST3 % "FrontDAW", None), "Sonimus",
       "clipper style", "colour", "style-frontdaw", ("mojo", 0.0, 100.0)),
    _u("decap_a", (SOUNDTOYS % "Decapitator", None), "Soundtoys",
       "Ampex 350 tape preamp (style A)", "colour", "style-decapitator", None),
    _u("decap_e", (SOUNDTOYS % "Decapitator", None), "Soundtoys",
       "Chandler/EMI TG12413 (style E)", "colour", "style-decapitator", None),
    _u("decap_n", (SOUNDTOYS % "Decapitator", None), "Soundtoys",
       "Neve 1057 (style N)", "colour", "style-decapitator", None),
    _u("decap_t", (SOUNDTOYS % "Decapitator", None), "Soundtoys",
       "Thermionic Culture Vulture triode (style T)", "colour",
       "style-decapitator", None),
    _u("decap_p", (SOUNDTOYS % "Decapitator", None), "Soundtoys",
       "Thermionic Culture Vulture pentode (style P)", "colour",
       "style-decapitator", None),
    _u("little_radiator", (SOUNDTOYS % "LittleRadiator", None), "Soundtoys",
       "Altec 1566A tube stage", "colour", "preamp-tube", None),
    _u("devil_loc", (SOUNDTOYS % "Devil-Loc", None), "Soundtoys",
       "Shure Level-Loc", "colour", "crush", None),
    _u("driver", (VST3 % "Driver", None), "Native Instruments",
       "analogue drive and filter", "colour", "crush", None),

    # ================================================== tape
    _u("studer_a800", (VST3 % "uaudio_studer_a800", None), "UAD",
       "Studer A800 multitrack", "tape", "tape-machine", None),
    _u("verve", (VST3 % "uaudio_verve", None), "UAD",
       "Verve analogue machines", "tape", "tape-machine", None),

    # ================================================== EQ
    _u("puigtec_eqp1a", (WAVES, "PuigTec EQP1A Stereo"), "Waves",
       "Pultec EQP-1A", "eq", "eq-pultec", None),
    _u("puigtec_meq5", (WAVES, "PuigTec MEQ5 Stereo"), "Waves",
       "Pultec MEQ-5 midrange", "eq", "eq-pultec", None),
    _u("passive_eq", (VST3 % "Passive EQ", None), "Softube",
       "Manley Massive Passive", "eq", "eq-massive-passive", None),
    _u("massive_passive", (VST3 % "uaudio_manley_massive_passive", None),
       "UAD", "Manley Massive Passive", "eq", "eq-massive-passive", None),
    _u("massive_passive_m",
       (VST3 % "uaudio_manley_massive_passive_m", None), "UAD",
       "Manley Massive Passive MST", "eq", "eq-massive-passive", None),
    _u("veq3", (WAVES, "VEQ3 Stereo"), "Waves", "Neve 1073 EQ", "eq",
       "eq-neve", None),
    _u("veq4", (WAVES, "VEQ4 Stereo"), "Waves", "Neve 1081 EQ", "eq",
       "eq-neve", None),
    _u("api550a", (WAVES, "API-550A Stereo"), "Waves", "API 550A", "eq",
       "eq-api", None, "stepped: three bands, seven frequencies each"),
    _u("api550b", (WAVES, "API-550B Stereo"), "Waves", "API 550B", "eq",
       "eq-api", None, "stepped: four bands"),
    _u("sieq", (SOUNDTOYS % "SieQ", None), "Soundtoys", "API 550-style", "eq",
       "eq-api", None),
    _u("api_vision_eq", (VST3 % "uaudio_api_vision_channel_strip", None),
       "UAD", "API 550L EQ section", "eq", "eq-api", None),
    # Softube's Solid EQ is deliberately absent. Its gain and frequency
    # controls accept a write and take effect, but report their old value
    # back, so a measurement could be taken and could not be labelled with
    # the setting that produced it. SSL's own plugin carries the SSL EQ
    # comparison, in both its E and G curves.
    _u("ssl_native_eq", (VST3 % "SSL Native Channel Strip 2", None), "SSL",
       "SSL 9000 channel EQ, E and G curves", "eq", "eq-ssl", None),
    _u("slickeq", (VST3 % "TDR VOS SlickEQ", None), "Tokyo Dawn",
       "switchable saturation at a fixed curve", "eq", "eq-digital", None),
    _u("tdr_nova", (VST3 % "TDR Nova", None), "Tokyo Dawn",
       "clean parallel dynamic EQ", "eq", "eq-digital", None),
    _u("kh_3band", (KILOHERTZ % "kHs 3-Band EQ", None), "Kilohearts",
       "clean three-band", "eq", "eq-digital", None),
    _u("q10", (WAVES, "Q10 Stereo"), "Waves", "clean parametric", "eq",
       "eq-digital", None),
    _u("elysia_niveau", (VST3 % "elysia niveau filter", None), "elysia",
       "passive tilt filter", "eq", "eq-tilt", None),

    # ================================================== compressors: FET
    _u("1176ln", (VST3 % "uaudio_ua_1176ln_rev_e", None), "UAD",
       "UREI 1176LN Rev E", "comp", "fet-1176", None),
    _u("1176a", (VST3 % "uaudio_ua_1176_rev_a", None), "UAD",
       "UREI 1176 Rev A bluestripe", "comp", "fet-1176", None),
    _u("1176ae", (VST3 % "uaudio_ua_1176ae", None), "UAD",
       "UREI 1176AE anniversary", "comp", "fet-1176", None),
    _u("vc76", (VST3 % "VC 76", None), "Softube", "UREI 1176", "comp",
       "fet-1176", None),
    _u("distressor", (VST3 % "uaudio_distressor", None), "UAD",
       "Empirical Labs Distressor", "comp", "fet-other", None),

    # ================================================== compressors: opto
    _u("la2a", (VST3 % "uaudio_teletronix_la-2a_tc", None), "UAD",
       "Teletronix LA-2A", "comp", "opto-la2a", None),
    _u("vc2a", (VST3 % "VC 2A", None), "Softube", "Teletronix LA-2A", "comp",
       "opto-la2a", None),
    _u("cla3a", (WAVES, "CLA-3A Stereo"), "Waves", "UREI LA-3A", "comp",
       "opto-la2a", None),
    _u("bx_opto", (VST3 % "bx_opto", None), "Brainworx", "opto pedal", "comp",
       "opto-other", None),

    # ================================================== compressors: VCA
    _u("dbx160", (VST3 % "uaudio_dbx_160", None), "UAD", "dbx 160 VU", "comp",
       "vca-dbx160", None),
    _u("vc160", (VST3 % "VC 160", None), "Softube", "dbx 160 VU", "comp",
       "vca-dbx160", None),
    _u("ssl_gbus", (VST3 % "uaudio_ssl_g_bus_compressor", None), "UAD",
       "SSL G series bus compressor", "comp", "vca-sslbus", None),
    _u("solid_bus_comp", (VST3 % "Solid Bus Comp", None), "Softube",
       "SSL G series bus compressor", "comp", "vca-sslbus", None),
    _u("ssl_native_bus", (VST3 % "SSL Native Bus Compressor 2", None), "SSL",
       "SSL G series bus compressor", "comp", "vca-sslbus", None),
    _u("solid_dynamics", (VST3 % "Solid Dynamics", None), "Softube",
       "SSL 4000 channel dynamics", "comp", "vca-sslchannel", None),
    _u("api2500", (WAVES, "API-2500 Stereo"), "Waves", "API 2500", "comp",
       "vca-api", None),
    _u("api_vision_comp", (VST3 % "uaudio_api_vision_channel_strip", None),
       "UAD", "API 527 channel compressor", "comp", "vca-api", None),

    # ================================================== compressors: vari-mu
    _u("fairchild670", (VST3 % "uaudio_fairchild_670", None), "UAD",
       "Fairchild 670 stereo", "comp", "varimu-fairchild", None),
    _u("fairchild660", (VST3 % "uaudio_fairchild_660", None), "UAD",
       "Fairchild 660 mono", "comp", "varimu-fairchild", None),
    _u("vari_comp", (VST3 % "Vari Comp", None), "Softube",
       "Manley Variable Mu", "comp", "varimu-other", None),
    _u("vcomp", (WAVES, "VComp Stereo"), "Waves", "Neve 2254 diode bridge",
       "comp", "diode-neve", None),

    # ================================================== compressors: digital
    _u("kotelnikov", (VST3 % "TDR Kotelnikov", None), "Tokyo Dawn",
       "clean digital, states its times", "comp", "digital", None),
    _u("molotok", (VST3 % "TDR Molotok", None), "Tokyo Dawn",
       "digital with a modelled character", "comp", "digital", None),
    _u("kh_comp", (KILOHERTZ % "kHs Compressor", None), "Kilohearts",
       "clean digital", "comp", "digital", None),
    _u("rcomp", (WAVES, "RCompressor Stereo"), "Waves",
       "Renaissance, opto and electro modes", "comp", "digital", None),
    _u("h_comp", (WAVES, "H-Comp Stereo"), "Waves",
       "hybrid, analogue-modelled", "comp", "digital", None),
    _u("c1_comp", (WAVES, "C1 comp Stereo"), "Waves", "clean digital", "comp",
       "digital", None),
    _u("supercharger_gt", (VST3 % "Supercharger GT", None),
       "Native Instruments", "tube compressor", "comp", "opto-other", None),

    # ================================================== Airwindows
    # Chris Johnson's, all free, all one-idea-per-plugin, and the reason they
    # are worth having beside the emulations is that they are *not* models of
    # anything -- so they mark where the axis ends.
    _u("aw_console7channel", (AW, "Console7Channel"), "Airwindows",
       "Console7 channel encode", "colour", "aw-console", None),
    _u("aw_consolemcchannel", (AW, "ConsoleMCChannel"), "Airwindows",
       "ConsoleMC channel", "colour", "aw-console", None),
    _u("aw_consolemdchannel", (AW, "ConsoleMDChannel"), "Airwindows",
       "ConsoleMD channel", "colour", "aw-console", None),
    _u("aw_consolelachannel", (AW, "ConsoleLAChannel"), "Airwindows",
       "ConsoleLA channel", "colour", "aw-console", None),
    _u("aw_console9channel", (AW, "Console9Channel"), "Airwindows",
       "Console9 channel", "colour", "aw-console", None),
    _u("aw_desk4", (AW, "Desk4"), "Airwindows", "desk colour", "colour",
       "aw-drive", None),
    _u("aw_density2", (AW, "Density2"), "Airwindows", "density saturation",
       "colour", "aw-drive", ("density", 0.0, 1.0)),
    _u("aw_drive", (AW, "Drive"), "Airwindows", "distortion", "colour",
       "aw-drive", None),
    _u("aw_purestdrive", (AW, "PurestDrive"), "Airwindows",
       "minimal tube-ish drive", "colour", "aw-drive", ("drive", 0.0, 1.0)),
    _u("aw_focus", (AW, "Focus"), "Airwindows", "focused saturation",
       "colour", "aw-drive", None),
    _u("aw_cojones", (AW, "Cojones"), "Airwindows", "midrange grit", "colour",
       "aw-drive", None),
    _u("aw_loud", (AW, "Loud"), "Airwindows", "loudness saturation", "colour",
       "aw-drive", None),
    _u("aw_hype", (AW, "Hype"), "Airwindows", "hyped saturation", "colour",
       "aw-drive", None),
    _u("aw_mackity", (AW, "Mackity"), "Airwindows", "Mackie 1202 preamp",
       "colour", "aw-drive", None),
    _u("aw_calibre", (AW, "Calibre"), "Airwindows", "Calrec-ish console",
       "colour", "aw-console", None),
    _u("aw_totape7", (AW, "ToTape7"), "Airwindows",
       "tape, the flagship algorithm", "tape", "aw-tape", None),
    _u("aw_tape", (AW, "Tape"), "Airwindows", "tape, the simple one", "tape",
       "aw-tape", None),
    _u("aw_ironoxide5", (AW, "IronOxide5"), "Airwindows", "tape", "tape",
       "aw-tape", None),
    _u("aw_ironoxideclassic2", (AW, "IronOxideClassic2"), "Airwindows",
       "tape, the older algorithm", "tape", "aw-tape", None),
    _u("aw_chromeoxide", (AW, "ChromeOxide"), "Airwindows", "cassette tape",
       "tape", "aw-tape", None),
    _u("aw_fromtape", (AW, "FromTape"), "Airwindows", "tape playback",
       "tape", "aw-tape", None),
    _u("aw_dubly2", (AW, "Dubly2"), "Airwindows",
       "companding noise reduction", "tape", "aw-tape", None),
    _u("aw_baxandall2", (AW, "Baxandall2"), "Airwindows",
       "Baxandall tone control", "eq", "eq-tilt", None),
    _u("aw_capacitor2", (AW, "Capacitor2"), "Airwindows",
       "capacitor-modelled filter", "eq", "eq-tilt", None),
    _u("aw_air3", (AW, "Air3"), "Airwindows", "top-end lift", "eq",
       "eq-tilt", None),
    _u("aw_buttercomp2", (AW, "ButterComp2"), "Airwindows",
       "bidirectional compressor", "comp", "aw-comp", None),
    _u("aw_pressure5", (AW, "Pressure5"), "Airwindows", "peak compressor",
       "comp", "aw-comp", None),
    _u("aw_blockparty", (AW, "BlockParty"), "Airwindows", "blocky compressor",
       "comp", "aw-comp", None),
    _u("aw_pop2", (AW, "Pop2"), "Airwindows", "transient-preserving comp",
       "comp", "aw-comp", None),
    _u("aw_logical4", (AW, "Logical4"), "Airwindows", "mastering compressor",
       "comp", "aw-comp", None),
    _u("aw_purestsquish", (AW, "PurestSquish"), "Airwindows",
       "minimal compressor", "comp", "aw-comp", None),
    _u("aw_compresaturator", (AW, "Compresaturator"), "Airwindows",
       "compression by saturation", "comp", "aw-comp", None),
]

UNITS = dict(_ROWS)


def open_unit(slug):
    """Load a unit however its format needs loading.

    Everything but Airwindows is a path and a plugin name. Airwindows is one
    shared VST3 shell asked to become one of four hundred processors, which
    `aw.select` does and verifies -- so the two cases cannot be collapsed and
    every measurement script goes through here rather than through
    `mlab.open_plugin` directly.
    """
    import mlab
    path, name = UNITS[slug]["spec"]
    if path == AW:
        import aw
        return aw.select(name)
    return mlab.open_plugin(path, name)

FAMILIES = ("colour", "tape", "eq", "comp")


def by_family(family):
    return [s for s, u in UNITS.items() if u["family"] == family]


def groups():
    out = {}
    for slug, u in UNITS.items():
        out.setdefault(u["group"], []).append(slug)
    return out


if __name__ == "__main__":
    for family in FAMILIES:
        rows = by_family(family)
        print("%-8s %d" % (family, len(rows)))
    print("total   %d units in %d groups" % (len(UNITS), len(groups())))
