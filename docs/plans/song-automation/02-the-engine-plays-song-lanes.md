# 02 — the engine plays song lanes

The renderer resolves song lanes in Song mode, beside pattern lanes, through
the one *base + offset* rule every automation site already applies. It is
headless and tested without a window.

## The question this step waits on (open: see its Linear issue)

**When a pattern lane and a song lane drive the same parameter at the same
moment, which wins?**

1. **The pattern lane, while its pattern is playing and the lane has points.**
   The song lane drives everywhere else. A pattern lane is clip automation,
   and the clip is the more specific thing. **Every song saved before this
   plays exactly as it did.** Recommended.
2. **The song lane, always.** The song lane is the final say; a pattern lane
   only plays where the song lane has no points.
3. **They combine.** The pattern lane is an offset on the song lane, the way
   a modulation route is.

Every site resolves "the lane for this target" once per block (below), so
the rule is one function wherever it lands. Nothing else in this step waits
on it. Build with 1 behind that function until the answer comes.

## Where it goes

- **The sequencer holds the song lanes.** `Sequencer::load_project`
  (`engine/src/sequencer.rs:311`) receives them prepared off-thread, as it
  does pattern lanes (`set_lanes`, with storage reserved first).
- **Opening and closing a lane** after load are commands carrying a lane
  already built on the control thread. A closed lane goes back over the
  return ring to be dropped off-thread. Point edits are in-place upserts with
  no allocation, exactly as `upsert_automation_point` (`:500`) does. Step
  01's growth swap lands here too.
- **`song_lane_at(target, song_tick)`**, beside `automation_lane_at`
  (`:754`):
  - It returns the lane and the song tick wrapped by the song length, as the
    pattern walk's position is.
  - A song lane needs no placement walk, because its ticks are already song
    ticks.
  - It is consulted in **Song mode only**.
- **`has_automation_at`** (`:659`) and **`visit_automation_targets_at`**
  (`:708`) report song-lane targets as well. Then `AutomationBlock` (`render.rs:1204`) is
  built whenever either kind of lane drives something.
- **The precedence rule** is one function both use: the pattern lane under the
  position, the song lane, or both. It stays one function so the answer above
  changes one place.
- **`LaneTargets`** (`render.rs:1229`) holds at most 64 targets a block.
  - Song lanes can push past that. Size it from the number of lanes the
    sequencer actually holds, off the audio thread.
  - Or keep a fixed bound and make overflow impossible by refusing the lane
    that would exceed it, with a status message.
  - It must not silently drop a target. That was MOO-73.
- **Handing the knob back.** `restore_lanes_left_behind` (`render.rs:6997`)
  runs on a pattern switch, a mode change and a seek. It walks the lanes of
  the position being left. It must include song lanes, so leaving Song mode
  returns a song-lane destination to its knob. `effect_is_driven` (`:7097`)
  must count song lanes too, so a knob doesn't fight a lane.

**The strip.** A song lane on a channel's volume or pan reaches
`resolve_strip_segments` (`render.rs:3888`) the same way a pattern lane does.
A track's strip is step 04.

**Export.** `offline.rs` renders through the same `RenderState`, so a Song
export plays song lanes with nothing extra. Test it; don't assume it.

## Done when

- **Engine tests:**
  - A song lane ramping a channel's filter cutoff across two placements of
    the same pattern moves continuously across the boundary. That is the ramp
    Adam's 2026-09-17 ruling had to do by cloning a pattern.
  - In Pattern mode the same lane does nothing.
  - Switching from Song to Pattern hands the knob back.
- **A test per precedence case.** A pattern lane and a song lane on one
  destination, under the rule as answered (1 until it is).
- **More driven destinations than today's 64** play without one being dropped
  or frozen, at 512 frames.
- **No allocation in the callback**, with the existing allocation tests
  (`executor.rs:1309-1427`) extended for a lane open, close, grow and point
  edit.
- **An offline Song export** carries a song lane.
- **`CURRENT.md`** says song lanes play in Song mode and how they share a
  destination with a pattern lane.
