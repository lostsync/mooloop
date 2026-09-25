# Packaging assets

Files here are used by `.github/workflows/release.yml` to build the AppImage,
`.deb`, and `.rpm` packages, and are referenced by path from
`crates/mooloop-app/Cargo.toml`'s `[package.metadata.deb]` and
`[package.metadata.generate-rpm]` sections.

- `mooloop.desktop` — freedesktop desktop entry installed into
  `usr/share/applications/`.
- `mooloop.png` — a 256x256 icon installed into
  `usr/share/icons/hicolor/256x256/apps/`. **This is a placeholder**: it's
  the existing `mooloop.png` wordmark padded onto a square transparent
  canvas, not a real app icon. Swap it for a proper square icon (a mark or
  monogram, not the wordmark) when you have one — same filename, same
  256x256 size, and everything downstream keeps working.
- `io.github.lostsync.mooloop.metainfo.xml` — AppStream metadata, what a
  software centre shows for an installed Mooloop. Installed into
  `usr/share/metainfo/` by all three packages. Check it with
  `appstreamcli validate packaging/io.github.lostsync.mooloop.metainfo.xml`.

`release.yml` compiles the binary once, strips it (`--strip-all`, symbols
published separately as `mooloop-<version>-x86_64.debug`), and packages that
one file into all three, then checks each package still holds it. Libraries
the binary loads with dlopen -- libjack, EGL/GL, xkbcommon, Wayland and the
X11 client libraries -- are invisible to `$auto` and to generate-rpm's
automatic requires, so they are named by hand in
`crates/mooloop-app/Cargo.toml`.

**The plugin scanner needs nothing packaged.** Its child process is the same
`mooloop` binary run as `mooloop --scan-plugin <path>` (MOO-80,
`crates/mooloop-plugin-host/src/scan.rs`), found through
`std::env::current_exe()`, so every package that carries `mooloop` carries
the scanner: `/usr/bin/mooloop` in the deb and rpm, the AppImage's mounted
`usr/bin/mooloop` while it runs, and `Contents/MacOS/mooloop` in the macOS
bundle. `mooloop-scan-child` in the plugin-host crate is a test double for
that crate's own tests and is never shipped.

## Minimum CPU: x86-64-v2

The Linux release binary, and so the `.deb`, the `.rpm` and the AppImage, is
built with `-C target-cpu=x86-64-v2`. It needs a CPU with SSE3, SSSE3, SSE4.1,
SSE4.2, POPCNT and CMPXCHG16B: roughly any Intel from 2009 (Nehalem) on and any AMD from
2011 (Bulldozer; the Bobcat-era low-power parts lack SSE4.1) on. On an older
CPU it dies with an illegal instruction (SIGILL), most likely at startup;
nothing checks the CPU first.

Adam's ruling, 2026-09-25, on MOO-263: "we can do v2, totally".

Why: at baseline x86-64 (SSE2 only) LLVM has no rounding instruction, so
`f32::floor`, `trunc`, `round`, `ceil`, and with them `fract()` and
`rem_euclid()`, are calls into libm, and the DSP makes them per sample in
oscillator wraps, table reads, interpolators and delay-line reads. SSE4.1's
`roundss` makes each one a single instruction.

Where it is set, and where it is not:

- `release.yml`'s `package-linux` and `build-rpm.yml` (which
  `scripts/build-rpm` runs) set `RUSTFLAGS` on their build step alone, then
  fail if the binary holds fewer than 100 SSE4.1 `round*` instructions
  (measured on the build box: 3 at baseline, source untraced, likely a
  dependency's runtime-dispatched SIMD path; and
  1,423 at v2), so a lost flag cannot ship
  a baseline build unnoticed.
- `release.yml`'s `verify`, `ci.yml`, `.cargo/config.toml`, the laptop and the
  build box (`scripts/antibox`, including `--release-bin`) stay at baseline.
  Tests therefore run on code that has no v2 instruction in it, and a
  dependency that quietly needed one would fail there first.
- The macOS build is Apple Silicon only and untouched; aarch64 has these
  instructions already.

`RUSTFLAGS` replaces `target.x86_64-unknown-linux-gnu.rustflags` from
`.cargo/config.toml` rather than adding to it. The workflows already clear it
(the runners have no mold), so the release steps lose nothing. To build a
v2 binary locally and keep mold, add to the config's list instead:
`cargo build --release --config 'target.x86_64-unknown-linux-gnu.rustflags=["-C","target-cpu=x86-64-v2"]'`
(Cargo concatenates `--config` arrays with the file's).

Measured on the build box, before and after: MOO-263.
