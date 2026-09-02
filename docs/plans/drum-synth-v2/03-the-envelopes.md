# The envelopes

v1 has one envelope shape: `ExpDecay`, a single time constant, no attack, no
curve, and no way to hold. Every drum it makes therefore starts at full
amplitude on sample zero and falls at the same rate law. That is why v1's
snare and its hat differ mostly in their noise content, and it is the largest
single reason its sounds are all recognisably the same instrument.

This step gives DS-01 four real envelopes and makes their shape a control.

## The shape

One envelope type, used four times: **AHD with curve, and optional gate**.

```text
          ┌──── hold ────┐
         /│              │\
        / │              │ \
       /  │              │  \___ decay (curve)
      /   │              │      \____
     /    │              │           \_____
    attack                                  \______

    gate mode on:  ... decay falls to Sustain, then Release at note-off
```

| Segment | Range | Notes |
| --- | --- | --- |
| Attack | 0 - 500 ms, log, default 0 | 0 means sample-zero onset, exactly as v1 |
| Hold | 0 - 500 ms, log, default 0 | Flat at peak. The 909 clap tail and the gated snare |
| Decay | 2 ms - 8 s, log | The main control |
| Curve | -1 .. +1, bipolar, default 0 | -1 logarithmic, 0 exponential (v1's law), +1 linear |
| Sustain | 0 - 1, default 0 | Only meaningful with Gate on |
| Release | 2 ms - 4 s, log | Only meaningful with Gate on |
| Gate * | Off (one-shot) / On | Structural, per envelope |

**Curve is the control that earns this step.** An exponential decay is a kick
and a hat; a near-linear decay is a synthetic tom and a reverse-swell; a
logarithmic decay is a long tail that stays audible and then stops. Implement
it as a shaping of the normalized envelope output rather than as three
different integrators, so it is continuous across zero and so latching one
value per hit is cheap.

Attack must not cost the transient. At `Attack = 0` the envelope's first
sample is the peak, with no one-sample ramp and no smoothing. A drum synth
whose attack cannot be zero is broken, and this is worth an explicit test.

## Gate mode

Default off, matching v1: note-offs end nothing and a hit runs to silence.

With Gate on, the envelope holds at Sustain while the note is held and
releases at note-off. This is how DS-01 gets a ride that rings for as long as
it is written, a held shaker, and a sustained noise wash — sounds v1 cannot
make at all.

Consequences to handle rather than discover:

- A voice with any gated envelope must not be reclaimed by age-based stealing
  while its note is held, unless the pool is genuinely exhausted.
- `Event::NoteOff` stops being universally ignored. Route it to gated
  envelopes only; a one-shot envelope in the same voice keeps ignoring it.
- Transport stop and choke still end everything, at Choke Time.

## The four

| Envelope | Ids | Gate | Destination |
| --- | --- | --- | --- |
| Amp | 40-46 | yes | The VCA, always |
| Pitch | 50-53 | no | Tone pitch, bipolar depth in semitones. Attack lets a pitch *rise* into the hit |
| Noise | 60-66 | yes | The noise layer's own level |
| Mod | 70-76 | yes | Nothing by default. A matrix source only |

The Mod envelope is what makes "more envelope control" mean something beyond
more knobs: a second contour with no fixed job, routable in step 07 to filter
cutoff, body damping, drive, burst spacing, or tone morph. It is the
difference between a hit with one shape and a hit with layers that move
against each other.

Envelopes are per voice and evaluated per sample. All of their times and
curves are latched at trigger per `01`.

**Their times are edited on the face by dragging the curve, not by turning a
knob** — see `08-the-face.md`. That is a layout decision, but it constrains
this step in one way worth stating here: attack, hold and decay must be
expressible as points on a plotted contour, and the plot's time axis is shared
across all four envelopes. A segment that cannot be drawn as a handle on that
axis does not belong in this envelope.

## Ids

```text
AMP     40 Attack  41 Hold  42 Decay  43 Curve  44 Sustain  45 Release  46 Gate *
PITCH   50 Attack            51 Decay  52 Curve  53 Depth
NOISE   60 Attack  61 Hold  62 Decay  63 Curve  64 Sustain  65 Release  66 Gate *
MOD     70 Attack  71 Hold  72 Decay  73 Curve  74 Sustain  75 Release  76 Gate *
```

42, 51 and 53 already exist from step 02 and keep their ids.

## Acceptance

- `Attack = 0` produces a first sample at full level; a test asserts the
  transient is not softened relative to v1.
- Curve at -1, 0, and +1 produce measurably different decay shapes at the same
  Decay time, and 0 matches v1's `ExpDecay` within tolerance.
- Hold produces a flat top of the stated length.
- A gated Amp envelope rings for the length of a held note and releases; a
  one-shot envelope in the same patch still ignores note-off.
- A gated voice is not stolen while held unless the pool is full.
- Every envelope terminates. No combination of attack, hold, decay, curve and
  gate leaves a voice active forever, and the existing "every mode makes sound
  and terminates" test grows into a sweep over envelope shapes.
