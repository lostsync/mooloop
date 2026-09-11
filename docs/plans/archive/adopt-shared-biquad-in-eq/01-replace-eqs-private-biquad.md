# Make the parametric EQ use the shared RBJ biquad

## Problem

`crate::biquad::Biquad` was promoted from EQ in the completed
`share-dsp-primitives` plan, but `effects/eq.rs` still carries its original
private copy. The two implementations are presently character-equivalent, so
the shared module has no production caller and fixes to the RBJ primitive can
silently diverge from the only EQ that needs it.

This is intentionally a separate follow-up rather than reopening the archived
plan: the earlier work delivered the shared primitives and all of its named
adoptions; this is the one remaining adoption that should be small and
independently reviewable.

## What to do

1. Replace EQ's private `Biquad` definition with `crate::biquad::Biquad`.
   Preserve its fixed-size left/right filter banks and its sample-timed
   coefficient-update schedule.
2. Delete the private implementation only after confirming the shared API
   supplies every operation EQ uses: `identity`, `process`, `peak`, `shelf`,
   and `pass`.
3. Keep behavior bit-exact. This is an adoption, not an EQ behavior change;
   do not combine it with the separately deferred coefficient-transition
   crossfade described in `effects/eq.rs`.

## Verification

- `cargo test -p mooloop-dsp --release`.
- Run the existing EQ test and the shared biquad tests. The EQ's response and
  its sample-timed parameter boundaries must remain unchanged.
