# Project Bundle Format

Status: format version 1, September 2026.

Mooloop songs are inspectable UTF-8 TOML files. When a song embeds samples,
ordinary copied audio files live in a sibling asset directory; samples are
never encoded into TOML. Kits, channel documents, and preset-library entries
remain directory bundles containing a TOML manifest and optional audio assets.

When the playlist grows clips (after 0.2.0), read the DAWproject section of
`IDEAS.md` before adding the fields. It compares what DAWproject, the open
exchange format between DAWs, stores in a song with what this format stores.

## Bundle Layout

The conventional suffixes are:

- `name.mooloop` for a song file.
- `name.mooloop-assets/` for that song's embedded assets, when any exist.
- `name.mooloop-kit/` for a kit directory bundle.
- `name.mooloop-channel/` for a channel directory bundle.

A song with embedded assets has this layout:

```text
|-- beat.mooloop
`-- beat.mooloop-assets/
    |-- samples/
    |   |-- 00-kick.wav
    |   `-- 01-snare.wav
    `-- recordings/
        `-- 20260922-141503-Sampler_1.wav
```

`recordings/` holds the song's recorded takes, under the names they were
recorded with. A save tells a take from any
other sample by the folder its file is in: the shared folder a take is
recorded into is also called `recordings`, and so is another song's on a
Save As. Everything else a song owns goes into `samples/`, prefixed with its
channel the one time it is copied in (`00-kick.wav`), and with `-2`, `-3`
before the extension if that name is taken.

Directory bundles retain the original layout:

```text
drums.mooloop-kit/
|-- manifest.toml
`-- samples/
    |-- 00-kick.wav
    `-- 01-snare.wav
```

**A save is durable, and the song is never missing.** The new song file is
written to a hidden sibling, flushed to the disk (`sync_all`), read back and
parsed, and only then renamed onto the song's
name. On POSIX that one rename is atomic, so a crash, a power cut or a reader
sees the old song or the new one and never neither; the folder is synced after
it so the rename survives too. What the save copied into the asset directory
is synced before the song names it. The version it replaced is kept beside it
as `<name>.bak` (a hard link made under a name that one save alone uses, then
renamed into place), and a save that fails anywhere before the rename leaves
the previous song exactly as it was. A legacy directory-style song, which a
file cannot be renamed over, is moved to `<name>.bak` first and put back if
the rename fails.

Kits and presets are directories, and a directory cannot be renamed over one
that has files in it, so the old bundle is moved aside under a name only that
save uses, the synced and read-back staging directory renamed into place, and
the old one removed.

**The asset directory is added to, never rebuilt.** A file
already in it stays where it is, under the name it has, and is not copied
again -- however the song's path is spelled: "already in it" is decided on
resolved paths, so a song opened through a relative path, a `..` or a
symlinked parent and saved through another is still recognised as owning its
samples (a name that grew under an earlier build keeps its length and stops
growing); a file new to the song is copied in once, through a hidden `.part`
sibling renamed into place, under a name nothing in the folder has. A file the
song stops using stays in the folder -- an undo can bring it back -- until the
clean-up dialog (`recording.clean-up`, `ACTIONS.md`) moves it to the trash. A
save that fails removes only what it copied in. Rebuilding the folder on
every save, as earlier builds did, copied every embedded sample again and
deleted a take the undo history still pointed at.

Loading and resaving an older directory-style `.mooloop` song migrates it to
the file and sidecar layout. Other bundle types retain their directory
replacement flow.

## Envelope

Every manifest starts with the same fields:

```toml
format_version = 1
document_type = "song" # "song", "kit", or "channel"
asset_mode = "embedded" # "embedded" or "referenced"

[document]
# document-type-specific fields
```

Readers reject unknown format versions and document types before installing
any state. Version 1 is fixed at PPQ 96 and 4/4. Its tagged source envelope
carries all eight generator kinds without changing the sampler representation.
Three of the eight tags were chosen rather than inherited from serde's
`rename_all`, and all three are frozen:

| Generator | `source.type` |
| --- | --- |
| Sampler | `sampler` |
| v1 drum synth (now the Gitdum DS-SX) | `drum_synth` — the interface name changed and the tag did not; a channel saved as "Drum Synth 1" keeps that name |
| v1 mono synth | `mono_synth` |
| v1 poly synth | `poly_synth` |
| ML-M1 | `ml1` — the device shipped under the wrong name and the tag is an on-disk identifier, so it was frozen rather than corrected |
| ML-P8 | `mlp8` — picked deliberately the first time, for the reason above |
| DS-01 | `ds01` — chosen the same way and for the same reason |
| Aux In | `aux_in` — what `rename_all` would have spelled it anyway, written out on purpose so nobody has to derive an on-disk identifier from an attribute |
| A hosted plugin instrument | `plugin`, matching the plugin effect's tag; its `state` is only the plugin slot number (see "Hosted plugins") |

`asset_mode` records the requested save policy. Each file sample also carries
its own `embedded` flag, which means **the song owns this sample**: a
referenced save keeps a bundle-owned sample when externalizing it would destroy
the only copy, and it also *copies in* an owned sample that is
not in the bundle yet -- a recorded take, which sits in the shared recordings
folder until the first save. Either way the report
carries a "stays embedded" warning. So a saved song never refers to the
recordings folder.

## Song Document

A song stores the complete editable and session state. This abridged manifest
shows the nesting; a written bundle also includes the full sampler parameter
table:

```toml
[document]
bpm = 120
swing_percent = 50
ppq = 96
beats_per_bar = 4
playback_mode = "pattern" # or "song"
current_pattern = 0
selected_channel = 0 # a channel id, not a position -- see below
pattern_lengths = [16]

[[document.playlist]]
pattern = 0
start_tick = 0

[document.loop_range]
start_tick = 0
end_tick = 0
enabled = false

[[document.channels]]
next_note_id = 2
notes = [[{ id = 1, start_tick = 0, duration_ticks = 24, note = 60, velocity = 100 }]]

[[document.pattern_meta]]
name = "Chorus"
color = "#EAB308"

[document.channels.setup.channel]
name = "Sampler 1"
kind = "sampler"
muted = false
volume = 0.8
pan = 0.0
color = "#84CC16"

[document.channels.setup.source]
type = "sampler"

[document.channels.setup.source.state.sample]
kind = "builtin"
id = "default_kick"
```

**Names and colours are content, and both default.** A channel carries an
optional `color`, and `pattern_meta` carries one `{ name, color }` entry per
pattern, parallel to `pattern_lengths` -- which stays the field that decides
how many patterns a song has. Three rules govern them:

- **A colour is a colour, not a palette index.** It is stored as `#RRGGBB`,
  so a song looks the same under every theme and nothing here commits the
  palette to having a fixed number of slots. `ENHANCEMENTS.md` holds the
  palette question this deliberately does not settle.
