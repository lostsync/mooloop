# Split the filter envelope

**This step lands on both synths.** It is the shared v2 foundation; the Poly
plan depends on it and does not repeat it. Do it here, once.

## What is wrong

Both synths own one `Adsr` per voice and use it for two jobs. In
`MonoSynth::render_range` the same `voice.env.level()` scales the VCA and
sweeps the filter:

```rust
let octaves = voice.env.level() * env_amount * 6.0 + lfo_value * to_filter;
// ...
let sample = apply_drive(filtered, drive) * voice.env.level() * velocity * tremolo;
```

(`crates/mooloop-dsp/src/monosynth.rs:276` and `:284`; `polysynth.rs:347` and
`:355` are the same two lines.)

That snippet records the current device-local LFO path, not the v2 target.
The channel-owned replacement is specified in section 4 below.

A pluck is a fast filter decay under a sustained amplitude. A pad is a slow
filter opening under a fast attack. Neither is reachable. This is the single
biggest limitation in the current architecture and everything else in both
plans is voiced against it.

There is also no keyboard tracking, so a patch voiced at C2 is a dull thud at
C5.

## Do this

### 1. Make `MonoSynthParams` safe to extend first

Before adding any field, put `#[serde(default)]` on the `MonoSynthParams`
struct (`crates/mooloop-core/src/synth.rs:345`), matching `PolySynthParams`.
Extend the round-trip test at `crates/mooloop-project/src/lib.rs:1763` to
assert that a manifest containing *only* the pre-v2 fields still loads to
something equal to `MonoSynthParams::default()` in the new fields.

### 2. Add the parameters

Nine new fields, on both structs:

| Field               | Range          | Default | Notes                                    |
|---------------------|----------------|---------|------------------------------------------|
| `filter_attack`     | 0-2 s          | 0.005   | Same mapping as `attack`                 |
| `filter_decay`      | 0-2 s          | 0.2     | The one that makes plucks and acid work  |
| `filter_sustain`    | 0-1            | 0.7     | Independent of `sustain`                 |
| `filter_release`    | 0-2 s          | 0.15    | Independent of `release`                 |
| `filter_keytrack`   | 0-1            | 0.0     | 1.0 ≈ one octave of cutoff per octave    |

`filter_env_amount` already exists and stays bipolar with its existing ID and
its existing six-octave depth mapping.

New IDs, appended without touching 0-16:

```
SYNTH_PARAM_FILTER_ATTACK   = 20
SYNTH_PARAM_FILTER_DECAY    = 21
SYNTH_PARAM_FILTER_SUSTAIN  = 22
SYNTH_PARAM_FILTER_RELEASE  = 23
SYNTH_PARAM_FILTER_KEYTRACK = 24
```

Defaults above are chosen so that a **fresh** patch is musical. Migration of
an **old** patch is different — see 4.

### 3. Split the descriptor tables

`POLY_DESCRIPTORS` currently copies `MONO_DESCRIPTORS` wholesale
(`crates/mooloop-core/src/generator.rs:295`). Break that now, while the two
tables are still nearly identical and the change is mechanical: keep
`osc_descriptors()` and a shared const block for the ADSR/filter entries both
devices genuinely share, and build `MONO_DESCRIPTORS` and `POLY_DESCRIPTORS`
independently from it. LFO descriptors belong to the channel `ModRack`, not a
synth table. Later steps add Mono-only Accent and Poly-only Unison entries into
tables that are already separate.

Both tables stay `static` and const-constructed — the engine must not allocate
to enumerate them.

### 4. Two envelopes per voice

`MonoVoice` and `PolyVoice` each grow a second `Adsr`:

```rust
env: Adsr,         // rename to amp_env
filter_env: Adsr,  // new
```

`configure` both wherever the current one is configured — `MonoSynth::new`,
`set_params`, `reset` and `PolySynth::apply_params_to_voices`. In the render
loop, `filter_env` advances alongside `amp_env`, feeds the `octaves`
expression, and `amp_env` alone scales the output. **Voice-idle detection
stays keyed on `amp_env`**: a filter envelope still in a long release must not
hold a silent voice alive.

Keytrack adds to the same `octaves` expression, referenced to middle C:

```rust
let keytrack_oct = keytrack * (note as f32 - 60.0) / 12.0;
let octaves = filter_env.level() * env_amount * 6.0 + keytrack_oct;
```

Channel modulation routes change the addressable filter parameters through the
ordinary descriptor event path; they do not add a second device-local LFO term
inside every voice. This keeps the LFO channel-owned while leaving the
per-voice envelope and keytrack calculation where it belongs.

`MonoVoice` does not currently store the note number (only `current_freq`) —
add it, or derive the offset from `current_freq` against middle C so glide
carries the tracking with it. Prefer deriving from `current_freq`: it makes a
glide sweep the cutoff along with the pitch, which is the musically expected
result and is free.

The existing filter-bypass fast path
(`cutoff >= 0.999 && env_amount == 0 && resonance == 0`) must also require
`keytrack == 0`. Channel routes have already resolved Cutoff before the source
receives its parameter event, so the path must not retain a separate
device-local LFO-depth exception.

### 5. Migrate old patches

An old patch has one ADSR doing both jobs. Loading it into the new struct with
the *fresh-patch* defaults above changes its filter motion. Per the spec,
initialize the filter ADSR from the old shared ADSR so an old patch starts
close to where it was.

`#[serde(default)]` alone cannot do this — the defaults are constants and
cannot see the sibling fields. Add an explicit post-deserialize fixup in
`crates/mooloop-project/src/lib.rs`: when a loaded manifest carried no
`filter_attack` key, copy `attack`/`decay`/`sustain`/`release` into the filter
envelope and leave `filter_keytrack` at 0. Detecting absence needs the fields
to deserialize as `Option<f32>` in a versioned intermediate, or a sentinel —
choose whichever fits the existing loader; do not guess by comparing against
default values, since a user could legitimately have those.

Note this only matters for patches with a non-zero `filter_env_amount`. Its
default is 0.0, so the majority of existing patches are unaffected either way.

### 6. UI

The AMP/FILTER page has to hold a second envelope editor, and this is where
the layout for the whole plan gets decided (see 01). Both faces
(`mono-device.slint:129`, `poly-device.slint`) currently split the page into a
5-knob AMPLITUDE panel and a 4-knob FILTER / DRIVE panel, each with a display
above the knobs.

Give the filter panel its own `EnvelopeEditor` and its own knob row. Mono's
filter panel will later need Model and Poly's will need Mode, so leave a slot
  for a selector in the panel header rather than assuming four knobs is the
  maximum. Glide currently sits in the AMPLITUDE knob row on both faces; on Mono
  it moves to PERF in step 03, so leaving it where it is for now is fine.

Verify with the software-rendered UI snapshot for both device faces per
`docs/AGENT_OPERATIONS.md`.

## Done when

- A pre-v2 project TOML loads without error and plays. Test asserts this for a
  manifest written before this step.
- Amp sustain 100% with filter decay 100 ms and env amount +50% gives an
  audible pluck under a flat amplitude — a shape that is not expressible
  today. Assert it as a spectral-centroid or filtered-energy drop over the
  first 200 ms while the amplitude envelope is still at sustain.
- Filter release is independent: a long amp release with a short filter
  release darkens the tail; the voice still goes idle on the amp envelope.
- Keytrack at 100% moves cutoff about one octave per played octave; at 0% the
  cutoff is note-independent. Assert with two notes an octave apart.
- Existing tests pass unchanged on both synths, in particular
  `resonant_filter_and_drive_stay_bounded` and the no-step smoothing tests.
