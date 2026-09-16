# 01 — A track can be moved

The model half. No UI, no build beyond `mooloop-core`.

## `TrackEdit::Moved`

`TrackEdit` (`crates/mooloop-core/src/structure.rs`) has `Removed` and
`Inserted`. Add the third variant with the same shape and the same doc as
`ChannelEdit::Moved`:

```rust
pub enum TrackEdit {
    Removed(u8),
    Inserted(u8),
    Moved { from: u8, to: u8 },
}
```

`TrackEdit::track` gets the same three `Moved` arms `ChannelEdit::channel`
has. **Do not write them a second time.** The renumbering is the same
function of `(from, to, old)` in both enums, and a copy is exactly the kind
of duplicate `AGENTS.md` warns about. Pull the arithmetic into one private
`fn moved_index(from: u8, to: u8, old: u8) -> u8` and call it from both. The
removal and insertion arms are also identical, but they are one line each and
have never drifted. Leave them alone unless the extraction makes it free.

`destination` and `address` need no change. A move never returns `None`, so
nothing falls back to the master and nothing is dropped.

## `Project::move_track(from, to) -> Option<TrackEdit>`

Put it beside `remove_track`, shaped like `move_channel`:

- `None` when either index is out of range or they are equal.
- **`None` when either index is `MASTER_BUS`.** The master is first, and it
  is first by being bus 0: `MixerBus::new` names it from its index,
  `is_legal_route` refuses it as a source by index, and `CURRENT.md` says
  *"master first"*. Moving another track *into* seat 0 would displace it, so
  `to == 0` is refused too, not clamped. Clamping is the UI's job (step 03).
- `buses.remove(from)`, `buses.insert(to, ..)`, then the existing
  `rescope_tracks_after(edit)`.

`rescope_tracks_after` already covers the four kinds of track address its
doc lists: a channel's destination, a track's output, a track's sends, and
lanes and routes scoped to a track's chain. It ends with `sanitize_bank`.
That call is still correct for a move and should find nothing to repair,
because a move relabels edges and does not add or remove any. The comment
says it can only be reached by removals, so update it to cover moves too.
`compile_bus_graph` is a Kahn sort and does not assume that a track feeds a
lower index. **Check that nothing else assumes it** before relying on this:
search `mooloop-engine` for bus loops that walk indices in order rather than
in `CompiledBusGraph::order`.

Update `add_track`'s doc comment. Its reason for appending ("a mixer's order
is not yet something a user arranges") stops being true here. Appending is
still right, because it renumbers nothing.

## Control bindings follow both kinds of edit

Nothing renumbers `Project::control_map` today (see `00-status.md`). Add
`ControlMap::rescope_channels(ChannelEdit) -> bool` and
`ControlMap::rescope_tracks(TrackEdit) -> bool` to
`crates/mooloop-core/src/control.rs`. They should mirror
`rescope_lanes`/`rescope_lanes_for_track`:

- A `ControlTarget::Param(addr)` goes through `edit.address(addr)`.
  `None` drops the binding, for the reason a lane is dropped: a binding left
  on a vacated seat would start moving whatever slid into it.
- `ControlTarget::Transport(_)` is untouched.

Call the channel one from `Project::rescope_after` and the track one from
`rescope_tracks_after`. The session holds its own clone of the map
(`Session::control_map`, copied in `replace_project`), so an install picks
up the renumbered map with no further work. Confirm that
`control_state.resolve` runs after an install, or a bound fader keeps its old
resolved target until something else re-resolves it.

The binding holds its seat on the *channel* list for the same reason, so
this is a fix for the channel reorder that shipped, not just preparation for
this plan. Say so in the commit message.

## Tests

- `ChannelEdit`'s existing `Moved` tests, run against the shared helper,
  keep passing unchanged. They check that the extraction changed nothing.
- **A new `track_edits_renumber_every_address_that_named_a_track`** beside
  `channel_edits_renumber_every_address_that_named_a_channel`. Build four
  tracks where track 3 is referenced by every kind of address: a channel
  routed to it, a track outputting to it, a send targeting it, a lane and a
  modulation route scoped to its chain, and a control binding on its strip
  volume. Move 3 to 1 and assert each address still names the same
  `MixerBus::name`. Then remove a track and assert the fallbacks
  `remove_track` documents. That is the first test `remove_track` has ever
  had.
- `move_track` refuses `from == 0`, `to == 0`, `from == to`, and
  out-of-range indices, and leaves the project byte-equal when it does.
- A move leaves `compile_bus_graph` succeeding with the same edge set, up to
  relabeling. `sanitize_bank` returns no repairs.
- Add a control-binding case to the channel test too: a binding on channel
  3 follows a move and is dropped by a removal.

**Mutation check.** Before the fix, the control-binding assertion in each
test has to fail. Run it against the unfixed tree first, for the reason
`AGENTS.md` gives about `bar-arithmetic`.

## Verification

`cargo test -p mooloop-core`. That is enough for this step. Nothing outside
the crate changes signature except `TrackEdit` gaining a variant, and
`cargo check -p mooloop-session` catches any exhaustive `match` on it.

`PROJECT_FORMAT.md` needs no change, because no persisted field is added.
`CURRENT.md` needs no change yet, because there is no gesture yet.
