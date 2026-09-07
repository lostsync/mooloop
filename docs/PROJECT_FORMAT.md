# Project Bundle Format

Status: format version 1, September 2026.

Mooloop songs are inspectable UTF-8 TOML files. When a song embeds samples,
ordinary copied audio files live in a sibling asset directory; samples are
never encoded into TOML. Kits, channel documents, and preset-library entries
remain directory bundles containing a TOML manifest and optional audio assets.

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
    `-- samples/
        |-- 00-kick.wav
        `-- 01-snare.wav
```

Directory bundles retain the original layout:

```text
drums.mooloop-kit/
|-- manifest.toml
`-- samples/
    |-- 00-kick.wav
    `-- 01-snare.wav
```

Saving replaces a song file and its asset directory through sibling staging
paths. Existing paths are moved to temporary backups until both replacements
are in place, so a failed save can restore the previous document. Loading and
resaving an older directory-style `.mooloop` song migrates it to the file and
sidecar layout. Other bundle types retain their directory replacement flow.

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
| v1 drum synth | `drum_synth` |
| v1 mono synth | `mono_synth` |
| v1 poly synth | `poly_synth` |
| ML-M1 | `ml1` — the device shipped under the wrong name and the tag is an on-disk identifier, so it was frozen rather than corrected |
| ML-P8 | `mlp8` — picked deliberately the first time, for the reason above |
| DS-01 | `ds01` — chosen the same way and for the same reason |
| Aux In | `aux_in` — what `rename_all` would have spelled it anyway, written out on purpose so nobody has to derive an on-disk identifier from an attribute |

`asset_mode` records the requested save policy. Each file sample also carries
its own `embedded` flag because a referenced save may retain a bundle-owned
sample when externalizing it would destroy the only copy.

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
selected_channel = 0
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

[document.channels.setup.channel]
name = "Sampler 1"
kind = "sampler"
muted = false
volume = 0.8
pan = 0.0

[document.channels.setup.source]
type = "sampler"

[document.channels.setup.source.state.sample]
kind = "builtin"
id = "default_kick"
```

`channels[].notes` is a pattern-indexed array of note lanes. Notes beyond a
pattern's current logical length remain stored, so shortening and re-extending
a pattern is lossless. The sampler state also contains every field in
`SamplerParams`: voice/retrigger/choke settings, trim, reverse, root and tune,
loop settings, ADSR, filter, drive, bit reduction, and rate reduction.

Slice mode adds `play_mode` and `slice_base_note` to the parameters, plus a
`slices` table beside them holding the slice boundaries as `{ id, frame }`
pairs sorted by source frame. All three default, so a song written before
slicing loads as an ordinary pitched sampler with no markers.

