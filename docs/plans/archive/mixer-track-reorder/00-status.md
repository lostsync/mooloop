# Mixer tracks can be reordered — plan status

**Written 2026-09-16. Every step landed the same day.** Adam asked for it directly:
*"it should be possible to reorder the mixer tracks by dragging them."* It is
outside the `FOCUS.md` sequence for that reason, the same standing
`coreaudio-driver/` has.

## What this is

A track's strip, dragged by its name plate, lands somewhere else in the
mixer, and everything in the song that named that track still names it. The
master stays first and cannot be dragged or displaced.

It is the channel rack's reorder
(`docs/plans/archive/console/01-a-channel-can-be-moved.md`) done again one
list over, and most of it is copying a shape that already works. The
`TrackEdit` docstring (`structure.rs`) predicted this:
`Project::add_track` says *"a mixer's order is not yet something a user
arranges. When it becomes one, `TrackEdit::Inserted` is already the edit for
it."* A reorder needs a `Moved` variant as well, for the reason
`ChannelEdit::Moved` exists.

## Steps

| Step | What | Rung | State |
| --- | --- | --- | --- |
| 01 | `TrackEdit::Moved`, `Project::move_track`, and control bindings follow both kinds of edit | 2 (`mooloop-core`) | landed 2026-09-16 |
| 02 | The session follows a track edit, and the rack stays on the moved track | 2 (`mooloop-session`) | landed 2026-09-16 |
| 03 | The drag: `TrackDrag`, the strip plate, one `main.slint` crossing | `slint-sketch`, then one `mooloop-ui` build on the box | landed 2026-09-16 |
| 04 | Two Track actions, so the gesture has a name | 2, then the step 03 build if batched | landed 2026-09-16, batched into 03 |

Steps 01 and 02 need no UI build and can land on the laptop. Step 03 is the
only one that crosses the face contract, and `AGENTS.md` wants that done
once, so **if step 04 is wanted, batch its `main.slint` edits into step
03's pass**.

## What the doing changed

- **`ListEdit` lives in `mooloop-core`, not in the session's `project.rs`.**
  Once both edit enums had a `target(EffectTarget)` method, both `address`
  methods became one line over it, and `ListEdit::address` is where that
  line lives. `ProjectEdit::edit` carries it, and the session walks four
  fields once for both lists in `Session::rescope_targets`.
- **The rack follows the moved track without a new field.** Step 02 had
  `queue_track_move` pass `Some(to)` as the rack's target. That was not
  needed: pressing a strip's plate already points the rack at it, so the
  pump's pre-install `rack_was` is `Bus(from)` and `rescope_after_track` sends
  it to `Bus(to)`. The same arm keeps the rack on its track through a removal
  of some *other* track, which the plan did not ask for and costs nothing.
- **`MixerStripRow.is-master` already existed**, so the grab reads it.
- **The master's slot is hot, and answers seat 1.** Step 03 said the master
  is never a landing and the release clamps. With discrete pointer events
  that left a drop over the master reporting whatever strip was last under
  the pointer, so the master's slot reports seat 1 and has no left edge, and
  the `+` button reports the last seat and has no right edge. The release
  still clamps to `1..count-1`, and the model still refuses seat 0.
- **No `z`.** Slint 1.17 wants `z` as a literal, so the held strip passes
  *under* its right-hand neighbour, as it does in both other racks. It wears
  a shadow and a slight dim instead. Recorded in `LOOSE_ENDS.md`.
- **`MixerMetrics.row-padding` and `strip-gap`** now name the strip row's
  8px and 4px, so the slot's pitch, `track_reorder.rs` and
  `mixer_snapshot.rs` read them instead of each holding a copy.
- **There was no Track menu**, so step 04 added one between Channel and View:
  Add Track, and the two moves, named for the track they move. Its enable
  flags come from `Session::can_move_track`, which calls the same
  `track_move_allowed` that `Project::move_track` does. `menubar.rs`'s View
  click moved from x 223 to 270.
- **The live check through `scripts/mooloop-mcp` was done, except by ear.**
  On the starter song, Reverb (track 3) was dragged onto Drums' plate. The
  mixer read Master, Reverb, Drums, Bass, the rack stayed on Reverb and still
  showed its two incoming sends, and all four channels' destination chips
  moved from `→1` to `→2` with Drums. Ctrl+Z put the order and the chips back,
  with the rack on a channel as `replace_project` intends. Dragging the
  master's plate onto the last strip did nothing. Nobody has *listened* across
  a drop; the engine takes the whole-project install path any track add or
  remove already takes, so a cut tail is the expected cost.

## Found while writing the plan

Two existing gaps. Both are in steps 01 and 02 because a reorder would make
them visible every time it is used, and they are each a few lines.

- **`Project::control_map` follows no structural edit at all.** A
  `ControlBinding` targets a `ParamAddr`, which carries an `EffectTarget`
  scope, and neither `Project::rescope_after` (channels) nor
  `rescope_tracks_after` (tracks) touches it. So a desk fader learned onto
  channel 3's volume moves channel 4's once channel 1 is deleted, and it has
  done so since control mapping landed. The channel reorder already has this
  bug. Step 01 fixes it for both lists.
- **A track removal leaves the session's track-keyed state on the old
  numbering.** `ProjectEdit` carries `channel_edit` and nothing for tracks,
  so `queue_track_remove` renumbers the song but not `selected_device`,
  `automation_target`, `pending_preset_save` or `effect_preset_names` when
  they hold an `EffectTarget::Bus`. After track 1 is removed, a preset label
  on track 3's device lands on whatever device track 2 has with the same id.
  Every chain mints its ids from zero, so there usually is one.
  Step 02 fixes it by carrying track edits the same way.

`Project::remove_track` also has no test in `mooloop-core`. Step 01's test
covers it as a side effect.

## Deliberately not in this plan

- **Stable track ids.** The `TrackEdit` docstring defers them to the
  `EffectTarget` unification, and that is still right. A reorder done with
  positions is the same size as the channel one and is consistent with it.
- **An incremental engine rotate.** Track edits go through the whole-project
  snapshot path, as channel edits do, and the console plan's step 01 explains
  why a rotate is its own piece of work. The cost is the same as for any
  track add or remove today: tails on the mixer are cut when the drop lands.
- **Scrolling the mixer during a drag.** Seventeen strips at 96px are wider
  than most panes, so a far move takes a drag, a scroll and another drag. The
  channel rack has the same limit and nobody has asked for more. Write it in
  `LOOSE_ENDS.md` when step 03 lands, not here.
- **The turned-over face following its track.** A strip's `page` is private
  to the instance, and a `for` reuses instances by seat. So a strip turned to
  its SENDS page stays turned at its old position after a move, and the moved
  track arrives on its fader page. Step 03 records this rather than fixing it.
  Fixing it means moving `page` into `MixerStripRow`, which is a separate
  change.
