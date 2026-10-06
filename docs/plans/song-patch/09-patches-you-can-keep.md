# 09 — patches you can keep

A selection of boxes can be saved as a patch preset and dropped into any
song. Its tags come with it, unbound, showing what they expect: *"kick goes
here [ ]"*. Channel presets carry the boxes their assignments come from, in
the same shape.

Adam (2026-09-23): *"in bitwig it would be up to you to set routing back up
again [...] this seems fine to me -- if we want to be nice we could try to
find some way to hint at what you are supposed to route there. like if we
made a pad designed to do pumping with a kick, and that kick wont be
routed, we need to somehow be like kick goes here => [ ]"*

## Patch presets

- **Save**: select boxes on the canvas, then *Save as patch preset*. The
  file holds the selected boxes, the wires among them, and their
  assignments. A wire to a box outside the selection is not saved.
- **Every tag is saved unbound, with a hint**: the name of what it was bound
  to when saved (`gate  [ kick ]`, `notes to  [ pad ]`). An assignment is
  saved as its destination's description (`Cutoff on ML-M1`) with no
  address. The hint is text, never an address, so a preset never reaches
  into a song it lands in.
- **Load**: the boxes land where the canvas was clicked, with fresh ids, and
  every tag and assignment is an empty slot showing its hint. Binding one is
  the inlet click (step 03) or the Assign drag.
- A new preset kind in the browser beside channel and effect presets, saved
  under the user preset folder (`PROJECT_FORMAT.md` has the preset
  layouts), in the same document format as a song's `modulation` table, so one
  reader loads both.

## Channel presets

Song modulation kept channel presets in the old rack shape
(`ChannelSetup::preset_modulation`, `core/src/project.rs:323`, under the
`modulation` key so 0.1.6 could read them), with the module's home seat
(`rack`) to find which modules belonged to the channel. Boxes made on the
canvas have no seat. Now, and `rack` goes:

- **Saving** a channel preset writes the patch fragment its assignments
  come from: every box upstream of an assignment into the channel, through
  its wires. A tag bound to the channel itself is saved as bound to "this
  channel"; any other tag is saved unbound with its hint.
- **Loading** one lands that fragment on the canvas beside the channel's
  other boxes (fresh ids), "this channel" bound to the receiving channel,
  and its assignments aimed at it.
- **Reading an old preset** (a `modulation` rack) converts it the way step
  01 converts a song, then lands it.
- ML-M1's factory patches (`project/src/factory.rs:295`) become fragments
  and still sound as they did.

What 0.1.6 does with a preset written in the new shape is recorded in
`PROJECT_FORMAT.md` beside MOO-379, as song modulation did for songs.

## Done when

- A patch preset saves and loads, with every tag and assignment unbound and
  hinted; a test round-trips the kick-pump patch.
- Channel presets save and load fragments; every ML-M1 factory patch
  renders as before (the null test).
- An old channel preset with a rack loads converted.
- `PROJECT_FORMAT.md` documents the patch preset.
