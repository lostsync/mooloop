# 02 — Size by what exists

The measurement in `00-status.md` says the expensive thing is not modules,
it is `MAX_CHANNELS`. At eight slots the engine preallocates 2 MiB of
control outputs and 200 KiB of DSP racks for **256 channels**, whatever
the project actually contains. Every capacity increase multiplies that
same waste.

This step is worth taking even if the slot count never moves again.

## The shape

- `RenderGraph` already holds `modulators: Vec<ModulatorRack>` and a
  parallel control-output buffer, both built with `(0..MAX_CHANNELS)` at
  construction. Grow them with the project's channel count instead, on
  the same structural path that already adds a channel.
- Growth allocates, so it happens where allocation is already legal: the
  UI thread, through the existing structural command route, with the old
  buffer handed back for the UI thread to drop. This is the install and
  reclaim pattern the effect chain already uses; unlike the modulator
  slots themselves, a whole-graph buffer is exactly the case it was
  built for.
- The realtime path keeps indexing by channel with no bounds surprise,
  because the graph is only ever asked for channels it has been told
  exist.
- `ModulatorMeters` is a lock-free published table read by the UI. It is
  8 KiB at eight slots and not worth the risk of resizing under a reader;
  leave it dimensioned at `MAX_CHANNELS` and say so.

## The number to beat

3.1 MiB total at eight slots and 256 channels. A sixteen-channel project
should land near 200 KiB for the same capacity. Measure it the way step 03
measured the ring rather than asserting it, and keep the table in
`00-status.md` honest.

## Explicitly not in this step

- No change to `MAX_CHANNELS` itself. 256 as a ceiling is fine once
  nothing is preallocated against it.
- No change to the command ring, which is step 03's business.

## Done when

- A project's engine memory tracks its channel count rather than the
  ceiling, measured before and after.
- Adding and removing channels while playing does not allocate on the
  audio thread, pinned by the existing structural-command tests.
- Offline render matches realtime, unchanged.
