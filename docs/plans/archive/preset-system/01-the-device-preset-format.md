# 01 — The device preset format

Read `00-status.md` first. This step is entirely in `mooloop-core` and
`mooloop-project`, has no UI, and every part of it is testable in under a
second. It is the whole of the night's first half.

## What is being built

**The missing half of a device preset: one rack row.** Generator presets
already cover the source slot. There is no effect-level preset at all, which
is `00-status.md`'s problem 1, and it is the one Adam asked for.

The unit is one `EffectSlotState` — `params`, `bypassed`, `wet_dry`,
`input_trim`, `output_trim`. The effect's identity does not need a field of
its own: `EffectParams::kind()` derives it from the payload
(`crates/mooloop-core/src/effect.rs:1830`), and a preset whose kind is
implied by its parameters cannot disagree with itself.

## Why this is the safe form

An `EffectSlotState` contains no `ModRoute` and no `EffectTarget`, so it
carries **no absolute addressing at all**. The rescoping problem that
`rescope_modulation` exists for cannot arise here. That is precisely why
`00-status.md` calls this form safe to build before the fragment question is
settled: there is nothing in it to rescope wrongly.

Do not add modulation to this format. A device-plus-modulation preset is a
different unit and it is the fragment question in disguise.

## The explicit record

`00-status.md` makes one condition on building the specific form first:

> A device-level preset built with relative addressing and an explicit record
> of what it contains is not wasted work if a fragment format later supersedes
> it.

So the manifest must state what the bundle contains, not merely what it is.
Add a `contains` list to the preset metadata — for this format, exactly
`["effect_params"]`. A future fragment reader can then tell a one-row preset
from a run of rows without guessing from the document type, and a reader that
does not understand a longer list can refuse cleanly instead of loading half a
patch.

This is three lines and it is the entire reason this step is allowed to
proceed ahead of the decision. Do not skip it.

## The work

1. **`DocumentKind::Effect`**, `as_str()` → `"effect"`
   (`crates/mooloop-project/src/lib.rs:29`).
2. **`LoadedDocument::Effect(Box<EffectSlotState>)`** (`:88`).
3. **`save_effect_preset(path, slot, info, mode)`**, following
   `save_channel_with_preset` (`:264`). An effect references no samples, so
   its asset closure returns an empty vec — **verify that is true for
   `EffectKind::Buffer`**, whose `BufferParams` is the one candidate for
   carrying something. If it does reference audio, say so in this file and
   handle it; do not assume either way.

   *Verified 2026-09-04:* `BufferParams` is `bars`, `offset_beats` and
   `crossfade_ms` — an allocation size, a read offset and a fade. The
   retained audio itself is runtime state the engine allocates on install and
   never persists, so a Buffer preset carries no asset and the empty closure
   is correct for every kind.

4. **Load** through the existing `load_bundle` path, validating the envelope
   against `"effect"` like the other kinds.
5. **Widen `PresetSummary.kind`.** It is a `DeviceKind` today (`:68`), which
   cannot name an effect. Replace it with

   ```rust
   pub enum PresetKind {
       Generator(DeviceKind),
       Channel(DeviceKind),
       Effect(EffectKind),
   }
   ```

   and update `summarize_preset` (`:800`) to produce it. **This is the
   structural half of `00-status.md`'s problem 3** — the flat list with no
   taxonomy — done without building any browser UI. Fixing the type now is
   what stops a third preset class making the list worse.

## Tests, all of which run in under a second

Put them beside the existing preset tests in `mooloop-project`.

- An effect preset round-trips: save, load, compare `EffectSlotState`.
- Every one of `EffectKind::ALL` round-trips. Twelve kinds, one loop; this is
  what catches a `serde` attribute missing on one params struct.
- Non-default `bypassed`, `wet_dry`, `input_trim` and `output_trim` survive.
  They default on load, so a bug here is silent.
- `list_presets` over a directory of effect presets returns
  `PresetKind::Effect` with the right `EffectKind`.
- A manifest whose `format_version` is wrong is skipped, not an error —
  matching `summarize_preset`'s existing contract.
- The `contains` list is present and reads `["effect_params"]`.
- A bundle whose `contains` names something unknown is refused rather than
  partially loaded. This is the forward-compatibility promise; test it now
  while it is cheap.

## Done when

`cargo test -p mooloop-project` is green, an effect preset saved from one
channel loads onto another with no rescoping needed, and `PresetSummary`
names all three preset classes.

Rung 2 covers this entire step — `cargo test -p mooloop-project` — and it is
about a second. Do not reach for the workspace.
