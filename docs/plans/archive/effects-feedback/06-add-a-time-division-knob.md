# Add a knob variant that toggles between ms and beat divisions

## Problem

From `docs/archive/EFFECTS_FEEDBACK.md`: "we should have a kind of knob that has
a toggle for ms and beat divisions. we'd use that e.g. in a delay but
maybe not for compressor attack - just raw ms there (although that could
be cool)."

The project already has a single global tempo: `MainWindow`'s `bpm`
property is read in `mooloop-ui/src/lib.rs` (`INITIAL_BPM`, `set_bpm`,
`project_snapshot(bpm, ...)`), and `DelayEffect`'s params carry their own
`bpm: 120.0` (`crates/mooloop-dsp/src/effects/delay.rs:254`) — check
whether that's meant to track the transport BPM already or is currently
independent/stale, since this knob's beat-division mode needs a live BPM
to convert against.

## What to do

1. Add a new control, e.g. `TimeDivisionKnob`, in `controls.slint`,
   composed from the existing `ParameterKnob` rather than duplicating its
   drag/scroll/keyboard logic: it adds a mode toggle (ms vs. beat
   division — quarter, dotted-eighth, triplet, etc., a fixed short list —
   per `docs/UI_DESIGN.md`'s "2-6 fixed modes → segmented selector, avoid
   dropdown") alongside the knob.
2. In beat-division mode, the knob's displayed/effective value is
   `division * (60000 / bpm)` ms, recomputed live as BPM changes — the
   underlying DSP parameter stays a plain ms value either way; only the
   knob's *input mode* changes. Confirm this against how `DelayEffect`
   already consumes its time parameter so the effect doesn't need two
   code paths.
3. Do not apply this to Comp/Gate/Limiter's attack/release — feedback
   explicitly keeps those as raw ms (noting it "could be cool" as a
   stretch idea only, not a requirement for this step).
4. This is additive/unused until a device asks for it —
   `08-delay-tempo-sync.md` is the first and, for now, only consumer.

## Verification

Software-rendered snapshot of the new knob in the control gallery
showing both modes; a focused test (or manual check) that beat-division
mode tracks a BPM change without the user having to re-set the knob.
