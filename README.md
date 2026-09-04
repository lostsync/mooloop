# mooloop

<p align="center">
  <img src="mooloop.png" alt="Mooloop logo" width="156">
</p>

<p align="center">
  <img src="mooloop-screenshot.png" alt="Mooloop's channel rack and Mono Synth" width="900">
</p>

Mooloop is a Linux-native, pattern-based groove sequencer and instrument. It has a channel rack, piano roll, playlist, mixer, automation, effects, sample playback, and a few small synths. It runs through JACK or PipeWire's JACK layer and is written in Rust with a Slint interface.

It started as an experiment with GLM-5.2.

I was testing the model, it kept producing surprisingly respectable prototypes, and eventually I tried to come up with something it shouldn't be able to one-shot: a GTK4 pattern sequencer in Rust. A few minutes later there was one running on my desktop.

That seemed worth pursuing.

The GTK version eventually hit the point where fighting the toolkit was becoming part of the project, so the interface moved to Slint. The original experiment kept going.

One important detail: **Mooloop is vibe coded.** Every line of code in this repository was written by AI under my direction. I don't know Rust, Slint, or DSP programming. I do know audio, sequencing, and enough software design to have opinions about how this should work.

At this point the experiment is mostly about seeing how far that distinction matters.

## What It Is

Mooloop is built around patterns rather than recorded audio. The channel rack is the quickest way in; the piano roll handles anything that needs more detail; the playlist turns patterns into an arrangement.

It is intended for sample-heavy and synthetic music where rhythm is doing most of the work. Broken beats, IDM, drum and bass, hip-hop, industrial noise, or whatever else fits.

It is Linux-first. There is no web layer, account, cloud service, or plugin store. It expects JACK, some audio files, and a computer.

## What It Does

- 256-channel pattern rack with independently sized patterns.
- Sample playback plus six sources: the sampler, a drum synth, and four
  synths — the original mono and poly pair, the filter-led ML-M1, and the
  eight-voice ML-P8 built around a three-oscillator network.
- Piano roll with 64th-note snap, velocity, note duration, multi-selection, and pattern operations.
- Playlist arrangement with layered pattern clips and Pattern/Song transport modes.
- Twelve reorderable insert effects: EQ, modulation, filter, drive, bitcrush,
  delay, hall and plate reverbs, gate, compressor, limiter, and a
  retained-audio buffer.
- Master bus and sixteen insert buses with faders, pan, mute, metering, and ordered routing.
- Per-parameter automation for channel and mixer effects, plus a per-channel
  modulation rack of LFO, envelope, step, random, and math modules.
- Versioned project, kit, channel, and generator preset formats.
- Optional sample embedding so projects can carry their assets with them.
- WAV and MP3 export.
- JACK integration with the rest of a Linux audio system.

## What It Doesn't Do

Mooloop is not a general-purpose recording DAW or a plugin host.

It currently does not:

- record audio into a project;
- have the composition workflow the retained-audio buffer is for — the
  device is an ordinary insert and works, but routing a source into it and
  sequencing the result is still the open product question;
- have parallel sends and returns;
- support sidechains or external inputs;
- export stems;
- provide full plugin-delay compensation;
- accept MIDI input, beyond a decoded port that currently reaches nothing;
- have parameter locks or a tracker-command editor.

Some basic application workflows are also still unfinished. Undo/redo, menus, shortcuts, and clipboard handling exist but do not reach everywhere; keyboard navigation, crash recovery, autosave, and responsive layout all have rough edges.

Linux with JACK or PipeWire/JACK is the supported platform.

## Get It

Packages are available from the [GitHub Releases page](https://github.com/lostsync/mooloop/releases).

Release builds currently include x86_64 `.deb`, `.rpm`, and AppImage packages.

```sh
# Debian / Ubuntu
sudo apt install ./mooloop*.deb

# Fedora / RPM systems
sudo dnf install ./mooloop*.rpm

# AppImage
chmod +x Mooloop-*.AppImage
./Mooloop-*.AppImage
```

Start JACK first, or use PipeWire with JACK compatibility enabled, then run `mooloop`.

If Mooloop is running but nothing is making noise, the JACK graph is a good place to start.

## Build It

You need the Rust toolchain specified in `rust-toolchain.toml`, `mold`, JACK development headers, and the normal Linux graphics/windowing dependencies required by Slint.

PipeWire's JACK compatibility layer is sufficient at runtime.

On Fedora:

```sh
sudo dnf install mold
```

Then:

```sh
git clone https://github.com/lostsync/mooloop.git
cd mooloop

command -v mold

cargo run --release -p mooloop-app --bin mooloop -j 2
```

Release mode is the normal baseline for audio use.

For a shared Cargo cache across worktrees:

```sh
export CARGO_TARGET_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/mooloop/cargo-target"
```

Cargo jobs are capped intentionally. Builds should run one at a time.

## Development

The documentation is split between what exists now and what the project is intended to become. [docs/README.md](docs/README.md) indexes all of it; the ones worth knowing about first:

- [Operations](docs/OPERATIONS.md) — worktrees, Cargo, checks, releases, and development mechanics.
- [Current system](docs/CURRENT.md) — implemented behavior and known gaps. The source of truth for the current application.
- [Product definition](docs/PRODUCT.md) — scope, boundaries, and non-goals.
- [UI design](docs/UI_DESIGN.md) — layout, controls, and interaction rules.
- [Application architecture](docs/ARCHITECTURE.md) — crates, components, and data flow.
- [Audio architecture](docs/AUDIO_ARCHITECTURE.md) — realtime engine, control plane, timing, and latency.
- [Project format](docs/PROJECT_FORMAT.md) — project files and asset bundles.
- [Modulation spec](docs/MODULATOR_SYSTEM_SPEC.md) — the modulation rack's sources, routes, and destination policy.
- [Retained-audio buffer](docs/BUFFER_ENGINE.md) — the buffer device's thesis, and what shipped against it.
- [Focus](docs/FOCUS.md) — the active work sequence; [Roadmap](docs/ROADMAP.md) orders the rest by dependency.

## Where It's Going

The immediate work is the synths — finishing the ML-M1 and building out the ML-P8 — and then turning the retained-audio buffer from a working device into a composition workflow. `docs/FOCUS.md` is the current sequence.

The original question was whether someone who understood the instrument but not the implementation could direct AI well enough to build one.

Apparently that question has changed.

## License

Mooloop is free software under the [GNU General Public License v3.0 or later](LICENSE).
