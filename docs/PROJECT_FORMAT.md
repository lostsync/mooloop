# Project Bundle Format

Status: format version 1, August 2026.

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

Slice mode adds `play_mode` and `slice_base_note` to the parameters, plus a
`slices` table beside them holding the slice boundaries as `{ id, frame }`
pairs sorted by source frame. All three default, so a song written before
slicing loads as an ordinary pitched sampler with no markers.

A committed time stretch stores a `commit` table: the stretch mode, resolved
ratio and grain that were baked, plus the start/end/loop fractions and marker
frames the editor held before the commit. The rendered audio is deliberately
not stored -- the render is length-determined by this spec, so loading decodes
the source as usual and re-renders. `slices` is expressed in the *published*
buffer's frames, so a committed song's markers come back without remapping.

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
- Playlist starts within the 64-bar playlist canvas.
- Tempo from 1 to 999 BPM.
- Swing from 50 to 75 percent.
- Unique nonzero note IDs, nonzero durations, MIDI notes `0..=127`, and
  velocities `1..=127`.
- Finite, bounded mixer and sampler values; polyphony and choke groups from
  their current engine limits.

The loader validates these limits before changing the running document.
