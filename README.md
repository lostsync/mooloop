# mooloop

<p align="center">
  <img src="mooloop.png" alt="Mooloop logo" width="156">
</p>

<p align="center">
  <img src="mooloop-screenshot.png" alt="Mooloop's channel rack and Mono Synth" width="900">
</p>

Mooloop is a Linux-native pattern instrument for making rhythm-heavy music
with samples and small synths. It runs against JACK or PipeWire's JACK layer,
and it is written in Rust with a Slint interface. Think channel rack, piano
roll, playlist, effects, and enough routing to make a proper mess of a beat.

This is not a revolution. We have all seen enough software announce a new
paradigm from a black-and-lime landing page. Mooloop is an early instrument
that tries to make a beat quickly, then let you pull it apart without opening
a full recording DAW. Nothing is cool, including Mooloop. It can still be
useful.

## What It Is

Mooloop is a pattern-first instrument for sample-driven, synthetic, and
rhythm-centred work: IDM, broken beats, industrial clatter, drum and bass,
hip-hop, or whatever happens after the hardware has been sampled too many
times. The rack is the fast route in; the piano roll and playlist are there
when the loop needs to become a track.

It is deliberately Linux-first. There is no browser wrapper, account, cloud
sync, or artificial scarcity. Bring a JACK server—or PipeWire with JACK
compatibility—and your own questionable folder of sounds.

## What It Does

- Builds patterns with a 256-channel rack and independently sized patterns.
- Plays samples plus DrumSynth, MonoSynth, and PolySynth source devices.
- Edits notes in a piano roll with 64th-note snap, velocity, duration,
  multi-selection, and pattern-level operations.
- Arranges layered pattern clips in a playlist, with Pattern and Song
  transport modes.
- Shapes channels and mixer buses with reorderable insert chains: EQ,
  modulation, filters, drive, bitcrush, delay, two reverbs, gate, compressor,
  and limiter.
- Mixes through a master and sixteen insert buses with faders, pan, mute,
  meters, and ordered bus routing.
- Draws and saves per-parameter automation for channel and bus effects.
- Saves versioned projects, kits, channels, and generator presets; optional
  embedded samples travel with a project.
- Exports WAV or MP3 and talks to the rest of your Linux audio setup through
  JACK.

## What It Doesn't

- It is not a general recording DAW, a REAPER replacement, or a plugin host.
- It does not record audio into the project. The proposed retained-audio
  buffer, resampling, and mutation workflow is not in the release.
- It does not yet have parallel sends, returns, sidechains, a metronome,
  external inputs, stem export, or full plugin-delay compensation.
- It does not have the full modulation system: no LFO rack, modulation matrix,
  parameter locks, or tracker-command editor yet.
- It does not finish every editing workflow. Undo/redo, keyboard navigation,
  menus, clipboard, project recovery, and responsive layout have known rough
  edges.
- It supports Linux with JACK or PipeWire's JACK compatibility layer. Other
  systems are not secretly supported because somebody got it to compile once.

## Get It

Grab the package for your distribution from the
[GitHub Releases page](https://github.com/lostsync/mooloop/releases). Release
CI produces x86_64 `.deb`, `.rpm`, and AppImage artifacts.

```sh
# Debian / Ubuntu
sudo apt install ./mooloop*.deb

# Fedora / RPM systems
sudo dnf install ./mooloop*.rpm

# AppImage, if you prefer one suspiciously self-contained file
chmod +x Mooloop-*.AppImage
./Mooloop-*.AppImage
```

Start JACK first, or start PipeWire with its JACK compatibility services. Then
run `mooloop` from your launcher or terminal. If sound does not happen, check
the JACK graph before writing a seven-page issue about it.

## Build It

You need the Rust toolchain named in `rust-toolchain.toml`, `mold`, JACK
development headers, and the normal Linux graphics/windowing development
libraries Slint needs. PipeWire's JACK layer is fine at runtime. On Fedora,
install `mold` with `sudo dnf install mold`; use your distribution's packages
for the rest.

```sh
git clone https://github.com/lostsync/mooloop.git
cd mooloop

# Mooloop's Cargo configuration expects this linker.
command -v mold

# Build and run the instrument. Release mode is the audio-performance baseline.
cargo run --release -p mooloop-app --bin mooloop -j 2
```

For a shared Cargo cache across several worktrees:

```sh
export CARGO_TARGET_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/mooloop/cargo-target"
```

Cargo jobs are intentionally capped. Do not turn this into a race to exhaust
your RAM; build commands should run one at a time.

## Develop It

The docs say what the system does, what it is trying to become, and where the
sharp edges live. Read the relevant one before discovering an old constraint
the hard way.

- [Operations](docs/OPERATIONS.md) — worktrees, Cargo, checks, releases, and
  the unglamorous mechanics.
- [Current system](docs/CURRENT.md) — the implemented user surface and known
  gaps. This is the source of truth for current behaviour.
- [Product definition](docs/PRODUCT.md) — the instrument's boundaries and
  non-goals.
- [UI design](docs/UI_DESIGN.md) — layout, controls, and interaction rules.
- [Application architecture](docs/ARCHITECTURE.md) — crate and data-flow map.
- [Audio architecture](docs/AUDIO_ARCHITECTURE.md) — control-plane,
  realtime-engine, timing, and latency contracts.
- [Project format](docs/PROJECT_FORMAT.md) — durable project and asset bundle
  behaviour.
- [Automation/modulation](docs/MODULATION_PLAN.md) and
  [retained-audio buffer](docs/BUFFER_ENGINE.md) — approved designs for work
  that is not all here yet.

## Next, Briefly

The next round is automation, modulation, mutation, generation, and the usual
work of making the existing instrument less annoying. That is the whole pitch.
No nebula of promises, no "ecosystem," no future-tense feature pile.

## License

Mooloop is free software under the [GNU General Public License v3.0 or
later](LICENSE).
