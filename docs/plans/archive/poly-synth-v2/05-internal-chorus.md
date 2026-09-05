# Voice allocation, character, and finishing

This step completes the fixed eight-voice pool. Unison, Drift, Spread, and
Chorus are available, but acceptance criteria explicitly prevent them from
carrying the instrument's identity.

## Fixed eight-voice allocation

Allocate exactly eight physical voice slots at construction. A fresh Note On
uses an idle slot; otherwise it steals the oldest sounding note group. Age and
event identity are group properties even at 1x.

Stealing and Note Off operate on complete groups. A voice slot being stolen:

1. begins a short de-click transition from its old output;
2. clears oscillator previous-sample taps, noise/filter state, and the voice
   feedback delay at the defined handoff point;
3. adopts the new note and restarts its envelopes according to the patch;
4. never emits one sample that combines old feedback state with new pitch.

No parameter may silently increase the physical pool above eight.

## Unison consumes the pool

Unison choices are 1x, 2x, 4x, and 8x, giving effective note polyphony of 8,
4, 2, and 1. A Note On allocates the complete group or steals complete older
groups until enough slots exist. Never allocate a partial group.

Group members receive symmetric intentional Detune and Pan offsets. Detune is
a bounded musical curve tuned in step 07; zero means exactly zero. Spread pans
unison members symmetrically. At 1x, Spread pans note voices by their stable
slot positions so a chord can occupy the field without moving randomly on
each render.

There is no automatic gain normalization by unison count. Physical voices sum
honestly like other sources. Factory defaults and patches must respect the
project's -12 dBFS operating reference rather than hiding a per-group divider.

Changing Unison while notes sound releases the old groups through the short
transition and applies the new topology to subsequent Note Ons. It does not
grow/shrink a live group in place.

## Drift is optional character

Drift is one 0-100% control with a default of **0**. It scales stable per-slot
offsets for:

- oscillator pitch, with a shared voice offset plus smaller independent
  oscillator offsets;
- filter cutoff;
- attack, decay, and release times, never sustain;
- oscillator start phase.

Initial maxima for the listening pass are approximately ±5 cents pitch,
±0.15 octave cutoff, and ±7% envelope time. These are ceilings to tune, not
identity claims. At Drift 0, pitches, times, cutoff, and reset phases are
exactly authored. Runtime entropy is forbidden.

## Chorus is a finisher

Keep four internal modes:

```rust
pub enum MlP8Chorus { Off, One, Two, Ensemble }
```

OFF is the default and a true bypass. I and II are fixed, useful chorus
policies; Ensemble is the wider existing algorithm. Reuse the existing
`ModulationEffect` DSP and tune its parameter policies in step 07. Do not add a
Mix control unless listening proves that the fixed modes cannot be made useful.

The chorus processes only ML-P8's summed output. Render voices into a fixed,
construction-time scratch bus, process that bus, then add it to the channel
bus. It must never read and rewrite audio that another generator already put
on the shared bus.

Chorus is not counted as a modulation source and its algorithmic delay LFO is
not published. It is part of the finishing processor, unlike the authored
ML-P8 LFO on the MOD page.

## Parameters and UI

Reserve ML-P8 IDs 64-68 for Drift, Unison, Detune, Spread, and Chorus mode.
Unison and Chorus mode are structural and default to ineligible modulation
destinations; Detune, Spread, and Drift are ordinary automation targets.

VOICE shows `8 voices` as fixed instrument information and displays derived
note polyphony beside Unison. Character contains Drift and Spread. Finish
contains Chorus. This hierarchy should make the oscillator ROUTE and ML-P8 MOD
pages feel primary and chorus feel optional.

## Done when

- 1x plays eight simultaneous notes; 2x, 4x, and 8x provide exactly 4, 2, and
  1 note respectively.
- Voice stealing, Note Off, choke, transport stop, and Unison changes never
  leave half a group or reuse stale oscillator/filter/feedback state.
- Detune and Spread are symmetric and deterministic and do not alter pitch or
  pan when set to zero.
- Drift 0 is exact authored behavior; Drift 100 differs measurably without
  changing sustain levels or using entropy.
- OFF contributes no chorus processing. I, II, and Ensemble are distinct and
  process only ML-P8's scratch bus.
- Topology changes and mode changes on sounding material do not click.
- A worst-case eight-voice patch stays within the measured realtime budget and
  remains finite; unison does not exceed the same eight physical voices.
- The factory identity test in step 07 passes with 1x Unison, Drift 0, and
  Chorus OFF.