- **Absent means "nobody chose one", and stays absent.** Both fields are
  skipped when they are empty, and `pattern_meta` is written with its
  trailing empty entries trimmed, so a song where nothing has been named or
  coloured writes exactly the bytes it wrote before these fields existed.
  Opening and saving does not rewrite it. A list longer than the pattern bank
  is repaired on load, and a shorter one -- the ordinary case -- is filled out
  in memory and left alone on disk.
- **A malformed colour costs the colour, not the song.** `color = "octarine"`
  reads as no colour rather than refusing the document, because a cosmetic
  field is the wrong thing to lose a song's worth of work to.

`channels[].notes` is a pattern-indexed array of note lanes. Notes beyond a
pattern's current logical length remain stored, so shortening and re-extending
a pattern is lossless. The sampler state also contains every field in
`SamplerParams`: voice/retrigger/choke settings, trim, reverse, root and tune,
loop settings, ADSR, filter, drive, bit reduction, and rate reduction.

`loop_crossfade_ms` is how long a forward loop's seam
is crossfaded, in milliseconds of the sample's own time, from 0 to 100. It
defaults to 0, a hard seam, so a song written before it existed loads and
renders exactly as it did. A value outside the range, or not a number, is
repaired into it on load.

`loop_quantize` is what a loop's bounds snap to:
`"off"`, `"slices"`, or a division of the bar (`"bar"`, `"half"`,
`"quarter"`, `"eighth"`, `"sixteenth"`, `"thirty_second"`). The bar is the
sample's musical length, `stretch_bars` bars over the playback region. It
defaults to `"off"`, so a song written before it existed loops where it
always did.

`glide`, `glide_mode` and `env_trigger` are the
sampler's mono glide: the portamento time in seconds (0 to 2), when a note
glides (`"Always"` or `"Legato"`), and whether an overlapping note restarts
(`"Retrig"`) or only changes pitch (`"Legato"`). They are the ML-M1's own
controls and values. They default to 0, `"Legato"` and `"Retrig"`, and a
sampler with no glide and `"Retrig"` plays exactly as one saved before they
existed, so an older song loads unchanged.

Key zones add two fields beside `sample`. `keys =
{ low, high }` is the MIDI key range the sampler's own sample, the *base
zone*, plays. `zones` is a list of extra zones, each
`{ keys = { low, high }, root_note, sample, velocity = { low, high } }`,
whose `sample` is a sample reference like the base's, embedded or referenced
by the same rules. A note plays the base zone if its keys hold it, else the
first extra zone whose keys do, else nothing. An extra zone is pitched from
its own `root_note`. The slices, the stretch commit and `root_note` in the
parameters belong to the base zone alone. `velocity` is stored and
round-tripped, but nothing plays it yet. It is there so velocity layers need
no migration. Both fields are omitted at their defaults: a full keyboard, no
zones, and a full velocity range. So a song written before zones loads as one
full-range zone and saves byte-identical. On load, a range outside 0 to 127
is clamped and an inverted one swapped (`channel.sampler.zone`), and a
missing zone file is a sample warning, like a missing base sample.

Zone regions make each extra zone its own region of its file. A zone gains
`region = { start, end, reverse, loop_start, loop_end, loop_mode,
loop_crossfade_ms, tune_semitones, tune_cents, level_db }`, in the
parameters' own units
(points are fractions of the zone's file; `level_db` is a trim from -48,
which is silence, to +12). A field missing from `region` takes a fresh
zone's value: the whole file, no loop, no tune, 0 dB. The base zone keeps
its region in the parameters' existing fields, and its level trim in a new
parameter `zone_level_db`, omitted at 0, so a song without zones saves
byte-identical. A zone saved before regions existed has no
`region`. Such a zone played the parameters' start, end, loop and tune, so
on load it is given a copy of them, once, and is independent from then on.
That copy is a migration and is not reported as a repair. A zone that
somehow reaches the engine without a region plays the parameters' region,
as it did before. A lane or modulation route on a region parameter moves
every zone by the same amount from its own setting; a lane on Reverse or
Loop mode plays its value in every zone while it differs from the knob.

Slice mode adds `play_mode` and `slice_base_note` to the parameters, plus a
`slices` table beside them holding the slice boundaries as `{ id, frame }`
pairs sorted by source frame. All three default, so a song written before
slicing loads as an ordinary pitched sampler with no markers.

Each marker also carries `hand`: whether it was placed
or moved by hand, rather than laid down by Divide or by transient detection.
Detection's Replace keeps hand-placed markers. A marker saved without the
field loads as hand-placed, so accepting a detection never drops a marker
from an older song.

