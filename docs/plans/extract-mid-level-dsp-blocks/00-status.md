# Extract mid-level DSP blocks: status

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
