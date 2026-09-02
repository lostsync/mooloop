# Internal modulation and published outlets

Step 02 made every DS-01 parameter reachable from the channel rack. This step
adds the modulation the channel rack *cannot* provide, and publishes what
DS-01 knows to the rest of the channel.

## Why DS-01 needs its own matrix at all

A channel source produces one number per control tick for the whole channel. A
drum channel can have eight hits ringing at once, each with its own velocity,
its own position in a burst, and its own envelopes. "This hit's velocity opens
this hit's filter" is not expressible as a channel-rate signal — reducing
eight voices to one number is exactly the category error
`docs/plans/buffer-implementation/02-control-and-modulation.md` describes.

So the split is the same one ML-P8 makes: the channel rack owns reusable and
cross-device modulation; DS-01 owns per-hit modulation. Neither substitutes
for the other, and DS-01 must make complete sounds with no channel routes.

## The sources

Eight, all per voice, all evaluated inside the voice.

| Source | Shape | Notes |
| --- | --- | --- |
| Velocity | Unipolar | Latched at trigger |
| Note | Unipolar | Latched. Normalized around middle C so a patch can key-track anything |
| Amp Env | Unipolar | Live |
| Noise Env | Unipolar | Live |
| Mod Env | Unipolar | Live. The one with no other job |
| Burst Index | Unipolar | 0 at the first impulse, 1 at the last. Constant within an impulse. At Repeats = 1 it is 0 |
| Hit Alternator | Bipolar | +1 and -1 on successive hits of this channel. Latched |
| Random | Bipolar | One deterministic value per hit. Latched |

Burst Index and Hit Alternator are the two that matter most for what this
instrument is for. Burst Index puts a shape across a flam or a roll — a pitch
fall across a drag, a filter opening across a clap. Hit Alternator is the 808
open/closed alternation and the every-other-hat ghost, and it is *consistent*
displacement rather than noise, which is the distinction the taste brief draws.

Random is covered by the rules in `01`: deterministic, routed explicitly with
a signed depth, never a global humanize.

## The routes

Eight rows. Each row is source, destination, signed amount, and a curve.

```text
100 + row * 4 + 0   Source *        stepped, the eight above plus None
100 + row * 4 + 1   Destination *   stepped, any continuous parameter id
100 + row * 4 + 2   Amount          -1 .. +1
100 + row * 4 + 3   Curve           -1 .. +1, shapes the source before scaling
```

Rows occupy 100-131. Source and Destination are structural and
modulation-ineligible; **Amount is modulatable**, which is how a channel LFO
gets to scale a per-hit relationship without knowing anything about voices.

Routes add an offset in normalized destination space around the base value,
identical to channel routes — never an absolute write. Destination eligibility
comes from the descriptor table, so a route cannot address Choke Group or
another route's Source.

A route to a latched destination is evaluated at trigger. A route to a
continuous destination is evaluated per control tick within the voice. This
falls straight out of `01`'s table and needs no separate rule.

**One default route ships in the default patch: Velocity to Amp, at full
amount.** DS-01 must feel normal before it is programmed, and this is also the
row that demonstrates the mechanism the first time anyone opens the device.
`Velocity Amount` at id 5 stays as the plain control for the common case; the
matrix row is for putting velocity somewhere else.

## Published outlets

Under `COMPOSABLE_DEVICE_UNITS.md` and `MODULATOR_SYSTEM_SPEC.md`, and with
ML-P8's vocabulary:

**Control outlets** — `Amp Envelope`, `Mod Envelope`, `Velocity`, `Note`,
`Gate`, `Trigger`. Per-voice signals reduce through the deterministic
focus-voice policy. Published into the per-channel table and read on the
following block, so there is one block of latency, it is identical offline and
live, and graph order stops mattering.

`Trigger` is the valuable one on a drum channel: it is what lets a kick duck a
bass, open a gate, or fire an envelope on another device without a sidechain
graph. `Gate` stays high only for gated-envelope patches.

**Audio outlets** — `Tone`, `Noise`, `Body`, `Pre-Shape`. These are audio and
are never presented as control signals. A zero-level layer still publishes its
pre-level tap, per `01`.

## Acceptance

- A route from Velocity to Tone Pitch makes soft hits a different pitch, and
  two simultaneous hits at different velocities get different pitches — the
  test that proves the matrix is per voice and not per channel.
- A route from Burst Index to Pitch, over a four-impulse burst, produces four
  distinct pitches within one voice.
- Hit Alternator produces strictly alternating values across a hat pattern and
  survives voice stealing.
- Random renders identically offline and live for the same event stream.
- A channel LFO on a route Amount audibly scales that route.
- `Trigger` from a DS-01 kick drives an envelope on a later device with the
  documented one-block latency.