A committed time stretch stores a `commit` table: the stretch mode, resolved
ratio and grain that were baked, plus the start/end/loop fractions and the
`{ id, frame }` markers the editor held before the commit. The rendered audio
is deliberately not stored -- the render is length-determined by this spec, so
loading decodes the source as usual and re-renders. `slices` is expressed in
the *published* buffer's frames, so a committed song's markers come back
without remapping.

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
was a generated-room convolution player through August 2026 and stored room
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
shares the v1 synths' parameter ids — see `CURRENT.md` on ML-P8's separate id
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
Its `state.params` is three fields: `source_channel` (the producing channel's
index, or `-1` for none), `source_outlet` (that device's durable outlet id),
and `level`. All three default, so an Aux In with no source loads as silent
rather than invalid, which is what the format's defaulted-field rule buys
here.

**The subscription is a channel-scoped address, and channel indices move.**
Deleting or pasting a channel renumbers every route destination and automation
lane that named a later one; a subscription goes through that same pass rather
than growing a repair path of its own. A subscription whose *producer* was the
deleted channel is retained and pointed at the last addressable index, where
it is refused visibly, rather than being handed to whichever channel closed
the gap. The integrity pass checks the three values are in range and
deliberately does not check the subscription's target: whether the named
channel exists and publishes that outlet is recompiled every time the project
changes, and repairing it at load would destroy the orphan state the consumer's
face is meant to show.

## Effects, modulation, and automation

Three later additions all hang off `#[serde(default)]`, so a manifest written
before any of them existed still loads:

- `channels[].setup.effects` is the ordered insert chain: one
  `EffectSlotState` per slot, each a tagged `EffectParams` enum. The
  pre-tag untagged filter shape still decodes. Each row also carries a
  durable `id`, and `channels[].setup.next_device_id` is the mint it comes
  from; buses carry the same pair. Both default, and a chain decoded without
  ids takes its **positions** as its ids — which is exactly what the routes
  and lanes in such a project already mean by `slot`, so an older song loads
  pointing where it pointed.
- A row of kind `chain` is a **container**: `state.children` says how many of
  the rows after it are inside it, and `state.mix` is the blend across that
  run. Both default. A container does not hold its children — they are
  ordinary rows of the same chain, so a reader that ignored `children`
  entirely would still play every device in the right order. Two invariants a
  well-formed chain holds, and the integrity pass reports: a span ends inside
  the chain, and spans nest rather than straddle. A file that breaks either
  loads with its containers flattened, because a chain of the right devices in
  the right order is recoverable and an impossible nesting is not.
- `channels[].setup.modulation` is that channel's `ModRack`. Only occupied
  slots are written, each with its slot index, its durable `id`, and its
  module parameters. Routes persist their durable `source` id alone; the
  runtime slot is derived on load, so a saved project cannot disagree with
  itself about where a module lives. A project written before durable
  identities existed carries `source_slot` instead, and its slot number
  becomes its id, which keeps its routes resolving to exactly what they
  meant. A route whose source or destination no longer resolves is dropped
  at load rather than parked.
- Automation lanes live inside the pattern lane that drew them — per
  (pattern, channel), at most one lane per destination — and address a
  `ParamAddr`, which may name a bus.

A `ParamAddr` naming a rack device writes `owner.effect.device`, the device's
durable id. Older projects wrote `owner.effect.slot`, a position, and decode
through a serde alias onto the same field — see the paragraph above for why
the two numbers agree. A channel is still named by *index*, so a route or lane
scoped to a channel is still renumbered when the channel list changes.

Because a channel index is a position, loading runs an integrity pass: a route
or lane stranded on another channel's index is pointed back at its own
channel, and one naming a device or control that is not present is dropped.
Addresses on a generator that has no descriptor table yet are left untouched.

A container also saves as a preset of its own: an `effect_run` document
holding the container and everything inside it, in rack order, with
`contains = ["effect_params", "effect_run"]`. The entry is **added** rather
than replacing `effect_params`, so a reader that predates run presets refuses
the bundle instead of loading its first device and dropping the box. Device
ids are stripped on save and minted fresh on load, because identity belongs to
the chain a device is on rather than to the patch. The modulation that drives
a run is not carried — a route's source is a module in the channel's rack, not
in the container — and `docs/plans/containers/00-status.md` records why that
is a deferred decision rather than an omission.

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

A channel document's modulation routes are rescoped as they load. A route
names its destination channel absolutely, so a preset saved from channel 3
would otherwise modulate channel 3 wherever it landed; `rescope_modulation`
rewrites those addresses onto the receiving channel. This is the concrete
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
for example `beat.mooloop-assets/samples/00-kick.wav`. Both forms are resolved
relative to their document container and checked for path traversal.

Embedded paths must remain below the document's `samples/` directory or its
matching song sidecar; absolute paths and `..` traversal are rejected. Embedded
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
- Playlist starts within the 64-bar playlist canvas, and a loop range within
  the same canvas.
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
- Seventeen buses (master plus sixteen inserts). A short stored bank is
  padded, an out-of-range destination is repaired to the master, and a bank
  whose routing contains a cycle is flattened to everything-to-master so the
  file still opens.

The loader validates these limits before changing the running document.
