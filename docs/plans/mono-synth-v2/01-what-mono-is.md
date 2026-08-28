# What Mono is

This is the reference document for the rest of this plan. It records the
decisions; the numbered steps implement them. Read this first even when
picking up a later step, and update it here — not in a step — if a decision
has to change.

## Why

Mono and Poly are currently the same instrument twice. `MonoSynth`
(`crates/mooloop-dsp/src/monosynth.rs`) and `PolySynth`
(`crates/mooloop-dsp/src/polysynth.rs`) run the same voice: three `Osc` into a
mix, one `Svf` low-pass, `apply_drive` after the filter, one `Adsr` doing both
amplitude and filter duty. `MonoSynthParams` and `PolySynthParams`
(`crates/mooloop-core/src/synth.rs:345` and `:415`) are field-for-field
identical apart from `polyphony` and `spread`. The two `.slint` faces are the
same file with one extra page.

Poly is Mono times N. That is why neither has a reason to exist.

## The decision

**Mono is a filter and performance instrument.** One voice, behaving
intensely. Its identity lives in three places and nowhere else:

1. **The filter is the instrument.** Two character models — LADDER and ACID —
   with saturation ahead of the filter, not after it. Turning the cutoff knob
   is the primary musical act.
2. **Note transitions are performance.** A real held-note stack with
   priority, legato/retrigger envelope modes, and glide that knows whether
   notes overlapped.
3. **Velocity is accent**, not expression: it pushes filter envelope
   intensity and pre-filter drive, not just gain.

Mono stays mono, stays dry, and stays small. It reaches Moog-ish weight and
303/101-adjacent sequence behaviour quickly. It is not an emulation of any of
those and no UI text or docs copy may claim it is.

## What Mono does not get

Fixed for v2, not open for reinterpretation inside a step:

- No chorus, reverb, or delay inside the device. Mono is dry; the rack has
  effects.
- No LP/BP/HP mode menu. The shared `Svf` can produce all three
  (`Svf::next_sample_lp_bp_hp`, `crates/mooloop-dsp/src/filter.rs:69`) and
  that is exactly why it is tempting. A mode menu turns Mono into the generic
  multimode synth that Poly is supposed to be. Model choice changes
  *character*, not response shape.
- No device-local modulation matrix or LFO. General LFOs and other control
  sources belong to the channel rack; Mono declares destinations and may later
  publish named outlets. Voice-local envelopes, keytracking, and Accent remain
  part of Mono because they are its synthesis behavior, not general sources.
- No unison stack. That is Poly's.
- No oscillator sync or cross-mod. Also Poly's.
- Noise or a sub oscillator is *optional*; if it lands, it is one global Noise
  level, not a fourth wave on every oscillator.

## The signal path

```text
OSC 1 --\
OSC 2 ----> MIX -> PRE-DRIVE -> FILTER -> VCA -> MONO OUT
OSC 3 --/                          ^         ^
                                   |         |
                              FILTER ADSR  AMP ADSR

CHANNEL MODULATION ROUTES / KEYTRACK / ACCENT -> FILTER
```

The move of saturation from after the filter to before it is the defining
change. Today `apply_drive(filtered, drive)` runs on the filter's output
(`crates/mooloop-dsp/src/monosynth.rs:284`); in v2 the oscillator mix level
drives the filter's input, so raising an oscillator's level changes the
filter's character and not merely the gain.

## Control surface

Three source pages, as today. The MOD page becomes PERF rather than growing a
fourth page; the common device frame exposes the channel modulation shelf.

| Page       | Section         | Controls                                              |
|------------|-----------------|-------------------------------------------------------|
| OSC        | OSC 1 / 2 / 3   | Wave, Semi, Fine, Level, Width (unchanged)            |
| AMP/FILTER | Amplitude       | Amp ADSR                                              |
| AMP/FILTER | Filter          | Model, Cutoff, Resonance, Env Amount, Keytrack, Drive |
| AMP/FILTER | Filter Envelope | Filter ADSR                                           |
| PERF       | Performance     | Glide, Glide Mode, Env Trigger, Priority, Accent      |

The AMP/FILTER page is already two full panels at 5 knobs and 4 knobs
(`crates/mooloop-ui/ui/mono-device.slint:129`). It has to hold a second
envelope, a model selector, and two more knobs. Step 02 makes that layout
call once, and later steps add controls into the shape it establishes rather
than re-deciding it.

Per the project tooltip convention, every new control's tooltip carries the
value only; explanatory text goes to the status bar.

## Parameter and serialization rules

These apply to every step and are not restated in each one.

- **`MonoSynthParams` is the fragile one.** `PolySynthParams` carries
  `#[serde(default)]` at the struct level
  (`crates/mooloop-core/src/synth.rs:414`); `MonoSynthParams` does not. Any
  field added without fixing that makes every pre-v2 project unreadable. **Step 02 adds
  `#[serde(default)]` to the struct before adding any field**, and
  `crates/mooloop-project/src/lib.rs:1763` already has the round-trip test
  shape to extend.
- **The present device-local LFO is legacy state.** The migration that removes
  it must create an equivalent channel LFO source and routes from old patches
  before deleting the source field or its descriptor IDs. A loaded old patch
  must keep its intended motion; a new Mono patch gets modulation from the
  channel shelf, not from a hidden Mono-owned LFO.
- **Never renumber an existing parameter ID.** The IDs in
  `crates/mooloop-core/src/generator.rs:166-183` are automation lane
  addresses. `SYNTH_PARAM_POLYPHONY = 15` and `SPREAD = 16` are Poly-only, so
  Mono's new IDs start at **20** and leave 15-19 alone; the oscillator block
  starting at 100 (`synth_osc_param`) is unaffected.
- **The descriptor tables stop being a superset.** `POLY_DESCRIPTORS` is
  currently built by copying `MONO_DESCRIPTORS` and appending two entries
  (`crates/mooloop-core/src/generator.rs:295`). Once Mono has Accent and Poly
  has Unison, that relationship is false. Step 02 splits them into two
  independent const tables sharing only `osc_descriptors` and genuinely
  shared source-parameter entries. Channel modulator descriptors do not belong
  to either synth table.
- **Old projects must open and play.** Exact pre-v2 timbre is desirable and
  secondary; broken deserialization or a dead automation lane is not
  acceptable. Adam has not waived compatibility here the way he did for gain.
- New continuous parameters that scale the signal go through `Smoothed`
  (`PARAM_SMOOTH_S`, 5 ms). New enum/bool fields do not need smoothing but do
  need a click-safe switch — see 04 for the filter-model case.

## Real-time rules

Unchanged from the rest of the engine, restated because several steps add
state:

- No allocation in `process()`, note handling, filter switching, or the
  held-note stack. The stack is a fixed-size array, not a `Vec`.
- Event IDs remain the identity of a note. A stale `NoteOff` must not release
  a newer note — `stale_note_off_does_not_release_a_retriggered_voice`
  (`crates/mooloop-dsp/src/monosynth.rs:401`) is the existing guard and the
  held-note stack must not weaken it.
- Extreme resonance × drive stays finite and bounded;
  `resonant_filter_and_drive_stay_bounded` covers this today and must be
  re-run against both new filter models.
- Sample-accurate event offsets survive. Every step keeps the
  render-split-at-offset loop in `AudioNode::process`.

## The end state

Load Mono and Poly with the same saw. Within a few knob moves Mono is a bass,
an acid line, or a legato lead, and Poly is a chord. If a change to Mono makes
it better at chords, it is the wrong change.
