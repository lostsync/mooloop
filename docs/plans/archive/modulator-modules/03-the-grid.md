# 03 — The grid

The presentation step. The shelf's tile row becomes a grid of modules;
selecting one expands it to its full param surface. "Given enough space
and a decent UI it's sorta not not a node editor" — but the data model
underneath is still the rack and its routes, which is what keeps this a
styling-and-layout step rather than an engine step.

## Shape

- Collapsed: the existing chip row, unchanged.
- Expanded: a grid of module tiles — preview shape, name, kind badge,
  live output meter (the `ModulatorMeters` path already animates this).
- Click a tile: the expanded module view, showing all params through the
  01 layout renderer, the assign switch, and the module's inputs (an
  envelope's gate channel, a math module's source slot) as explicit,
  labeled jacks rather than params buried in a knob row.
- Eurorack flavor is welcome in the visual language; the interaction
  contract from the spec (assign gesture, destination inspector,
  base + excursion arcs) is already decided and stays.

## Capacity

Four slots stop being enough the moment modules are cheap. Growth is a
protocol edit, not a UI edit — `MAX_MODULATORS_PER_CHANNEL` is baked
into `ModRack`/`ModulatorRack`/`ModulatorMeters`/`ControlOutputs` and
the whole rack rides the command ring by value, so the cost of eight
slots is measured (ring entry size) before it is assumed. The sparse
persisted format already tolerates any capacity; old projects load
into a bigger rack unchanged.

## Durable source identity

Before modules can be reordered or compacted in a grid, routes must
stop meaning "slot 2". This is where `ModSourceRef::Id` finally
persists: new routes save durable ids, legacy `source_slot` routes
decode through the adapter `mod_metadata.rs` already ships, and the
relative-addressing habit from `COMPOSABLE_DEVICE_UNITS.md` ("assume
any unit may be moved") starts holding for the rack itself.

## Deliberately later, still

Plugging modules in "all over the app" — cross-channel sources, device
outlets as grid citizens, strip destinations — stays sequenced behind
the control table and outlet contract in the spec's delivery order.
The grid must first be worth living in on one channel.

## Done when

A channel can hold a grid of mixed modules, each editable in its
expanded view, reorder without breaking a route, save/reload/render
identically, and the whole thing reads at a glance — snapshot-verified
at the gallery and in the real window.
