# Per-voice drift

Do this first. It is a small amount of code and it is the change that makes a
chord stop sounding like one timbre stacked five times.

## What is wrong

Every voice is identical in every respect that matters:

- `PolyVoice::note_on` calls `osc.reset()` on a fresh slot
  (`crates/mooloop-dsp/src/polysynth.rs:228`), so every note in a chord starts
  every oscillator at phase 0. Three saws at phase 0 across five voices means
  the attack transients sum coherently — a hard, artificial edge — and the
  detune beating starts from a fixed relationship rather than a natural one.
- Every voice reads the same `params.filter_cutoff`, the same envelope times,
  and the same pitch ratios. There is no analog-poly variation because there
  is no variation at all.

## Do this

### 1. Deterministic per-slot seeds

The requirement is repeatability: the same project, patch, note data, and
render settings produce the same audio, offline or live. So the offsets are a
**pure function of the voice slot index**, computed once at construction and
stored on the voice — not drawn from a runtime RNG, not reseeded on note-on,
not derived from time or allocation order.

A small fixed table of pseudo-random constants for 16 slots, or a cheap
integer hash of the slot index, both satisfy this. Prefer whichever reads more
obviously deterministic to someone auditing it later; a literal table is hard
to get wrong.

Each slot needs, as normalized bipolar values in `[-1, 1]`:

- one pitch offset per oscillator (3 values)
- one filter cutoff offset
- one envelope-time offset
- one starting phase per oscillator (3 values, in `[0, 1)`)

Store them as fixed fields on `PolyVoice`, computed in `PolyVoice::new` from
the slot index. Note `PolyVoice::new` currently takes only `sample_rate` and
the voices are built with `std::array::from_fn(|_| ...)`
(`polysynth.rs:108`) — pass the index in.

### 2. Scale by the Drift knob

At `drift = 1.0`, applied to the base offsets:

| Target           | Maximum deviation                                       |
|------------------|---------------------------------------------------------|
| Oscillator pitch | ≈ ±5 cents per voice, with smaller independent per-osc offsets on top |
| Filter cutoff    | ≈ ±0.15 octave                                          |
| Envelope times   | ≈ ±7% on attack, decay, and release — **not sustain**   |
| Oscillator phase | Non-identical start phase                               |

These are starting points for listening tests, not constants to defend. Record
what they end up as, here.

Two implementation notes that are not negotiable:

- **Sustain is a level, not a time.** Varying it would make voices in a
  sustained chord sit at different volumes, which reads as a bug. Vary
  attack/decay/release only.
- Cutoff drift is an octave offset added to the same `octaves` expression that
  the filter envelope, LFO, and keytrack already feed — not a separate filter
  coefficient path.

Envelope-time drift means `Adsr::configure` gets per-voice values, so
`apply_params_to_voices` (`polysynth.rs:147`) computes them per slot rather
than passing `self.params.attack` to all sixteen.

### 3. Phase

Replace the unconditional `osc.reset()` for a fresh slot with a seek to the
slot's stored start phase. `Osc` (`crates/mooloop-dsp/src/osc.rs`) needs a
`reset_to_phase(phase)` alongside `reset()`; `reset()` stays for the mono
synth and the drum synth.

At `drift = 0`, phase offsets go to zero and behaviour is exactly today's —
which keeps the change null-testable and keeps the door open for an explicit
phase-reset mode later.

### 4. Parameter

| Field   | Range | Default | ID |
|---------|-------|---------|----|
| `drift` | 0-1   | 0.1     | 40 |

Default 0.1 rather than 0: a small amount of variation is what the instrument
should *be*, and 10% is below the threshold where anyone would call it
detuned. An old project loading at 0.1 will sound very slightly different from
before; that is inside the spec's "exact pre-v2 timbre is secondary" allowance
and it is the right trade. If a listening test disagrees, default to 0 and say
so here.

### 5. UI

The VOICE page gets its real structure now (see 01): an **Allocation** section
holding Polyphony and, later, Unison and Detune; a **Character** section
holding Drift, Spread, and later the Chorus mode. Drift is a knob in Character
beside Spread, which already exists.

## Done when

- Rendering the same chord twice in the same process gives bit-identical
  output; so does rendering it in a fresh process. Assert both.
- A five-note chord with drift at 100% differs measurably from the same chord
  at 0% — not just in phase, but in the beating over a two-second sustain.
- Fresh voices in a chord no longer all start at phase 0. Assert directly on
  oscillator phase after a simultaneous three-note NoteOn.
- Voice slot 3 always gets slot 3's offsets, whichever note lands on it.
- Sustained chord voices reach the same sustain level; only their times differ.
- `drift = 0` is bit-identical to the pre-drift build.
- Existing tests pass, in particular the polyphony, stealing, and spread tests.
