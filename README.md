# mooloop

Mooloop is an experimental, Linux-native, tracker-inspired pattern instrument.
It combines a channel rack, piano roll, playlist, sampler, and small synthesis
engines in a workflow aimed at making rhythm-centered music quickly. It is
written in Rust, uses Slint for the interface, and talks directly to JACK or
PipeWire's JACK compatibility layer.

**Mooloop is also explicitly an experiment in vibe coding.** It has been built
primarily through natural-language collaboration with AI coding agents, guided
by human product decisions, testing, and taste. The point of the project is in
part to find out how far that process can be taken on a technically demanding
realtime audio application. Treat it as experimental software, not as a mature
or dependable production tool.

## What Works

- Up to 16 channels using a sampler, drum synth, or mono synth.
- Up to 256 patterns, each from 1 to 256 sixteenth-note steps long.
- A step rack with pitch and velocity data.
- A zoomable piano roll and pinned velocity lane.
- A layered playlist with separate Pattern and Song transport modes.
- Sample loading, folder navigation, waveform display, trim, reverse, and
  forward or ping-pong loops.
- Sampler tuning, ADSR, resonant low-pass filtering, drive, bit reduction, and
  rate reduction.
- Versioned song, kit, and channel-preset bundles with optional embedded
  samples.
- Offline WAV and MP3 export.
- Sample-accurate event delivery inside JACK blocks.
- A reusable Slint audio-control layer, appearance settings, and peak meters.

## Known Issues And Limitations

- Much of the menu bar is scaffolding. Some file operations work, but many
  menu items are disabled, incomplete, or not wired to an implementation yet.
- There are no channel insert effects. The sampler and synths have their own
  tone-shaping controls, but the visible device-chain insertion point is empty
  and there is no effect insertion, bypass, reordering, or automation.
- There are no sends, returns, groups, sidechains, external inputs, or routing
  controls.
- General parameter automation does not exist. The lower parameter lane only
  edits velocity; there are no parameter locks, probability controls, or
  user-facing microtiming controls.
- Mooloop is tracker-inspired, but it is not currently a tracker. In
  particular, there is no tracker-style event editor or hexadecimal command
  and automation entry.
- The proposed retained-audio buffer engine is not implemented. Channels do
  not record or retain their output, and none of the buffer playback,
  mutation, or resampling workflow described in the design documents exists
  in the application yet.
- Editing is still incomplete: there is no undo, clipboard command layer,
  autosave, crash recovery, dedicated missing-sample relinking, or playlist
  clip dragging.
- Metering is master-only, despite unfinished per-channel meter visuals. There
  is no metronome.
- The interface still has interaction and responsive-layout edge cases. Many
  workflows are mouse-first and keyboard navigation is incomplete.
- Linux with JACK or PipeWire's JACK compatibility layer is the only supported
  platform and audio setup.

For the detailed implementation snapshot, including smaller edge cases, see
[docs/CURRENT.md](docs/CURRENT.md).

## Proposed Direction

Mooloop is not intended to become a general-purpose DAW. Its working product
definition is in [docs/PRODUCT.md](docs/PRODUCT.md), with the dependency-ordered
plan in [docs/ROADMAP.md](docs/ROADMAP.md).

One unproven product hypothesis is an insertable retained-audio buffer: a
channel device that continuously remembers recent audio and exposes its record
and playback heads to patterns and automation. This is a design and planned
spike, not a feature of the current application. The hypothesis is specified
in [docs/BUFFER_ENGINE.md](docs/BUFFER_ENGINE.md).

Another planned direction is tracker-like automation in the parameter lane,
including compact hexadecimal value or command entry where that representation
is musically useful. The exact interaction and command set have not been
designed, and no part of this workflow is implemented yet.

## Build

Requirements:

- Rust toolchain from `rust-toolchain.toml`.
- JACK development libraries.
- A running JACK server or PipeWire JACK compatibility layer.

Run the application:

```sh
cargo run -p mooloop-app --bin mooloop
```

Run the audio control gallery:

```sh
cargo run -p mooloop-ui --example control-gallery
```

Verify the workspace:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Project Documentation

- [Current system](docs/CURRENT.md): what the code does today.
- [Product definition](docs/PRODUCT.md): what Mooloop is trying to become.
- [Roadmap](docs/ROADMAP.md): dependency-ordered future work.
- [Buffer engine hypothesis](docs/BUFFER_ENGINE.md): the unimplemented
  retained-audio experiment.

## License

Mooloop is free software licensed under the
[GNU General Public License v3.0 or later](LICENSE).
