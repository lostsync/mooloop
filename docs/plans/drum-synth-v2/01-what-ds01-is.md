# What DS-01 is

This is the reference document for the rest of the plan. It records the
decisions; the numbered steps implement them. Read this first even when
picking up a later step, and update decisions here rather than allowing the
steps to disagree.

## The decision

**DS-01 is a percussion synthesizer built on one universal voice.** It has no
drum-type mode. Kick, snare, rim, clap, tom, hat, ride, cowbell, and the
sounds that have no name are configurations of the same architecture, and
every control is live in every one of them.

It is not an emulation, and no UI or documentation may claim that it
reproduces a specific instrument or drum machine.

## Why one voice and not several engines

Tattoo and DrumSpillage 2 both give a pad a selectable synthesis model.
Microtonic does not: its eight channels are the same oscillator, noise
generator, filter, envelopes, and modulation matrix, and its range comes from
how far those controls reach. Microtonic is the right precedent here for a
reason that is specific to this codebase rather than a matter of taste.

A selectable-engine device reintroduces exactly the union that made v1
unaddressable. Each engine would need its own descriptor namespace, routes
would be scoped to whichever engine was active, and the engine selector would
be a structural discrete sitting upstream of everything. That is v1's problem
with more code in it.

One voice means one descriptor table, one set of never-renumbered ids, and a
modulation route that keeps meaning what it meant. The instrument's breadth
then has to come from the depth of the architecture — which is the harder and
better problem.

## The three layers

DS-01's voice is three sound sources into one shaper. The sources are chosen
so that between them they span what percussion actually is: a pitched body, a
noise band, and a resonance.

1. **TONE** — a pitched oscillator with a continuous wave morph, a bipolar
   pitch envelope, and an optional bank of detuned partials. This is the kick's
   sweep, the tom's fundamental, the snare's shell tone, the 808 hat's metal,
   the blip, and the zap.
2. **NOISE** — a noise generator with a selectable color and a rate reducer,
   through its own morphing state-variable filter with resonance and its own
   envelope. This is the snare's rattle, the hat, the cymbal wash, the vinyl
   crackle, and the air on everything else.
3. **BODY** — a bank of three tuned resonators excited by the impulse and by
   the noise, with a ratio spread from harmonic to inharmonic and its own
   decay and damping. This is the layer v1 has no equivalent of, and it is
   where most of the new drum types come from: rim, clave, conga, tabla, bell,
   cowbell, and the ringing metal that is neither a tone nor a noise.

They sum, and the sum goes through a **SHAPE** stage — drive with a selectable
character, plus a bias/fold and a rate reducer — and then the amplitude
envelope and the output.

## Signal path

```text
PER VOICE

  trigger ──> BURST ──> impulse train ──┬──────────────────────────┐
              (n, spacing, spread,      │                          │
               level step, pitch step)  │                          │
                                        v                          v
   PITCH ENV ────────> TONE OSC ─────────────────────────> level ──┐
                       (morph, partials, spread)                   │
                                                                   │
   NOISE ENV ────────> NOISE GEN ──> RATE ──> SVF ────────> level ──┼──> SUM
                       (color)              (morph, res)           │
                                                                   │
                       BODY ──> 3 tuned resonators ─────> level ───┘
                       (pitch, ratio, decay, damping)
                         ^
                         └── excited by impulse ↔ noise (Excite)

   SUM ──> SHAPE ──> AMP ENV ──> OUT (mono)
           (drive, character, bias, bits)

   MOD ENV, VELOCITY, NOTE, BURST INDEX, HIT ALTERNATOR, PER-HIT RANDOM
       ──> DS-01's internal matrix ──> any continuous parameter
```

The voice is mono. The channel strip places it in the stereo field, as it
does today.

## Every control is live

The rule that separates DS-01 from v1: **no control is inert because of the
value of another control.** A parameter that does nothing is a parameter that
cannot be honestly modulated.

Two exceptions exist and are named here rather than hidden:

- **Tone Spread** does nothing at **Tone Partials = 1**. Partials is a stepped
  count, the one structural control inside a source section.
- A layer at **Level = 0** is inaudible. This is a mix decision, not a mode,
  and it follows ML-P8's rule that a zero-level source still exists at its
  pre-level tap: the layer keeps running, and its signal is still available to
  the body resonator's excitation and to the published audio outlets.

