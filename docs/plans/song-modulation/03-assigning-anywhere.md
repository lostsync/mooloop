# 03 — assigning anywhere

The Assign gesture reaches every knob in the song, not just the selected
channel's. The gesture itself does not change: select a module, arm Assign,
drag a knob.

## Destinations

`channel_modulation_destination` (`session.rs:1262`) returns `None` for any
scope but the selected channel (*"Buses and another channel's controls stay
deliberately outside this pass"*). Replace it with a destination check that
takes any scope:
- any channel's source, inserts and strip;
- any track's inserts, fader and pan, and the master's;
- still refusing what `modulation_policy` refuses today
  (`plugin_params.rs:115`): a stepped parameter, a container's Mix (which
  the engine cannot read, `LOOSE_ENDS.md`), a hidden plugin parameter.

The UI callbacks build an address only when `effect_target ==
Channel(selected)` (`ui/src/lib.rs:12779-12862`). Build it for whatever
chain the rack is showing, a bus included.

**The order on one press stays as it is**: name first (the control menu),
then MIDI learn if armed, then the gesture (`learn_param_if_armed`,
`ui/src/lib.rs:6560`). Assign, learn and Automate all hang off that one
press.

## Showing what is modulated

- **Route-count dots** (`descriptor_route_counts`, `ui/src/lib.rs:3670`)
  count the song's routes landing on the chain the rack shows, so a track's
  inserts and a channel that is not selected show their dots too.
- **Live offsets** (`refresh_modulation_offsets`, `:5933`) do the same.
- **The rack's modulation shelf** is hidden while a bus is in the rack
  (`main.slint:6801`). Step 04 moves the shelf out of the rack altogether,
  so this step only has to make the faces right.

## Inputs

The module surface's input picker (today the Envelope's channel list,
`modulation-input-channels`, `ui/src/lib.rs:6412`) becomes one list for all
four kinds that take notes: **None**, then every channel by name. It writes
`set_module_input` (step 01). The list is built from the outlets that send
gates, not from the channel list, so the next kind of gate source joins it
without a new picker.

## Done when

- A module on the song set can be assigned, by the ordinary drag, to a knob
  on two different channels, on a track insert, on a track's fader and on
  the master, and each moves (a session test per scope, and one UI
  agreement test that the dots appear on a face that is not the selected
  channel's).
- MIDI learn and the control menu still take the press first
  (`midi_learn_gesture.rs` passes unchanged).
- An Envelope, an LFO's retrigger, a Step's advance and a Random's trigger
  can each be fed by another channel's notes from the picker.
- `shelf_agreement.rs` passes, rewritten for song-wide modules.
