# mooloop

<p align="center">
  <img src="mooloop.png" alt="Mooloop logo" width="156">
</p>

<p align="center">
  <img src="mooloop-screenshot.png" alt="Mooloop 0.1.4: the step sequencer, the mixer with its channel strips, a Mono Synth into Reverb with a step modulator, and the sample browser" width="900">
</p>

Mooloop is a Linux-native, pattern-based groove sequencer and instrument. It has a channel rack, piano roll, playlist, a console-style mixer, automation and modulation, effects, a sampler, and a handful of synths. It runs through JACK or PipeWire's JACK layer and is written in Rust with a Slint interface.

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

**Sequencing**

- A 256-channel pattern rack with independently sized patterns. Channels and tracks have names and colours, and the left sidebar edits the selected one.
- A piano roll with 64th-note snap, velocity, note length, multi-selection, and pattern operations.
- A playlist of layered pattern clips, with Pattern and Song transport modes and a loop section.

**Sound sources**

- A sampler with slicing and time-stretch.
- Two drum synths: the original, and the DS-01, one universal percussion voice.
- Four synths: the original mono and poly pair (the poly can play as a monosynth), the filter-led ML-M1, and the eight-voice ML-P8 built around a three-oscillator network.
- Aux In, which plays another channel's audio.

**Effects**

- Thirteen reorderable insert effects: EQ, modulation, filter, preamp, drive, bitcrush, delay, hall and plate reverbs, gate, compressor, limiter, and Buffer.
- Containers, which blend a whole run of devices and save as one preset.
- Buffer is always recording the last couple of bars. JUMP, REVERSE, STUTTER and FREEZE turn that history into an instrument, and its playhead can be automated or modulated.

**Mixing**

- A console-style mixer with a master and up to sixteen tracks you create.
- Sends: a track can send a copy of itself to another track, with latency compensation.
- Every track has a channel strip (input stage, four-band EQ, compressor, polarity) under one of four voicings (Moo, Grip, Punch, Iron).
- Analog sum on any track, and solo in place.

**Automation and modulation**

- Per-parameter automation lanes.
- A per-channel modulation rack of LFO, envelope, step, random, and math modules.

**MIDI**

- Each channel picks its MIDI input port and channel, and a keyboard plays the selected channel.
- Record-arm captures notes into the playing pattern.
- Controller learn: press a control, move a knob, and they're bound. Pickup or jump takeover, relative encoders, and transport control are supported.

**Interface**

- The whole application is reachable from the keyboard, and shortcuts can be rebound.
- Fourteen themes (Dracula, Nord, Gruvbox, Catppuccin and more), plus one that follows your pywal or wallust wallpaper palette. Light, dark, or follow the desktop, with adjustable text size, density and fonts.

**Files and platform**

- Versioned project, kit, channel, device and container preset formats. Sample embedding is optional, so a project can carry its assets with it.
- WAV and MP3 export.
- Editing while the song plays: moving, pasting or deleting channels and tracks, and undo and redo, don't stop the transport.
- JACK integration with the rest of a Linux audio system.

## What It Doesn't Do

Mooloop is not a general-purpose recording DAW or a plugin host.

It currently does not:

- record audio. Recording and resampling into the sampler are in progress;
- host plugins. CLAP is planned, then VST3;
- take audio from a hardware input, or sidechain one track from another;
- send MIDI out;
- export stems;
- have parameter locks, probability, microtiming, or a tracker-command editor;
- autosave, recover from a crash, or relink missing samples through a dialog.

Undo, menus, shortcuts and the clipboard exist but don't reach everywhere yet, and the layout has rough edges at small window sizes.

Linux with JACK or PipeWire/JACK is the supported platform. macOS runs through Core Audio and Core MIDI, and each release includes an unsigned Apple Silicon build as a convenience. It is not a supported target.

## Get It

Packages are available from the [GitHub Releases page](https://github.com/lostsync/mooloop/releases).

Each release includes x86_64 `.deb`, `.rpm`, and AppImage packages (one binary, the same in all three), a zipped Apple Silicon `Mooloop.app`, a `SHA256SUMS` file, the changelog, and the Linux binary's debug symbols. A release is published only after its tests and an engine self-test on a JACK server have passed.

```sh
# Debian / Ubuntu
sudo apt install ./mooloop*.deb

# Fedora / RPM systems
sudo dnf install ./mooloop*.rpm

# AppImage
chmod +x Mooloop-*.AppImage
./Mooloop-*.AppImage

# macOS (Apple Silicon). The app is unsigned, so clear the quarantine flag
# once, or right-click it and choose Open the first time.
unzip Mooloop-*-macos-arm64.zip
xattr -dr com.apple.quarantine Mooloop.app
```

Start JACK first, or use PipeWire with JACK compatibility enabled, then run `mooloop`.

File choosers come from the desktop's file chooser portal (xdg-desktop-portal, which KDE, GNOME and most desktops run), and fall back to `zenity` and then `kdialog`. The `.deb` and `.rpm` also install `zenity`. With none of the three, Open, Save and Export say so rather than doing nothing.

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

On macOS the engine plays through Core Audio, so there is no JACK to install and no `mold` to find. The Xcode command-line tools and Rust are enough; `rust-toolchain.toml` fetches the toolchain on the first build:

```sh
xcode-select --install
brew install rustup
export PATH="$(brew --prefix rustup)/bin:$PATH"

cargo run --release -p mooloop-app --bin mooloop
```

Preferences > Audio lists the system default and every output device. MIDI input comes from Core MIDI, with nothing to set up.

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
- [Modulation spec](docs/MODULATION.md) — the modulation rack's sources, routes, and destination policy.
- [Retained-audio buffer](docs/BUFFER_ENGINE.md) — the buffer device's thesis, and what shipped against it.
- [Themes](docs/THEMES.md) — how to write a theme.
- [Scope](docs/SCOPE.md) — everything left before the 0.2.0 feature freeze. [Focus](docs/FOCUS.md) is the active work sequence, and each plan in `docs/plans/` tracks its own steps.

## Where It's Going

0.2.0 is the line: mooloop is 0.2.0 when sound can get in, it can be driven without the mouse, and its devices are ones you would choose rather than tolerate. The work between here and there is recording and resampling into the sampler, CLAP plugin hosting, sampler key zones, a master bus compressor with its own character, and a browser that can be searched and driven from the keyboard. `docs/SCOPE.md` has the whole list, and how big each item is.

The original question was whether someone who understood the instrument but not the implementation could direct AI well enough to build one.

Apparently that question has changed.

## License

Mooloop is free software under the [GNU General Public License v3.0 or later](LICENSE).