**A committed time stretch is a render that replaces the sample** (Adam,
2026-09-30: *"a committed sample should be treated pretty much like a
rendered one"*). The render is a WAV file of its own: `sample`
names it, owned (`embedded = true`) the way a recorded take is, so a save
copies it into the sidecar's `samples/` by the ordinary rule and a reload
plays it as stored. The `commit` table says how it came to be (shown inline;
a save writes `steps` as an array of tables):

```toml
[document.channels.setup.source.state.commit]
original = { kind = "file", path = "beat.mooloop-assets/samples/00-break.wav", embedded = true }
steps = [
  { mode = "music", ratio = 2.0, grain = 1024, start = 0.0, end = 1.0, frames = 48000 },
  { mode = "music", ratio = 1.2, grain = 1024, start = 0.0, end = 1.0, frames = 96000 },
]
```

`original` is the sample before the first commit, a sample reference like
`sample` and embedded, referenced and resolved by the same rules, so a REVERT
can find it in the bundle. `steps` is every commit since, oldest first: each
rendered the whole of the one before at the stored ratio, which is how
commits stack. `start` and `end` default to the whole input and `frames` (the
input's length, for noticing an original that changed on disk) to unknown.
`slices`, the start/end/loop fractions and the stretch parameters are the
render's own, so a committed song's markers come back without remapping.

A commit with **no `original`** has no stored render: `sample` is the
original, and the render is re-made from it through `steps` when the song is
installed (`render_stretched` is length-determined by its spec). That is how
a commit whose render could not be written is saved, and it is how 0.1.5
saved every commit -- a table of `mode`, `ratio`, `grain`, `source_start`,
`source_end`, `source_loop_start`, `source_loop_end` and `source_markers`,
which still loads, as one such step over `source_start`..`source_end`, and
plays exactly the render 0.1.5 made; its free `stretch_ratio`, which 0.1.5
left at the ratio it baked rather than resetting to 1, is read as already
baked (a step with no `frames` is 0.1.5's). Its other `source_*` fields are not
read, because a REVERT now maps the markers on screen back instead of
restoring a record. Every new field is defaulted, so the defaulted-field rule
covers the old table; a build before this change cannot read the new one.

A hand-edited `slices` table is normalised on load rather than refused:
markers are sorted by frame, duplicate frames dropped, and the list capped,
so the invariant is a property of the type rather than of well-formed files.

`swing_percent` is global sixteenth-note swing from `50` (straight) through
`75` (strong shuffle); `66` is approximately triplet swing. Readers default
the field to `50` for version 1 manifests written before swing was added.

Generated sources use the same tagged envelope and store their complete
parameter sets without an asset reference:

```toml
[document.channels.setup.source]
type = "drum_synth"

[document.channels.setup.source.state.params]
mode = "hat"
kick_character = "kit"
snare_character = "pop"
hat_character = "tight"
choke_group = 1
decay = 0.05
punch = 0.35
```

Reverb readers default every field of `ReverbParams` individually. The device
was once a generated-room convolution player and stored room
geometry (`shape`, `material`, `width_m`, `depth_m`, `height_m`, `capture_x`,
`capture_y`); the feedback delay network that replaced it stores `size`,
`decay_s`, `damping`, `predelay_ms`, `diffusion`, `width`, `modulation`, and
`low_cut_hz`. The geometry fields have no counterpart and are ignored on
load, `decay_s` carries across unchanged, and the rest take the new device's
defaults. The old parameter ids 0..=7 are retired rather than reused, so a
modulation route saved against a room control resolves to nothing instead of
landing on a different knob.

Drum synth readers default `kick_character`, `snare_character`,
`hat_character`, `punch`, `snare_tone2_hz`, `snare_tone2_mix`, and
`snare_noise_color` for version 1 manifests written before the punch/noise
controls were added.

`mono_synth` uses the same `state.params` envelope for its oscillator,
envelope, filter, glide, and drive fields. Its `params.lfo` table holds the
LFO wave, rate, retrigger flag, and one depth per destination; readers default
the whole table for version 1 manifests written before the LFO was added.
`poly_synth` shares that shape with a voice count and stereo spread.

`ml1` and `mlp8` each store their own complete parameter set under the same
`state.params` envelope: the ML-M1's two ADSRs, filter model, drive,
keytracking, note-priority and glide fields; the ML-P8's oscillator network
matrix, sub, noise, sync sources, two envelopes, filter, own LFO and internal
routes, its Unison, Detune, Spread, Drift and Chorus settings, and its output
stage's volume and pan. Neither
shares the v1 synths' parameter ids — see `current/devices.md` on ML-P8's separate id
namespace — but both are ordinary `#[serde(default)]` structures, so a field
added later reads as its default rather than failing the load.

`ds01` stores its universal percussion voice's whole parameter set — the tone
and noise layers, the body resonators, the burst schedule, four envelopes and
its eight-row modulation matrix — under the same envelope and the same
defaulting rule.

`loop_range` is the section of the arrangement the transport repeats: two
absolute PPQ ticks on the same grid as a playlist start, and a flag. Every
field defaults and the whole table does, so a version 1 manifest written
before the song loop existed opens with looping off. `enabled` is stored
apart from the points because switching a loop off keeps its section, which
is what makes the toggle worth having. A range reaching past the end of the
song is not rejected on load: the transport plays the part of it that exists,
so a song shortened outside the editor loads rather than failing.

`aux_in` is the smallest of them and the only one that names another channel.
Its `state.params` is four fields: `source_channel` (the producing channel's
index, or `-1` for none), `source_id` (that channel's durable identity, and
the field that decides where the index points), `source_outlet` (that device's
durable outlet id), and `level`. All four default, so an Aux In with no source
loads as silent rather than invalid, which is what the format's
defaulted-field rule buys here; `source_id` is also skipped when unassigned,
so a song with no source is byte-identical to one written before the field
existed. Why the index stayed an index is under **channels carry a durable
identity** below.

**The subscription names a channel, and channel indices move.** The index is
derived from the identity after every structural edit
(`Project::reseat_channel_references`), and a subscription written before
identities existed takes one on the way in from the seat it already named. A
subscription whose *producer* was deleted is retained and pointed at the last
addressable index, where it is refused visibly, rather than being handed to
whichever channel closed the gap -- and it keeps saying which channel it lost,
so restoring that channel makes the edge live again. The integrity pass checks the three values are in range and
deliberately does not check the subscription's target: whether the named
channel exists and publishes that outlet is recompiled every time the project
changes, and repairing it at load would destroy the orphan state the consumer's
face is meant to show.

## Effects, modulation, and automation

The later additions below hang off `#[serde(default)]`, so a manifest written
before any of them existed still loads:

- `channels[].setup.effects` is the ordered insert chain: one
  `EffectSlotState` per slot, each a tagged `EffectParams` enum. The
  pre-tag untagged filter shape still decodes. **Every kind's `state`
  table fills a missing field from its default** (a struct-level
  `#[serde(default)]` on every params struct), so a song or
  preset written before a parameter existed loads, with that parameter at
  its default -- where a field names a default of its own for an older
  song's sake (the Modulation's `rate_division`), that one wins. A table that
  still cannot be read, because a value has the wrong type or the kind is
  unknown, fails with an error naming the kind ("the `reverb` effect's
  parameters could not be read"). `a_saved_effect_missing_any_one_field_still_loads`
  removes each field of each kind in turn, and
  `mooloop-project/tests/preset_corpus.rs` keeps one preset per kind, as
  0.1.4 saved it, that must keep opening as the device it was. Each row also carries a
  durable `id`, and `channels[].setup.next_device_id` is the mint it comes
  from; buses carry the same pair. Both default, and a chain decoded without
  ids takes its **positions** as its ids — which is exactly what the routes
  and lanes in such a project already mean by `slot`, so an older song loads
  pointing where it pointed.
- **A device may be folded**: `collapsed: true` on an
  `EffectSlotState` means the rack draws it as its header on its side, and a
  folded container hides its run. It is view state -- no engine command, no
  DSP reads it, and toggling it is not an undo step (undo and redo carry the
  live folds across the snapshot they install). Defaulted and not written
  while false, so a song saved with nothing folded is byte-identical to one
  written before the field existed.
- **A Buffer's `freeze` is not saved.** Adam, 2026-09-23: *"dont save freeze
  state or data. freeze is temporary."* It is
  never written, and a `freeze` key in an older song or preset is ignored, so
  a Buffer saved frozen opens unfrozen, recording, with the rest of its
  settings as saved. A copied or pasted Buffer arrives unfrozen too. The
  ring's audio was never saved. Undo is not a save: it keeps the freeze and
  the ring.
- **A channel carries a durable `id`, and `next_channel_id` is the mint it
  comes from.** Both default and are skipped when unset, so a song written
  before channels had identities is byte-identical to one saved now with
  none. A bank decoded without any ids takes its **positions** as its ids --
  which is what every address in such a song that said `channel = 3` already
  meant, so it loads pointing where it pointed and `FORMAT_VERSION` does not
  move. `Project::assign_channel_ids` is that pass, and it runs beside
  `assign_device_ids` on the way in.

  **Two channels may not wear one id.** It is the invariant everything built
  on the identity assumes, and nothing this program does can break it:
  `Project::insert_channel` mints on the way in, so a pasted channel is
  another channel rather than another view of the one it was copied from, and
  a kit entry that lands past the end of the song is minted the same way. A
  duplicate is therefore a hand-edited file; the integrity pass reports it as
  `channel.id.duplicate` and corrects it by reminting the later channel,
  which keeps every note, device and lane and leaves the addresses naming
  that id meaning the first channel wearing it. A removed channel's id is
  never reused, so an address left holding it resolves to nothing rather than
  to whichever channel closed the gap.

  **`selected_channel` is the first field to hold one.** It is written as a
  bare number exactly as it always was, and a song written before channels
  had identities reads its old index as an id -- which names the same
  channel, because a bank with no identities takes its positions. So the
  format did not change; only what the number means did, and it means the
  same thing for every file written so far.

  As an identity there is nothing to renumber after a structural edit. A
  selection naming a channel the song does not have is repaired to the first
  channel (`song.selected_channel`), which is a different question from a
  range check: an id of 40 is perfectly ordinary in a song that has been
  edited forty times.

  **The other three fields that name another channel** each took a different
  shape, because each had a different reason not to be an id outright.
  `docs/plans/archive/channel-identity/06` records the reasoning; what a file
  holds is this:

  - A **control binding**'s target stopped being a `ParamAddr` and became a
    `ParamKey`, whose `scope` is a `ChainKey` -- `Channel(ChannelId)` or
    `Bus(u8)`. **The saved form did not change one byte.** `ChannelId` is
    transparent over its `u32` and `EffectTarget::Channel` was already written
    as `{ channel = 3 }`, so both spellings produce the same TOML, and an
    older file's index 3 decodes as `ChannelId(3)` -- the same reading
    `assign_channel_ids` gives that file's channels. Nothing renumbers a
    binding now. One whose channel is deleted is **not dropped**: it stops
    resolving, the mapping list draws it as an unavailable parameter, and it
    works again if that channel comes back.
  - **Aux In's `source_channel` stays a position**, and a defaulted
    `source_id` sits beside it as the authoritative field. The seat could not
    become an id because it is an *addressable parameter*: its descriptor's
    range is `NO_SOURCE ..= MAX_CHANNELS - 1` and its curve is
    `Stepped(MAX_CHANNELS + 1)`, so id 47 -- ordinary after enough edits --
    fits neither. Two fields, each with one meaning, rather than one field
    with two (Adam, 2026-09-17).
  - **The envelope gate's `input_channel` stays a position** for a different
    reason -- it is read on the audio thread as an index into the gate array,
    and `ModRack` is `Copy` and ships to the engine verbatim -- and takes a
    defaulted `input_channel_id` beside it the same way. Its parked marker
    `u8::MAX` is **not** reread as an id, unlike every other old index here:
    255 is an ordinary `ChannelId`, so a parked gate is decoded as naming
    nothing.

  Both defaulted ids are skipped when unassigned, so a song with no Aux In and
  no envelope is byte-identical to one written before they existed. A song
  that has them is given ids on the way in by
  `Project::identify_channel_references`, which `assign_channel_ids` ends with
  so that the two cannot come apart -- a load that handed out channel ids and
  forgot to identify the references would leave them positional, silently.
  `Project::reseat_channel_references` is its other half, and runs after every
  structural edit.
- **A channel's `audio_input` is where it records audio from**, beside
  `midi_input` and independent of it: `"input"` (the hardware input),
  `"master"`, `{ track = 4 }` or `{ channel = 7 }`. A track or channel is
  named by **id** rather than seat, so it survives a move and a deleted
  source resolves to nothing.
  Omitted when `off`, so a song written before it is byte-identical. Any
  channel may hold one whatever its device, and any number may. **A kit or
  channel document brings no audio input** -- it would name a channel of
  another song -- and one is cleared silently on load.

- **A track carries a durable `id`, and `next_track_id` is its mint** --
  the channel rule above, one list over, with the same defaulting, the same
  positions for a bank that has none (`Project::assign_track_ids`, run beside
  `assign_channel_ids`), and the same uniqueness rule, reported as
  `track.id.duplicate`. A master the repair pass has to restore is minted
  like any other track. **Only a channel's `audio_input` names a track by
  id**: a channel's `bus`, a track's `output` and sends, and
  `EffectTarget::Bus` are still seats. Otherwise the id exists so the engine
  can match a track across an install
  (`docs/plans/archive/incremental-structure/`).

- **Analog sum is one defaulted boolean per track.** `buses[].bus.console`
  says whether that track's output is encoded on its way into its destination,
  to be decoded there with everything else that opted in. It defaults to
  `false`, which is a linear mixer and is what every manifest written before
  this existed reads as. The master's value is ignored, because the master
  feeds nothing.

  Adam settled that analog sum is a track's switch and not a channel's. A
  `channels[].setup.channel.console` from the day one briefly existed is an
  unknown field, and serde drops it.

  There is deliberately **no field naming the algorithm**. One curve exists,
  and a `console_mode` added later with `#[serde(default)]` is the same no-op
  migration whenever it lands, so a saved field with one legal value would buy
  nothing now. See `docs/plans/archive/console/02-console-summing.md`.
- **A track's channel strip is one defaulted struct per track.**
  `buses[].bus.strip` carries the voicing, the three `in` switches, the drive,
  the four EQ bands and the compressor's seven values; `buses[].bus.polarity`
  is the invert beside it. Both default, and the default is every switch off
  and every value neutral -- so a manifest written before the strip existed
  loads **bit-identical** to the file it was saved from, which is the same
  claim analog sum makes one entry up and is a test rather than an intention
  (`a_default_strip_changes_nothing_about_the_mix`).

  **A band stores a frequency `position`, not a frequency.** `position` is an
  index into that band's stepped list -- 5 positions for the two outer bands
  and 7 for the two mid ones -- and the hertz it is worth is the *voicing's*,
  from `StripEqTable`. The format therefore survives a voicing being retuned,
  and a band's hertz can differ between Moo and Iron without either manifest
  or face lying. A manifest written against the earlier `frequency_hz` field
  loads with each band centred rather than at zero; nothing shipped with the
  old field, so no reader converts one.

  **The default for a missing `position` is per band, and it has to be.** The
  middle of a five-position outer band is 2 and the middle of a seven-position
  mid band is 3, so there is no one number a `#[serde(default)]` on the field
  could return -- it has no array index. `bands` decodes through a wire
  type whose `position` is optional and filled from `DEFAULT_POSITIONS[i]`.
  Nothing else about a band became optional: `kind`, `gain_db` and `q` each
  still refuse to load when absent.

  A band stores its `kind` as the full `EqBandKind` (`bell`, `low_shelf`,
  `high_shelf`) although the face offers each band only two of the three: the
  type switch chooses between a bell and that band's *own* shelf. Storing the
  kind rather than the switch position is what lets the two outer bands and
  the two mid bands share one struct, and what would let a later face offer
  the third value without a format change.

  **The master's own section rides in its strip**:
  `buses[].bus.strip.master`, a `MasterSectionParams` holding the bus
  compressor -- `comp_in`, `voicing` (`Grip`, `Punch` or `Tube`, by name),
  `threshold_db`, `makeup_db`, `mix`, and each voicing's own switch
  **positions** (`grip_ratio`, `grip_attack`, `grip_release`, `punch_ratio`,
  `punch_attack`, `punch_release`, `tube_time`). A position is an index into that voicing's law table in
  `mooloop_dsp::strip::bus_comp`, as a band's is, so a retuned law never
  needs the file to change, and an index past a table's end clamps to its
  last position. The struct is `serde(default)` field by field and **skipped
  when it is the default**, so a song that never touched the section writes
  nothing and is byte-identical to one saved before it existed; an older song
  opens with the section out. **`lookahead_ms` is ignored**: songs saved on
  2026-09-23/24 may carry it (the safety limiter's lookahead knob), and the
  limiter no longer has a lookahead, so the key is read as unknown and
  dropped, with no repair, and the song plays as if it were 0.
  It is not written back. Its parameter id, 56, is retired. Every track's
  strip may carry one, and only the master's is run: the session refuses its ids on any other
  track. See `docs/plans/archive/master-bus-compressor/`.

  **The same compressor as an insert** is an ordinary effect row,
  `type = "bus_comp"`, whose `state` is a `BusCompParams`: the master
  section's compressor fields under the same names and meanings -- `voicing`
  by name, `threshold_db`, `makeup_db`, `mix`, and the seven switch
  **positions** -- with no `comp_in` (the row's `bypassed` is its in/out) and
  no `lookahead_ms` (that was the limiter's, and is gone). `serde(default)`
  field by field, the defaults being the master section's. Its parameter
  ids, which automation lanes and modulation routes store, are the master
  section's ids 45..=55 moved down to 0..=10 and frozen with the rest
  (`param_id_freeze_tests.rs`). A reader that predates the kind refuses the
  row by name, as it would any unknown effect.

  There is deliberately **no field for where the strip sits in the chain.**
  `mooloop_core::mixer::STRIP_PIN` is a constant, not a project value: the
  pinned position is a policy the application states once, and a per-track
  copy of it would be a thing to keep in step for a feature nobody has asked
  for. See `docs/plans/archive/console/03-the-channel-strip-device.md`.
- **A channel's solo is stored the same way, and omitted when it is off.**
  `channels[].setup.channel.solo` is a defaulted bool that is skipped when
  false, so a song written before channel solo existed loads with none and
  saves byte-identically; only a song with something soloed grows the field.
  What it silences is derived from the whole bank every pump tick and never
  written, for the reason the track's below is not.
- **A track's solo is stored, and what solo *does* is not.** `buses[].bus.solo`
  is a defaulted bool, so a project reopens with the same tracks soloed. What
  it silences is derived every pump tick from the whole bank -- a soloed
  track's feeders and destinations stay audible -- and never written, because
  it is a function of the graph and storing it would let a hand-edited file
  describe a silence the graph disagrees with. Muting is the switch; solo is
  the question the switch asks.
- **A track's sends are a defaulted list.** `buses[].sends` is `{ target,
  level, tap, enabled }` per send, where `target` is a track index, `tap` is
  `post_fader` (the default) or `pre_fader`, and `enabled` defaults to `true`
  so that a hand-written send without one passes audio rather than silently
  not. The list defaults to empty, so every manifest written before sends
  existed loads with none and plays bit-identically.

  **`level` and `enabled` are both stored** because a send turned down and a
  send switched off are different statements: the disabled one keeps its
  level and gets it back when it is switched on.

  Repair on load is deliberately not the repair an *output* gets. A send
  naming a track that is not there is **dropped**, where an output naming one
  falls back to the master: a producer with nowhere to go must still be heard,
  and a send with nowhere to go is simply not a send — re-pointing it at the
  master would put a wet path into the mix at full level. A bank whose sends
  close a loop gives up **the edges on the loop and no others**, so the file
  still opens and the sends elsewhere in it survive; a send is given up before
  an output, for the same reason the paragraph above gives. See
  `docs/plans/archive/console/05-sends.md`.
- **Two effects follow the transport, and both persist a division rather
  than its result.** A delay carries `tempo_sync` and `time_division`, and a
  modulation effect carries `tempo_sync` and `rate_division`; all four
  default, so a manifest written before either existed loads free-running.
  The stored value is the musical division, so a project reopened at another
  tempo plays at that tempo rather than at the milliseconds or hertz it was
  saved with. Both name the same twenty-one-entry `ModTimeDivision` grid.

  A delay written before 2026-09-08 named the five-entry `DelayTimeDivision`
  instead, whose serde names are a subset of this one's. Four of the five
  carry over unchanged. The fifth, `half`, was worth half a beat where the
  grid it now shares says two — an eighth note under a label that read `1/2`
  — so a delay saved on that division reopens four times slower, playing the
  half note its own label always claimed.
- A modulation effect carries `state.width` (0 to 1, the wet signal's
  stereo width), parameter id 8. It defaults to
  1.0, full width, where the DSP's width stage is the identity to the bit,
  so a file written before it plays exactly as it did. 0.1.5 ignores the
  key and plays such a song at full width.
- A row of kind `chain` is a **container**: `state.children` says how many of
  the rows after it are inside it, and `state.mix` is the blend across that
  run. Both default. A container does not hold its children — they are
  ordinary rows of the same chain, so a reader that ignored `children`
  entirely would still play every device in the right order. Two invariants a
  well-formed chain holds, and the integrity pass reports: a span ends inside
  the chain, and spans nest rather than straddle. A file that breaks either
  loads with its containers flattened, because a chain of the right devices in
  the right order is recoverable and an impossible nesting is not.
- A container (`chain` or `layer`) also carries `state.level` (linear gain on
  its run's output before the blend, default 1.0), `state.mute` and
  `state.solo` (default false). Mute and
  solo act only when the container is a branch of a layer. Every file written
  before them reads as unity, unmuted and unsoloed, which is how it sounded.
- A container row's own `input_trim` and `output_trim` (the host fields
  every row carries) have always been saved and are **heard**: the input
  trim before the run and its dry copy, the output trim after the blend.
  Earlier builds ignored them, but no field changed and nothing migrates.
  A file that saved a box's trim away from unity therefore reopens at that
  level rather than at the level it used to play at, which is deliberate:
  the saved value is what the knob showed, and silently resetting it would
  change the file behind the face.
- `modulation` is **the song's** modulation set (song-modulation step 01,
  0.1.7): every modulator module, and every route from a source to a
  parameter anywhere in the song. A channel holds none. The table is left
  out when the song has no modulation, so such a song writes nothing new.
  - `modulation.modules[]`: each module's song-wide `id`, its `name`
    (`<channel name> <kind> <n>` when made), its random `seed`, its `input`
    (`{ channel_notes = <ChannelId> }`, left out for none), and its `params`
    as a rack slot wrote them. Until step 02 a module also writes `rack`
    (`{ channel, slot }`): the channel identity and slot the engine still
    runs it in. Step 02 removes it.
  - `modulation.routes[]`: one of `source` (a module id), `outlet` or
    `performance` (with `channel`, the identity of the channel whose
    generator or keyboard it is), then `destination`, `depth` and
    `polarity`. A route has no slot; the runtime one is derived on load. A
    route whose source or destination no longer resolves is dropped at load
    rather than parked.
  - `modulation.next_source_id` is the id mint, left out at zero.
- **A song written by 0.1.6 or earlier** carries a `ModRack` per channel at
  `channels[].setup.modulation`. Loading lifts every rack into the song's
  set, in channel order: each module gets a fresh song-wide id and its
  routes follow it; it is seated on the channel it came from, at its old
  slot, so the engine runs exactly the racks it ran before; an LFO, Step or
  Random listens to its old channel's notes and an Envelope to the channel
  its gate named; a route's channel scope and an outlet or keyboard route's
  channel are its old channel's. The song saves in the new shape and writes
  no per-channel rack again. Inside a rack, only occupied slots are
  written, each with its slot index, its durable `id` and its parameters,
  and routes persist their durable `source` id alone; a rack written before
  durable identities carries `source_slot` instead, and its slot number
  becomes its id.
- **A song written now, opened by 0.1.6**, opens without its modulation:
  0.1.6 ignores the unknown table, plays with no modulators, and drops the
  table on its next save. 0.1.6 checks `contains` only on effect presets and
  validates a song's envelope alone, so nothing this build writes can make
  it refuse the song. This is a known case of MOO-379 (*a song saved by a
  newer build opens silently in an older one*), to be fixed there in
  general.
- Automation lanes live inside the pattern lane that drew them — per
  (pattern, channel), at most one lane per destination — and address a
  `ParamAddr`, which may name a bus.

A `ParamAddr` naming a rack device writes `owner.effect.device`, the device's
durable id. Older projects wrote `owner.effect.slot`, a position, and decode
through a serde alias onto the same field — see the paragraph above for why
the two numbers agree. A channel is still named by *index*, so a route or lane
scoped to a channel is still renumbered when the channel list changes.

**A `ParamAddr` or `ParamKey` naming the channel's generator carries the
kind of device it was made on**. The owner is
still written `owner = "source"`, exactly as before, and the kind is a
sibling key on the same table, `source_kind`, spelled with the `source.type`
tags above (`"sampler"`, `"ds01"`, `"ml1"` ...). It is written for every
generator address and for no other owner, so every other address is
byte-identical to what it was. It exists because a descriptor id is stable
*per kind*: id 12 is the sampler's Cutoff and the v1 drum synth's snare tone,
and a channel whose device was changed used to hand its old lanes, routes
and bindings to whatever the new device calls the same id. An address whose
`source_kind` is not the kind its channel runs is **inert and kept**. It
drives nothing, the loader keeps it and does not report it as a repair, and
it is saved back unchanged. It works again if the channel is switched back
(Adam's "never drop" ruling on MOO-74, 2026-09-23).

**`FORMAT_VERSION` stays 1, because the key is optional in both
directions.** A song written before it has no `source_kind`. Each such
address takes its own channel's kind on load (`ChannelSetup::assign_device_ids`
for a rack's routes, in songs, kits and channel presets alike;
`Project::assign_channel_ids` for lanes and bindings). That is what the
address meant when it was saved, so an older song loads with every generator
address intact, reports no repair, and gains the key on its next save. A
`source_kind` this version cannot read (a device added later, or a hand
edit) is treated the same way rather than failing the song. **0.1.5 and
earlier ignore the key**: their address type has no `deny_unknown_fields`,
so they open a song written now. But they read every generator address as
the old unkinded one, so there a lane recorded for another kind drives the
current device's control with the same id. That is their old behaviour, and
only a song whose device was changed after the lane was made can show it.

Because a channel index is a position, loading runs an integrity pass: a lane
stranded on another channel's index is pointed back at its own channel, and a
route or lane naming a device or control that is not present is dropped. A
route belongs to the song, not to a channel, so one aimed at another channel
is checked where it points rather than pointed home. The song's set is
checked too: module ids are unique, a route naming a module the song does not
have is dropped, an input naming a channel the song does not have becomes
none, and a depth that is not finite is fitted.
A generator address is judged against the table of the kind it *names*, not
the kind the channel runs, so only a control that kind never had is dropped.
Addresses on a generator kind that has no descriptor table yet are left
untouched. Two lanes on one id of two different kinds are two destinations,
not a duplicate.

A container also saves as a preset of its own: an `effect_run` document
holding the container and everything inside it, in rack order, with
`contains = ["effect_params", "effect_run"]`. The entry is **added** rather
than replacing `effect_params`, so a reader that predates run presets refuses
the bundle instead of loading its first device and dropping the box. The
head may be any container: a layer saves this way too, its branches and all,
and the bundle lists under its head's kind, so a layer's preset is offered on
a layer's rail and a chain's on a
chain's. Nothing in the document changed for that; a chain-headed bundle
written before reads exactly as it did. Device
ids are stripped on save and minted fresh on load, because identity belongs to
the chain a device is on rather than to the patch. **A kit or channel document
carries no channel identity at all** -- both hold `ChannelSetup`, which is a
channel's devices rather than the channel -- so there is nothing to strip, and
loading a kit entry onto a seat the song does not have yet mints one for it. The modulation that drives
a run is not carried — a route's source is a module in the channel's rack, not
in the container — and `docs/plans/archive/containers/00-status.md` records why that
is a deferred decision rather than an omission.

**A container with a hosted plugin inside** carries the plugin. Each plugin
row's slot is renumbered to a key into the run itself, from `0`, and the run's
own `plugins` table holds each key's
`PluginSlotState`, exactly as a song's `plugins` table writes one (see
"Hosted plugins" below). The list becomes
`contains = ["effect_params", "effect_run", "effect_plugin"]`:

```toml
format_version = 1
document_type = "effect_run"
asset_mode = "embedded"
contains = ["effect_params", "effect_run", "effect_plugin"]

[[document.effects]]
[document.effects.params]
type = "chain"

[document.effects.params.state]
children = 1

[[document.effects]]
[document.effects.params]
type = "plugin"
state = 0

[document.plugins.0.plugin]
format = "clap"
id = "org.example.gain"
name = "Gain"

[[document.plugins.0.state]]
tag = "clap"
data = "..."
```

- The state is what each plugin held when the preset was saved, asked of the
  live instance (`Session::lift_run_live`), not the song's last capture.
- Loading it mints each plugin row a **new slot** in the song it lands in,
  opened with the carried state, as a copied or duplicated container does.
  A key is never a slot number in any song.
- **0.1.5 refuses it.** 0.1.5 checks an `effect_run` bundle's list against
  `["effect_params", "effect_run"]` before it parses the document, so it meets
  `effect_plugin` and refuses the bundle rather than landing the row on
  whatever the song has at that number
  (`an_older_reader_refuses_a_container_preset_with_a_plugin`).
- A bundle with a plugin row whose key has no entry is refused whole and not
  listed. That includes a container preset holding a plugin saved by an
  earlier build, which wrote the row's song slot number and no plugin.
  A save refuses such a run too.
- A run with no plugin in it writes neither the table nor the entry, so it
  is byte-identical to one written before.

## Hosted plugins

A song that uses a third-party plugin (`docs/plans/plugin-hosting/`) says two
things about it, and both are additive under format version 1.

The device on the chain is an ordinary effect slot whose parameters name a
**plugin slot**:

```toml
[[document.channels.setup.effects]]
id = 2
bypassed = false

[document.channels.setup.effects.params]
type = "plugin"
state = 0
```

A channel whose **source** is a plugin instrument names its slot
the same way, and its channel's `kind` is `plugin`:

```toml
[document.channels.setup]
next_device_id = 3
source_device = 2

[document.channels.setup.source]
type = "plugin"
state = 1
```

**`source_device` is the instrument's device identity.** A lane or route on
one of its parameters is `owner.plugin_param = { device = <source_device> }`
with the plugin's own
parameter id, exactly as on a plugin effect (MOO-74's address). It is minted
from the channel's `next_device_id`, the namespace its effects draw from, so
it never equals an effect's `id`.

- **Written only when assigned.** A native source has none, so a song with
  no plugin instrument is byte-identical to one written before the field
  existed. `FORMAT_VERSION` does not move.
- **A plugin source saved without one** (before 0.1.6) is given one on load,
  by `ChannelSetup::assign_device_ids`, after the chain's own ids, so it lands
  above them and the mint moves past it. No repair is counted: no such song
  can hold an address on it, so any fresh id is the right one. An assigned id
  an effect on the same chain also wears (a hand edit) is re-minted.
- **The integrity pass keeps an address on it** whether the plugin is
  installed or not, and whatever parameter id it names. One naming a device
  that is neither the source nor on the chain is dropped and counted, as for
  an effect.
- **Replacing the instrument** mints the new one a fresh id, and a native
  source gets none; the old instrument's lanes and routes go with it, as a
  deleted effect's do, and undo brings them back.
- An older build ignores the key, but refuses the song anyway on
  `source.type = "plugin"` (below).

The slot itself lives in the song's `plugins` table, keyed by
`PluginSlotId`, beside the mint the ids come from:

```toml
[document]
next_plugin_slot = 1

[document.plugins.0.plugin]
format = "clap"
id = "org.example.gain"
name = "Gain"
vendor = "Example"
version = "1.0.0"

[[document.plugins.0.params]]
id = 4000000000
name = "Gain"
min = 0.0
max = 2.0
default = 1.0

[[document.plugins.0.state]]
tag = "clap"
data = """
AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4
..."""
```

- **Slot ids are per song, not per chain**, and never reused. A device's
  `DeviceId` is minted by its chain, so a device moved to another channel
  would be renumbered. The table is keyed by something that is not.
- **`plugin` names the plugin, not a file.** `format` is `clap`, `vst3` or
  `au`. `id` is the plugin's own identifier (for CLAP, its reverse-DNS id).
  No path is saved, because the song should find the plugin on another
  machine through that machine's scan.
- **`params` is the parameter list as the plugin last reported it.** Ids are
  the plugin's own: sparse, arbitrary `u32`s. Optional keys are left out at
  their defaults: `module` (empty), `stepped` (none), `automatable` and
  `modulatable` (true), `hidden` (false). This list is what lets a song whose
  plugin is missing still show and keep its lanes and routes.
- **`state` is the plugin's saved state, as base64 inside the TOML.** Each
  chunk is `{ tag, data }`, and `data` is wrapped at 76 columns. Whitespace
  inside `data` is ignored on read, so a hand-rewrapped file still loads.
  CLAP writes one chunk. VST3 will write a component chunk and a controller
  chunk. The bytes are the plugin's own: mooloop never reads inside them,
  and the plugin versions them itself, so no plugin ever needs a format
  migration here.

**Why plugin state may go in the TOML when samples may not** (Adam,
2026-09-16). A sample is media, often large, and it is shared across songs
and kits. A plugin's state is small in the common case, and it belongs to its
device the way a native device's parameters do. The cost is known and
accepted: a sampler plugin's state can reach megabytes, and the file stays
valid with it.

**A missing plugin** (not installed, not found by the scan, or refusing to
load) does not stop the song opening. Its device plays as a pass-through (an
instrument plays as silence), and its slot, parameters, state, lanes and
routes are kept and saved back byte for byte. A source naming a slot the table
does not have is kept too, and plays silence; the integrity pass does not
repair it. A lane or route naming a plugin
parameter that no longer exists is kept too, and shown as missing (Adam,
2026-09-23, MOO-74).

**What an older build does.** An older reader ignores keys it does not know,
so on its own the `plugins` table would be dropped without complaint. But
that reader also meets `type = "plugin"` on the device, which it does not
know either, and an unknown effect tag fails the whole document. So a song
with a plugin device in it is **refused** by an older build, not half loaded.
A plugin source is refused the same way: `source.type = "plugin"` is an
unknown source tag to an older reader.
`an_unknown_effect_tag_is_refused_not_read_as_a_filter` holds the untagged
pre-tag filter fallback (`LegacyFilterParams`, whose three original fields
stay required) to that. A song whose table has slots but no device
naming them loses only those orphaned slots in an older build.

A song with no plugins writes neither key, so it is byte-identical to one
written before the table existed.

**A plugin device's preset** is an ordinary `effect`
document whose row is the plugin device, with the slot written unassigned
(`state = 4294967295`), and a top-level `plugin` table beside `document`
holding that slot's `PluginSlotState`: the `plugin` reference, `params`,
`pinned` and `state`, exactly as a song's `plugins` table writes them. Its
list is `contains = ["effect_params", "effect_plugin"]`:

```toml
format_version = 1
document_type = "effect"
asset_mode = "embedded"
contains = ["effect_params", "effect_plugin"]

[document.params]
type = "plugin"
state = 4294967295

[plugin.plugin]
format = "clap"
id = "org.example.gain"
name = "Gain"

[[plugin.state]]
tag = "clap"
data = "..."
```

- The state is what the plugin held when the preset was saved, asked of the
  live instance, not the song's last capture. Saving a preset writes nothing
  into the song.
- Loading one onto a plugin device of the same plugin (same `format` and
  `id`; the version may differ) mints the preset's plugin a **new slot** in
  the song it lands in and opens it with that state. The device keeps its
  identity, so its lanes and routes stay. The old slot leaves the song, and
  its instance is retired once its processor is back. An undo brings the old
  slot back as it was.
- **0.1.5 refuses it.** 0.1.5 checks an `effect` bundle's `contains` against
  `["effect_params"]` before it parses the document, so it meets
  `effect_plugin` and refuses the bundle
  (`an_older_reader_refuses_a_plugin_preset`). A bundle that says
  `effect_plugin` and has no `plugin` table, or whose row is not a plugin
  device, is refused as well.
- Presets are kept under `presets/effects/plugin/<vendor>/<id>/`, one folder
  per plugin, and a plugin device's rail lists only its own plugin's folder.
  An `effect_params` bundle holding a plugin row was never written by the
  application. It still opens as it did, but no row loads it.
- Every other document leaves the `plugin` key out, so each writes
  byte-identical to before.

## Kit And Channel Documents

A kit document contains `document.channels`, an array of channel setups. It
does not contain notes, patterns, playlist placements, tempo, or transport
state. Loading a kit replaces the rack setup and retains note lanes for channel
indices that remain present; removing populated channels requires confirmation.

A channel document is one reusable instrument preset directly under
`[document]`. Loading it replaces the selected channel's mixer and source state
while retaining that channel's notes. Sampler presets include parameters and a
sample reference; every generated source's preset is its generator parameters
and requires no audio asset.

**A channel document, and each kit entry, carries the song modulation that
plays on its channel** as a `ModRack` under the setup's `modulation` key: the
key 0.1.6 read, so a preset saved now opens there with its modulation. An
Envelope gated by the channel's own notes is written with no gate identity,
meaning the channel it lands on. Loading a channel preset drops the song's
routes into the receiving channel and from its outlets and keyboard, then
**adds** the preset's modules to the song as new ones, seated on that channel
with their routes aimed at it, wherever the preset was saved from. The song's
other modules stay. Loading a kit replaces every channel and so all the
song's modulation, then lands each entry's the same way. This is the concrete
case behind `COMPOSABLE_DEVICE_UNITS.md`'s rule that a saveable fragment must
not name its neighbours by index.

## Sample References

Built-in samples use a stable identifier:

```toml
[document.channels.setup.source.state.sample]
kind = "builtin"
id = "default_kick"
```

File samples use a path plus ownership flag:

```toml
[document.channels.setup.source.state.sample]
kind = "file"
path = "samples/00-kick.wav"
embedded = true
```

For a song file, the corresponding embedded path includes the sidecar name,
for example `beat.mooloop-assets/samples/00-kick.wav` or
`beat.mooloop-assets/recordings/20260922-141503-Sampler_1.wav`. Both forms
are resolved relative to their document container and checked for path
traversal.

Embedded paths must remain below the document's `samples/` directory, or a
song sidecar's `samples/` or `recordings/`; absolute paths and `..` traversal
are rejected. A build from before 2026-09-22 rejects a `recordings/` path, so
a song holding a take does not open in one. **The sidecar's name
is not required to match** -- a song renamed in a file manager, together with
its sidecar, stores a first component naming the *old* name, and the loader
substitutes the one this song actually has and reports it as an asset warning.
The containment property is the `Component::Normal` filter, not the name: a
path admitting no `..`, no root and no prefix cannot leave the directory it is
joined to whatever its first component is called. Requiring the name as well
was what made a renamed song refuse to open at all. The check is **lexical**,
so a symlink under `samples/` is still followed. Embedded
saves copy audio files byte-for-byte, preserve their extensions, and deduplicate
channels that use the same source file. Referenced saves write paths relative
to the bundle when possible. Relative paths are resolved from the bundle
directory when loading.

Missing or undecodable samples produce warnings and load as silent slots. The
rest of the song, kit, or channel is still installed, and the sampler editor
shows the missing path so it can be relinked by loading another supported
audio file.

## Version 1 Limits

- 1 to 256 channels and 1 to 256 patterns. Channel count follows the complete
  `u8` realtime-address space, not a small product cap.
- Pattern lengths from 1 to 256 sixteenth-note steps.
- Playlist starts within the 64-bar playlist canvas. A loop range is *not* in
  this list: it is clamped against the song length at playback by
  `LoopRange::active`, not validated on load, which is what lets a range
  reaching past the end of the song survive a shortening edit.
- Tempo from 1 to 999 BPM.
- Swing from 50 to 75 percent.
- Unique nonzero note IDs, nonzero durations, MIDI notes `0..=127`, and
  velocities `1..=127`.
- Finite, bounded mixer and sampler values; polyphony and choke groups from
  their current engine limits.
- Up to 256 effect slots per channel — the same complete `u8` space, for the
  same reason.
- Up to `MAX_MODULATORS_PER_CHANNEL` (8) modules and
  `MAX_MOD_ROUTES_PER_CHANNEL` (16) routes per channel. Both are engine
  constants rather than format fields: a manifest carrying more is truncated
  at load, not refused.
- Up to `MAX_AUTOMATION_LANES_PER_CHANNEL` (8) automation lanes per (pattern,
  channel), and at most one lane per destination. Also an engine constant, and
  truncated on the same terms: the integrity pass takes the same first eight
  that `Pattern::set_lanes` does and says how many went, so the document and
  the engine agree about which.
- Up to seventeen buses (master plus sixteen inserts), and a bank may hold
  fewer. A short stored bank is a small mixer and is **left as it is** --
  padding it back to seventeen was removed because it silently added fifteen
  tracks to every song it opened. The master is the one track that is not
  optional and is restored if it is missing.

  Two routing repairs run, and **they do not both run in the same place.** An
  out-of-range destination is repaired to the master by the integrity pass,
  which reports it through `Doctor`. A bank whose routing contains a cycle has
  the edges on that cycle removed, and a send naming a track that is not there
  is dropped, by `mooloop_core::mixer::sanitize_bank` -- which `Session` calls
  after the load. Those repairs are named, one line each, but they go to the
  **log** rather than to `LoadReport::repairs`: `sanitize_bank` runs on every
  project install and not only on a load, so folding it into the report means
  moving it into the integrity pass. That half is still in
  `docs/LOOSE_ENDS.md`.
- **No limit on sends.** Nothing in the format, the plan or the face reserves
  for a number of them; a track carries as many as it was given.
  `docs/CAPACITY_POLICY.md` is why.

The loader validates these limits before changing the running document.
