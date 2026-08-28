# Accent

## The decision

**Do not add a new event type.** Velocity is the carrier. `Event::NoteOn`
already has a `velocity` field, the sequencer already writes it, and the
project format already stores it. Adding an `Event::Accent` would mean a new
variant in `Event`, new handling in every generator that matches on it, a new
lane in the sequencer, and a migration — for information velocity already
carries.

One Accent knob controls *how much* velocity pushes the classic mono
behaviours. The synth never learns where a velocity value came from. A later
Accent lane or accent button in the sequencer is then purely a sequencer
feature: it writes an agreed velocity, and Mono responds because it already
does.

## Do this

### 1. The mapping

Velocity keeps doing what it does today — scaling amplitude through
`voice.velocity_amp` (`crates/mooloop-dsp/src/monosynth.rs:153`). That is
unchanged and independent of Accent.

Accent adds two velocity-dependent pushes, both zero when Accent is zero:

| Target                 | At Accent 100%, full velocity                       |
|------------------------|------------------------------------------------------|
| Filter envelope amount | Scales `filter_env_amount` up, adding roughly a further two octaves of sweep on top of the knob's six |
| Pre-filter drive       | Adds a bounded amount to `drive` — enough to hear, not enough to need a trim |

Both scale with `velocity / 127`, so a low-velocity note in an accented patch
is still a soft note. Accent is a depth control on velocity, not a switch.

Concrete numbers are a starting point for listening tests. What is fixed:

- **The mapping is bounded.** At Accent 100% with velocity 127 the output must
  not need the channel fader pulled down. This is the "not a gain-staging
  trap" requirement and it is a test, not a preference.
- **Accent 0% is exactly today's behaviour** for any patch, bit-identical.
  That makes the default safe and the migration free.
- Accent's drive contribution is added to the *smoothed* drive value, not
  applied as a separate stage, so it inherits click-safety for free.

### 2. Per-note capture

Accent depth is sampled at NoteOn and held for the note. It must not follow a
mid-note velocity change, because there isn't one — but it must survive the
step 03 fallback: retargeting to a held note on NoteOff keeps the *winning
note's* accent, which means the stack entry stores velocity too. Add
`velocity: u8` to `HeldNote`.

In `Legato` env trigger, an overlapping NoteOn does not restart envelopes but
*does* update accent, since it's a new note taking the voice. Amplitude
velocity already slides in this case via `velocity_amp`; accent should slide
with it rather than step.

### 3. Parameter

| Field    | Range | Default | ID |
|----------|-------|---------|----|
| `accent` | 0-1   | 0.0     | 29 |

Mono only. `unit(SYNTH_PARAM_ACCENT, "Accent", 0.0)` in `MONO_DESCRIPTORS`.

### 4. UI

One knob in the Performance section of PERF, beside Glide / Glide Mode /
Env Trigger / Priority from step 03. `Theme.warning` fill, matching Drive and
the other character controls. Tooltip is the value only.

## Done when

- Accent 0% is bit-identical to the pre-Accent build across the existing test
  suite. Assert with a rendered null test against a fixed patch.
- At Accent 100%, velocity 127 vs velocity 40 on the same note produces an
  audibly larger filter sweep and more drive, not merely more level.
- Bounded: Accent 100%, velocity 127, maximum resonance, maximum drive, and
  three oscillators at full level stays finite and does not exceed the
  peak bound the existing `resonant_filter_and_drive_stay_bounded` test uses.
- Note priority fallback keeps the winning note's accent, not the released
  note's.
- No step in the output when an accented note lands over a sounding voice in
  `Legato` env trigger mode.
