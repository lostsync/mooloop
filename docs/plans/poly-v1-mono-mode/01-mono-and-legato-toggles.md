# Mono and legato toggles

Read `00-status.md` first — particularly the scope boundary and the open
question about whether mono mode is a toggle or `Voices = 1`.

## What is wrong

At `polyphony == 1` the v1 poly is a pool of one voice, not a monosynth. See
`00-status.md`, "What is actually wrong today", for the three specific
consequences in `crates/mooloop-dsp/src/polysynth.rs:200`.

## Do this

### 1. Parameters, in the reserved block

Three ids are free at 17-19 and this needs at most three. Add to
`crates/mooloop-core/src/generator.rs`:

```rust
pub const SYNTH_PARAM_MONO_MODE: u32 = 17;
pub const SYNTH_PARAM_POLY_GLIDE_MODE: u32 = 18;
pub const SYNTH_PARAM_POLY_NOTE_PRIORITY: u32 = 19;
```

They are separate constants from the ML-1's `SYNTH_PARAM_GLIDE_MODE` (25) and
`SYNTH_PARAM_NOTE_PRIORITY` (27) even though they carry the same meaning,
because ids are per-device and the ML-1 block must stay clear of the v1
synths. The duplicate-id test walks each table independently, so this is
legal; the naming should make the parallel obvious to a reader.

`POLY_DESCRIPTORS` grows from 32 to 35 entries. It still copies the first 30
from `MONO_DESCRIPTORS`; append the three new descriptors after `Spread`
rather than renumbering anything.

On `PolySynthParams` (`crates/mooloop-core/src/synth.rs:418`), which is
already `#[serde(default)]`:

```rust
/// Collapse to a single voice with monosynth note behaviour.
pub mono_mode: bool,
/// Whether glide applies always or only under overlapping notes.
pub glide_mode: GlideMode,
/// Which held note wins when several are down.
pub note_priority: NotePriority,
```

Defaults must be the current behaviour exactly: `mono_mode: false`, and the
other two at whatever value reproduces today's output when `mono_mode` is
false. Old projects must be bit-identical. Pin that with a test, not with
care.

### 2. The DSP

The held-note stack already exists and is already shared. In
`crates/mooloop-dsp/src/polysynth.rs`, add a `HeldNotes` to the synth (not to
each voice) and use it **only** when `mono_mode` is true:

- `note_on`: push, then drive voice 0 from `winner(note_priority)`. Retrigger
  the envelopes only when the stack was empty before the push, or when
  `glide_mode` is `Always`. Under `Legato` with a note already down, change
  pitch and leave both envelopes running.
- `note_off`: remove by `event_id`. If the stack is now empty, release. If it
  is not, move voice 0 to the new `winner` as a **pitch change, never a
  retrigger** — this is the rule 03 settled for the ML-1 and it should not be
  re-litigated here.
- `release_all` and transport stop must clear the stack, or a stale held note
  survives a stop and steals the next note's pitch.

When `mono_mode` is false, none of this executes and the existing path is
untouched. Prefer an early branch over threading a flag through the voice
loop; the poly path is the calibrated one and should stay legible.

Voices above index 0 must be deactivated on entering mono mode, and the
`voice_pan` spread centred — `voice_pan` already returns centre at
`polyphony <= 1` (`polysynth.rs:27`), so route mono mode through the same
clamp rather than adding a second centring rule.

### 3. The face

The v1 poly's face gains two controls, on the page where `Voices` and
`Spread` already live. Do not add a third page. If mono mode ships as a
toggle rather than as `Voices = 1`, `Voices` should read as disabled while it
is on, rather than disappearing — a control that vanishes is harder to find
again than one that greys out.

Note priority is a three-way and belongs next to the mono toggle, not next to
glide.

## Verify

- **Old projects are unchanged.** A v1 poly project saved before this change
  loads and renders bit-identically. This is the test that matters most.
- **Legato does what it is named after.** Two overlapping notes under
  `Legato` produce one continuous envelope and one pitch movement; the same
  two under `Always` produce two attacks.
- **Fallback.** Play C, add G, release G: the voice returns to C without
  retriggering. Play C, add G, release C: the voice stays on G, also without
  retriggering.
- **Priority.** The same three-note gesture under `Last`, `Low` and `High`
  picks three different winners.
- **No stranded notes.** Transport stop with notes held, then play again: the
  new note sounds at its own pitch.
- **Gain is untouched.** The existing gain-structure calibration test still
  passes for `PolySynth`, in both modes.

## Done when

The v1 poly plays a bass line with intentional note priority and legato,
old poly projects are bit-identical, and a v1 mono patch can be reproduced on
it closely enough that migrating `MonoSynth` channels is a mechanical job
rather than a judgement call.

That last clause is the real acceptance test, because it is what the next
branch depends on. If it does not hold, say so in `00-status.md` before
starting the migration — the gap is the finding.