Neither is v1's situation, where two thirds of the panel was dead at any
moment.

## Latched versus continuous

A drum hit is short and its interesting part is the first few milliseconds.
That makes "what does changing this parameter do to a hit already sounding?"
a real question with a musical answer, and DS-01 publishes it as a rule rather
than inheriting it from where the code reads a struct.

**Latched at trigger.** Resolved once when the hit starts, from the parameter
values current at that sample, and not revisited for the life of that hit:

| Latched | Why |
| --- | --- |
| All envelope attack / hold / decay / curve values | Changing a running envelope's rate steps its output |
| Pitch Env Depth | It is a fixed excursion for this hit |
| Burst repeats, spacing, spread, level step, pitch step | The structure of the hit is decided when it begins |
| Note pitch and Tune | The hit has one pitch, already true in v1 |
| Per-hit Random and Hit Alternator values | One value per hit, by definition |

**Continuous.** Read every control tick for the life of the hit, and smoothed
where a step would click:

| Continuous | Why |
| --- | --- |
| All three layer levels, and the output level | Mix moves must be audible mid-tail |
| Noise cutoff, resonance, filter morph, rate | These are the sweepable ones |
| Tone wave morph and spread | Timbre moves within a long hit |
| Body decay and damping | Damping a ringing tail is a gesture |
| Drive, bias, bits, output high-pass | The shaper is a colour control |

Latched is not a lesser status. Because each hit re-latches, an LFO on Amp
Decay produces a hat pattern whose hits differ from one another — which is the
musically useful reading, and the one a per-sample re-derivation would destroy.

The two lists above are the contract. A parameter that is neither is a bug in
this document, not a free choice at the call site.

## Event ordering

Modulation and automation reach a device as timed `Event::ParamValue` at the
32-frame control rate, and DS-01 latches parameters inside `trigger()`. So:

> A parameter event at offset *n* is visible to a note-on at offset *n*.

The renderer must deliver parameter events before note-ons at the same offset,
and DS-01 must apply them before triggering. Without this rule a route aimed at
a hit lands on the *next* hit, which is both wrong and untestable-looking.
Step 02 states where this is enforced and tests it directly.

## Modulation ownership

The split follows `MODULATOR_SYSTEM_SPEC.md` and the ML-P8 precedent.

**The channel rack owns** reusable channel-level sources and every route that
crosses a device boundary. DS-01 publishes a complete descriptor table, so any
channel LFO, envelope, macro, or outlet can reach any of its continuous
parameters. This is the thing v1 could not do at all.

**DS-01 owns** modulation endemic to hitting a drum: velocity, note, its four
envelopes, where it is in a burst, whether this is an odd or an even hit, and
a deterministic per-hit random value. These are per-hit and cannot be
expressed as a channel-rate signal without collapsing eight simultaneous
voices into one number.

DS-01 must make complete sounds with no channel routes at all.

### On per-hit random

The taste brief is explicit that randomized "humanize" is the wrong model and
that real feel is consistent displacement plus dynamics, not noise. DS-01's
per-hit random is not a humanize button and must never be presented as one:

- it is a matrix source with an explicit destination and a signed depth, like
  every other source, and it is off until routed;
- it is deterministic. The value is derived from a per-voice hit counter and
  the node seed, so an offline render and a live take of the same event stream
  produce identical samples;
- it sits beside two sources that are *not* random and are the ones most
  likely to earn their place: **Burst Index**, which varies a value across the
  impulses of a single flam or roll, and **Hit Alternator**, which alternates
  between successive hits. Both are consistent displacement.

There is no global amount, no dice button, and no default route.

## Velocity is a source, not a multiply

v1 multiplies the voice output by `velocity / 127` and stops there. DS-01 makes
velocity a first-class matrix source with a default route to amplitude, so it
still behaves normally out of the box, and so that a patch can put velocity on
pitch, decay, noise colour, filter cutoff, drive, or burst spacing.

This is the single control that most decides whether ghost notes read as part
of the groove or as quiet copies of the same hit, and it is worth the matrix
row it costs.

## Published vocabulary

