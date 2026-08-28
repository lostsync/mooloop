# Poly factory patches

The listening pass, and where several deliberately-deferred decisions get
made.

## Why this is a step and not a checklist item

Four things in this plan are explicitly "tune by ear" and one is "decide by
listening":

- Drift's maximum deviations (02)
- LP24's resonance placement and cutoff compensation (03)
- Detune's cent range (04)
- Chorus modes I and II's rate/depth/color/spread, and whether Amount is
  needed at all (05)
- **Where Drive goes** — post-filter as today, or a mild pre-filter stage (03)

Doing that tuning against six concrete patches is what settles them. Expect to
change 02-05 during this step and to record the results there.

## The bank

| Patch        | What it proves                                          |
|--------------|---------------------------------------------------------|
| Warm Saw     | Basic analog-poly body — the default a user starts from |
| Brass        | Independent filter envelope plus velocity expression    |
| PWM Pad      | PWM, drift, and stereo working together                 |
| Wide Strings | Voice spread plus chorus/ensemble                       |
| Sync Stab    | Oscillator hard sync and filter punch (needs 06)        |
| Unison Lead  | Real voice-stack detune and spread                      |

Each has to be reachable quickly from the default saw. If a patch needs
fifteen precise settings, the defaults or the ranges are wrong and that is the
finding, not the patch's fault.

If step 06 was skipped, Sync Stab is deferred with it — note that here rather
than substituting a different patch.

## The Expression section

Poly's control surface (01) lists a MOD page **Expression** section:
Velocity → Filter, and Velocity → Amp amount "if needed". Nothing earlier in
this plan implements it, because whether it is needed is a listening question
and Brass is the patch that answers it.

Velocity currently scales amplitude only, through `velocity_amp`
(`polysynth.rs:234`). If Brass needs velocity to open the filter to be
playable — and brass patches usually do — add Velocity → Filter as a 0-1 depth
at **ID 46** and put it on the MOD page. This is Poly's counterpart to Mono's
Accent and it is deliberately the milder, expression-shaped version: no drive
push, no per-note character change, just filter depth.

If Brass plays well without it, do not add it. Record which, and why, here.

## Checks against the whole instrument

- Every patch at maximum velocity, full polyphony, and 8× unison stays within
  the peak bound. This is the case where sixteen voices sum, and it is the
  most likely place for the instrument to clip. Cross-reference
  `docs/plans/gain-structure/` — honest summing is the intended behaviour, so
  the answer may be "the default level is too high", not "add a limiter".
- Determinism end to end: render the full bank offline twice and diff.
- Transport stop during each patch releases cleanly, including the chorus tail.
- Automating cutoff, resonance, drift, detune, and spread across their full
  range mid-chord produces no clicks.
- A pre-v2 project loads alongside the new bank and still sounds close to what
  it did.

## Done when

- All six patches (five if 06 was skipped) exist, load, and reach their
  intended territory.
- Warm Saw and Wide Strings are unmistakably the same instrument; Warm Saw and
  a Mono bass are unmistakably not.
- Every deferred decision listed above has an answer written into its step.
- **The definition of done holds:** Mono and Poly loaded with the same saw
  lead to two clearly different workflows within a few knob moves. Poly
  invites voicing, drift, chords, unison, spread, and chorus.
