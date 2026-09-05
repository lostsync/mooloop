# What ML-P8 is

This is the reference document for the rest of the plan. It records the
decisions; the numbered steps implement them. Read this first even when
picking up a later step, and update decisions here rather than allowing the
steps to disagree.

## Why the old plan was not enough

The old proposal tried to distinguish Poly from Mono mostly by multiplying a
basic voice: three oscillators per voice, up to sixteen drifting voice slots,
unison groups, stereo spread, and a chorus after the sum. That is several ways
to layer nearly the same signal. It makes a large sound, but does not make a
deep instrument.

ML-P8 keeps three oscillators because the implementation and panel space
already exist, then makes the relationship between them the point. An
oscillator can be audible, a modulation source, a sync source, or all three.
Muted oscillators remain useful modulators. Sub and noise are proper voice
sources. Feedback and audio-rate cross-modulation make the voice capable of
harmonic, metallic, unstable, and damaged sounds before any insert effect is
added.

## The decision

**ML-P8 is an eight-voice programmable polysynth built around a three-
oscillator signal network.** It is not an emulation and no UI or documentation
may claim that it reproduces a specific instrument.

Its identity lives in five places:

1. **An oscillator network, not three layers.** Every oscillator can
   cross-modulate and hard-sync the others. Each has self-feedback. Noise can
   excite the same modulation inputs.
2. **A complete per-voice source mixer.** Three oscillators, a derived sub
   oscillator, and colored noise enter the voice independently. A zero-level
   oscillator still exists at its pre-level modulation tap.
3. **A nonlinear voice path.** The multimode filter has a deliberately bounded
   output-to-input feedback loop, with drive inside the loop.
4. **Native per-voice modulation.** Separate amplitude and filter envelopes,
   velocity, key, gate, and the device's own LFO can route within ML-P8 without
   borrowing a channel modulator.
5. **A published interface.** Its LFO, envelopes, note values, gate/trigger,
   and oscillator audio taps have deliberate typed identities so the larger
   routing system can subscribe to them.

Drift, unison, stereo spread, and chorus remain useful, but they are finishers.
The definition of ML-P8 must still hold with Drift at zero, Unison at 1x, and
Chorus off.

## Eight voices means eight voices

ML-P8 owns exactly eight physical voice slots. Ordinary playing provides eight
simultaneous notes. Unison consumes those slots: 2x leaves four-note
polyphony, 4x leaves two, and 8x is monophonic. There is no 1-16 Polyphony
knob. A fixed pool makes the voice-stealing rules, published focus-voice
policy, CPU ceiling, and name honest.

Each physical voice contains three oscillator cores, one sub divider, one
deterministic noise generator, two ADSRs, two SVF stages, drive, and fixed
state for feedback and modulation. Storage is static and prepared before the
audio callback.

## Signal path

```text
PER PHYSICAL VOICE

 note/pitch -------> OSC 1 ----\
                 ^   ^          \
                 |   |           \
 note/pitch -------> OSC 2 --------+--> SOURCE MIX --> DRIVE
                 ^   ^           /          ^            |
                 |   |          /           |            v
 note/pitch -------> OSC 3 ----/             +-- z^-1 -- FILTER --> VCA --> PAN
                    ^  ^
                    |  +--- oscillator self-feedback
                    +------ six-way oscillator XMOD + noise modulation

 selected osc phase ---> SUB -------/
 deterministic noise ----------------/

 AMP ADSR -------------------------------> VCA
 FILTER ADSR ----------------------------> FILTER, plus internal routes
 VELOCITY / KEY / GATE / ML-P8 LFO ------> native internal modulation routes

EIGHT VOICES --> SUM --> optional CHORUS/ENSEMBLE --> ML-P8 OUT
```

`z^-1` is one sample of delay. It makes the filter feedback loop and cyclic
oscillator modulation graph causal and independent of oscillator evaluation
order.

## Internal modulation and channel modulation

The channel rack owns reusable channel-level modulation sources and all routes
between devices. ML-P8 owns modulation that is endemic to its synthesis
algorithm:

- amplitude and filter envelopes are per voice;
- velocity, key, and gate are per note/voice;
- oscillator cross-modulation and feedback are audio-rate voice topology;
- the ML-P8 LFO is part of the instrument and works in a saved patch without
  any channel shelf state.

This is not a private copy of a channel LFO. It has its own stable parameters,
trigger behavior, routes, and public outlet. Channel sources may still target
ML-P8 parameters, and ML-P8's published outlets may modulate later devices.
The two systems extend one another; neither is a substitute for the other.

