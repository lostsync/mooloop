# 04 — Lazy effect slots

**Landed 2026-09-01** on `perf/lazy-slots`.

The principle this plan is really about, in Adam's words: a cap is fine,
but nothing should be allocated at launch — more should be allocatable up
to a high limit. The effect chain was the clearest violation, and the
largest.

## What it was

`EffectChain` kept eight parallel arrays sized at
`MAX_EFFECTS_PER_CHANNEL`: kinds, base params, resource keys, a pending
parameter queue, bypass, wet/dry, and input and output trims. At 256
addressable slots that is 141 KiB per chain, and a chain lives on every
one of 256 channels — 35 MiB reserved before a project exists.

## What changed

Those eight fields became one `EffectSlot`, held as
`Option<Box<EffectSlot>>`. An occupied slot costs 496 bytes; an empty one
costs a pointer. The box is allocated on the control thread and shipped in
`StructuralCommand::InstallEffect` beside the node, the dry-path aligner
and the analyzer, which were already allocated and installed exactly that
way — so the realtime thread gained no new work, and the displaced state
leaves through the reclaim ring rather than being dropped on the audio
thread.

    EffectChain   143,936 -> 20,544 bytes
    ChannelStrip  150,904 -> 27,512 bytes
    graph total      42.8 -> 11.6 MiB

Neither ceiling moved. A channel may still hold 256 effects.

## The subtlety worth keeping

Host controls belong to the **slot**, not to the device in it. Replacing
an effect in a populated slot has always kept the wet/dry and trims dialled
into that position, and a naive port would have reset them by installing a
fresh box. `install` carries them across explicitly.

The second-order version of the same point: `load` seeds a slot from the
kind's defaults and then overrides with what was persisted, so the order of
those two writes matters and is commented where it happens.

## What is left

`ChannelStrip` is now 27.5 KiB, and the per-channel total 47.7 KiB. The
remaining 11.6 MiB is buffers reserved for 256 channels a project does not
have — `02-size-by-what-exists.md`.
