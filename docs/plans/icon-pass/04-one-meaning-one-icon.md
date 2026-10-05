# 04 — one meaning, one icon

About 45 call sites draw an icon as a text glyph through the platform font,
and several meanings are drawn with different glyphs in different places (see
the README's survey). This step gives each **meaning** one registry icon,
converts every site, and ends with `scripts/dupe-audit icon-literal` reading
**zero**.

## Decide the meanings first

This is a naming job before it is a drawing job. The inconsistencies to
settle, one icon each:

| Meaning | Today |
| --- | --- |
| `remove` | `×` (9 sites), `✕` (remove device, `device-rack.slint:289`), `−` (remove channel, `main.slint:4334`) |
| `close` / cancel | a Path X, and `×` (`modulation-shelf.slint:913`) |
| `previous` / `next` | `<` `>` (`plugin-device.slint:219`), `‹` `›` (every `StepperField`, mixer pages), Path chevrons (sampler) |
| `fold` / `unfold` | `<` `>` (device fold, `device-rack.slint:437`, `:908`). These are the same glyphs as previous and next, so fold needs its own shape. |
| `disclose` (a tree row) | `▾` `▸` (browser), `⌃` `⌄` (modulation shelf) |
| `dropdown` | `▾`, both literal and `\u{25be}` (3 sites) |
| `save-preset` / `load-preset` | `⌑` and `▱` (the rack's rail), `💾` (the channel sidebar, `main.slint:4520`) |
| `enabled` / `bypassed` | `●` `○` text, the `StripIcons.dot` Path, the word "ON" (`eq-device.slint:277`) |
| `wrap` / `unwrap` / `duplicate` | `❏` `⇱` `⧉` |

- **Remove versus close.** They are different actions: remove deletes a
  thing, close dismisses a view. Keep them as two icons, even if they look
  alike.
- **`●` is overloaded**: bypass, pinned, "has a lane", and record arm. Record
  arm keeps a red dot; it's the transport convention. Pinned and "has a lane"
  get their own icons.
- **`♪` means both "sample" and autoplay** (`main.slint:7712`, with no
  tooltip). Give autoplay its own icon and its tooltip.

## The rest

The single-meaning glyphs:

- the transport: `▶` `⏸` `■` `●`;
- preferences: `⚙`;
- zoom: `↔` with `−` and `+`;
- `add`: `+` in 8 sites, one with no tooltip at `main.slint:7743`;
- the browser's row kinds: `♪` `◈` `◇`;
- the menubar's check: `✓`;
- the jack input: `◀`;
- polarity invert: `ø`, which is a Latin letter standing in for the symbol.

The signal-flow `→` between devices is a drawing in the rack
(`device-rack.slint:1077`), so it gets an entry. `→` inside a word label, such
as mixer.slint's `→M`, is step 05's question, since it is part of a label.
`container-device.slint:113-120`'s empty-box hint, "empty — + to fill", keeps
its words. Its comment still points at the rail's `+`, which MOO-218 moved to
the joins, so correct the comment.

Every converted icon keeps or gains a tooltip (`UI_DESIGN.md:980`).

## Who does it

The glyphs sit in every team's markup, so this runs as parallel team work
once the meanings are settled: at most three teams at a time, each converting
only its own files.

- **Interface** (the registry, and the shell): `device-rack.slint`,
  `main.slint`, `toolbar.slint`, `controls.slint`, `menubar.slint`,
  `appearance-dialog.slint`, `plugin-device.slint`.
- **Mixer**: `mixer.slint`, `strip.slint`, `bus-device.slint`.
- **Control**: `modulation-shelf.slint`.
- **Instruments**: `sampler-device.slint`, `mlp8-device.slint`,
  `ds01-device.slint`.
- **Effects**: `layer-device.slint`, `eq-device.slint`.
- **Sequencing**: `channel-rack.slint`.

Interface draws each new entry, since the registry is its file. A team asks
for the meanings it needs, and doesn't draw its own.

## Done when

- `scripts/dupe-audit icon-literal` reports zero.
- Each meaning in the table above has exactly one registry entry.
- No icon lacks a tooltip.
- Rung 4 on antibox, and the snapshots that changed have been looked at.

0.1.8. Milestone: **One family, one meaning.**
