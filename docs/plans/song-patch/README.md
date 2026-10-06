# Song Patch

A proposal, not a work order. **Nothing here is built and no steps are
written.** It is the later direction that
`docs/plans/archive/song-modulation/` was the first stage of, moved out of
that plan's README when the plan was archived (2026-10-06), so the points
Adam agreed stay where the next plan will find them.

## What it is

Adam's *Song Patch* prototype (designed 2026-09-23 to 09-27, not in the
repo): one song-wide patching canvas in its own pane, with typed boxes
(`lfo`, `step`, `chance`, `chord`, `* -0.5` ...), control wires and note
wires, and song inlets and outlets as tags. Prototype:
https://claude.ai/artifact/BBYu543x1WZ8VAf2MrGCUY.

## What Adam agreed

These are the points Adam agreed in that conversation. They are a proposal,
not a ruling:

- one canvas for the whole song, in its own pane;
- a curated set of box kinds typed as text, not an open language (Pd itself
  may come later as one box that runs a `.pd` file);
- two kinds of wire: control (a value every control tick) and note (events,
  with each NoteOff following its NoteOn through every box);
- a box's face opens in place on the canvas;
- song inlets and outlets are tags at the canvas edge. A preset keeps them
  as empty slots ("kick goes here [ ]");
- depth is set with today's Assign drag; math boxes do the scaling;
- cable bends can be dragged, and cable activity is a setting.

And on the push that came first, 2026-10-05:

> usually for something of this size we'd scope it into pushes, make plans
> for each push, file issues for the plans, and then work from the issues.
> let's start by moving what we have and making it work document-wide.
> we'll see how that went

## What it stands on

Song modulation (0.1.7) built the ground the canvas would stand on, and
`MODULATION.md` is the contract for it:

- one song-wide set of modules and routes (`SongModulation`), with modules
  named by `ModSourceId`, never by position;
- module inputs picked from a list (`InputSource`: none, a channel's notes,
  or another module), with room for a note outlet as a later variant;
- an evaluation order that is not part of the save format;
- the Modulation pane (`PaneViews.modulation`), whose contents are today's
  module grid and can be replaced without moving anything underneath.

## Not in it yet

Everything that makes it a canvas: boxes, wires, note wires and boxes that
make notes (`chord`, `chance`, `scale`), new module kinds, modulators inside
containers (MOO-160), and durable-id destinations. The open ends song
modulation left are in `docs/plans/archive/song-modulation/00-status.md`,
*Open*.
