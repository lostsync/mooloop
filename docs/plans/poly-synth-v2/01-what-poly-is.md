# What Poly is

This is the reference document for the rest of this plan. It records the
decisions; the numbered steps implement them. Read this first even when
picking up a later step, and update it here — not in a step — if a decision
has to change.

## Why

Poly is currently Mono times N. `PolySynth`
(`crates/mooloop-dsp/src/polysynth.rs`) runs the same voice as `MonoSynth`:
three `Osc`, one `Svf` low-pass, `apply_drive` after the filter, one `Adsr`
doing both amplitude and filter duty. `PolySynthParams`
(`crates/mooloop-core/src/synth.rs:415`) is `MonoSynthParams` plus
`polyphony` and `spread`. `POLY_DESCRIPTORS` is literally built by copying
`MONO_DESCRIPTORS` and appending two entries
(`crates/mooloop-core/src/generator.rs:295`).

Worse, every voice is *identical*. `PolyVoice::note_on` resets every
oscillator's phase to zero (`polysynth.rs:228`) and every voice reads the same
parameters, so a five-note chord is one timbre summed five times with slightly
different pitches. That is why chords sound static and why the instrument
sounds smaller than its voice count.

## The decision

**Poly is a wide programmable analog poly.** Many voices behaving slightly
differently. Its identity lives in four places:

1. **Per-voice variation.** Stable, deterministic pitch, cutoff, envelope-time
   and phase offsets per voice slot. This is the single biggest change and it
   costs almost nothing.
2. **A broad multimode filter.** LP12 / LP24 / BP12 / HP12 off the existing
   `Svf`, which already computes all three outputs and is a genuinely good
   clean filter.
3. **Unison as a real voice multiplier**, allocated and stolen as groups —
   not an oscillator-detune macro.
4. **Stereo behaviour**: voice spread plus a small internal chorus/ensemble
   lane.

The neighbourhood is Prophet / Jupiter / OB, with Juno influence confined to
the chorus. Three full oscillators per voice is already that architecture, so
**do not reduce the oscillator section to chase a Juno.** No UI text or docs
copy may claim emulation of any specific instrument.

General modulation is channel state, not a Poly subsystem. Poly declares
parameters that the channel's sources may target and keeps only genuinely
voice-local synthesis behavior — envelopes, keytracking, drift, and unison —
inside its voice pool. Its internal chorus may retain an algorithmic LFO
because that is part of the sound processor, not a routable control source.

## What Poly does not get

Fixed for v2:

- No acid semantics. No Accent, no note priority, no legato/retrigger modes,
  no held-note stack. Those are Mono's and they are meaningless across a voice
  pool.
- No Mono character filter models. Poly's filter is the clean SVF, expanded.
  Drive stays a gentle *color* control, not the centre of the instrument.
- No device-local modulation matrix or LFO. The shared channel modulation rack
  is outside Poly's initial voice-feature scope; it reaches Poly through its
  descriptors and the common device frame.
- No per-voice user editing of drift offsets. Drift is one knob; the offsets
  are hidden by design.
- No arbitrary oscillator routing matrix. Sync is one fixed pair (step 06).

## The signal path

```text
PER VOICE:
3 OSC -> MIX -> MULTIMODE FILTER -> VCA -> PAN --\
                     ^                ^           |
                     |                |           +-> SUM -> CHORUS/ENS -> STEREO OUT
                FILTER ADSR       AMP ADSR        |
... additional voices ---------------------------/

VOICE CHARACTER: stable per-voice pitch/filter/env offsets + non-identical phase
```

Note where the chorus sits: **after the voice sum, inside the device.** See
step 05 for why that placement is the one thing in this plan that can go
audibly and confusingly wrong.

Drive's placement is deliberately *not* fixed. Mono moves it ahead of the
filter because that is Mono's identity; on Poly it is a color control and
whether it stays post-filter or moves to a mild pre-filter stage is a
listening call. Make it in step 03 and record the answer there.

## Control surface

Four pages, as today — Poly already has the VOICE page Mono lacks.

| Page       | Section         | Controls                                                      |
|------------|-----------------|---------------------------------------------------------------|
| OSC        | OSC 1 / 2 / 3   | Wave, Semi, Fine, Level, Width; Sync when implemented          |
| AMP/FILTER | Amplitude       | Amp ADSR                                                       |
| AMP/FILTER | Filter          | Mode, Cutoff, Resonance, Env Amount, Keytrack, Drive/Color     |
| AMP/FILTER | Filter Envelope | Filter ADSR                                                    |
| VOICE      | Allocation      | Polyphony, Unison, Detune                                      |
| VOICE      | Character       | Drift, Spread, Chorus mode (Amount only if needed)             |

The VOICE page is currently two controls floating in a `space-around` layout
(`crates/mooloop-ui/ui/poly-device.slint:233`). It becomes a real page with
two sections. Step 02 makes that layout call and later steps add into the
shape it establishes.

The common device frame's `MOD` affordance opens the channel shelf; Poly has
no device-local MOD page. Velocity remains native note behavior and may later
be published as a named channel outlet, rather than becoming a competing
device-local expression panel.

Per the project tooltip convention, every new control's tooltip carries the
value only; explanatory text goes to the status bar.

## Parameter and serialization rules

- `PolySynthParams` already carries `#[serde(default)]`
  (`crates/mooloop-core/src/synth.rs:414`), so added fields are safe. Keep it.
- **Never renumber an existing parameter ID.** IDs 0-16 are in use
  (`crates/mooloop-core/src/generator.rs:166-183`) and are automation lane
  addresses. The Mono plan claims **20-29**; Poly's new IDs start at **40** so
  the two never collide even by accident.
- The descriptor tables are split by Mono step 02. Do not reintroduce
  `POLY_DESCRIPTORS` being built from `MONO_DESCRIPTORS`.
- Old projects must open and play. Every added enum needs a deterministic,
  musically conservative default for an unmarked patch: LP12 for filter mode,
  1× unison, OFF chorus, and Drift at its low default. An old project loads
  sounding essentially as it did.

## Real-time rules

- No allocation in `process()`, note handling, filter mode switching, drift,
  or unison group allocation.
- **Determinism is a hard requirement, not a nicety.** Drift and phase offsets
  come from stable per-slot seeds, never from runtime entropy. The same
  project rendered offline twice, and rendered vs played live, must produce
  the same audio. This is what makes offline render trustworthy.
- Changing filter mode, unison count, chorus mode, or polyphony must never
  leave an orphaned active voice. `apply_params_to_voices`
  (`polysynth.rs:147`) currently just flips `active = false` on out-of-range
  slots, which cuts them dead mid-note; step 04 has to do better than that
  once groups exist.
- Extreme resonance × drive stays finite and bounded, per mode.

## The end state

Load Mono and Poly with the same saw. Poly invites voicing, drift, chords,
unison, spread, and chorus. If a change to Poly makes it a better bass
monosynth, it is the wrong change.
