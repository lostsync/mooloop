"""Print each surveyed unit's real controls, which is what voicings are written from."""

import glob
import json
import re
import sys

# Waves plugins expose the whole MIDI CC map as parameters; none of it is a
# control anyone sets, and printing it buries the eight that matter.
MIDI = re.compile(
    r"^(control_\d+|.*_(lsb|msb)$|after_touch|all_notes_off|all_sound_off"
    r"|attack_time|balance|bank.*|breath_controller|brightness|celeste_depth"
    r"|channel_volume|chorus_depth|data_.*|decay_time|effect.*|expression.*"
    r"|filter_resonance|foot_controller|general_control_\d+|general_purpose.*"
    r"|harmonic_content|hold_?2?_?pedal.*|legato.*|local_control.*|modulation.*"
    r"|mono_mode_on|mono_operation|nrpn.*|omni_mode.*|pan_position|phaser_depth"
    r"|poly_mode_on|poly_operation|portamento.*|program_number|release_time"
    r"|reset_all.*|reverb.*|rpn.*|soft_pedal.*|sostenuto.*|sound_control_\d+"
    r"|sustain_pedal.*|timbre|tremolo_depth|vibrato.*)$")


def show(slug, width=110):
    d = json.load(open("out/survey/%s.json" % slug))
    if "error" in d:
        print("===== %s  ERROR %s" % (slug, d["error"][:70]))
        return
    print("===== %s -- %s %s [%s]" % (slug, d["maker"], d["models"], d["family"]))
    for key, v in d["parameters"].items():
        if MIDI.match(key):
            continue
        vals = v.get("valid_values")
        rng = ""
        if "min_value" in v:
            rng = " (%s..%s %s)" % (v.get("min_value"), v.get("max_value"),
                                    v.get("units") or "")
        extra = ""
        if vals and len(vals) <= 20:
            extra = "  {%s}" % "|".join(str(x) for x in vals)
        elif vals:
            extra = "  {%s .. %s} n=%d" % (vals[0], vals[-1], len(vals))
        print("   %-24s = %-14s%s%s" % (key, str(v.get("string"))[:14],
                                        rng, extra[:width]))


if __name__ == "__main__":
    args = sys.argv[1:]
    if args and args[0] == "--family":
        import roster
        args = sorted(roster.by_family(args[1]))
    for slug in args or sorted(
            f.split("/")[-1][:-5] for f in glob.glob("out/survey/*.json")):
        show(slug)
