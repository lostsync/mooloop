# Outcome: superseded by replacing the algorithm

Closed August 2026. The problem this plan identified was real and the
measurements in `01-understand-current-partition-schedule.md` hold up. The
fix it proposed — spreading the partition MACs across the intervening JACK
blocks — was not taken, because the device was replaced wholesale instead.

## Why

Step 02 would have flattened the spike while leaving two other problems in
place:

- The convolution node could not accept a parameter change. Reverb knob edits
  bypassed `EngineCommand::SetEffectParam` entirely and went through an
  off-thread IR rebuild plus a node swap, and `ReverbEffect::process` ignored
  `events_in`. Meanwhile `ModDestinationDescriptor::for_param` declared all
  six continuous reverb parameters legal modulation destinations, so the rack
  emitted `ParamValue` events at a node that dropped them: modulating the
  reverb was a silent no-op, not a missing feature.
- The generated room — a fourth-order image-source set plus a filtered noise
  tail — is static. Nothing in the response moves, so it rang instead of
  blooming.

An eight-line feedback delay network solves all three at once, and solves the
load problem better than amortization does: cost becomes flat *and* an order
of magnitude smaller, rather than the same total spread thinner.

## Measured, before and after

One reverb instance, release build, 48 kHz, 64-frame period (1333 us budget):

```
                 mean      p50       max     worst
convolution, 0.5s tail    21.6us    0.2us    710us      53%
convolution, 1.0s tail    32.9us    0.2us    957us      72%
convolution, 2.0s tail    54.0us    0.2us   1400us     105%   <- xrun
FDN,         0.5s tail    13.8us   13.5us     60us     4.5%
FDN,         2.0s tail    13.6us   13.5us     45us     3.3%
FDN,        20.0s tail    13.6us   13.5us     40us     3.0%
```

`p50` moving from 0.2 us to the mean is the whole point: there is no longer a
cheap block and an expensive block, just one cost. Decay length no longer
appears in the timing at all, so a 20 s tail — which the convolution player
could not reach, capped at `MAX_IR_SECONDS = 2.0` — is now free.

See `docs/REVERB.md` for the replacement's contract.
