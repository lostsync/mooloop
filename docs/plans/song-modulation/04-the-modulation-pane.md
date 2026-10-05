# 04 — the modulation pane

Modulators get a pane of their own and leave the device rack. Adam: *"it
will just go in its own pane."* The pane holds what the shelf holds today,
widened to the song. **It is not the canvas**: the Song Patch prototype
(`README.md`) is the later direction, and this pane is laid out so the
canvas can replace its middle without moving anything else.

**Adam sees a mock-up before the markup is built.** His prototype is the
reference for where things sit (a large work area, a narrow list of what
can be added on the right, the device knobs below), not for what the work
area draws. Draft it with `scripts/slint-sketch` against the current shelf.

## A sixth view

The work area has five views in three slots (`plans/archive/pane-layout/`;
`ui/src/lib.rs:1180`, `settings.rs:1121`). Add **Modulation**:
- `view::MODULATION = 5`, a `PaneViews` entry, a `ViewSlot` and
  `PaneToolbar` block;
- an action `view.pane-modulation`, **Show Modulation**, on `Ctrl+6`
  (`actions.rs:364-371`);
- `VIEW_COUNT` to 6, and a settings migration so a saved layout of five
  views opens with Modulation hidden rather than refused
  (`LayoutSettings::sanitized`, `settings.rs:1181`).

Read `plans/archive/pane-layout/README.md` and its `00-status.md` (the four
Slint constraints step 01 found) before starting.

## What is in it

From the shelf (`ui/modulation-shelf.slint`, 1735 lines), moved, not
rewritten:
- **the module grid**: one tile per module in the song, with its name and
  live meter, and **Add source**. No fixed row of empty slots; the grid
  scrolls (`MODULATION.md`, *Shelf and common frame*);
- **the selected module's surface**, as today, with its input picker
  (step 03);
- **Assign**, as today.

New, because the song is wider than a channel:
- **the selected module's routes**, one row each: the destination's channel
  or track name, device, parameter, depth and polarity, and a remove
  button. Today a route is only visible as dots on the face it lands on; a
  module that drives three channels needs one place that lists them.
- **the module's name**, editable, since a song has many LFOs.

The OUTLETS pane beside the grid (a generator's published outlets) stays,
listing the outlets of **every** channel that publishes some, grouped by
channel.

## Leaving the rack

- Remove `ModulationShelf` from the editor dock (`main.slint:6801-6807`) and
  its height logic (`:1905-1907`). The device rack gets that height back.
- A face's route-count dots stay on the face (step 03). Clicking one selects
  the module that drives it and reveals the Modulation view, so a dot is
  still a way in from the rack.
- The selected module survives a channel change: modules are the song's.

## Done when

- Adam has seen the mock-up and said go (recorded in `00-status.md`).
- Show Modulation (`Ctrl+6`) reveals the pane in any slot; a five-view
  layout saved by 0.1.6 opens without error.
- Every module in the song is in the grid, whatever channel is selected,
  and a bus in the rack no longer hides modulation.
- A module's routes are listed with where they land, and removing one there
  removes it from the face.
- The shelf is gone from the rack. `scripts/dupe-audit` counts do not rise.
- `LISTENING.md` gets a look item: the pane, in each slot.
