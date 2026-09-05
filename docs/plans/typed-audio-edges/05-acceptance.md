# Step 05: acceptance

The measurements that decide whether this worked, rather than whether it
compiles.

## The case the two instrument plans are waiting on

ML-P8 on channel 0 with `Osc 3` at level zero — muted in its own mix, so the
channel is provably not carrying it — and an Aux In on channel 1 subscribed to
`Osc 3`. **Channel 1 is audible and channel 0 does not contain the
oscillator.** Both halves are the test: an edge that worked by leaking the
oscillator into ML-P8's output would pass the first assertion and fail the
second.

The same through DS-01's `Tone`, because the mechanism is not ML-P8's and one
device passing is not evidence that the edge type is general.

## Same block, and the same at any block size

The producer's samples reach the consumer in the block they were made in, not
the one after. Render an impulse on the producer and assert the consumer's
copy lands in the same frame — the same shape of test as the compensation
plan's impulse alignment, and it fails by exactly one block if delivery ever
becomes deferred.

Then the whole-project null: render a project containing an edge twice, at
different block sizes, and compare sample for sample. This is the assertion
that a block-latency edge could never pass, and it is why step 01 rules that
design out rather than treating it as a simpler alternative.

## Offline and live agree

The offline renderer compiles its own plan, exactly as it compiles its own
compensation. Export a project with an edge and compare against the live block
path sample for sample, with the same rules the compensation acceptance found
were necessary: Float32 so the comparison is exact, and an assertion that the
window is audible before an assertion that the two agree.

## Cycles are refused and survive being broken

Two Aux In channels subscribed to each other: both silent, both subscriptions
retained, both reasons legible. Then break one and assert the other resolves
and sounds — the edge comes back rather than needing to be authored again.

A three-channel ring, because a two-cycle can be caught by an equality check
that a longer ring walks straight past.

## Cost

The footprint test records what a project with one edge pays, and what a
project with none pays, and the second figure is the one that matters: an
unused feature that costs 448 KB a channel is the design this plan exists to
avoid.

## Done when

Every measurement above passes, `docs/CURRENT.md` describes Aux In and the
edge, `AUDIO_ARCHITECTURE.md`'s migration sequence marks step 6 landed, and
`poly-synth-v2/` and `drum-synth-v2/` both move to `docs/plans/archive/` —
which is the point of the whole plan.
