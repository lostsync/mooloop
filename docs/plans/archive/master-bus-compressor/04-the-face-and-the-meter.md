# 04 · The per-voicing face, the gain-reduction meter, and the lookahead box

**Files:** a new `crates/mooloop-ui/ui/master-comp.slint` (Mixer's),
`bus-device.slint`, `strip.slint`, `ui/src/meter.rs`, and one crossing of
`main.slint` and `ui/src/lib.rs` (Interface's shell; ack on the board, and the
Effects team is in both files for the layer device, so read the board before
landing).

## What it builds

- **`MasterCompFace`**, drawn in the master's rack between its inserts and its
  fader, where the engine runs it. Three rack units, the master's warning
  colour on its header, and always present on the master even while out: it is
  the master's own section, not an insert, so it has no rails.
  - Shared: In, a three-way voicing selector, Threshold, Makeup, Mix.
  - **Grip and Punch**: Ratio, Attack and Release, each a stepped knob over
    its own unit's markings.
  - **Tube**: one six-position TIME selector in their place.
- **A real gain-reduction meter**, replacing the 9 px lamp for the master: a
  needle over a 0 to 20 dB arc, fed by a new `StripMeters.master-reduction-db`
  that the pump sets from `take_master_comp_reduction` through a ballistic in
  `meter.rs` (instant rise, a VU-like fall), so it reads like the meter on the
  unit rather than a flickering bar.
- **The lookahead box** on the master's Out face, beside its clip lamp (the
  limiter is after the fader, so that is where it is drawn): a small number box
  in milliseconds, 0.0 default, double-click to 0.
- Every value the markup shows comes from the descriptor table through
  `StripSpec` (ranges, steps, positions' labels), held by
  `slint_face_agreement.rs`; the position labels come from `bus_comp`'s law
  table, never spelled in markup.

`scripts/slint-sketch` first, at the real face size; one `main.slint`/`lib.rs`
crossing, batched.

## Done when

The face draws each voicing's controls and only those, every control reaches
the engine through `bus-strip-param`, undo records it (the strip's existing
path), `CURRENT.md` says what the master rack shows, and MOO-13 and MOO-169
close with the listening pass named as owed.
