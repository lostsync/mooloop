# 01 — Modulators join the param system

The misstep to correct, and the whole reason a third modulator kind costs
500–800 lines today: effects follow one paradigm — a static
`ParamDescriptor` table, `get(id)`/`set(id, value)`, addressable by
`ParamAddr` — and modulators follow none of it. `modulation.rs` even
declares `LFO_PARAM_RATE_HZ..=LFO_PARAM_PHASE` with a comment promising
descriptor ids, and nothing in the workspace reads them. `ParamOwner::
Modulator { slot }` exists and is a dead branch at every consumer.

The engine needs no change. `SetChannelModulation` already ships the whole
`ModRack` by value, `offset_for` indexes outputs by slot without knowing
kinds, and the renderer never matches on `ModulatorParams`. This step is
core metadata plus a UI collapse; the realtime path is untouched.

## Core

- `ModulatorKind::descriptors() -> &'static [ParamDescriptor]` and
  `ModulatorKind::descriptor(id)`, mirroring `EffectKind`.
- `ModulatorParams::get(id) -> Option<f32>` and `::set(id, value)`,
  mirroring `EffectParams`. Enum fields (waveform, divisions, channel)
  travel as `ParamCurve::Stepped(n)` indices, exactly as effect mode
  selectors do. Booleans (retrigger, sync flags) are `Stepped(2)`.
- Ids: keep the four already declared for the LFO, append the rest, never
  renumber. Envelope gets its own table. A tempo-syncable time is three
  ids — free value, sync flag, division — because that is what it already
  is in the params structs; the descriptor table is a projection, not a
  new format. Persistence stays the existing typed structs; nothing about
  `SavedModRack` changes.
- A sidecar `ModDestinationDescriptor` policy for modulator params comes
  from `for_param` like everyone else's, but stays unconsumed until the
  modulate-a-modulator door is opened deliberately (03 at the earliest).
  Structural fields — envelope input channel, retrigger — are `Stepped`,
  so the default policy already refuses them.

## UI

Replace the per-field surface with the descriptor-indexed pattern the
effect faces already use:

- One callback: `modulation-param-changed(slot, id, value)`, plus the
  existing edit-started/edit-finished pair for gesture coalescing. The
  ~24 per-field callbacks, ~39 shelf properties, and the ~97 forwarding
  assignments in `main.slint` go away.
- Selected-source state becomes one descriptor-id-indexed value array
  pushed from Rust, like `modulation-depths` already is.
- The editor stays authored, not auto-generated: each kind declares a
  compact layout table (knob rows, `SyncMiniKnob` triples naming their
  three ids, selector banks) instead of a hand-wired tree. The point is
  not generated UI; it is that a kind's editor is data plus one shared
  renderer, so a new kind adds a table, not a plumbing project.
- Param edits record undo through the existing coalescing guard. Today
  turning an LFO rate knob is unundoable; that gap closes here for free,
  because there is finally one code path to put the snapshot in.
- Add the missing **remove source** verb: clears the slot, drops routes
  whose `source_slot` points at it, restores their destinations' bases
  (the `set_channel_modulation` diff already does this), records undo.

## Done when

- Adding, editing, and removing an LFO or envelope behaves exactly as
  before from the user's chair, and every one of those edits undoes.
- `grep ModulatorParams::` in `mooloop-ui` returns a handful of sites
  (row construction, preview shape) rather than dozens.
- A saved project from `main` loads unchanged; the sparse `SavedModRack`
  format is byte-compatible.
- The descriptor tables round-trip: for every kind, `set(id, get(id))`
  is identity across the whole table, pinned by a test.
- `cargo test --workspace` on the build box, plus a software-rendered
  shelf snapshot.
