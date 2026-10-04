# 01 — the song lane store

The document, the session and the save file learn about song lanes. Nothing
plays them yet (02) and nothing draws them yet (03). The whole step is
headless and is tested without a window.

## The type

**A song lane is an `AutomationLane`**, the type pattern lanes already use
(`core/src/automation.rs:54`). Only its tick's origin changes: a song lane's
`tick` counts from the song's start, not from a pattern's.
- Don't add a parallel type. One point type is what keeps the tracker
  question (MOO-159) a view and not a third editor (`README.md`).
- `value_at`, `upsert`, `remove`, `clear` and the capacity rules carry over
  unchanged.

The project gains `song_automation: Vec<AutomationLane>`, beside `playlist`
and `loop_range` (`core/src/project.rs:933`):
- **At most one lane per destination**, the same rule a pattern's lanes keep.
- **No count cap.** The pattern store's 8 per channel per pattern exists
  because that store is preallocated for every pattern. This one is sized
  from the song (`CAPACITY_POLICY.md`).
- **A point's tick** is clamped to the playlist canvas,
  `MAX_PLAYLIST_BARS * TICKS_PER_BAR` (`playlist.rs:8`), as a placement's
  start is.

## Destinations

Any `ParamAddr` a pattern lane may name is a song lane destination:
- a channel's source, inserts or strip;
- a track's inserts (and its strip, once step 04 makes the engine honour it).

Reuse `Session::lane_allowed` (`session/src/plugin_params.rs:144`).

**Seats, and keeping them right.** A `ParamAddr` names a channel or track by
seat. Every edit that renumbers seats already rescopes pattern lanes. Song
lanes must ride the same paths:
- `rescope_lanes` and `rescope_lanes_for_track` (`core/src/structure.rs:999`,
  `:1017`), from `rescope_tracks_after` (`project.rs:1872`) and the channel
  edits;
- removing a channel or track removes the song lanes that named it;
- removing a device removes the song lanes on it, as `forget_device` does for
  pattern lanes;
- a source change that leaves a lane inert lists it the way MOO-135/MOO-270
  list pattern lanes (`session/src/automation.rs:33`).

This is the part most likely to be missed, because every existing test of it
builds pattern lanes. Write the song-lane version of each rescope test first,
against the unchanged tree, and watch it fail.

## Save and load

- A new top-level `song_automation` array (`docs/PROJECT_FORMAT.md`, beside
  the playlist). It is **omitted when empty**, so a song with no song lanes
  saves byte-identical to today. That is the precedent key zones set.
- Lanes serialise like pattern lanes, through `SavedAddress`
  (`PROJECT_FORMAT.md`, *Effects, modulation, and automation*).
- `integrity::repair_project` checks song lanes with the same rules as
  `check_lanes` and `check_lane_addresses` (`project/src/integrity.rs:1109`,
  `:1716`):
  - one lane per destination;
  - point ids unique;
  - ticks in range;
  - values finite and within 0..1;
  - an address that names something missing is repaired or dropped with a
    warning.
- No `FORMAT_VERSION` bump. A field with `#[serde(default)]` is how this
  format grows (`project/src/lib.rs:40`).

## Session verbs

Mirror `session/src/automation.rs`, keyed by destination rather than by
channel and pattern:
- `open_song_lane(target)`;
- `close_song_lane(target)`;
- `clear_song_lane(target)`;
- `upsert_song_point(target, tick, value)`;
- `move_song_point(target, id, tick, value)`;
- `remove_song_point(target, id)`.

Each returns the `EngineCommand` that step 02 handles. Add the variants to
`core/src/bridge.rs` now, and have the engine ignore them until 02.

**Undo:**
- Open, close and clear record through `record_project_history`.
- A point drag goes through `with_gesture_history` (`ui/src/lib.rs:1703`,
  `:2152`), so one press-and-drag is one entry.
- `project_snapshot` must clone `song_automation` (`session.rs:597`).
  Otherwise undo would install a song with every song lane missing.
- `scripts/dupe-audit unrecorded-edit` must not go up.

**Growing a lane.** When an upsert would pass a lane's point capacity, the
session builds a copy with twice the capacity and sends it as a replacement.
The audio thread swaps it in and hands the old one back to be dropped off the
thread, the way `ReclaimedEffect` does. The user never meets the 1024.

## Done when

- `Project.song_automation` exists. A song with song lanes round-trips through
  save and load, and a song without any saves byte-identical (a fixture test
  against `tests/fixtures/songs/`).
- Moving, removing and adding a track or a channel rescopes song lanes the
  same way it rescopes pattern lanes, with a test for each. Removing a device
  removes its song lanes.
- The six verbs exist, each records undo correctly, and an undo after a point
  drag restores the lane and nothing else.
- A lane past its point capacity grows without the audio thread allocating:
  an `executor.rs`-style allocation test once step 02 handles the command;
  until then, a session test that the replacement is sent.
- `PROJECT_FORMAT.md` documents the field and its integrity rules.
