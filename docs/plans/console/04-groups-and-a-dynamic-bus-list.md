# 04 — Groups, and a dynamic bus list

The structural step, and the one Adam's policy is actually about. Everything
above ships without it.

> A mixer strip is created by a musical act, not an administrative one.
> Channels group; the group is summed; the group owns the strip; a grouped
> channel has no direct out.

## The model

- **`Channel.group: Option<GroupId>`**, under the defaulted-field rule:
  absent means ungrouped, which reproduces today exactly and keeps this inside
  `FORMAT_VERSION = 1`.
- **Buses get stable ids**, minted from position on load the way
  `Project::assign_device_ids` mints device ids -- *"a no-op rather than a
  migration"*. That is what avoids the v2 format `MIXER_PLAN.md` proposes.
- **`buses: Vec<BusSetup>` stops being padded to 17.** `default_buses`
  (`mixer.rs`) and `normalized_buses` (`session.rs`) both exist to guarantee
  every index is materialised; both go. A group creates a bus; the last member
  leaving destroys it. Manual `+ Bus` still exists, for returns and submixes
  with no members.

`compile_bus_graph` currently skips channels as edges on the grounds that they
all render before any bus. **That holds under grouping and should stay** -- a
group is a bus, and its members are channels, so the ordering argument is
unchanged.

## The mixer's strip list becomes derived

```text
[MASTER] | one strip per group | one strip per ungrouped channel | manual buses
```

`sync_mixer` (`mooloop-ui/src/lib.rs`) currently walks the whole `buses` vec.
It becomes a derivation over that list. `MixerPane` already scrolls rather
than compresses (`mixer.slint`), so a variable strip count needs no layout
change -- but `tests/mixer_snapshot.rs` hard-codes strip coordinates and will
need updating.

## The rack becomes two levels

Group headers with channels under them. The drag from step 01 grows a
drop-into-region, for which **drag pattern B** (`PaneDrag`, `main.slint`, with
its `drop-slot` region logic) is the existing analogue: pattern A lands
*between* rows, and this has to land *inside* something as well.

## The `RenameBus` gap closes here

`MixerBus.name` saves and loads and nothing can set it (`LOOSE_ENDS.md`). A
group without a name is useless, so this is the step that has to close it
rather than the step that notices it.

## Verification

`cargo test -p mooloop-core -p mooloop-session`, then the realtime/offline
equality harness that already exists -- `compensation_renders_the_same_at_any_
block_size` and the offline/live equality test in `render.rs`, and
`audio_edge_tests.rs`. A dynamic bus list changes what `compile_bus_graph` and
`compile_latency` are handed, and those three tests are what say the two
renderers still agree.
