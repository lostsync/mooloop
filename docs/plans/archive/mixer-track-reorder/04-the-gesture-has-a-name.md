# 04 — The gesture has a name

Optional. Do it only if Adam wants it, and if so, batch its `main.slint`
edits into step 03's pass.

`UI_DESIGN.md`: *"A gesture with no affordance needs a menu that names it."*
A name plate that shows a move cursor on hover is a weak affordance. The
channel plate already has a right-click menu (`channel-context-menu` in
`main.slint`), and the mixer strip's plate has none. The Track category in
`ACTIONS.md` already has two actions aimed at "the track the rack is
editing" (solo on Ctrl+Shift+M, mute on Ctrl+Alt+M), so a move fits there.
That gives the move a menu row and a chord that can be bound. A right-click
menu on the strip plate that names the same two actions is the natural
place for them, but it is a new popup in `mixer.slint` and can wait.

## Two actions

- `track.move-left`: move the track the rack is editing one seat towards
  the master.
- `track.move-right`: move it one seat away.

Both are disabled when the rack is on a channel, when the track is the
master, and at the ends: `move-left` from seat 1, and `move-right` from the
last seat. The enable state comes from one session predicate,
`Session::can_move_track(index, delta)`, and the menu and the chord both read
it.

Both call `queue_track_move(from, from ± 1)`. The drag does the same, so
there is still only one mutation path, which the channel-edit block in
`lib.rs` insists on.

## Chords

Leave them **unbound by default**, as `pattern.clear` is, and list them so
they can be bound. The nearby Ctrl+Shift and Ctrl+Alt combinations with
arrows are either taken by Navigation or too close to transpose to be worth
the collision. `ACTIONS.md`'s scope rules decide this more reliably than a
guess here, so read that document before choosing, and if a chord is added,
update its table in the same commit.

## Verification

The existing `tests/menubar.rs` pattern covers a new menu row. Add one case
that checks the rows are disabled with the rack on a channel and on the
master.
