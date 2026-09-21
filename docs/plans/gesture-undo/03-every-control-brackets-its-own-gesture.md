# 03 · Every control brackets its own gesture

The batched markup pass. One crossing into the shared widgets, every control
in it, then one build.

## The controls

Nine in `controls.slint`, one in `envelope.slint`, and the graph handle the EQ
and reverb plots are built from:

**`MiniKnob` already has it. Copy it, do not redesign it.** Its ungated
`edit-started`/`edit-finished` pair (`controls.slint:1653-1654`) is emitted at
`:1747` and `:1763` for the drag, `:1775-1779` for the double-click,
`:1786-1795` for the scroll and `:1810-1816` for the arrow keys -- which is
every path in the section below, already worked out, in the widget most like
the others.

| Widget | Line | Ungated pair | Note |
| --- | --- | --- | --- |
| `MiniKnob` | `controls.slint:1608` | **yes** | the reference; `TrimKnob` (`:1845`) inherits it |
| `ParameterKnob` | `:647` | no | the labelled knob, the one most faces use; gated pair only |
| `KnobField` | `:925` | no | gated pair only; see step 05 for the typed half |
| `TimeDivisionKnob` | `:1116` | no | gated pair only; stepped, but its drag is still a drag |
| `KnobStack` | `:2041` | no | gated pair only |
| `ParameterFader` | `:1240` | no | no pair of any kind |
| `SyncMiniKnob` | `:1951` | no | no pair of any kind |
| `MixerFader` | `:2263` | no | no pair of any kind; its `pointer-event` already switches on `down`/`move`, so this is a `.up` arm. Step 02 |
| `DraggablePoint` | `:28` | n/a | the EQ and reverb plots' handle; `grabbed`/`dragged` already mark the start, so nearly free |
| `EnvelopeEditor` | `envelope.slint:17` | n/a | several handles, one gesture per drag |

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
bracket. `MiniKnob` carries both pairs, mutually exclusive on the same press
(`:1746-1747`), and that is the right answer: merging them looks like a
simplification and would break MIDI learn.

The modulation shelf's existing routing of `edit-started` through
`param-edit-started` (`modulation-shelf.slint:1218-1219`) also stays. It
works, it is the proof the pair is sound, and the global does not replace it
-- it means the other faces never need their own copy of it.

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
