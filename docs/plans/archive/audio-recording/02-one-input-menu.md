# 02 — One input menu

> **Amended 2026-09-18 (decisions 5, 6 and 10 in `00-status.md`).** The
> non-sampler rule, the one-recorder rule and the single menu are withdrawn:
> any channel holds an audio input, any number may, and it gets its own
> AUDIO row beside MIDI IN, independent of it. The sections below are kept as
> written; where they disagree with that, the decisions win.

Adam's decision 3: the sidebar's IN row picks the channel's input, MIDI or
audio, and that choice decides what record-arm captures.

## The non-sampler rule (Adam, 2026-09-17)

- On a channel whose source is not a Sampler, the audio-input rows are
  **shown but greyed out**, with the reason in the status bar. They are
  never hidden.
- Switching a channel's source away from Sampler while an audio input is
  selected moves the input to the **no-input** row (`Off`), not back to Follow
  Selection. That happens in the session, in the same edit as the source
  change, so undo restores both. Switching back to Sampler does not restore
  the audio input.
- A saved file can't hold an audio input on a non-sampler channel. If one
  does, `integrity` repairs it to `Off` with a doctor message.

## Build

**Model** (`core/src/midi.rs`, or a new `core/src/input.rs` if the combined
type outgrows the MIDI module)
- Add a `ChannelInput` that holds either the existing `ChannelMidiInput`, or
  `Audio(AudioInputSource)`.
- `AudioInputSource` is `Off`, `Channel(ChannelId)`, `Track(TrackId)`,
  `Master`, or `Port(String)`, where the string is a hardware port name
  stored as MIDI ports are today. **This step builds every variant except
  `Port`**, which arrives with step 01; until then there is nothing to list.
- **App sources are named by id, never by seat.** `channel-identity` spent a
  week converting fields that named another channel by position, and this is
  a new field that names another channel. `ChannelId` and `TrackId` both
  exist. The engine is index-addressed, so the session resolves the id to a
  seat when it builds `AudioInputRouting`, the way step 06 of that plan does
  for the envelope gate. A source that has been deleted resolves to nothing
  and the row shows as missing, as a missing MIDI port does.
- **The tap point is after the fader and pan** (open question 6 in
  `00-status.md`): a take of a channel sounds like that channel.
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
  sources -- Master, the tracks, the channels, and (from step 01) the
  hardware inputs.
- The channel itself is listed (open question 7: resampling in place is
  allowed).
- The rows are generated from the project each time the menu opens, so a
  renamed track or channel reads correctly without anything being stored.
- The tests pin the new numbering. Write them before changing the rows.

**Engine routing**
- A resolved `AudioInputRouting` table (a plain `Box` replaced by
  `StructuralCommand::SetAudioInputRouting`, like `MidiRouting`) says which
  channel, if any, records, and which buffer it records from: a channel
  seat, a track seat, the master, or (from step 01) the input bus. It was an
  `ArcSwap` cell until 2026-09-20; a guard held on the audio thread could be
  the last owner of a table the control thread had just replaced
  (`reports/fable-2026-09-19.md`, finding 2).
- **Set it in `install_project`.** Leaving the MIDI routing out of that
  function is exactly the bug found on 2026-09-17, and it must not happen
  twice. `prepare_render_state` sets both tables from `InputState` before the
  renderer reaches the audio thread; the test that installs a project and
  checks the table is the handle's own still guards it.

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
- A channel recording from another channel keeps recording from it across a
  channel move and a track move (it is an id), and shows as missing once that
  channel is deleted.
- The audio rows are disabled on every non-sampler source kind, and enabled
  on Sampler.
- Switching Sampler → any other kind with an audio input selected leaves the
  input `Off`, and one undo restores both the source and the input.
- A project saved with an audio input reloads with the same input.
- An old project, with only `midi_input`, loads unchanged.
- The routing cell survives an install.
