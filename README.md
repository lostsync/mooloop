# mooloop

Mooloop is a Linux-native, pattern-first sampling instrument for building
rhythms quickly and then pushing them into detailed, unstable, buffer-based
sound design. It is written in Rust, uses Slint for the interface, and talks to
JACK or PipeWire's JACK compatibility layer directly.

The project is an early working prototype. It is already useful as a small
sample groovebox, but it does not yet have project persistence, song
arrangement, expressive note duration, channel effects, or export.

## What Works

- Up to 16 sampler channels and 8 patterns.
- Per-pattern lengths from 1 to 256 sixteenth-note steps.
- A step rack with pitch and velocity data.
- A zoomable piano roll and pinned velocity lane.
- Sample loading, folder navigation, waveform display, trim, reverse, and
  forward or ping-pong loops.
- Sampler tuning, ADSR, resonant low-pass filtering, drive, bit reduction, and
  rate reduction.
- Sample-accurate event delivery inside JACK blocks.
- A reusable Slint audio-control layer, appearance settings, and peak meters.

## Product Direction

Mooloop is not intended to become a general-purpose DAW. Its working product
definition is in [docs/PRODUCT.md](docs/PRODUCT.md), with the dependency-ordered
plan in [docs/ROADMAP.md](docs/ROADMAP.md).

The main product hypothesis is that a channel can be more than a track with an
instrument on it: it can own musical audio memory, with record and playback
heads that are addressable by the same patterns and parameter lanes as notes.
That idea is specified as a testable design in
[docs/BUFFER_ENGINE.md](docs/BUFFER_ENGINE.md).

[docs/CURRENT.md](docs/CURRENT.md) describes what the code actually does today.
It is intentionally separate from the desired product.

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

## Documentation Authority

When documents disagree, use this order:

1. Current, explicit decisions from Adam and current UI designs.
2. `docs/PRODUCT.md` and accepted decisions recorded there.
3. `docs/ROADMAP.md` for sequencing work.
4. `docs/CURRENT.md` and the code for implemented behavior.
5. `reference/ADAM.md` only as fallible background taste context.

## License

Mooloop is free software licensed under the
[GNU General Public License v3.0 or later](LICENSE).
