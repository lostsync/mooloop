# Step 05: acceptance

The measurements that decide whether this worked, rather than whether it
compiles.

## Impulse alignment

Two channels, one impulse each at the same tick, one channel carrying a Drive.
Render the master. **The two impulses arrive in the same frame.** Before this
plan they arrive `OVERSAMPLER_LATENCY_FRAMES` apart, so the test is written to
fail on `main` before it passes here.

Then the same through a bus: channel with Drive into bus 1, plain channel into
bus 1, bus 1 into master. And the asymmetric case: one channel straight to
master, one through a bus that itself carries a Drive.

## The whole-project null

Render a project twice at different block sizes and compare sample for sample.
Compensation must not make output depend on the block boundary — a delay whose
ring is advanced per block rather than per frame would pass every alignment
test above and fail this one.

## Offline and live agree

The offline renderer builds its own `RenderState`, so it is a second place the
plan is compiled. Render the same project offline and through the block path
and compare.

## Bypass does not move the channel

Toggle a Drive's bypass and assert the channel's arrival is unchanged, which is
the rule step 03 states and the one a listener would notice first: A/B-ing an
effect must A/B the effect.

## Cost

The footprint test records the delays' bytes. A compensation delay is
`4 * 2 * frames`; the figure that matters is what a project with one Drive
actually pays, not the addressable worst case.

## Done when

Every measurement above passes, `docs/CURRENT.md` says the mixer is time
aligned, and `AUDIO_ARCHITECTURE.md`'s migration sequence marks step 5 landed
so step 6 can start against a compensated tree.
