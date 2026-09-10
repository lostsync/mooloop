"""Emit the Markdown tables RESULTS.md quotes, straight from the JSON.

Numbers in a document that were retyped from a terminal are numbers that can
drift from the measurement without anything noticing. These are generated.
"""

import json
import sys

PROFILE_HZ, TOP_HZ = 80, 5000


def load(kind, slug):
    return json.load(open("out/%s/%s.json" % (kind, slug)))


def cell(d, label, freq, level):
    return d["settings"][label]["rows"]["%d/%g" % (freq, level)]


def preamp_summary(units, label_of, level=-12.0):
    print("| unit | drive | h2 | h3 | h4 | h5 | h2-h3 | h2 at 5 kHz | tilt |")
    print("| --- | --- | --- | --- | --- | --- | --- | --- | --- |")
    for slug, setting in units:
        d = load("preamp", slug)
        lo, hi = cell(d, setting, PROFILE_HZ, level), cell(d, setting, TOP_HZ, level)
        if lo.get("clipped") or hi.get("clipped"):
            continue
        print("| %s | `%s` | %.1f | %.1f | %.1f | %.1f | %+.1f | %.1f | %+.1f |"
              % (label_of(slug), setting, lo["h2"], lo["h3"], lo["h4"], lo["h5"],
                 lo["h2"] - lo["h3"], hi["h2"], lo["h2"] - hi["h2"]))


def preamp_curve(units, label_of, freqs=(40, 60, 80, 110, 220, 1250, 5000, 10000)):
    print("| unit | drive | " + " | ".join("%d Hz" % f for f in freqs) + " |")
    print("| --- | --- |" + " --- |" * len(freqs))
    for slug, setting in units:
        d = load("preamp", slug)
        ref = cell(d, setting, 1250, d["levels"][0])["gain_db"]
        row = [cell(d, setting, f, d["levels"][0])["gain_db"] - ref for f in freqs]
        print("| %s | `%s` | %s |" % (label_of(slug), setting,
                                      " | ".join("%+.2f" % v for v in row)))


def preamp_tilt_shape(slug, setting, level=-12.0):
    d = load("preamp", slug)
    print("| frequency | " + " | ".join("%d Hz" % f for f in d["freqs"]) + " |")
    print("| --- |" + " --- |" * len(d["freqs"]))
    row = [cell(d, setting, f, level).get("h2") for f in d["freqs"]]
    print("| h2, dB below fundamental | %s |"
          % " | ".join("%.1f" % v if v is not None else "-" for v in row))


def comp_timing(slugs):
    """Marked attack/release settings against what they actually measure."""
    for slug in slugs:
        d = load("comp", slug)
        if "error" in d:
            continue
        c = d["calibration"]
        print("\n**%s** — %s. Calibrated `%s` = %s for %.1f dB of gain "
              "reduction at %g dBFS.\n"
              % (d["note"], d["element"], c["control"], c["value"],
                 c["measured_gr_db"], d["ref_level_dbfs"]))
        print("| setting | attack to 63% | attack to 90% | release to 63% "
              "| release to 90% |")
        print("| --- | --- | --- | --- | --- |")
        for key, t in d["timing"].items():
            print("| `%s` | %s | %s | %s | %s |"
                  % (key, ms(t["attack_t63_ms"]), ms(t["attack_t90_ms"]),
                     ms(t["release_t63_ms"]), ms(t["release_t90_ms"])))


def comp_static(slugs):
    print("| unit | " + " | ".join("%s" % k for k in
                                   ("-36", "-30", "-24", "-18", "-12", "-6", "0")) + " |")
    print("| --- |" + " --- |" * 7)
    for slug in slugs:
        d = load("comp", slug)
        if "error" in d:
            continue
        row = d["static_curve"]
        print("| %s | %s |"
              % (d["note"].split(",")[0],
                 " | ".join("%.1f" % row[k]["gr_db"] if k in row else "-"
                            for k in ("-36", "-30", "-24", "-18", "-12", "-6", "0"))))


def comp_detector(slugs):
    print("| unit | element | tone | 20% bursts, same peak | tone at the "
          "bursts' RMS |")
    print("| --- | --- | --- | --- | --- |")
    for slug in slugs:
        d = load("comp", slug)
        if "error" in d:
            continue
        det = d["detector"]
        print("| %s | %s | %.1f | %.1f | %.1f |"
              % (d["note"].split(",")[0], d["element"],
                 det["gr_continuous_tone_db"], det["gr_bursts_same_peak_db"],
                 det["gr_tone_at_burst_rms_db"]))


def comp_release_shape(slugs, grid=("1", "10", "50", "100", "200", "500",
                                    "1000", "2000", "5000")):
    print("| unit | held | " + " | ".join("%s ms" % g for g in grid) + " |")
    print("| --- | --- |" + " --- |" * len(grid))
    for slug in slugs:
        d = load("comp", slug)
        if "error" in d:
            continue
        for hold, t in d["programme_dependence"].items():
            cur = t["release_curve_db"]
            print("| %s | %s | %s |"
                  % (d["note"].split(",")[0], hold.replace("hold_", ""),
                     " | ".join("%.2f" % cur[g] if g in cur else "-"
                                for g in grid)))


def transient(units):
    """Crest-factor retention and the slew index, at a working level."""
    print("| unit | setting | crest change at -12 dBFS | at -3 dBFS | "
          "slew index at -3 dBFS |")
    print("| --- | --- | --- | --- | --- |")
    for slug, setting in units:
        d = load("transient", slug)
        if "error" in d:
            continue
        rows = d["settings"][setting]["rows"]
        print("| %s | `%s` | %+.2f dB | %+.2f dB | %.3f |"
              % (d["note"], setting, rows["-12"]["crest_change_db"],
                 rows["-3"]["crest_change_db"],
                 rows["-3"].get("slew_index", float("nan"))))


def ms(v):
    return "%.2f ms" % v if v is not None else "-"


if __name__ == "__main__":
    NAMES = {"nls_nevo": "NLS Nevo (Neve 5116)",
             "nls_mike": "NLS Mike (SSL 4000 G+)",
             "nls_spike": "NLS Spike (EMI TG12345)",
             "scheps73": "Scheps 73 (Neve 1073)",
             "ssl_ev2": "SSL EV2 (SSL 4000 E)",
             "bx_console_n": "bx_console N (Neve VXS)"}
    what = sys.argv[1]
    trio = [("nls_nevo", "drive_6"), ("nls_mike", "drive_6"),
            ("nls_spike", "drive_6")]
    if what == "consoles":
        for drive in ("drive_0", "drive_3", "drive_6", "drive_9"):
            preamp_summary([(s, drive) for s, _ in trio], NAMES.get)
    elif what == "curve":
        preamp_curve([(s, d) for s, _ in trio
                      for d in ("drive_0", "drive_6", "drive_12")], NAMES.get)
    elif what == "tilt":
        preamp_tilt_shape(*sys.argv[2:4])
    elif what == "transient":
        pairs = [tuple(a.split(":")) for a in sys.argv[2:]]
        transient(pairs)
    elif what.startswith("comp_"):
        comps = sys.argv[2:] or ["la2a", "cla3a", "1176ln", "distressor",
                                 "dbx160", "ssl_gbus", "api2500",
                                 "fairchild670", "kotelnikov"]
        {"comp_timing": comp_timing, "comp_static": comp_static,
         "comp_detector": comp_detector,
         "comp_release": comp_release_shape}[what](comps)
