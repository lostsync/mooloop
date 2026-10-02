# Song automation

Automation lanes that belong to the **song's timeline**, shown in a panel
under the playlist, beside the pattern lanes that already exist. Planned
2026-09-30. Linear: project **Song automation**, one issue per step
(`00-status.md` has the list).

## Why, in Adam's words

**2026-09-30, on MOO-419** (a track's fader can't be automated), asked where
a track's lane should live:

> i think the most rational place to put it is the playlist editor, similar
> to how we did it for the piano roll — idk how much of that we can just
> reuse? not sure its worth it to try bc there's a lot i'd like to update
> there eventually…so maybe this new one is the updated version.

and what the piano roll's lane lacks, which is this plan's spec:

> selecting the param from the dropdown is super cumbersome. there should be
> cascading menus
>
> there are no curves. not a dealbreaker but it'd be nice to have
>
> points align to grid properly but the curve is slightly shorter than it
> needs to be (x axis)
>
> no edit controls, no point selection or multi-selection, no context menu.
>
> we need to be able to see multiple lanes at once. for the song automation
> it needs to group by track, then by plugin.
>
> we should be able to vertically resize the lanes independently and all at
> once.
>
> we should be able to drag the automation panel's divider to resize the
> panel
>
> and at some point we have to think about automation write modes

**The same day**, asked to confirm, because it reverses a ruling:

> yeah i guess i changed my mind. yeah pattern lanes stay. that's basically
> "clip automation". yeah probably before 0.2.0, almost certainly. you can
> write a plan.

## What it reverses, and what it keeps

**Reversed:** *"There is no song-level automation"* (Adam, 2026-09-17,
MOO-24, `PRODUCT.md` *Working Decisions*). Song lanes now exist.

**Kept:** pattern lanes, unchanged. They are the clip automation. Every saved
song plays exactly as before, and a per-pattern wobble is still written on
the pattern. Where a pattern lane and a song lane drive the same parameter,
neither wins: they combine, each lane's distance from the knob adding (Adam,
2026-10-02; the rule is in step 02).

**Kept: the playlist's rows.** A playlist row is still a pattern
(`main.slint`, `for pattern in root.pattern-count`). `PRODUCT.md` warns that
DAW-style lanes must not repeat *"how FL Studio grafted them onto its
playlist"*. So this plan does not turn rows into tracks. The automation panel
is a separate region under the rows, sharing their timeline and nothing else.

**Kept: the tracker question stays Adam's** (MOO-159, `IDEAS.md`). That entry
warns that answering song automation on its own is *"how a project ends up
with two editors that nearly agree"*. This plan avoids that in two ways
without designing the tracker:
- **one point type**: a song lane is an `AutomationLane` like a pattern lane,
  only its ticks count from the song's start;
- **one editor**: step 09 moves the piano roll onto the lane editor this plan
  builds.

A tracker-style view, if Adam ever raises it, is then a third view of the
same data.

## What exists (survey, 2026-09-30, `main` at `862972a8`)

**Pattern lanes, end to end.**
- **Points:** `AutomationPoint { id, tick, value }`, with the value
  normalised 0..1 (`core/src/automation.rs:36`).
- **Lanes:** `AutomationLane { target: ParamAddr, points }` (`:54`).
  `value_at` (`:294`) is linear between points and flat outside them. There is
  no shape field.
- **Capacity:** 8 lanes per channel per pattern, and 1024 points per lane
  (`:22`, `:27`). `LanePool` keeps the audio thread from allocating a lane.
- **The document** holds them in `ProjectChannel.automation[pattern]`
  (`project.rs:623`). Undo is whole-project snapshots.
- **Session verbs** live in `session/src/automation.rs` and each returns an
  `EngineCommand` (`core/src/bridge.rs:209-241`). A point edit is incremental,
  never a reinstall.
- **The engine** resolves a lane once per block in
  `Sequencer::automation_lane_at` (`engine/src/sequencer.rs:754`). In Song
  mode it walks the placements covering the position: the latest-starting
  placement wins, and within it the lowest channel. The engine then evaluates
  the lane per 32-frame control tick (`render.rs` `AutomationBlock`, `:1204`).
  - Every site applies one rule: *base = lane value, else the knob; value =
    base + modulation offset.*
  - `LaneTargets` holds at most 64 targets a block (`render.rs:1229`).

**Destinations.**
- A lane names a `ParamAddr` (`core/src/modulation.rs:269`): a scope
  `EffectTarget::{Channel(u8), Bus(u8)}`, an owner and a descriptor id.
- Channels and tracks are still addressed by **seat** in it. `TrackId` exists
  (`effect.rs:4044`), but nothing names a track by it yet (`project.rs:1369`).
