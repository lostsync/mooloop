# 03 · Every control brackets its own gesture

The batched markup pass. One crossing into the shared widgets, every control
in it, then one build.

## The controls

Nine in `controls.slint`, one in `envelope.slint`, and the graph handle the EQ
and reverb plots are built from:

| Widget | Line | Note |
| --- | --- | --- |
| `ParameterKnob` | `controls.slint:647` | the labelled knob; the one most faces use |
| `KnobField` | `:925` | knob plus a typed field — see step 05 for the field half |
| `TimeDivisionKnob` | `:1116` | stepped, so its drag is still a drag |
| `ParameterFader` | `:1240` | |
| `MiniKnob` | `:1608` | and `TrimKnob` (`:1845`), which inherits it |
| `SyncMiniKnob` | `:1951` | |
| `KnobStack` | `:2041` | |
| `MixerFader` | `:2263` | done in step 02 |
| `DraggablePoint` | `:28` | the EQ and reverb plots' handle; `grabbed`/`dragged` already mark the start, so this one is nearly free |
| `EnvelopeEditor` | `envelope.slint:17` | several handles, one gesture per drag |

Adding the pair to the widget covers **every face that uses it**, which is the
whole point of doing it here rather than per face.

## The paths that are not drags

This is the part a pass over the pointer handlers misses, and each of these is
a real way to change a value today:

- **The scroll wheel** (`controls.slint:878-884` in the knob's
  `scroll-event`). One notch is one edit. Bracket each notch as
  `begin(); change; end()` — or let it fall through to the recorder's
  no-gesture path, which already makes it one entry. Prefer the fall-through:
  fewer calls, same result, and it is the rule step 02 wrote down.
- **Arrow keys** (`:889-905`, the knob's `FocusScope`). Same answer, with one
  difference worth thinking about: holding an arrow key repeats, and a held
  repeat reads as a gesture to the user. Either is defensible; if you bracket
  it, the key-release is the end.
- **Double-click reset** (`:864-877`). One discrete edit. No bracket needed;
  it must produce exactly one entry and must not be swallowed by a gesture
  that a stray press left open.
- **A typed value** in `KnobField` and the toolbar fields. Step 05.

## What not to touch

`modulation-edit-started`/`-finished` stays exactly as it is, gated as it is.
It means *this parameter was named*, which is what MIDI learn and modulation
assignment need (`controls.slint:19-23` says so), and it is not a value
bracket. Two pairs on one widget is the right answer here, and the plan says
so out loud because merging them looks like a simplification and would break
MIDI learn.

## How to work it

`scripts/slint-sketch` type-checks a widget file in about 0.05 s, and the
whole real window in about 2.7 s — so the markup iterates locally with no
build at all. Cross into a `cargo check -p mooloop-ui` **once**, when every
widget is done. `AGENTS.md`'s device-work ordering is written for exactly this
shape, and in a cloud container the check is about five minutes and the test
run about sixteen, so a pass that batches is the difference between one
afternoon and three.

## Done when

- [ ] Every widget in the table emits `Gesture.begin()`/`end()` around a
      pointer drag, ungated.
- [ ] Wheel, arrow-key and double-click paths each produce exactly one undo
      entry per user action, whichever way step 02's rule is applied to them.
- [ ] No face file changed. If one had to, say why in the commit — it means
      a control was built inline instead of from the vocabulary, which is
      `WIDGET_INVENTORY.md`'s problem and worth knowing about.
- [ ] `slint_face_agreement.rs`'s hand-written face list is unchanged and
      still passes — this step adds no face and no parameter.
