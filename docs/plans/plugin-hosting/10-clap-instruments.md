# 10 — CLAP instruments (#29)

Steps 06–07 and 09 are combined here: `ClapProcessor` becomes a channel
source.

## Negotiation

Accept a plugin that has **no main audio input, a stereo main output, and
at least one note input port**. Use the first note input port. Prefer the
CLAP note dialect, then MIDI. Everything else is `Incompatible`, and the
browser says why.

## Translating notes

`Event` is defined at `dsp/src/event.rs:19`.

| mooloop | CLAP |
| --- | --- |
| `NoteOn { id: u64, note, velocity: u8 }` | `note_on`: `note_id` from a per-processor table that maps the `u64` to an `i32`, `key = note`, `channel = 0`, `velocity = velocity / 127.0` |
| `NoteOff { id, note }` | `note_off` with the mapped `note_id`. **A `NoteOff` whose id is unknown** (it arrived after a choke or a stop) is dropped, not sent with id -1 |
| `Choke` | a `note_off` for every live id, then clear the table |
| transport stop / loop wrap | the same as `Choke`, where the engine already sends one. Follow what native generators do today and don't invent new behaviour |
| `ParamValue` | the same as step 06 |

The `u64` → `i32` table has a fixed capacity, allocated at activation. When
it is full, the oldest note is released. The plugin's `note_end` events
remove ids from the table.

When the plugin supports MIDI and not CLAP notes, send `0x90` and `0x80`
bytes and drop the ids. **Notes the plugin generates** are counted and
shown, not routed (#29's non-goal, which it asks to be made visible).

## The session and UI path

- The add-source popup (`main.slint:3645`) and "Add Channel" get a
  **Plugin…** entry, which opens step 08's browser filtered to instruments.
  The browser's instrument rows turn on.
- The channel's source face is step 08's plugin face.

## Tests

- The test sine plugin plays a pattern offline and in realtime, with
  identical output and note boundaries accurate to the sample. That
  includes a note that is held across the loop point, a choke, stop, and
  a stale `NoteOff`.
- Save and load restore the instrument and its state.
- A search test: no `clack` type is named in `mooloop-engine`'s sequencer or
  in `mooloop-core`'s note data.

## Manual

Surge XT and Dexed, played from a pattern and from a MIDI keyboard
(`midi-control` routes live notes, and no one has run that against a
keyboard yet, so this is a good time to).

## Done when

- [ ] #29's checklist.