Internal routes use the same base-plus-offset rule as channel routes, but are
evaluated inside each voice so an envelope from one note is not collapsed into
the envelope of another. Structural selectors are not modulation targets;
route amounts and synthesis parameters are automatable.

## Published vocabulary

`Gate` is the correct term for a signal that stays high while a note is held.
`Trigger` is a momentary pulse at note-on. ML-P8 publishes both rather than
using one word for two timing contracts.

Control outlets are `LFO`, `Amp Envelope`, `Filter Envelope`, `Velocity`,
`Note`, `Gate`, and `Trigger`. Per-voice signals use the deterministic focus-
voice reduction in step 06. Audio outlets are `Osc 1`, `Osc 2`, `Osc 3`,
`Sub`, `Noise`, `Pre-Filter Mix`, and `Filter`. Audio ports never masquerade
as slow control telemetry.

## Control surface

Five pages keep the complete instrument local without turning the ordinary
channel rack into a patch-cord UI:

| Page | Sections | Controls |
| --- | --- | --- |
| OSC | Oscillators; Sources | Oscillator, Sub, and Noise controls |
| ROUTE | XMOD; Sync; Feedback | Audio-rate topology and feedback |
| AMP/FILTER | Amp; Filter; Filter Env | Envelopes and tone shaping |
| MOD | ML-P8 LFO; Internal Routes | Local source and route editing |
| VOICE | Allocation; Character; Finish | Unison, Drift, stereo, chorus |

The common frame's `MOD` affordance still opens the channel shelf. ML-P8's
`MOD` page edits the instrument's own saved synthesis topology. The labels and
distinct placement must make that ownership visible.

Per the project tooltip convention, a tooltip carries the value only;
explanatory text belongs in the status bar.

## Parameter and persistence rules

- Add a new `MlP8` generator kind with its own `MlP8Params`, project state,
  descriptors, DSP node, and device face. Do not silently reinterpret the
  existing `PolySynth` kind.
- Parameter IDs are stable within the new kind. Assign them once in the step
  that introduces them and never renumber them. The numeric bands in the
  following steps are reservations for review, not permission to reuse an ID.
- IDs 0-14 are the three five-control oscillator blocks. IDs 15-19 are Amp
  ADSR plus Glide. Later steps reserve non-overlapping contiguous bands from
  20 onward; the implementation must assert descriptor-ID uniqueness.
- Internal route rows persist stable source and destination enums plus signed
  amount. Empty rows are not serialized as UI placeholders.
- Factory patches persist only authored values; deterministic seeds and delay
  state are runtime implementation details.
- Discrete topology changes such as sync source, route source/destination,
  unison, and chorus mode are automatable only when the step defines a
  click-free transition. They default to ineligible for channel modulation.

## Realtime and determinism rules

- No allocation, locks, I/O, container growth, or graph discovery in
  `process()`, note handling, route evaluation, or outlet publication.
- Cyclic audio-rate paths use explicit fixed delay/state. Their result must not
  depend on iteration order or runtime entropy.
- Noise, Chaos LFO, drift, oscillator start phase, and voice stealing are
  deterministic. Offline render and live playback of the same event stream
  produce identical samples.
- Every feedback path is finite and bounded for every control combination. A
  safety shaper is part of the sound, not an invisible master limiter.
- Source levels sum honestly under `docs/GAIN_STRUCTURE.md`. One oscillator at
  0 dB remains the device reference; enabling sub, noise, feedback, or another
  oscillator never turns the first one down.
- A topology change on sounding voices either crossfades/smooths or waits for a
  documented safe boundary. It never clears half a unison group or leaves a
  stale feedback state attached to a new note.

## What ML-P8 does not get

- No acid held-note or accent semantics; those belong to the mono instrument.
- No automatic drift, unison, or chorus as a substitute for programming.
- No arbitrary cross-channel audio feedback hidden inside the generator.
- No claim that control-rate published outlets can perform audio-rate FM.
- No general graph editor as part of this device plan.
- No automatic gain normalization by oscillator, note, or unison count.

## End-state test

Start from one plain saw with Drift 0, Unison 1x, Chorus off. Within a few
controls ML-P8 must reach all of these without an insert or channel modulator:

- stable warm poly chords;
- envelope-shaped cross-mod brass;
- clangorous inharmonic stabs;
- sync sweeps;
- sub-heavy industrial bass chords;
- noise-excited and feedback-torn textures;
- moving pads whose modulation comes from the instrument itself.

If the impressive patches all depend on stacking, detuning, or chorus, the
design has failed even if they sound good.
