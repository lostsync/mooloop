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
