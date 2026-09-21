# 02 · The gesture global, and one recorder

The mechanism, proved on the smallest surface that already wants it.

## The markup half

A new global beside `ControlAssign` in `controls.slint`:

```slint
export global Gesture {
    callback begin();
    callback end();
}
```

`ControlAssign` is the precedent for the shape and for why it is a global: it
is read by every parameter control in every face and wired in Rust exactly
once (`lib.rs:553`), with no face declaring, forwarding or even knowing about
it. A `Gesture` global inverts the direction — the widget calls *out* rather
than reading *in* — and buys the same thing, which is that **no face is
touched at all**. The feared `main.slint` contract change across every device
face is not what this costs.

Wire it once in Rust, next to the `ControlAssign` wiring, so the two live
together and the reason is written once.

## The Rust half

Generalise what modulation already does. Today: `modulation_edit_before:
Option<ProjectSnapshot>` (`session.rs:202`) holds the snapshot taken at the
first change of a gesture, `modulation_gesture_open` (`modulation.rs:83`)
tells a handler a gesture is in flight, and the entry is recorded when it
closes (`session.rs:1195`). Rename that pair to something that is not about
modulation, keep the mechanism exactly, and let the modulation path keep using
it — the modulation handlers should end up *shorter*, not longer.

The rule a handler follows:

- **Gesture open, first change:** snapshot `before`, apply the edit, record
  nothing yet.
- **Gesture open, later changes:** apply the edit. Nothing else.
- **Gesture closes:** record one entry, `before` to the project as it now is.
- **No gesture open:** snapshot, apply, record — one entry, as a discrete
  edit already does.

That last line is what makes step 06's surfaces work without a bracket, and
what makes a wheel-click or an arrow-key nudge correct even before step 03
gives those paths their own pair.

**One snapshot pair per gesture, not per frame.** This is the reason not to
reach for `History::record`'s token coalescing (`history.rs:125-159`) here:
that route works — the piano roll uses it — but it pays two whole-project
clones on every move frame and then throws all but the first and last away.

## Two failure modes to design for now

**A gesture that never ends.** Pointer capture lost, a window closed
mid-drag, a face torn down under a popup. Left open, it would swallow every
later edit into one entry. Two rules: `begin()` while a gesture is open closes
the previous one first, and the recorder closes an open gesture on project
install. Both are cheap; neither is obvious after the fact.

**A gesture that spans an install.** Undo, redo and every structural edit
replace the project. A `before` snapshot from the previous document must not
be recorded against the new one. Close and discard on `replace_project` — the
same place the refused-command latch is now cleared (`session.rs`).

## The proof

The mixer's four continuous controls — channel volume and pan, bus volume and
pan — switch from `with_continuous_history` to the recorder, and
`with_continuous_history`, `continues_gesture` and `CONTINUOUS_GESTURE_GAP`
are **deleted**. Their replacement is not a heuristic, and the diff that
deletes them is the evidence.

`MixerFader` (`controls.slint:2263`), `MiniKnob` (`:1608`) and `TrimKnob`
(`:1845`) are the widgets involved, which is three of the nine step 03 covers
— so this step also proves the widget edit is small before the rest are done
in bulk.

## Done when

- [ ] `Gesture.begin()`/`end()` exists, is wired once in Rust, and no face
      declares or forwards anything.
- [ ] The modulation gesture is the same mechanism under a general name, and
      modulation's own behaviour is unchanged — its existing tests pass
      untouched.
- [ ] A fader drag is one undo entry, with one snapshot pair taken, not one
      per frame. Assert the snapshot count, not just the entry count: the
      entry count was already right under the timer.
- [ ] A gesture left open by a lost pointer, and one interrupted by an
      install, each end without swallowing the next edit. Both tested.
- [ ] The 400 ms timer and its test are gone from `mooloop-ui`.
