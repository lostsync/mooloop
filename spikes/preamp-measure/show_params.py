"""Print a unit's real parameters, minus the MIDI CCs Waves plugins expose."""

import json
import re
import sys

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

for slug in sys.argv[1:]:
    d = json.load(open("out/probe/%s.json" % slug))
    if "error" in d:
        print("=====", slug, "ERROR")
        continue
    rows = [(k, v) for k, v in d["parameters"].items() if not MIDI.match(k)]
    print("=====", slug, "--", d["note"])
    for k, v in rows:
        print("   %-32s %s" % (k, str(v.get("string"))[:28]))