Following ML-P8: `Gate` stays high while a note is held; `Trigger` is a
momentary pulse at the hit. DS-01 publishes both.

Control outlets: `Amp Envelope`, `Mod Envelope`, `Velocity`, `Note`, `Gate`,
`Trigger`. Per-voice signals use the deterministic focus-voice reduction.
Audio outlets: `Tone`, `Noise`, `Body`, `Pre-Shape`. Audio ports never
masquerade as control telemetry.

## Voices, choke, and retriggering

- The voice pool stays a small fixed pool with oldest-first stealing. Eight is
  the current `MAX_DRUM_VOICES` and is enough.
- Note-offs are ignored **unless** an envelope is in gate mode (step 03). A
  gated envelope is how a ride cymbal or a held cabasa becomes possible; the
  default is one-shot, as now.
- Choke stays a cross-channel group mechanism and keeps working exactly as it
  does today. DS-01 adds a **Choke Time** so a choke can be a fast fade or an
  audible damp rather than always a fixed 5 ms. Open and closed hats on two
  channels remain the canonical case.
- DS-01 adds a **Retrigger** mode: `Poly` stacks hits as v1 does, `Mono` makes
  a new hit cut the previous one from the same channel at the Choke Time.
  Mono is what a real 808 does and what a fast hat pattern usually wants.

## Parameter and persistence rules

- Add a new `Ds01` generator kind with its own `Ds01Params`, project state,
  descriptors, DSP node, and device face. Do not reinterpret `DrumSynth`.
- **DS-01's parameter ids are their own namespace starting at zero**, and its
  descriptor table lives in `crates/mooloop-core/src/ds01.rs` beside its
  parameters, following ML-P8. The shared `SYNTH_PARAM_*` ids exist because
  Mono and Poly are one voice with a different count; this device is not.
- Ids are assigned once, in the step that introduces them, and never
  renumbered. The bands in step 02 are reservations for review, not permission
  to reuse an id. The implementation asserts descriptor-id uniqueness.
- Structural discretes — noise color, envelope gate mode, retrigger mode,
  partial count, drive character, matrix source and destination — are
  automatable only where the step defines a click-free transition. They default
  to ineligible for modulation.
- Matrix rows persist stable source and destination enums plus a signed
  amount. Empty rows are not serialized as UI placeholders.
- Factory patches persist only authored values. Seeds and filter state are
  runtime implementation details.

## Realtime and determinism rules

- No allocation, locks, I/O, or container growth in `process()`, trigger
  handling, matrix evaluation, or outlet publication.
- Noise, per-hit random, and voice stealing are deterministic. An offline
  render and live playback of the same event stream produce identical samples.
- Every resonator and shaper is bounded for every control combination,
  including under modulation at the extremes of every range. The safety
  behaviour is part of the sound, not an invisible limiter.
- Layers sum honestly under `docs/GAIN_STRUCTURE.md`. Adding the body or the
  noise never turns the tone down. Step 06 owns the output reference and
  replaces v1's single `OUTPUT_REFERENCE` constant with a documented one.
- The preview renderer keeps v1's best property: it renders through the
  production voice path, so the drawn hit is the hit.

## What DS-01 does not get

- No drum-type mode selector, in any disguise, including a preset dropdown
  that changes which controls exist.
- No second selectable synthesis engine.
- No sample layer. If a sound needs a sample, the sampler is the device.
- No multi-part internal mixer; the kit is the set of channels.
- No global humanize, swing, or groove control. Feel belongs to the sequencer,
  and putting a second timing model inside one drum would fragment it.
- No automatic gain normalization by layer or by velocity.

## End-state test

From one default patch, DS-01 must reach all of these with its own controls
and no insert effect or channel modulator:

- a sub kick, a punchy kit kick, and a distorted DnB kick;
- a tight cracking snare and a deep rimshot;
- a clap that reads as a clap, not as a snare with a long noise tail;
- toms that tune across a range and still sound like the same drum;
- a closed hat, an open hat that chokes correctly, and a ride that rings
  while held;
- a cowbell, a clave, and at least one metallic sound with no name;
- a hat pattern where ghost hits are audibly *different*, not just quieter.

If the range depends on the factory patches being carefully hand-tuned rather
than on the controls reaching, the architecture has failed even if the kit
sounds good.
