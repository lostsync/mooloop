# Extract mid-level DSP blocks: status

## 2026-10-01: step 01 answered, step 02 cancelled (MOO-149, MOO-150)

The delay-layer question is answered, and the answer is no: there is **no
"modulated tap" block**. `DelayEffect` and `ModulationEffect` agree on the
nouns and differ on every policy between them:

- what goes back into the line: one damped tap, or a mix of two or three
  undamped taps;
- where the damping sits: inside the loop, or on the wet output;
- how the loop is bounded: a `tanh` knee (MOO-124), or a linear trim on the
  output whose doc rejects the knee (MOO-200);
- the NaN rule: MOO-174 for the delay, MOO-176 for the modulation;
- whether the head fades: `ReadHead`, or bare reads recomputed every sample.

The policies they share already live in shared code (`DelayLine`,
`feedback_tail_frames`, `invalidates_tails`, `write_silence`, `OnePoleLp`,
`Smoothed`). What is still copied is arithmetic. Step 02 is cancelled, and
its file now says what would reopen it: a third device that needs a tap.

There is **no gain-stage block to add** over `shaper.rs` either. The four
entry points already there (`apply_drive` and its hoisted pair, `PreDrive`,
`shape` with `drive_compensation`, and `reference_drive_compensation` with
`Oversampler2x`) are each a different anchoring policy, and every caller
already uses one of them.

The search found one copied **number** with no guard: DS-01's
`DRIVE_GAIN_RANGE = 15.0` claims to be shared with `apply_drive`, but it is a
second copy of a bare literal in `shaper.rs`, and no test reads both. That is
MOO-482 (DSP Foundations). Step 03, the graph canvas (MOO-151, Interface),
is unaffected and is the plan's last open step.

## 2026-09-23: step 01 re-aimed at the voice blocks (MOO-144)

Step 01 was written aimed at the delay layer (`DelayEffect` and
`ModulationEffect`). The 2026-09-22 team review (`reports/teams-2026-09-22.md`,
finding F7) found a middle layer that was missing in a more costly place:
five instruments each wrote out their own voice filter and four their own
glide. MOO-144 re-aimed step 01 there and landed it:

- **`voice_filter::VoiceCutoff`** -- the Cutoff knob through the one
  knob-to-Hz law (`scale::cutoff_hz_from_normalized`, 20 Hz-20 kHz at every
  rate), plus octaves of modulation, clamped to what the filters run at.
  `env_octaves` and `keytrack_octaves` are the envelope's and keytracking's
  shares; `FILTER_ENV_OCTAVES` and `KEYTRACK_REFERENCE_HZ` are each defined
  once. The v1 mono and poly synths, the sampler, the ML-M1 and the ML-P8
  all ask it where their filter goes. What is shared is the cutoff, not the
  filter: the five run four different filters and keep them.
- **`glide::Glide`** -- a pitch that slides linearly in log frequency and
  arrives in the Glide time (MOO-145). The four synths hold one per voice.
- **The saturation stages** (`apply_drive`, `drive_compensation`,
  `apply_drive_compensated`, `soft_ceiling`, `PreDrive`) moved from
  `filter.rs` to `shaper.rs`, beside the curves and the oversampler, with the
  anti-aliasing policy that covers them written at the head of that section.

The original step 01 question -- the delay layer's "modulated tap" block --
is still open and still investigation-first; steps 02 and 03 are unchanged.
