# 08 — note boxes

The boxes that work on notes, typed like any other (step 04). After this
step Adam's patches 1 to 3 can be made and heard: chord inversions, a root
note to a modal chord, and a note probability filter.

## The boxes

| Box | Arguments | Inlets | Outlets | What it does |
| --- | --- | --- | --- | --- |
| `chord` | type, inversion (`chord min7 /1st`) | `notes` | `notes` | builds the chord on each note; each inversion moves the lowest note up an octave |
| `modal` | root, mode, size (`modal d dorian 7`) | `notes` | `notes` | snaps each note to the mode, then builds that degree's diatonic chord (triad or seventh) |
| `scale` | root, mode (`scale c minor`) | `notes` | `notes` | snaps each note to the nearest note of the mode, down on a tie |
| `transpose` | semitones | `notes`, semitones | `notes` | a control wire into the second inlet adds whole semitones |
| `chance` | probability (`chance 0.7`) | `notes`, probability | `notes` | lets each NoteOn through with that probability; a wire into the second inlet adds to it |
| `gate` | | `notes` | `gate`, `pitch`, `velocity` | notes to control: `gate` as a gate tag is (step 06), `pitch` and `velocity` of the latest NoteOn as 0 to 1 |

- **Chord types**: `maj`, `min`, `dim`, `aug`, `sus2`, `sus4`, `maj7`,
  `min7`, `7`, `dim7`, as the prototype's list plus the obvious rest.
  **Modes**: the seven church modes plus harmonic and melodic minor.
  There is no song key in mooloop; the root and mode are the box's own.
- Chord notes take the input note's velocity. A note pushed above 127 or
  below 0 is dropped (and its NoteOff with it), not folded.
- **`chance` is deterministic**: it draws from a generator seeded by the
  box's `seed` (step 01 kept it), reset on the transport's start like
  `random` (`dsp/modulator.rs:497`), so a bounce plays the same notes as the
  playback before it. The draw is per NoteOn; the NoteOff follows its
  NoteOn's verdict (step 07's table).
- Two notes of one chord that land on the same pitch play once; the table
  maps both to one output and releases it with the last.

The pitch and mode tables live once, in `core` beside the chord tables, and
the face's selectors read them (`AGENTS.md`, *Duplication*).

## Faces

Each box's face (step 05) shows its arguments as selectors and knobs:
`chord`'s type and inversion as segmented rows, as the prototype draws
them; `modal` and `scale` a root row and a mode list; `chance` and
`transpose` one knob.

## Done when

- Each box has a unit test from known notes in to the exact notes out,
  NoteOffs included, and a test that nothing hangs when its input is
  choked mid-chord.
- `chance` renders the same notes in an offline bounce as in playback.
- `LISTENING.md` has the three patches: Keys taken through `chord min7
  /1st`; a bass line through `modal d dorian 7` to a pad; and a hat
  pattern through `chance 0.6` with a `step` into the probability inlet.
