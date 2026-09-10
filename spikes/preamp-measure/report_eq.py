"""Read the EQ measurements back as curves and distortion-vs-frequency."""

import json
import sys


def show(slug):
    d = json.load(open("out/eq/%s.json" % slug))
    if "error" in d:
        print(slug, "ERROR", d["error"])
        return
    print("\n===== %s -- %s" % (slug, d["note"]))
    labels = list(d["settings"])

    print("  magnitude response at %g dBFS, dB" % d["curve_level"])
    print("    %8s | %s" % ("freq", " ".join("%18s" % l for l in labels)))
    for f in d["curve_freqs"]:
        cells = [d["settings"][l]["curve"].get("%g" % f) for l in labels]
        if all(c is None for c in cells):
            continue
        print("    %8.1f | %s"
              % (f, " ".join("%18.2f" % c if c is not None else "%18s" % "-"
                             for c in cells)))

    dist = [l for l in labels if "distortion" in d["settings"][l]]
    if not dist:
        return
    for level in d["dist_levels"]:
        print("  h2 at %g dBFS in, dB below the fundamental" % level)
        print("    %6s | %s" % ("freq", " ".join("%18s" % l for l in dist)))
        for f in d["dist_freqs"]:
            cells = [d["settings"][l]["distortion"]["%d/%g" % (f, level)]
                     for l in dist]
            print("    %6d | %s"
                  % (f, " ".join("%18.1f" % c.get("h2", float("nan"))
                                 for c in cells)))


if __name__ == "__main__":
    for s in sys.argv[1:]:
        show(s)
