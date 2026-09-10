"""Read the harmonic surfaces back as tables a person can argue with."""

import json
import sys


def row(d, label, level, key="h2"):
    r = d["settings"][label]["rows"]
    return [r["%d/%g" % (f, level)] for f in d["freqs"]]


def show(slug, level=-12.0):
    d = json.load(open("out/preamp/%s.json" % slug))
    if "error" in d:
        print(slug, "ERROR", d["error"])
        return
    print("\n===== %s -- %s" % (slug, d["note"]))
    labels = list(d["settings"])
    for key in ("h2", "h3"):
        print("  %s at %g dBFS in, dB below the fundamental" % (key, level))
        print("    %6s | %s" % ("freq", " ".join("%9s" % l for l in labels)))
        for i, f in enumerate(d["freqs"]):
            cells = []
            for l in labels:
                c = row(d, l, level)[i]
                v = c.get(key)
                flag = "c" if c.get("clipped") else " "
                cells.append("%8.1f%s" % (v, flag) if v is not None else "        -")
            print("    %6d | %s" % (f, " ".join(cells)))
    print("  response at %g dBFS (the voicing curve), dB" % d["levels"][0])
    print("    %6s | %s" % ("freq", " ".join("%9s" % l for l in labels)))
    for i, f in enumerate(d["freqs"]):
        cells = [row(d, l, d["levels"][0])[i].get("gain_db", 0) for l in labels]
        print("    %6d | %s" % (f, " ".join("%9.2f" % c for c in cells)))


if __name__ == "__main__":
    for s in sys.argv[1:]:
        show(s)
