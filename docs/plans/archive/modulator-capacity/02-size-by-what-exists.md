# 02 — Size by what exists

**Landed 2026-09-01** on `perf/lazy-channels`, as shape A.

**This step was rewritten on 2026-09-01 after measuring properly.** The
first draft said the expensive per-channel arrays were the modulation ones
and put the total at 3.1 MiB. Both were wrong, because the measurement
behind them only tallied what the modulator work had touched.

## What is actually reserved

`the_render_graph_preallocates_a_measured_amount` in `render.rs` pins it:

| Per channel | Size | × 256 channels |
| --- | --- | --- |
| `ChannelStrip` | 150,904 B | **37.7 MiB** |
| `EventList` | 10,248 B | 2.5 MiB |
| `ControlOutputs` | 8,192 B | 2.0 MiB |
| `ModRack` | 932 B | 233 KiB |
| `ModulatorRack` | 800 B | 200 KiB |
| **Total** | 171,076 B | **42.8 MiB** |

Modulation is 433 KiB of that — one percent. The strip is 88%, and inside
the strip it is not the generators (all five together are under 7 KiB); it
is `EffectChain` at 140 KiB.

## The actual cause: a product of two index spaces

`MAX_CHANNELS` and `MAX_EFFECTS_PER_CHANNEL` are both `u8::MAX + 1 = 256`.
Each is defensible on its own — the limit is the width of the index, and
`EFFECTS_FEEDBACK.md` explicitly asked for an effect ceiling "high enough
that your cpu would choke from DSP before you hit it". Neither array looks
unreasonable at its own definition.

The cost is that they multiply. The graph reserves **65,536 effect slots**,
each carrying a `PendingEffectParams` queue (320 B) and an
`Option<EffectParams>` (140 B), for a project that will populate a few
dozen. That is 21 MiB of pending-event queues alone.

So this is not a modulation problem and never was. It is one line of
arithmetic that is invisible at every individual definition and only
appears when the definitions are multiplied.

## Two shapes, and the one to take

**Shape A — materialize channels on demand.** Make the per-channel
collections `Vec<Option<Box<ChannelStrip>>>` and friends, with a channel
built on the UI thread and installed through a structural command, the way
`InstallEffect` already works. The vector of pointers is 2 KiB; a live
sixteen-channel project pays 2.7 MiB instead of 42.8 MiB. It also makes
the block loop skip absent channels instead of walking 256 strips.

**Shape B — size the effect chain to its installed length.** Keep 256
channels but stop giving each one 256 effect slots up front. `bound`
already tracks the populated prefix, so the chain knows it is sparse.

A is the better trade. It fixes all five arrays at once rather than the
biggest one, it preserves both ceilings exactly as they are — nobody has
to argue about how many effects a channel may have — and it reuses an
install/reclaim pattern the codebase already runs on the audio path. B
only moves the effect line and still leaves 5 MiB of events and control
outputs reserved for channels that do not exist.

Take A. Take B only if A's structural-command plumbing turns out to fight
something in the block loop that is not visible from here.

## The risk to respect

This is the realtime path, not a UI surface. The rules that must hold:

- No allocation and no `Box` drop on the audio thread. A new channel
  arrives pre-built through the structural ring; a removed one leaves
  through reclaim, exactly as effect nodes do.
- An absent channel behaves as a silent one, not as a panic. Every
  `strips[i]` becomes a checked access, and the block loop skips rather
  than processes.
- Offline render and realtime render stay identical, which the existing
  render tests already pin.

## What it turned out to need

Three things the write-up above did not anticipate:

- **The per-channel vectors had to stay separate.** Bundling them into one
  `ChannelSlot` per channel reads better, but the block loop borrows
  strips, events and control outputs with different mutabilities at the
  same time. As separate fields those are disjoint borrows; inside one
  struct reached through `Vec<Box<..>>` they are a conflict. Storage moves
  as a unit through `ChannelStorage` and is unpacked on arrival.
- **`EngineCommand::AddChannel` had to go.** Adding a channel allocates
  now, so a POD command on the realtime ring would have silently done
  nothing in exactly the case it was needed — when no spare storage
  existed. It is structural, like an effect node.
- **Two channel counts can disagree.** A fresh `RenderState` claimed one
  channel from its sequencer and had storage for none, which panicked the
  block loop rather than rendering silence. `live_channels` is that
  invariant stated once; every per-channel pass reads it instead of
  `active_channels`.

## Done when

- A project's engine memory tracks its channel count, measured before and
  after, with `the_render_graph_preallocates_a_measured_amount` updated to
  describe the live figure rather than the ceiling.
- Adding and removing channels while playing allocates nothing on the
  audio thread.
- `cargo test --workspace` on the build box, with the existing render and
  structural-command tests unchanged.
