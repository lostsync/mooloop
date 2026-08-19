# Project Bundle Format

Status: format version 1, August 2026.

Mooloop songs, kits, and channel presets are inspectable directory bundles. A
bundle contains a UTF-8 TOML manifest and, when requested, ordinary copied WAV
assets. Samples are never encoded into TOML.

## Bundle Layout

The conventional directory suffixes are:

- `name.mooloop` for a song.
- `name.mooloop-kit` for a kit.
- `name.mooloop-channel` for a channel preset.

Every bundle contains `manifest.toml`. Embedded samples live below `samples/`:

```text
beat.mooloop/
|-- manifest.toml
`-- samples/
    |-- 00-kick.wav
    `-- 01-snare.wav
```

Saving replaces the bundle through a sibling staging directory. An existing
bundle is moved to a temporary backup until the staged bundle is in place, so
a failed save does not leave a partially written document.

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
supports sampler, drum synth, and mono synth states without changing the
sampler representation.

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
choke_group = 1
decay = 0.05
```

`mono_synth` uses the same `state.params` envelope for its oscillator,
envelope, filter, glide, and drive fields. Its `params.lfo` table holds the
LFO wave, rate, retrigger flag, and one depth per destination; readers default
the whole table for version 1 manifests written before the LFO was added.

## Kit And Channel Documents

A kit document contains `document.channels`, an array of channel setups. It
does not contain notes, patterns, playlist placements, tempo, or transport
state. Loading a kit replaces the rack setup and retains note lanes for channel
indices that remain present; removing populated channels requires confirmation.

A channel document is one reusable instrument preset directly under
`[document]`. Loading it replaces the selected channel's mixer and source state
while retaining that channel's notes. Sampler presets include parameters and a
sample reference; drum and mono synth presets include their generator
parameters and require no audio asset.

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

Embedded paths must remain below `samples/`; absolute paths and `..` traversal
are rejected. Embedded saves copy WAV files byte-for-byte and deduplicate
channels that use the same source file. Referenced saves write paths relative
to the bundle when possible. Relative paths are resolved from the bundle
directory when loading.

Missing or undecodable samples produce warnings and load as silent slots. The
rest of the song, kit, or channel is still installed, and the sampler editor
shows the missing path so it can be relinked by loading another WAV.

## Version 1 Limits

- 1 to 16 channels and 1 to 256 patterns.
- Pattern lengths from 1 to 256 sixteenth-note steps.
- Playlist starts within the 64-bar playlist canvas.
- Tempo from 1 to 999 BPM.
- Swing from 50 to 75 percent.
- Unique nonzero note IDs, nonzero durations, MIDI notes `0..=127`, and
  velocities `1..=127`.
- Finite, bounded mixer and sampler values; polyphony and choke groups from
  their current engine limits.

The loader validates these limits before changing the running document.
