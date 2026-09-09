# 02 — Console summing

Wanted most, cheapest to reach, and it needs **none** of the routing work.
It is the only step in this plan that changes how the mixer sounds.

## The mechanism

Airwindows' Console series works by putting a non-linearity at the *point of
departure* and its exact inverse at the *point of arrival*, so that one
channel alone is untouched and the character is entirely in what happens
between several of them.

- **Encode** is per-strip, at its output. The two places a signal leaves a
  strip are the channel sum and the bus sum in `render.rs`, and both already
  sit immediately after the output stage and the compensation delay. That is
  the encode point.
- **Decode** is per-bus, at its input, at the head of the bus phase before
  `effects.process`.

Start with the **sin/asin** pair, because its identity property is exactly
testable: `asin(sin(x)) == x`, so one strip alone nulls against console-off
and the character lives entirely in the interaction between strips. Later
curves plug into the same encode/decode pair. Our own implementation, in the
spirit of the Console series, not a port.

## The two accumulators

A bus that has any encoded input needs **two input accumulators**: an encoded
sum and a linear sum. Decode the first, add the second.

That is exactly Adam's *"the decode stage is mixed with master to pick up any
channels that don't have it switched on"*, generalised from master to every
bus -- one extra `StereoBus`, allocated only when a bus actually has an
encoded feed.

**Nesting falls out for free.** A bus that is itself console-on encodes at its
own output; its destination decodes. No special case, and no parallel
decoders.

## The shape of the setting

One algorithm for the whole mixer -- a `ConsoleMode` on the project -- and a
boolean per strip. Off is the default and is bit-identical to today, which is
what makes the null test meaningful rather than approximate.

## The thing to decide first, because it contradicts a standing rule

`GAIN_STRUCTURE.md` says *"Nothing bounds a sample in the live path"* -- the
only clip in the tree is the 24-bit WAV encoder. Console decode is `asin`,
whose domain is +/-1, so **a decode point hard-limits at 0 dBFS**.

At mooloop's -12 dBFS operating level a full mix of 8-16 sources lands right
at that ceiling, so console mode will be doing real work at default levels
rather than being decorative -- which is the glue people want it for. But it
does mean switching it on changes the summing law from honest-and-unbounded to
bounded-at-every-decode.

**Recommendation: accept it, say so in `GAIN_STRUCTURE.md`, and meter it,**
since that ceiling *is* the effect. The alternative -- a hidden headroom trim
-- buries a gain, which is the thing this codebase keeps refusing to do.

## Acceptance

- Two channels console-on, summing into master, audibly glue and are
  measurably not the linear sum.
- One channel alone nulls sample-for-sample against console-off.
- A bounce matches a live take.

## Verification

`summing_stays_linear_however_the_faders_sit` (`gain_structure_tests.rs`)
keeps holding -- for console-off, which stays the default. Console-on gets its
own pair of tests rather than weakening that one:

- a null test: one console strip into master == console off;
- a non-linearity test: two console strips != the linear sum of the two.

Plus a listening pass, which for this step is the point rather than a
formality.
