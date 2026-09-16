# 03 — The drag

The gesture, and the only step that crosses `main.slint`.

## The shape: drag pattern A, horizontally

The device rack (`RackDrag`, `device-rack.slint`) and the channel rack
(`ChannelDrag`, `channel-rack.slint`) share one pattern. The **grab** belongs
to the element that knows which item it is. The **landing** is reported by
whichever slot the pointer is over, from that slot's own bounds. The slots
between the grab and the landing slide aside by the dragged slot's pitch, and
the gap they open is the drop indicator. The mixer is a horizontal row, so
this is `RackDrag`'s axis with `ChannelDrag`'s two-index callback.

Use `absolute-position.x + mouse-x`, never `mouse-x - pressed-x`.
`RackDrag.dx`, `PaneDrag` and `ChannelDrag` all record the reason: the touch
area is inside the element that slides.

### A fourth global, not a shared one

`TrackDrag` goes in `mixer.slint`, next to `MixerPane`, and has
`ChannelDrag`'s fields with `x` in place of `y`. Reusing `RackDrag` looks
cheaper, but every device-rack row reads `RackDrag.active`. If both panes
are on screen, a mixer drag would slide the rack's rows as well. A Slint
global cannot be instantiated twice, so a separate global is the only way to
keep two lists separate.

The four globals do state one pattern four times, and that is a fair
objection. The part that could drift is the `shift` expression, the
three-way test that decides which slots slide and in which direction. Write
it **once**, as a pure function in a `.slint` file all three racks import:

```slint
export global ReorderMath {
    // How far slot `i` slides while `source` is held over `target`.
    pure public function shift(i: int, source: int, target: int, pitch: length) -> length { ... }
}
```

Switch `TrackDrag` over to it now. Switching the two existing racks is a
follow-up and not part of this plan. Note it in `LOOSE_ENDS.md` so it is not
lost.

## The grab: the strip's name plate

`MixerStrip`'s name plate (`mixer.slint`, the `name-touch` `TouchArea`)
currently has a `clicked => selected()` and nothing else. Replace it with a
`pointer-event` handler shaped like the channel rack's name plate
(`main.slint`, the `ChannelDrag.source = ch` block):

- **Down:** select the strip. That is the existing click, so keep the
  `root.selected()` call. Then, unless `project-edit-pending` is set, seed
  `source`, `target`, `grab-x` and `pointer-x`, and set `active`. There is
  no movement threshold, and the channel rack does not need one either: a
  press that never leaves its own strip ends with `target == source`, which
  already means "no move".
- **Move:** update `pointer-x` and `dx`.
- **Up:** if `target != source`, call
  `reorder-requested(source, clamp(target, 1, count - 1))`, then `end()`.

`MixerPane` does not know `project-edit-pending` today. It needs the property
passed in, for the reason the channel plate reads it: a drag started while a
previous drop is still installing would be computed against the old order.

**The master refuses at the grab**, following `UI_DESIGN.md`'s rule for a
drag that is not allowed. When `strip.is-master` is true, its name plate
selects and never starts a drag. It is also never a landing: the slot's
`hot` is false for the master, and the clamp above keeps a release over it
from asking for seat 0. Step 01 refuses seat 0 in the model as well. The two
checks are independent on purpose.

The plate is `mouse-cursor: pointer` today. Change it to `move` on a
non-master strip, which is the channel plate's cursor, so both plates mean
the same thing.

## The landing: the strip slot

In `MixerPane`'s `for strip[i] in root.strips`, wrap `MixerStrip` in a slot
that keeps its place in the `HorizontalLayout`. Its contents slide:

- `hot`: `TrackDrag.active && !strip.is-master` and the pointer's x is
  inside this slot's own `absolute-position.x .. + width`. On change, set
  `TrackDrag.target = i`.
- On `dragging`, publish `TrackDrag.source-width = self.width + 4px`. The 4px
  is the row's spacing, which a slot's pitch includes. Read it from the
  layout's `spacing`. Do not type it in a second place.
