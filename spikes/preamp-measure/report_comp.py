"""Read the compressor measurements back: timings, curves, detector."""

import json
import sys

GRID = ["0.1", "0.2", "0.5", "1", "2", "5", "10", "20", "50", "100",
        "200", "500", "1000", "2000", "5000"]


def show(slug):
    d = json.load(open("out/comp/%s.json" % slug))
    if "error" in d:
        print(slug, "ERROR", d["error"])
        print(d.get("traceback", ""))
        return
    print("\n===== %s -- %s  [%s]" % (slug, d["note"], d["element"]))
    c = d["calibration"]
    print("  calibrated: %s = %s, giving %.1f dB GR at %g dBFS"
          % (c["control"], c["value"], c["measured_gr_db"], d["ref_level_dbfs"]))

    print("  static curve: in dBFS -> gain reduction dB")
    row = d["static_curve"]
    keys = sorted(row, key=float)
    print("    " + "  ".join("%6s" % k for k in keys))
    print("    " + "  ".join("%6.2f" % row[k]["gr_db"] for k in keys))

    print("  timing, ms to 63% and 90% of the final excursion")
    print("    (as_set is the unit as configured; the rest sweep its knobs)")
    print("    %-22s %9s %9s %9s %9s %8s"
          % ("setting", "atk t63", "atk t90", "rel t63", "rel t90", "GR dB"))
    for key, t in d["timing"].items():
        print("    %-22s %9s %9s %9s %9s %8.2f"
              % (key, fmt(t["attack_t63_ms"]), fmt(t["attack_t90_ms"]),
                 fmt(t["release_t63_ms"]), fmt(t["release_t90_ms"]),
                 t["gr_at_end_of_hold_db"]))

    print("  release curve shape, dB of gain reduction still present at t ms")
    print("    %-12s %s" % ("held for", " ".join("%7s" % g for g in GRID)))
    for key, t in d["programme_dependence"].items():
        cur = t["release_curve_db"]
        print("    %-12s %s"
              % (key, " ".join("%7.2f" % cur[g] if g in cur else "%7s" % "-"
                               for g in GRID)))

    det = d["detector"]
    print("  detector: tone %.2f dB GR | %.0f%% duty bursts at the same peak "
          "%.2f dB | tone at the bursts' RMS %.2f dB"
          % (det["gr_continuous_tone_db"], 100 * det["burst_duty"],
             det["gr_bursts_same_peak_db"], det["gr_tone_at_burst_rms_db"]))

    print("  harmonics at %g dB GR, dB below the fundamental"
          % c["measured_gr_db"])
    for f, h in d["harmonics_at_gr"].items():
        print("    %5s Hz  h2 %7.1f  h3 %7.1f  thd %.4f%%"
              % (f, h.get("h2", float("nan")), h.get("h3", float("nan")),
                 h.get("thd_pct", float("nan"))))


def fmt(v):
    return "%9.3f" % v if v is not None else "        -"


if __name__ == "__main__":
    for s in sys.argv[1:]:
        show(s)