- A track edit renumbers every pattern lane through `rescope_lanes_for_track`
  (`structure.rs:999`).
- A track's inserts take lanes already. **Its fader and pan don't**: the bus
  walk applies a plain `strip.output.apply` (`render.rs:10128`), where a
  channel uses `resolve_strip_segments` (`:3888`). That gap is MOO-419,
  step 04.

**The playlist.**
- A placement is `PatternPlacement { pattern, start_tick }` (`playlist.rs:40`).
  Its length is always the pattern's.
- `TICKS_PER_BAR = 384`. The canvas is `MAX_PLAYLIST_BARS = 64` (`:8`).
- The view is one `ScrollView` over both axes (`main.slint:7352`), with
  nothing pinned: 104 px of row header, then the loop strip, the ruler and
  rows of 26 px at a 28 px pitch.
- x per tick = `playlist-bar-width / ticks-per-bar`. Zoom steps the bar width
  between 12 and 96 px, and is not persisted.

**What doesn't exist yet.**
- **No submenu or cascading menu** anywhere (`main.slint:3385`).
- **No reusable splitter.** Each grip is an inline `TouchArea` with the same
  idiom: the dock grip (`:7915`), the split divider (`:7859`) and the sidebar
  grips.
- **No row or lane in the app resizes vertically.**
- A hand-built `PopupWindow` blocks hover outside itself
  (`menubar.slint:163-175`).
- Closing a popup tears down its rows, so a callback must run before
  `close()` (`scripts/dupe-audit popup-close-order`).

## The shape

**A song lane is an `AutomationLane` on the project**, in
`Project.song_automation`.
- At most one lane per destination.
- Ticks count from the song's start.
- It drives its destination **in Song mode only**. Pattern mode loops one
  pattern and has no song position, so its pattern lanes are all that play
  there.

**Capacity follows `CAPACITY_POLICY.md`:** there is no small cap a user meets.
- The song may hold as many lanes as it has destinations.
- A lane's point storage is sized off the audio thread. When it fills, it is
  replaced by a larger copy through a command, never grown in the callback.
- The engine's per-block target list must hold every lane it can be handed.
  MOO-73 was the overflow of exactly that list.

**The panel sits under the playlist's pattern rows**, behind a divider you
drag.
- Its lanes share the rows' x axis and scroll with them.
- They are grouped **by track, then by device**:
  - a track's own strip and inserts first;
  - then each channel that feeds the track, with its source, inserts and
    strip.
  - A channel routed to the master sits under Master.
- Groups fold.
- Lanes resize vertically, each on its own or all together.

**Choosing a parameter is a cascade** of track → device → parameter. It is
built as **columns inside one popup**, because a nested popup can't take hover
from its parent (`menubar.slint:163-175`).

## Steps

| # | Step | Team | Milestone |
| --- | --- | --- | --- |
| 01 | The song lane store | Parameters & Control | A song lane you can hear |
| 02 | The engine plays song lanes | Parameters & Control | A song lane you can hear |
| 03 | The automation panel | Sequencing & Time | A song lane you can hear |
| 04 | A track's fader and pan follow a lane (MOO-419) | Mixer & Routing | Tracks, picking and editing |
| 05 | Choosing a parameter: a cascade | Parameters & Control | Tracks, picking and editing |
| 06 | Editing points | Parameters & Control | Tracks, picking and editing |
| 07 | Lane heights | Sequencing & Time | Tracks, picking and editing |
| 08 | Curves between points | Parameters & Control | Curves and one editor |
| 09 | One lane editor: the piano roll's lane joins | Parameters & Control | Curves and one editor |

The order puts **something audible first**. After 03 you can draw a song lane
in the playlist and hear it. Each later step adds to a panel that already
works.

## Not in this plan

- **Write modes** (latch, touch, write). Adam: *"at some point we have to
  think about"*. That is recording knob moves into a lane, and it needs a
  model of a knob being touched that nothing has yet. It is filed
  unscheduled in the project as MOO-478, not as a step.
- **A track's sends and mute as lane destinations.** MOO-419's title names
  them. Volume and pan come first (step 04). A send level is the next
  candidate. Mute is a toggle, and a toggle lane is its own design.
- **The tracker view** (MOO-159). It stays Adam's to raise.
- **Lanes that name a track by `TrackId`.** They use seats, as pattern lanes
  do, and are rescoped on every track edit (step 01). Moving every lane to
  durable ids is a separate change, for both kinds of lane at once.
- **Long-form audio.** Adam expects song automation to be part of how that is
  eventually handled (`IDEAS.md`), but this plan doesn't attempt it.
