# Gesture undo status

**Not started.** Added 2026-09-21 from Adam's answer to the open question in
`reports/fable-2026-09-21.md` finding 7: build the general gesture pair, fully,
in one batched pass. MOO-50 is the parent issue and predates this plan by three
weeks; the plan is what that issue's *"why it is awkward"* section asked for.

## The problem, stated as what it costs

Undo installs a whole-project snapshot taken when the edit it undoes happened.
So an edit that never reached the history is not merely un-undoable — it is
**destroyed by the next Ctrl+Z, with no redo path**. Turn a filter cutoff,
draw a note, undo the note: the cutoff goes too, silently, and nothing in the
interface said it would.

The unrecorded surfaces are most of the instrument. From
`docs/LOOSE_ENDS.md`, "Edits that do not undo": every device and generator
parameter, every step-grid edit, pattern length, add-pattern, playlist
placements, both renames, preset loads, the MIDI IN and AUDIO input picks, the
sampler Record page's CLIP and LENGTH, and swapping a channel's device kind —
which is the worst of them, because the swap destroys the outgoing device's
state outright.

## Why it has not been done, and what changed

Not for want of noticing. The reason is one sentence: **a knob drag emits a
value on every pointer frame**, so recording per callback would put hundreds of
whole-project snapshots into the history and make one gesture cost dozens of
undos. Something has to say where a gesture starts and stops, and only two
surfaces in the app can — the piano roll (`main.slint:1728-1729`,
`piano-gesture-begin`/`-end`) and the slice editor (`:1941-1942`). Every other
control reports values with no brackets at all.

Three things are true now that were not when MOO-50 was filed, and together
they make this a plan rather than a research problem:

1. **The session side already exists and is proven.** Modulator parameters
   coalesce a whole knob gesture into one entry today:
   `Session::modulation_edit_before` (`session.rs:202`) holds the snapshot
   taken at the first change of a gesture, `modulation_gesture_open`
   (`modulation.rs:83`) suppresses the per-value snapshots while it is open,
   and the entry is recorded when the gesture closes (`session.rs:1195`). The
   work is to generalise that, not to invent it.
2. **A global is the way in, not a per-face callback.** `ControlAssign`
   (`controls.slint:18-24`) is an exported Slint global that every parameter
   control reads and that Rust wires exactly once (`lib.rs:553`). A `Gesture`
   global with `begin()` and `end()` callbacks lets the shared widgets report
   their own brackets **without a single face declaring or forwarding
   anything**. That is the difference between touching nine widgets and
   touching a hundred and ten callbacks, and it is why the "contract change
   across every face" this was feared to be is not what it will cost.
3. **The mixer verbs landed on 2026-09-21 with a stand-in.**
   `with_continuous_history` treats move frames arriving within 400 ms as one
   drag (`mooloop-ui/src/lib.rs`, `CONTINUOUS_GESTURE_GAP`). It works and it is
   a heuristic in the place where the markup already knows the answer.
   Retiring it is step 02's acceptance test.

**One widget has already solved this, and it is the reference
implementation.** `MiniKnob` (`controls.slint:1608`) declares an **ungated**
`edit-started`/`edit-finished` pair at `:1653-1654` -- *"base-value gesture
boundaries, so a caller can coalesce one drag into one undo step"* -- emitted
in the `else` of the assign check (`:1747`, `:1763`) and again from the
double-click, the scroll and the arrow keys (`:1775-1779`, `:1786-1795`,
`:1810-1816`). It covers every path step 03 has to cover, and `TrimKnob`
(`:1845`) inherits it. **Copy its emission sites into the other seven widgets
rather than designing them again.**

It is already wired end to end for one surface: the modulation shelf routes
it (`modulation-shelf.slint:1218-1219`) to `param-edit-started`/`-finished`
in `main.slint`, and Rust turns that into the gesture
`Session::begin_modulation_edit` opens. So the mixer's pan knobs need
*wiring* rather than a new callback. (`LOOSE_ENDS.md` calling the piano roll
and the slice editor the only gesture pair is out of date by this third one,
which is also the best of the three to copy.)

**The trap is narrower than it looks, and it is still a trap.**
`ParameterKnob`, `KnobField`, `TimeDivisionKnob` and `KnobStack` carry only
`modulation-edit-started`/`-finished`, whose every emission is gated on
`assign-active` or `modulation-active` (`controls.slint:847`, `:861`,
`:869-872`, `:880-883`). On those four the bracket looks present and unwired,
and an ordinary value drag emits neither. That pair means *this parameter was
named*, which is what MIDI learn needs; it stays as it is, and what those four
need is `MiniKnob`'s other pair.

## The design in one paragraph

A widget calls `Gesture.begin()` when a value edit starts and `Gesture.end()`
when it finishes. Rust wires those two once. A handler that changes project
state asks the session whether a gesture is open: if it is, the first change
snapshots `before` and the rest change nothing but the eventual `after`; the
entry is recorded when the gesture closes. **One snapshot pair per gesture, not
per frame** — which is also why this is cheaper than the token-coalescing route
`History::record` offers (`history.rs:125-159`), where every frame still pays
for two whole-project clones before the entries are collapsed. Token
coalescing stays for the piano roll, which already uses it, and for anything
that genuinely cannot bracket.

## Steps

| Step | What it does | Size | Issue |
| --- | --- | --- | --- |
| 01 | A check that reports every value callback nothing records — written against the unfixed tree, so its count is this plan's progress bar | small | MOO-62 |
| 02 | The `Gesture` global, and one recorder generalised from the modulation precedent; proved on the mixer's four continuous controls, retiring the 400 ms timer | medium | MOO-63 |
| 03 | Every shared widget brackets its own gesture, including the paths that are not drags: wheel, arrow keys, double-click reset | medium | MOO-64 |
| 04 | Device and generator parameters route through the recorder — the bulk of MOO-50, including the modulated-and-automated base value rule | large | MOO-65 |
| 05 | Typed fields and renames: one entry per editing session, not one per keystroke | medium | MOO-66 |
| 06 | The discrete surfaces that never needed a bracket and were simply never recorded — step grid, pattern length, playlist, presets, the input picks, the device-kind swap | medium | MOO-67 |
| 07 | The check reads zero; `CURRENT.md` and `LOOSE_ENDS.md` stop describing a gap that has closed | small | MOO-68 |

Order matters between 01 and 02 and nowhere else after that: 03 through 06 are
independent of each other once the mechanism exists, and can go to different
sessions in parallel if the markup edits are kept out of each other's way.

## Two things to decide while doing it, not before

**Whether an undo entry is a snapshot or a value.** Everything here assumes the
existing whole-project snapshot, because that is what the history is and
changing it is a different plan. If step 04 finds the snapshot cost per gesture
unacceptable on a large song, the narrower before/after record keyed by
`ParamAddr` that MOO-50 raises is the fallback — measure with
`edit_cost.rs` before reaching for it.

**What a label says.** The history is shown to the user, so sixty parameter
labels are sixty strings somebody writes. The cheap answer is the descriptor's
own name ("Cutoff"), which is already in `EffectKind::descriptors()` and cannot
drift from the face. Decide it in 04 and apply it, rather than inventing a
string per callback.

## Where this sits against FOCUS.md

Outside the sequence, and Adam put it there on 2026-09-21: *"i think the undo
thing should be the next big push."* It is not a feature; it is the thing that
makes every existing feature safe to use, and it is the largest correctness
hole in the application that the user can reach without doing anything unusual.
