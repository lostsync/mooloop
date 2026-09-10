# 03 — A channel is a thing you named

Adam, 2026-09-05: *"i want to make a sidebar on the left that lets you change
channel settings like name, track color, input channel, etc. its also
illustrated in the mockup."*

This step takes the *content* of that sidebar and leaves the sidebar itself
for later. What a channel is called and what colour it is are facts about the
project; where the controls for them live is a layout decision, and this plan
exists because we stopped making layout decisions all at once.

## The three pieces, and how different they are

**A channel name exists and is editable.** `channel.setup.channel.name` is
real, saved, and already has a rename path — `copied_channel_name`
(`crates/mooloop-session/src/channel.rs:161`) even knows how to derive
"Kick copy" and disambiguate it. Nothing to build in the model.

**A bus name exists and cannot be set.** `MixerBus.name` is in the project
format, saves and loads, and defaults to `"Bus 1".."Bus 16"`
(`crates/mooloop-core/src/mixer.rs:64`). `grep` finds no `RenameBus` or
`SetBusName` anywhere in the tree: there is no command, no action, no Slint
field. This is the cheapest real win in the whole plan — for a drum bus you
want "Drums", and every layer beneath the control is already written.
`LOOSE_ENDS.md` has been carrying it.

**A channel colour does not exist at all.** No field in the UI, none in the
session model, none in the project format. Nothing in the rack, mixer, or
playlist is colour-coded by channel. This is the only part of this step that
adds persisted state, and `PROJECT_FORMAT.md`'s defaulted-field rule applies:
an old project must load without one and be indistinguishable from a new
project that has not chosen one.

## The bug this step has to fix, found 2026-09-09

`Session::reset_channel_source` (`crates/mooloop-session/src/session.rs`)
rewrites the channel's name from its index whenever the source device changes:

```rust
channel.name = match kind {
    DeviceKind::Sampler => format!("Sampler {}", index + 1),
    ...
```

So a channel called "Kick" that is switched from the sampler to the DS-01
comes back called "DS-01 1". **A name the user typed is thrown away by a
gesture that is not about the name.** That is fine while a name is derived
from the device, which is what it is today, and stops being fine the moment
this step says a name is a thing you chose.

The fix belongs here rather than in its own branch: this step is where the
name stops being derived. A default name still comes from the device for a
channel that has never been named, so the distinguishing question is whether
the current name is still the default one for the *outgoing* device -- if it
is, re-derive it; if it is not, the user named it and it stays.

Found while writing `docs/plans/console/`, which is why it is dated later than
the rest of this file.

## Pattern colour comes with channel colour

Adam's 2026-09-09 list asked for pattern colours as well. Patterns can already
be *renamed* (`main.slint`, `pattern-renamed`); colour is missing in exactly
the same way it is missing for a channel, and it is the same defaulted-field
shape in `PROJECT_FORMAT.md` terms: absent means "no colour chosen", an old
project loads without one, and opening it does not rewrite it.

It is folded in here rather than given a `LOOSE_ENDS.md` entry of its own
because the two share the whole design -- the storage rule, the "a colour the
project owns, not a palette index" ruling below, and the judgement about how a
chosen colour sits against a theme it was not chosen under. Deciding those
twice is how they end up decided differently.

The "do not colour-code everything at once" rule below applies to both.

## The colour question this must not answer

`ENHANCEMENTS.md` holds an open design question — whether a named scheme like
Nord is *three seeds* or a full sixteen-colour ramp — and says base16 and
pywal are what decide it. Appearance already derives the whole palette from
three seeds plus roundness and contrast.

**A channel colour is not part of that question and must not be allowed to
settle it.** It is content in the project file, like a channel's name: it
travels with the song, not with the theme. Store it as a colour the project
owns. Do *not* store it as an index into the current palette, which would make
a song look different under a different scheme and would quietly commit the
palette to having a fixed number of slots.

The judgement that does belong here is how a chosen colour sits against a
theme it was not chosen under. `reference/ADAM.md` is the standing brief for
that kind of call.

## The MIDI rows

The mockup draws MIDI input/output/channel rows. `FOCUS.md` is explicit and
this step inherits it without reopening it: **build the setting, let it stay
inert, and do not let the sidebar pull MIDI configuration forward.** MIDI is
decoded and routed; it is not configurable, and making it so is its own work.

Note the adjacent case in `LOOSE_ENDS.md` — `EngineHandle::set_buffer_midi_map`
is the only way to install a Buffer MIDI map and nothing in `mooloop-ui` or
`mooloop-session` calls it. Same shape, same answer: not here.

## Where the controls go for now

Wherever they are cheapest to reach and easiest to move. The point of this
step is that a channel *has* a name and a colour and that both survive a save;
the left sidebar is a home for them, not a prerequisite. Putting them
somewhere plain now and moving them when the sidebar is built is the
iterate-and-let-it-take-shape method working as intended — and it is much
cheaper than the reverse, because a `main.slint` contract change is the
expensive crossing and moving a control within the file is not.

## Do not

- **Do not build the sidebar in this step** unless it turns out to be the
  cheapest place to put three controls, in which case build it small and
  expect it to move.
- **Do not colour-code everything at once.** Get the field saved and shown in
  one place first. The rack, mixer and playlist can each adopt it afterwards,
  and each is a place to check the colour actually reads at that size.
- **Do not give buses colours** in the same pass. A bus rename is a one-line
  decision with the model already built; a bus colour is a second design
  question about what colour means when signals merge.

## Done when

A channel's name and colour are set from the interface and survive save,
reload and offline render; a project written before this step loads with a
default colour and is not rewritten by having been opened; a bus can be called
"Drums"; and the MIDI rows exist, are inert, and look it. A pattern can be given a colour on the
same terms as a channel, and a channel that has been named keeps its name when
its device is changed.
