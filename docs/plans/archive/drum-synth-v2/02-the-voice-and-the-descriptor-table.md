# The voice and the descriptor table

This step creates DS-01 as a playable, addressable device: a new generator
kind, the full parameter id map, the tone and noise layers, a working
amplitude and pitch envelope, and the voice pool. It deliberately lands
descriptor addressing first, because that is the reason the instrument exists.

At the end of this step DS-01 makes kicks, snares, and hats — roughly v1's
range — and every one of its continuous parameters automates and modulates.
The layers that extend the range come in `04` and `05`.

## The new kind

Add alongside the existing kinds, not in place of them:

- `DeviceKind::Ds01` and `GeneratorParams::Ds01`;
- `Ds01Params` in `crates/mooloop-core/src/ds01.rs`, with
  `#[serde(rename_all = "snake_case")]` and an explicit `#[serde(rename =
  "ds01")]` tag on the variant;
- `DeviceKind::Ds01 => &crate::ds01::DESCRIPTORS` in `descriptors()`;
- `Ds01` in `crates/mooloop-dsp/src/ds01.rs` implementing `AudioNode`;
- a `SetChannelDs01Params` bridge command;
- an entry in the source picker.

`DrumSynth` is untouched. Its `descriptors()` arm stays `&[]`, and the comment
there is amended to point at this plan instead of claiming the work is
mechanical.

## Parameter id map

Ids start at zero in DS-01's own namespace. The bands below are reserved for
the whole plan; a step may only assign inside its own band. `*` marks a
structural discrete that is ineligible for modulation by default.

| Band | Section | Step |
| --- | --- | --- |
| 0-9 | Global | 02 |
| 10-19 | Tone | 02 |
| 20-29 | Noise | 02 |
| 30-39 | Body | 04 |
| 40-49 | Amp envelope | 02, extended in 03 |
| 50-59 | Pitch envelope | 02, extended in 03 |
| 60-69 | Noise envelope | 03 |
| 70-79 | Mod envelope | 03 |
| 80-89 | Burst | 05 |
| 90-99 | Shape and output | 06 |
| 100-131 | Matrix, eight rows of four | 07 |

This step assigns:

```text
GLOBAL
  0  Tune            semitones   -48 .. +48, step 1        default 0
  1  Level           0 .. 1 unit                           default 0.8
  2  Choke Group *   stepped 0 .. MAX_CHOKE_GROUP          default 0
  3  Choke Time      s, log, 0.001 .. 0.5                  default 0.005
  4  Retrigger *     stepped Poly | Mono                   default Poly
  5  Velocity Amount 0 .. 1 unit                           default 1.0

TONE
 10  Tone Level      0 .. 1 unit                           default 1.0
 11  Tone Pitch      Hz, log, 20 .. 8000                   default 160
 12  Tone Wave       0 .. 1 morph sine>tri>saw>pulse       default 0.0
 13  Tone Partials * stepped 1 .. 6                        default 1
 14  Tone Spread     0 .. 1 unit                           default 0.5
 15  Tone FM Amount  0 .. 1 unit                           default 0.0
 16  Tone FM Ratio   0.25 .. 16, log                       default 2.0

NOISE
 20  Noise Level     0 .. 1 unit                           default 0.0
 21  Noise Color *   stepped White | Pink | Velvet | Metal default White
 22  Noise Rate      Hz, log, 500 .. 48000                 default 48000
 23  Filter Morph    0 .. 1 morph LP > BP > HP             default 1.0
 24  Filter Cutoff   Hz, log, 20 .. 18000                  default 7500
 25  Filter Res      0 .. 1 unit                           default 0.1

AMP ENVELOPE (step 03 adds 40, 41, 43, 44, 45, 46)
 42  Amp Decay       s, log, 0.002 .. 4                    default 0.24

PITCH ENVELOPE (step 03 adds 50, 52)
 51  Pitch Decay     s, log, 0.001 .. 2                    default 0.045
 53  Pitch Depth     semitones, bipolar, -60 .. +60        default +21
```

The pitch envelope is **bipolar and in semitones**, which is the one place
DS-01 refuses to copy v1. v1 spells the kick sweep as a start frequency and an
end frequency, which is why its ranges could not be shared and why the sweep
could not track the note. A depth in semitones around the tone pitch tracks
correctly, modulates meaningfully, and expresses an upward blip with a
negative number. The default of +21 semitones over 45 ms from 160 Hz is
approximately v1's default kick.

