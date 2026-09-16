# Mixer tracks can be reordered — plan status

**Written 2026-09-16. Step 01 has landed.** Adam asked for it directly:
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
| 02 | The session follows a track edit, and the rack stays on the moved track | 2 (`mooloop-session`) | not started |
| 03 | The drag: `TrackDrag`, the strip plate, one `main.slint` crossing | `slint-sketch`, then one `mooloop-ui` build on the box | not started |
| 04 | Two Track actions, so the gesture has a name | 2, then the step 03 build if batched | not started, optional |

Steps 01 and 02 need no UI build and can land on the laptop. Step 03 is the
only one that crosses the face contract, and `AGENTS.md` wants that done
once, so **if step 04 is wanted, batch its `main.slint` edits into step
03's pass**.

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
