# Sequencer cursor status

Plan A of `reports/fable-2026-09-22.md` (finding 1) landed
2026-09-22: the sequencer reads its notes as the sorted store they already
are, instead of walking every note of the song on every block.

## What changed

- `crates/mooloop-core/src/pattern.rs`: `ChannelPattern` keeps a second store,
  `offs: Vec<(u32, u16)>` -- `(end_tick, index into notes)`, sorted by end
  tick -- beside `notes` (sorted by `(start_tick, id)`), maintained in
  lockstep by `upsert_note`/`remove_note`/`clear` with no heap allocation.
  `notes_starting_in`/`notes_ending_in`/`note_indices_ending_in` expose
  `partition_point`-based range queries over both stores, and `max_end_tick`
  answers the largest end tick stored in O(1).
- `crates/mooloop-engine/src/sequencer.rs`: `schedule_pattern`, `schedule_once`
  and `schedule_song` no longer walk every note of the active pattern (or
  every placement's pattern) per block. Each channel's candidates come from
  `notes_starting_in`/`notes_ending_in` over a tick window reduced modulo the
  pattern's (or song's) period, widened by the largest swing offset; a note
  that sustains past its own pattern's length is covered by also checking the
  window shifted forward one period, with a full-walk fallback for the rarer
  case of a note sustaining past *two* periods. `schedule_note_edge`'s cycle
  math is unchanged -- it is now called only for candidates, and the
  candidate set is always a safe superset (a spurious candidate costs a
  rejected comparison; a missed one would be a bug).

  Song mode keeps a second playlist view, `playlist_by_start` (sorted by
  `(start_tick, pattern)`, maintained incrementally at
  `set_playlist_placement`), used by `automation_lane_at`'s Song arm to avoid
  walking every placement per destination per block. `schedule_song` itself
  walks `playlist` in its own order rather than this view, with a cheap
  per-placement skip -- see "A deviation from the plan's literal wording"
  below.

## A deviation from the plan's literal wording

The plan's step 4 asked for `schedule_song` to `partition_point` a
start-sorted playlist view directly, the way `automation_lane_at` now does.
The property test below caught why that is unsafe: two placements can each
contribute an event at the very same block offset, and `EventList::push_ordered`
keeps same-offset, same-type ties in the order they were pushed. The
start-sorted view visits placements in a different relative order than
`playlist` (pattern-then-start) does whenever two placements have different
start ticks, so pruning `schedule_song`'s walk with it reordered exactly
those ties -- an actual, reproducible mismatch against the brute-force
reference, not a theoretical one.

`schedule_song` instead walks `playlist` in its own order (matching the
original full walk exactly) with an O(1) per-placement skip
(`window_reaches`, reusing the same reduced-window math) that avoids the
expensive per-channel, per-note work for placements that cannot possibly
overlap the block. This is a smaller asymptotic win than a `partition_point`
prune would be -- O(placements) rather than O(log placements) -- but
`MAX_PLAYLIST_PLACEMENTS` is 512, so an O(placements) walk of cheap
comparisons is still negligible next to the O(placements × channels × notes)
walk it replaces, and the measurement below confirms it. `schedule_once`'s
Song arm has the same shape for the same reason (no periodicity to speak of,
so its skip is a direct interval-overlap check).

For the same reason, note-off candidates found via `notes_ending_in` (which
answers in end-tick order) are re-walked through `notes()` in its own
`(start_tick, id)` order via a fixed-size candidate mask
(`[bool; MAX_NOTES_PER_CHANNEL_PATTERN]`, no allocation) rather than emitted
directly in the order `notes_ending_in` returns them -- the same
same-offset-tie hazard, one level down. Both of these were found by the
property test below, not reasoned out in advance; they are recorded here
because "keep the widened range generous" (the plan's stated fallback for
correctness doubt) is not enough on its own when the risk is event
*ordering* rather than a missed event.

## Tests

- `crates/mooloop-core/src/pattern.rs`: `offs_index_tracks_notes_through_inserts_replaces_and_removals`
  and `notes_starting_and_ending_in_bound_correctly` check the new index
  directly, by brute force against `notes()` sorted by end tick.
- `crates/mooloop-engine/src/sequencer.rs`: every existing `schedule_range`/
  `schedule_once_range` test is unchanged and passes byte-for-byte.
  `indexed_scheduling_matches_brute_force_over_random_patterns` is the new
  property test: a small deterministic PRNG (no `rand` dependency) builds
  random patterns (including very short ones, to exercise the "span covers a
  whole pass" fallback, and notes whose duration exceeds their pattern's
  length, to exercise the note-off fallback), random playlists (including
  placements sharing a start tick, to exercise the Song-mode layering
  tie-break), and random query windows across both playback modes and both
  `schedule`/`schedule_once`, comparing the indexed path's output against a
  brute-force reference that calls the same `schedule_note_edge`/
  `schedule_edge_once` for every note rather than only the candidates. It
  caught two real bugs during development (a placement search that widened
  the wrong side of a reduced window, missing a note-off; and the two
  same-offset-tie reordering issues described above) before landing clean
  across eight distinct seeds.

## Measured: `scheduling_cost`, `cargo test -p mooloop-engine --release block_cost::scheduling_cost -- --ignored --nocapture`

Sixteen channels, each holding a full 1,024-note pattern (`NoteEvent::new` in
a loop, one note per sixty-fourth across the whole `MAX_PATTERN_STEPS`
capacity), idle samplers so the number is scheduling rather than synthesis.
Song mode places the same sixteen full patterns 64 times, end to end on the
playlist. Measured before this change (the original full-walk
`schedule_pattern`/`schedule_song`/`schedule_once`, with this same harness)
and after, median of 400 blocks:

| mode    | frames | before (full walk) | after (indexed) | speedup |
| ------- | -----: | ------------------: | ---------------: | ------: |
| Pattern |     64 |          105,309 ns |         12,328 ns |    8.5x |
| Song    |     64 |        8,067,380 ns |         12,852 ns |   628x  |
| Pattern |    512 |          134,520 ns |         20,069 ns |    6.7x |
| Song    |    512 |        8,130,188 ns |         19,097 ns |   426x  |

For reference, `loaded_project` (one note per channel, the figure
`pattern-bank-floor/00-status.md` reads as "the engine is not a problem")
reads 52,390 ns/block at 64 frames and 44,235 ns/block at 512 -- but that
project uses ML-P8 (the heaviest synthesis device) rather than an idle
sampler, so it is not an apples-to-apples comparison with the numbers above;
it is printed alongside them only because the plan asked for it, and because
it is itself the number finding 1 says has never included a note-dense
pattern or a Song-mode project. It still has not, on its own terms -- this
plan adds the scheduling-only figure beside it rather than changing what
`loaded_project` measures.

The "before" numbers were captured by temporarily reverting
`crates/mooloop-core/src/pattern.rs` and `crates/mooloop-engine/src/sequencer.rs`
to their pre-Plan-A content (the benchmark harness in `block_cost.rs` is new
and does not depend on either), running the release benchmark, then restoring
the indexed implementation. Both the before and after runs used the same
harness and the same machine in the same session.

## Verification

`cargo test -p mooloop-core` and `cargo test -p mooloop-engine` (via
`scripts/exit-code`) are green: 348 passed / 0 failed, and 313 passed /
0 failed / 16 ignored (the `block_cost.rs` and `idle_skip_tests.rs`
`#[ignore]`d wall-clock measurements -- 15 pre-existing plus this plan's new
`scheduling_cost`).
