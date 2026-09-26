# 01 — the registry

One place every icon lives, one component that draws them, and a check that
counts every icon drawn anywhere else. **Nothing looks different when this
step lands.** The existing icons move in unchanged, and the snapshots stay
byte-identical.

## Write the check first

`AGENTS.md`'s lesson from `bar-arithmetic` and `navigation-sends` is to run a
check against the tree before its fix, or it is decoration. So the first
commit adds **`scripts/dupe-audit icon-literal`**, and it must report the
survey's numbers against today's tree before anything moves. It reports three
kinds of hit:

1. **A path string outside `icons.slint`**: any `icon:` or `commands:` bound
   to a string literal that is an icon. Displays such as knob arcs, envelopes
   and curves build their commands from values, so they are not string
   literals, and the check must not report them. A literal it has to allow
   goes in the allowlist with its reason.
2. **An icon glyph in markup**: a `text:` (or `glyph:`) literal containing a
   character from the symbol blocks the survey found. That means arrows,
   geometric shapes, dingbats, misc technical, `×`, `±`, `−`, `♪` and emoji,
   whether written literally or as `\u{…}`.
   - Letters, digits and ordinary punctuation are not icons.
   - The signal-flow `→` inside a label is a lead, not a failure; decide it in
     step 04.
3. **Two registry entries with the same path string**: the duplicate Adam
   asked the registry to prevent.

Its count on 2026-09-26 should be about 45 glyph sites plus the path literals
in the four sets below. Record the first run's number in `00-status.md`. Like
`unrecorded-edit`, it is a progress bar: step 04 ends it at zero.

## `ui/icons.slint`

```slint
// Every icon, by what it means, on a 16x16 grid. Filled (Adam, 2026-09-26:
// "filled everywhere"). Nothing outside this file spells a path string.
export global Icons {
    out property <string> remove: "M ...";
    // ...
    // By kind number, the numbers `device_kind_to_int` and
    // `effect_kind_index` already send across the markup boundary.
    out property <[string]> source-kinds: [ ... ];
    out property <[string]> effect-kinds: [ ... ];
    out property <string> plugin: "M ...";
}

export component Icon inherits Rectangle {
    in property <string> icon;
    in property <brush> tint: Theme.text;
    in property <length> size: 16px;
    width: self.size; height: self.size;
    Path { viewbox-width: 16; viewbox-height: 16; commands: root.icon; fill: root.tint; }
}
```

That the `commands` of a `Path` can be *bound* to a global's string is
established: `ToolButton.icon` does it today with `ToolIcons.*`. Slint's
documentation says `commands` can be set only in a binding and can't be read
back, and the registry never needs to read it.

## Move the four sets in, unchanged

The existing icons are **outlines** (a 1.3 px stroke, no fill). Step 03
redraws them filled. Until then, `Icon` carries a transitional `outline:
bool` that draws a stroke instead of a fill, so this step moves strings
without changing a pixel. **Step 03 deletes it.**

| Set | Lives in | Owner (`docs/TEAMS.md`) |
| --- | --- | --- |
| `ToolIcons` | `toolbar.slint:14-60` | Interface |
| `StripIcons` | `strip.slint:223-229` | Mixer |
| `SamplerDeviceIcons` | `sampler-device.slint:114-119` | Instruments |
| the EQ face's private shapes | `eq-device.slint:159-164`, `:296` | Effects |
| the layout chips, the dialog's close X, the colour picker's "none" | `main.slint:8298`, `appearance-dialog.slint:1503`, `color-picker.slint:45` | Interface |

Four teams own those files, so this runs as a relay on one branch, the way
MOO-140 did:

1. Interface writes `icons.slint`, `Icon`, `ToolButton` drawing through
   `Icon`, the check, and its own sets.
2. Mixer, Instruments and Effects each point their set at the registry, on
   top of Interface's branch.
3. The branch lands once.

The EQ's two sets, strip and face, are **not** merged here. They are two
drawings of the same meanings, and choosing one is a visual change, so it
belongs to step 03. Both move in, named for now `eq-*` (strip) and
`eq-face-*`. The duplicate check will not flag them, because the strings
differ.

`ConsoleButton`'s sine is built from `MoveTo`/`CubicTo` elements, not a
string. Leave it until step 03 decides whether it is an icon.

## Measure before committing to Paths

The recent performance work found FemtoVG paying per element per frame
(MOO-256, MOO-258). So before anything is converted, measure on the laptop in
headless sway, using MOO-261's recipe (`MOOLOOP_PROFILE_UI`, `perf
--call-graph lbr` on the UI thread):

- a test window of **200 icons** repainting every frame, drawn three ways:
  as filled `Icon` Paths, as glyph `Text`, and as SVG `Image`s with
  `colorize` (SVG is already in the build: `resvg` is in the lock);
- the saved layout of `housey-dropout-factory`, before and after `ToolButton`
  draws through `Icon`.

**The gate.** Paths go ahead if, at the number of icons a real layout shows
(count them on housey's saved layout, probably 60 to 100), they add less than
0.5 ms to a frame against today's glyphs. The frame is 18.4 ms after MOO-268.
If they add more, `Icon` draws an `Image` instead. The registry's names stay
as they are; only the storage behind them changes, from strings to `@image-url`
SVGs. Record the numbers in `00-status.md` either way.

## Also in this step

- **`docs/TEAMS.md`:** add `ui/ui/icons.slint` to Interface's files, and a row
  under *Seams that have an owner now*. Interface owns the registry and the
  drawings. A team that needs an icon on its face asks for it, the way a face
  asks the shell for a rail.
- **`docs/WIDGET_INVENTORY.md`:** `Icon` and `Icons` are the entry to reach
  for, and `ToolButton.icon` names a registry entry.
- **`ui/tests/icon_registry.rs`** reads `icons.slint`, the production markup,
  not a copy. It checks that:
  - `source-kinds` has one entry per position of `SOURCE_KINDS_IN_PICKER_ORDER`
    and `SourceKinds.labels`;
  - `effect-kinds` has one per `EffectKind::ALL`;
  - no two entries are equal;
  - every entry is used somewhere in `ui/`. An unused icon is a lead: report
    it, don't fail on it.

  The kind tables stay empty strings in this step. Step 02 fills them.

## Done when

- `icons.slint` holds every path string in the program, and `Icon` draws them.
- `scripts/dupe-audit icon-literal` reports no path literal outside the
  registry, and its glyph count is recorded.
- Every snapshot is byte-identical to `main` before the step.
- The cost is measured and the gate is decided, with the numbers in
  `00-status.md`.
- Rung 4 on antibox with `--locked --no-fail-fast`.

Owner: **Interface**, with Mixer, Instruments and Effects moving their own sets.