- The contents' `x` is `ReorderMath.shift(...)`, plus `TrackDrag.dx` for the
  dragged strip. Animate the slide with `Motion.duration` / `Motion.curve`,
  which the device rack's slide-aside already uses. The dragged strip itself
  does not animate: it follows the pointer.
- The dragged strip is drawn above its neighbours (`z`) and takes the dimmed
  treatment the pane tab drag uses at its origin.

The `+` button after the strips is not a slot and is never a landing.

`MixerPane` gains `callback track-reorder-requested(int, int)`.

## The crossing

The `main.slint` change is **two lines** on the `MixerPane` instance: the
callback forwarded, and `project-edit-pending` passed in. It also adds one
callback on `MainWindow`, `callback track-reorder-requested(int, int)`. If step 04 is being done, its
two action ids go into this same pass.

Rust side (`crates/mooloop-ui/src/lib.rs`):

- `queue_track_move(tx, state, window, from, to) -> bool`, next to
  `queue_track_remove`. Clone the snapshot, call `project.move_track`,
  return `false` on `None`, and send it through `queue_structural_edit`
  with `Some(ListEdit::Track(edit))`, labelled `"Track moved"`. There is no
  samples sidecar to rotate, because samples belong to channels.
- `window.on_track_reorder_requested`, a copy of
  `on_channel_reorder_requested`, including the `project_edit_pending`
  guard. That guard stops a second drop from landing on a document the
  first one has not finished installing.

## Iterate without building

Do all of the markup above against `scripts/slint-sketch` with
`crates/mooloop-ui/ui/mixer.slint` and a small scratch file that
instantiates `MixerPane` with four strips. Build `mooloop-ui` once, on the
box, once the sketch looks right. `AGENTS.md` has the numbers: 0.05s against
four minutes.

## Test: `tests/track_reorder.rs`

Model it on `tests/channel_reorder.rs`, driven by real pointer events, with
computed coordinates. **Differ from it in one respect.**
`channel_reorder.rs` rebuilds the rack row inside its own `slint!` block, so
it tests a copy of the markup `main.slint` actually runs. That is the
one-sided-test shape `AGENTS.md`'s duplication section describes: nothing
reads the copy it checks. The mixer does not need a copy. `MixerPane` is
exported from its own file, which `tests/mixer_snapshot.rs` already
exercises, so the harness should `import { MixerPane } from
"../ui/mixer.slint"` and drive the real component.

Cases:

- Dragging strip 3 onto strip 1 reports `(3, 1)`.
- Dragging strip 1 past strip 3 reports `(1, 3)`, and the landing is the
  strip under the pointer, not one computed from travel.
- A press and release without movement selects and reports no reorder.
- A drag started on the master reports nothing.
- A drag released over the master reports seat 1, not seat 0.
- Releasing over the `+` button, or past the last strip, reports the last
  seat.

Run `scripts/dupe-audit one-sided-test` after writing it. It should stay at
zero hits.

## Documentation, in the same commit

- `CURRENT.md`: add a sentence after the mixer bullet, parallel to the
  channel-reorder one. It should say that tracks move by dragging their name
  plate, the master stays first, and every address follows, including
  channels routed to the track, sends, lanes, routes and control bindings.
  Also say that the move is one undoable edit.
- `UI_DESIGN.md`, **Mixer Strips**: add a rule that the name plate is the
  grab, that the master refuses at the grab, and that the turned-over face
  stays at its seat for now (see `00-status.md`).
- `LOOSE_ENDS.md`: the two follow-ups, `ReorderMath` for the two older
  racks and auto-scroll during a mixer drag, plus the page-state limitation.

## Verification

`cargo test -p mooloop-ui --test track_reorder` and
`--test mixer_snapshot` on the box. Then rung 4 in the background before the
merge, because this is the milestone. Last, one look at the live app through
`scripts/mooloop-mcp`: drag a track that has a send, a routed channel and an
automation lane. Play the song before and after the drop, and confirm that
undo puts the track back.
