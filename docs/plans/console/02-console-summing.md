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

## The bus is invisible, and that is the requirement

Adam, 2026-09-09, on how the Airwindows pair is actually used: *"you put a
'Channel' plugin on the individual tracks, generally last in the chain. that's
the encode step. then you bus that and whatever else you want to a single buss
where you have the 'Buss' plugin ... a buss like that would decode on input.
i think what i was trying to communicate is that i want that buss to be
invisible."*

So the mechanism is the plugin pair's and the *gesture* is not. There is no
device to place and no bus to make: the decode happens at whatever summing
point the encoded strips converge at, and the master is already one. Two
channels switched to console-on glue with nothing created and nothing
configured, which is the whole difference from the plugins.

It also composes with step 04 for free. Route some channels to one track and
that track becomes their decode point, with its own fader as the level.

## Which fader is the drive and which is the volume

This is the part worth being deliberate about, because it is what the ceiling
above turns into at the controls.

A bus's fader sits **after** its input sum in the block order -- input
accumulate, then decode, then the insert chain, then the output stage. So:

- **Turning a channel down is less drive.** It arrives at the decode smaller,
  further from the bound, and the summing law does less to it.
- **Turning the bus down is volume.** The drive is already decided by the time
  the fader is reached, so the character does not change.

Which is Adam's own instruction for using it: *"if I want those sources to be
quieter I need to turn down the mixer busses that have this analog/nonlinear
summing mode enabled."* Nothing has to be built to make that true -- it falls
out of where the decode goes -- but it does have to be true, so it is an
acceptance case rather than a note.

## The shape of the setting

One algorithm for the whole mixer -- a `ConsoleMode` on the project -- and a
boolean per strip. Off is the default and is bit-identical to today, which is
what makes the null test meaningful rather than approximate.

## The thing to decide first, because it contradicts a standing rule

`GAIN_STRUCTURE.md` says *"Nothing bounds a sample in the live path"* -- the
only clip in the tree is the 24-bit WAV encoder. Console decode is `asin`, so
**a decode point hard-limits at `PI/2`, which is +3.92 dBFS.**

(This plan first said 0 dBFS, reading the unscaled `asin`'s *input* domain as
its output range. Corrected while building it; the real figure is more
headroom than was agreed to, not less.)

At mooloop's -12 dBFS operating level a full mix of 8-16 sources lands right
at that ceiling, so console mode will be doing real work at default levels
rather than being decorative -- which is the glue people want it for. But it
does mean switching it on changes the summing law from honest-and-unbounded to
bounded-at-every-decode.

**Adam accepted it, 2026-09-09**, and on the product's grounds rather than the
engine's: *"the goal with the mixer is that, yeah you can do a perfectly clean
mix on it if you want but you could also drive it and get something nice in
return."* Console-off stays the clean path and stays the default; console-on
is a thing you drive. So the bound is not a cost of the feature, it is the
feature.

It is therefore said out loud in `GAIN_STRUCTURE.md` and metered, rather than
softened. The alternative -- a hidden headroom trim -- buries a gain, which is
the thing this codebase keeps refusing to do, and it would have removed the
one control that makes the drive playable.

## Acceptance

- Two channels console-on, summing into master, audibly glue and are
  measurably not the linear sum -- **with no bus created and nothing placed in
  a chain**.
- One channel alone nulls sample-for-sample against console-off.
- Pulling the destination bus down is level and not character; pulling its
  feeders down is character. Measurable as well as audible: the bus-fader case
  scales the output and leaves the ratio between harmonics where it was.
- A bounce matches a live take.

## Verification

`summing_stays_linear_however_the_faders_sit` (`gain_structure_tests.rs`)
keeps holding -- for console-off, which stays the default. Console-on gets its
own pair of tests rather than weakening that one:

- a null test: one console strip into master == console off;
- a non-linearity test: two console strips != the linear sum of the two.

Plus a listening pass, which for this step is the point rather than a
formality.
