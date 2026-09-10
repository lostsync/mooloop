"""Re-measure every unit whose result file is an error stub.

A measurement script writes a JSON either way, so a unit that failed leaves a
file with an `error` key in it rather than no file. That is deliberate -- a
missing file is ambiguous and a recorded traceback is not -- but it means
"what still needs doing" is a question about file contents, which is what
this answers.

Also reports units in the roster that have no file at all, which is the other
way to be missing: a run that was killed part-way through.
"""

import glob
import json
import subprocess
import sys

import roster

SCRIPTS = {"colour": "measure_colour.py", "eq": "measure_eqc.py",
           "comp": "measure_compc.py", "noise": "measure_noise.py"}

# Which families a unit is expected to appear in, given what it is.
EXPECTED = {"colour": ["colour"], "tape": ["colour"], "eq": ["eq"],
            "comp": ["comp"]}


def failed(family):
    out = []
    for path in sorted(glob.glob("out/chart/%s/*.json" % family)):
        d = json.load(open(path))
        if "error" in d:
            out.append((d["slug"], d["error"]))
    return out


def missing(family):
    have = {path.split("/")[-1][:-5]
            for path in glob.glob("out/chart/%s/*.json" % family)}
    want = {slug for slug, u in roster.UNITS.items()
            if family in EXPECTED.get(u["family"], [])}
    return sorted(want - have)


if __name__ == "__main__":
    families = sys.argv[1:] or ["colour", "eq", "comp"]
    dry = "--dry" in families
    families = [f for f in families if not f.startswith("--")] or \
        ["colour", "eq", "comp"]
    for family in families:
        bad = failed(family)
        gone = missing(family)
        slugs = [s for s, _ in bad] + gone
        print("== %s: %d errored, %d missing" % (family, len(bad), len(gone)))
        for slug, err in bad:
            print("   %-22s %s" % (slug, err[:90]))
        for slug in gone:
            print("   %-22s (no file)" % slug)
        if slugs and not dry:
            subprocess.run([sys.executable, SCRIPTS[family]] + slugs,
                           stdout=subprocess.DEVNULL)
