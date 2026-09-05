# The burst

One trigger, several impulses. This is how DS-01 makes a clap, and it is the
control most likely to produce something nobody planned.

A clap is not a snare with a longer noise tail. It is three or four noise
bursts a few milliseconds apart followed by a longer one, and no amount of
envelope shaping reaches it from a single hit. Once the mechanism exists for
the clap it is also a flam, a drag, a buzz roll, a stutter, and a machine-gun
fill — all from controls that are live in every patch.

## The controls

| Id | Control | Range | Latched |
| --- | --- | --- | --- |
| 80 | Repeats * | stepped 1-8, default 1 | yes |
| 81 | Spacing | 1-500 ms, log, default 12 ms | yes |
| 82 | Spread | -1 .. +1, bipolar, default 0 | yes |
| 83 | Level Step | -1 .. +1, bipolar, default 0 | yes |
| 84 | Pitch Step | -24 .. +24 semitones, default 0 | yes |

**Repeats = 1 is an ordinary hit**, and it is the default, so the whole
section is inert-looking but not inert: every other control still has an
effect the moment Repeats moves, and Repeats itself is a matrix destination
worth having.

**Spread** makes the spacing non-uniform. Negative accelerates — each gap
shorter than the last, which is the clap and the buzz roll. Positive
decelerates — a drag. Zero is even, which is a machine-gun.

**Level Step** and **Pitch Step** apply per impulse, cumulatively. A negative
level step is the natural clap and flam shape. A positive pitch step across a
four-impulse burst is a fill that climbs; a negative one is a tom roll that
falls.

## How it works

One trigger produces **one voice** that internally re-fires its envelopes,
rather than allocating one voice per impulse. This matters for three reasons:

- an eight-repeat burst does not consume the whole voice pool;
- the body resonator keeps ringing *across* the impulses instead of being
  restarted, which is what makes a burst into a clap rather than four claps;
- the impulse index is available as a per-impulse modulation source, which
  step 07 exposes as **Burst Index**.

So the voice carries an impulse schedule latched at trigger: a sample count to
the next impulse, a remaining count, and the accumulated level and pitch
offsets. On each impulse the amplitude, noise, and pitch envelopes retrigger
from their current level rather than from zero, so overlapping impulses add
rather than cutting each other off.

Total burst length must be bounded: `Repeats * Spacing` with Spread applied is
at most a few seconds, which is fine, but the schedule must not be able to
extend a voice's lifetime indefinitely, and the idle test in step 03 must
cover bursts.

## Why this is not a groove feature

DS-01 has no swing, no humanize, and no timing offset. A burst is a single
event's internal structure, not a placement decision, and it must not become a
back door to a second timing model. Feel belongs to the sequencer; if a burst
control ever starts wanting to know the tempo, that is the signal to stop.

Spacing is in milliseconds and stays in milliseconds. Tempo-syncing it would
make a clap change shape when the project tempo changed, which is wrong.

## Acceptance

- Repeats = 1 renders sample-identical to the same patch before this step.
- A four-impulse burst with negative Spread and negative Level Step, over a
  noise layer, is recognisably a clap and goes into step 09's kit.
- The body resonator rings continuously across a burst rather than restarting.
- An eight-repeat burst uses one voice.
- Every burst configuration terminates, and the voice-idle test covers them.
