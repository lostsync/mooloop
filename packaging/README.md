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