Assert at construction that ids are unique and that every id in the table has
a descriptor.

## Tone

One oscillator per voice with a **continuous wave morph** rather than a
four-way selector. Morph beats a selector here for a reason that is not
cosmetic: a selector is a structural discrete and would be modulation-
ineligible, and sweeping timbre across a hit is a percussion gesture. Use the
existing band-limited `Osc` shapes and crossfade between adjacent pairs across
the 0-1 range. Anti-aliasing must hold at the top of the pitch range with the
pitch envelope at full positive depth, which is where the sweep starts near
Nyquist.

**Partials** turns the single oscillator into a bank of up to six, at fixed
inharmonic ratios spread by **Spread**. At `Partials = 1` the bank is one
oscillator and Spread is inert, which is the exception `01` names. At 6, with
a pulse morph, this is the 808/606 metal hat that v1 hardcodes as two squares
at 587.33 Hz and 845.07 Hz; DS-01 reaches those as a patch instead of as
constants. Skip the loop entirely for unused partials.

**FM** is the tone oscillator modulated by a second sine at Ratio. It is the
cheapest route to bells, clangs, and damaged kicks, and it costs two
parameters. It is not a full operator matrix and will not become one.

## Noise

- **Color** is a stepped source selection: White, Pink, Velvet (sparse
  impulses — crackle, vinyl, brush), Metal (the ring-modulated square cluster
  that gives cymbal grit). It is structural because the generators differ;
  changing it between hits is fine and changing it mid-hit is not defined.
- **Rate** is a sample-rate reducer applied to the noise regardless of color,
  so it is never inert. It is also the control that makes noise sound digital
  and cheap, which is a sound this instrument should have.
- The filter replaces v1's `OnePoleHp` with the shared state-variable filter,
  with a **morph** across low-pass, band-pass, and high-pass rather than a
  mode selector, for the same reason the wave is a morph. Resonance is
  bounded and self-oscillation is not a feature at this stage.

Noise defaults to `Level = 0` so the default patch is a clean tone hit and the
first thing a user does is add something.

## Voices, choke, retrigger

Keep v1's structure: a fixed pool of `MAX_DRUM_VOICES`, free-voice-first, then
oldest-age stealing. Keep the reset-on-steal behaviour.

- **Choke Time** replaces the `CHOKE_DECAY_S` constant. The fade is applied to
  the amplitude envelope as a release, not as a separate coefficient stamped
  over the envelope, so step 03's envelope shapes do not have to special-case
  it.
- **Retrigger = Mono** chokes this channel's sounding voices at Choke Time
  before allocating the new one. `Poly` is v1's behaviour and stays the
  default so nothing about the feel of a fast pattern changes by accident.
- Transport stop still chokes everything.

## The two contracts this step must get right

### Latched versus continuous

Implement the tables in `01-what-ds01-is.md` explicitly. Concretely:

- `trigger()` takes a snapshot of the latched values into the voice. Nothing
  else in the voice reads latched parameters afterward.
- The render loop reads continuous parameters from a per-control-tick view,
  not once per block and not once per hit. Smooth the ones that would click:
  levels, cutoff, resonance, morphs, and the output level. A drum tail is
  short enough that a slow smoother is audible as a swell, so keep the
  smoothing time small and constant.
- Add a test that proves each half: modulating Amp Decay changes the *next*
  hit and not the current one, and modulating Filter Cutoff changes the
  current one.

### Parameter events precede note-ons at the same offset

The renderer must order parameter events before note-ons within an offset, and
DS-01's `process()` must apply them in the order given. Verify where this is
enforced — it is a property of how the modulation tick and the note stream are
merged, and it may already hold by accident. If it holds by accident, make it
hold on purpose and comment it, because a later reordering would break drum
modulation silently and no synth test would notice.

Test: one block containing a parameter event setting Pitch Depth and a note-on
at the same offset produces a hit at the new depth.

## Acceptance

- DS-01 appears in the source picker, plays from the piano roll and the step
  grid, and saves and reloads.
- `DeviceKind::Ds01.descriptors()` is non-empty, ids are unique, and every
  continuous parameter round-trips through `ParamAddr` get/set.
- A channel LFO assigned to Filter Cutoff audibly sweeps a hat pattern. **This
  is the acceptance case the whole plan exists for; do not defer it.**
- An automation lane on Tone Pitch renders identically offline and live.
- The default patch is a kick within a dB of the device reference at full
  velocity, matching v1's calibration.
- v1 `DrumSynth` projects load and sound unchanged.
