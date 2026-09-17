# 02 — One input menu

Adam's decision 3: the sidebar's IN row picks the channel's input, MIDI or
audio, and that choice decides what record-arm captures.

## Needs an answer first

Open question 1 in `00-status.md`: what an audio input means on a channel
that is not a sampler.

## Build

**Model** (`core/src/midi.rs`, or a new `core/src/input.rs` if the combined
type outgrows the MIDI module)
- Add a `ChannelInput` that holds either the existing `ChannelMidiInput`, or
  `Audio(AudioInputSource)`, where `AudioInputSource` is `Off` or `Port(String)`
  and the string is the port name, as MIDI ports are stored today.
- `MidiChannelFilter` belongs only to the MIDI arm, so the CH row is hidden
  (or disabled, with a reason) when an audio input is selected.

**Saved form**
- `Channel.midi_input` already exists in saved files and is omitted when it
  holds the default. Keep that field for the MIDI arm, and add a defaulted
  `audio_input` beside it rather than renaming anything.
- "One menu" is a presentation rule. The document holds two fields, and the
  session keeps at most one of them non-default.
- Record the rule in `PROJECT_FORMAT.md`.

**Picker rows**
- `picker_rows`, `row` and `from_row` in `midi.rs` define the menu, and their
  round-trip tests pin the row numbers. Extend them in the same place: Follow
  Selection, Off, All Inputs, the MIDI ports, a separator, then the audio
  inputs.
- The tests pin the new numbering. Write them before changing the rows.

**Engine routing**
- A resolved `AudioInputRouting` table (`ArcSwap`, like `MidiRouting`) says
  which channel, if any, the input bus is routed to.
- **Attach it in `install_project`.** Leaving the MIDI routing cell out of
  that function is exactly the bug found on 2026-09-17, and it must not
  happen twice. Add a test that installs a project and then checks the cell
  is the handle's own.

**Session**
- `set_channel_input` replaces `set_channel_midi_input` as the entry point.
- Selecting an audio input on one channel clears it from any other channel
  that had it, because only one channel records at a time (see "Not in this
  plan").

**UI**
- The IN row's model and its missing-port warning cover both kinds.
- Draft the row in `scripts/slint-sketch`. The `main.slint` contract change is
  made once, together with step 05's (see `AGENTS.md`, "Order device work so
  the face contract comes last").

## Test

- Picker round-trips for every row.
- A project saved with an audio input reloads with the same input.
- An old project, with only `midi_input`, loads unchanged.
- The routing cell survives an install.
