# 05 — Sends and returns

The step that forces the compiler change, and the reason it is last.

Because of decision 1 in the `README.md`, a send is a strip-level feature and
therefore lands on channels *and* buses at once -- which is how "a way to do
sends and returns without a mixer bus", the thing the brief asked for, costs
nothing extra here.

## The model

A strip gains `sends: Vec<AuxSend { target, level, tap: Pre | Post, enabled }>`,
per `MIXER_PLAN.md`. An explicit `enabled` because a zero level is a valid
setting and not the same statement.

## Three things have to change shape, not stretch

**`CompiledBusGraph::destinations: [u8; MAX_BUSES]`** is one output per node
and must be **replaced, not widened**. The assumption it rests on is stated at
the top of `mixer.rs`: every bus owns one permanently allocated buffer and no
two nodes ever share one, which is what removes the pooled reference-counted
buffer assignment a general graph engine needs. A second outgoing edge is
exactly the thing that assumption excludes.

**`CompiledLatency` becomes per-edge.** Its doc comment currently says *"Each
producer has exactly one destination, so 'per edge' and 'per producer' are the
same thing and the simpler one is honest until step 6 makes them differ."*
This is that step. `ChannelStrip::compensation` and `BusStrip::compensation`
become a delay per outgoing edge, and `StructuralCommand::SetCompensation`,
keyed by `EffectTarget` today, needs an edge id.

**`OutputStage` has to learn to smooth.** There is no gain smoothing at strip
level at all today: raw gain is stamped per block, or per 32-frame control
tick when modulated. A send level must be smoothed, so this fills a gap rather
than adding a feature. `mooloop-dsp/src/smooth.rs::Smoothed` is what to use.

## The tap points already exist

They are frames in the block loop rather than new concepts: pre-fader is
before `apply_pan_segments` (channels) / `apply_balance` (buses), post-fader
is after.

`OutletTap::Output` and `is_upstream_of_chain()` (`outlet.rs`) are the
vocabulary, and `OutletTap::Output` is currently declared with nothing
publishing one -- this is what publishes it. `EdgeRefusal::TapIsLate` is the
refusal that has to learn about latency-bearing taps instead of refusing them
outright.

## Acceptance

`MIXER_PLAN.md`'s, unchanged:

- a delay on a return;
- post-fader sends from two strips following their faders;
- one switched to pre-fader, with the documented different result;
- dry and wet paths still latency-aligned at several block sizes.
