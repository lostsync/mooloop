# The held-note stack

## What is wrong

`MonoSynth::note_on` (`crates/mooloop-dsp/src/monosynth.rs:149`) keeps exactly
one note: the newest. `voice.event_id` is overwritten, `env.note_on()` is
called unconditionally, and there is no record that any earlier note is still
held. Three consequences:

- **No legato.** Every overlapping NoteOn restarts both envelopes. Glide is
  available but the thing glide exists for — a slide under a continuous
  envelope — is not.
- **No fallback.** Play C, add G, release G: the voice stays on G's frequency
  even though C is still held. Releasing G doesn't even release the voice,
  because `note_off` only matches the current `event_id`.
- **No note priority.** Which note wins is always "the last one that
  arrived", which is one of three musically standard answers.

This is the difference between "a synth with one voice" and "a monosynth".

## Do this

### 1. A fixed-size held-note stack

```rust
const MAX_HELD_NOTES: usize = 16;

#[derive(Clone, Copy)]
struct HeldNote {
    event_id: u64,
    note: u8,
}

struct HeldNotes {
    notes: [HeldNote; MAX_HELD_NOTES],
    len: usize,
}
```

Array, not `Vec` — this is touched from `process()`. Push on NoteOn, remove by
`event_id` on NoteOff. On overflow, drop the oldest entry rather than
rejecting the new note; a 17-note-deep mono chord is not a real performance
and dropping the newest would be the audible failure.

**Event IDs stay the identity.** Removal matches on `event_id`, never on
`note`, so a stale NoteOff cannot evict a newer entry with the same pitch.
`stale_note_off_does_not_release_a_retriggered_voice`
(`crates/mooloop-dsp/src/monosynth.rs:401`) must still pass and is the
regression guard for this whole step.

### 2. Priority

```rust
pub enum NotePriority { Last, Low, High }  // default Last
```

Selecting the winner:

- `Last` — the most recently pushed entry.
- `Low` — the lowest `note` currently held; ties broken by most recent.
- `High` — the highest, same tie-break.

On NoteOff, if the released note was the winner and the stack is non-empty,
the voice retargets to the new winner. **Retargeting is a pitch change, not a
note-on**: envelopes do not retrigger on fallback regardless of Env Trigger
mode. That is what makes trills work. If the stack empties, the voice
releases.

### 3. Env Trigger and Glide Mode

```rust
pub enum EnvTrigger { Retrig, Legato }  // default Retrig
pub enum GlideMode  { Always, Legato }  // default Legato
```

The two are independent and both keyed on the same question: *were notes
overlapping?* — i.e. was the stack non-empty before this NoteOn.

| Situation                   | `Retrig`             | `Legato`                    |
|-----------------------------|----------------------|-----------------------------|
| Stack was empty (new note)  | both envelopes start | both envelopes start        |
| Stack was non-empty         | both envelopes start | pitch changes only          |

| Situation                   | Glide `Always`       | Glide `Legato`              |
|-----------------------------|----------------------|-----------------------------|
| Stack was empty             | no glide from silence| no glide from silence       |
| Stack was non-empty         | glide                | glide                       |
| Voice active, stack emptied and refilled within the release tail | glide | jump |

That last row is the distinction that matters: `Always` glides between
successive pitches whenever the voice is still sounding, including across a
releasing tail; `Legato` glides only when the notes actually overlapped. The
current code's `was_active` check (`monosynth.rs:150`) is the `Always`
behaviour and can be kept for that mode; `Legato` needs the stack.

`Legato` env trigger with `Retrig`-style glide, and every other combination,
must be legal. These are two knobs, not one four-position switch.

### 4. Parameters and IDs

| Field         | Kind                    | Default  | ID |
|---------------|-------------------------|----------|----|
| `glide_mode`  | `GlideMode` enum        | `Legato` | 25 |
| `env_trigger` | `EnvTrigger` enum       | `Retrig` | 26 |
| `priority`    | `NotePriority` enum     | `Last`   | 27 |

`glide` itself keeps ID 0 and its 0-2 s range. All three new fields are
`MonoSynthParams`-only — do not add them to `PolySynthParams`. Descriptors are
`stepped(...)`, following `SYNTH_PARAM_LFO_WAVE`
(`crates/mooloop-core/src/generator.rs:245`). Each enum needs
`from_index`/`to_index` for the descriptor bridge and the Slint selector,
matching `OscWave` (`crates/mooloop-core/src/synth.rs:201`).

Defaults are chosen so an existing project's behaviour is unchanged where it
can be: `Retrig` matches today exactly. `Legato` glide mode does *not* match
today (today is effectively `Always`), but glide defaults to 0 ms so no
existing patch can hear the difference unless it set glide, and `Legato` is
the better default for a performer.

### 5. Choke and transport stop

`release_all` must also clear the stack. Otherwise a transport stop leaves
held entries that resurrect the voice on the next NoteOff retarget. Same for
`Event::Choke` and `reset`.

### 6. UI

The MOD page becomes **PERF**. It holds Glide (moved off the AMP/FILTER knob
row), Glide Mode, Env Trigger, Priority, and — once step 06 lands — Accent.
The common device frame's `MOD` affordance opens the channel modulation shelf;
it is not a column on this source face.

Three `SelectorBank`s provide `ALWAYS`/`LEGATO`, `RETRIG`/`LEGATO`, and
`LAST`/`LOW`/`HIGH`. The page currently has a fixed 214px LFO column and a
stretchy knob row; remove the LFO column and let Performance use the complete
intentional module area rather than preserving a blank placeholder.

Poly's face keeps no device-local MOD page either. These are now different
files in behaviour as well as content.

## Done when

- Overlapping NoteOn in `Legato` env trigger glides pitch without restarting
  either envelope. Test asserts `amp_env` level is continuous across the
  second NoteOn and does not return to zero.
- `Retrig` restarts both envelopes on an overlapping NoteOn.
- `Last`/`Low`/`High` each pick the documented winner, and releasing the
  winner falls back to the next eligible held note without an envelope
  restart. One test per mode with a three-note stack.
- Glide Mode `Legato` does not glide between separated notes; `Always` does.
- Releasing a non-winning note changes nothing audible.
- The stale-NoteOff guard still passes, plus a new case: a stale NoteOff for
  a pitch that has since been re-pressed does not evict the live entry.
- No allocation: the stack is an array and `process()` is unchanged in that
  respect.
