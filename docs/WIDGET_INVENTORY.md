# Widget Inventory

Status: standing list. Last audited 2026-10-04 against `main` at
`4a2aace5`.

UI patterns that exist in `crates/mooloop-ui/ui/` but have no reusable
component behind them. This is the complement of the mockup tool's
UNCATALOGUED group: that group lists widgets that exist and the tool cannot
place, this file lists widgets that do not exist and probably should.

Nothing here is scheduled. It is a menu, ordered by what it would pay back
first, and it is where the answer to "is there already a thing for this?"
should live. Every count below was measured against the tree at the audit
date above; treat them as of that date, not as invariants. Sites are named by
component, by `:=` id, or, in `main.slint`, by the `// ===== ... =====`
section banner they sit under, because line numbers drift with every edit.
`PaneTabs`, `PaneToolbar` and `ViewSlot` in `main.slint` are the pane shell.

**Six globals hold a drag the same way**: what is dragged, where it would
land, and the pointer, for a drag whose grab and landing live in elements
that do not contain each other. They are `RackDrag` and `BrowserDrag`
(`device-rack.slint`), `ChannelDrag` (`channel-rack.slint`), `BranchDrag`
(`layer-device.slint`), `TrackDrag` (`mixer.slint`) and `PaneDrag`
(`main.slint`). That shape is a component this file should be asking for.

Two entries are live bugs rather than duplication, marked **bug** and filed.
They are here because the missing component is why they happened.

**Closed, and the thing to reach for:**

- **`KnobStack`** (`controls.slint`) — the stacked, editable knob a device
  page uses. `UI_DESIGN.md` > Synth faces says why.
- **`PickerChip`** (`controls.slint`) — a chip that opens a list and reports
  the index picked, for an option set past what a cycling chip or a segmented
  bank can carry.
- **`StepperChip`** (`controls.slint`) — one box stepping through a list short
  enough to walk, like a compressor's three voicings; a longer one wants
  `PickerChip`. Shown on `StepperChipSheet`.
- **`Icons` and `Icon`** (`icons.slint`) — reach for them before drawing any
  shape that stands for an action or a kind. `ToolButton.icon` and
  `ToolModeButton.icon` take a registry entry (`icon: Icons.tool-select`).
  Every device kind has one (`Icons.source-kinds[n]`, `Icons.effect-kinds[n]`,
  `Icons.plugin`), shown on `DeviceKindSheet`. A face that needs an icon the
  registry lacks asks Interface for one; it does not spell a path string or a
  glyph of its own, and `scripts/dupe-audit icon-literal` counts the ones that
  still do.
- **`controlled`** is the default on every shared value control;
  `UI_DESIGN.md` > The Ranged-Control Contract has the rule.

---

## 1. `PolylinePlot` — there is no plotting primitive

The highest-value gap. Slint's `Path` elements are static children rather
than a model, so nobody can express one path over a series; the workaround
is one element per adjacent sample pair, and that workaround is hand-rolled
**22 times across 9 files**:

| File | Plots |
| --- | --- |
| `device-displays.slint` | 9, the EQ response (`EqResponseDisplay`) among them |
| `ds01-device.slint` | 3 |
| `plate-device.slint` | 2 |
| `modulation-device.slint` | 2 |
| `sampler-device.slint` | 2 (the waveform editor and the RECORD page's peaks) |
| `buffer-device.slint` | 1 |
| `modulation-shelf.slint` | 1 (in `ModulatorShape`) |
| `reverb-device.slint` | 1 |
| `main.slint` | 1 (the automation lane, under the `NOTES` banner) |

**bug (MOO-508) —** `DisplayPrefs.smooth-curves` is a user preference, and
honouring it means writing the loop twice: once as Rectangles, once as `Path`
segments, under a `!DisplayPrefs.smooth-curves` / `DisplayPrefs.smooth-curves`
pair. That pair is written out four times, **all four in
`device-displays.slint`**. The other eighteen plots ignore the preference:
the automation lane renders only the smooth version, and the rest only the
hard-edged one. Turning smooth curves off changes four displays out of
twenty-two.

One `PolylinePlot { points: [float]; ... }` that owns the pair internally
fixes the bug and deletes the doubled loops. It is the one component on this
list that pays for itself immediately.

## 2. `GridLines` / `TimeRuler` / `FrequencyAxis`

Inline grid and ruler loops are everywhere; `MeterScale` (`meters.slint`) is
the only axis that was ever factored out, and it is dB-only.

`PianoGrid` (`piano-grid.slint`) and `main.slint`'s `NOTES` section hold the
same bar-division loop twice, differing only in the property prefix
(`snap-ticks` vs `piano-snap-ticks`) and two opacity constants; the
`PLAYLIST` section writes a third variant of it. The black-key row test is
duplicated the same way: `piano-grid.slint` and the `NOTES` section share
the identical `mod(note-number, 12)` predicate with different colours, and
they sit side by side on screen — the `main.slint` copy is the keyboard
gutter for the `PianoGrid` instantiated just after it. Two copies of one
predicate, adjacent, is one edit away from the gutter and the grid
disagreeing about which rows are black.

Neither log-frequency plot (the EQ response and the filter response, both in
`device-displays.slint`) has frequency labels at all, because there is no
component that would draw them.

## 3. `MenuPopup` / `ContextMenu`

There are **19 `PopupWindow`s in 9 files**: `main.slint` ×6,
`device-rack.slint` ×3, `controls.slint` ×3, `layer-device.slint` ×2, and
one each in `channel-rack.slint`, `color-picker.slint`, `menubar.slint`,
`mixer.slint` and `toolbar.slint`. Seventeen of them write the same frame by
hand — `Theme.surface`, a `Theme.hairline` border, `Theme.radius-md` — and
the other two pick their own background. What goes inside varies: most use a
padded `VerticalLayout`, and some a `Flickable` or `ScrollView`.

Worse than the boilerplate: `EffectTypeMenu` (`device-rack.slint`) and
`AutomationLaneMenu` (`main.slint`) are two independent takes on the same
thing — a filterable picker over a fixed list — written by different hands
and behaving differently.

## 4. The editors trapped inside `main.slint`

`main.slint` is 9061 lines and exports exactly one component, `MainWindow`.
The editors below are written inline in it, though they have nothing to do
with the window. None of them exists as a named component, which is the
point — the names below are what they would be called:

| Would-be widget | Where |
| --- | --- |
| `StepGrid` | under `// ===== STEPS =====` |
| `PlaylistLane` | `playlist-canvas :=` |
| `PianoKeyboard` gutter | the `mod(note-number, 12)` black-key test, which duplicates the identical one in `piano-grid.slint` |
| `VelocityLane` / `AutomationLane` | under `// ===== NOTES =====`, around `lane-picker :=` |
| `BrowserTree` | under `// ===== Browser sidebar =====` |

None of them can be placed in the mockup tool, snapshot-tested in isolation,
or reused, and the file is too large to navigate. Extracting them is
mechanical; it is only large.

## 5. `WaveformView`

The sampler's `waveform-view` (`sampler-device.slint`) is a full waveform
editor — ruler, region dimming, loop band, playheads, slice markers — sharing
nothing with `SampleTrace` (`device-displays.slint`), which is the read-only
version of the same picture. There is no overview or minimap widget at all,
so anything else that wants to show a buffer starts from zero.

## 6. `DialogShell`

Seven copies of scrim + card + title + footer. `#00000099` is hardcoded in
all seven (`about-dialog.slint`, `export-dialog.slint`,
`appearance-dialog.slint`, `save-preset-dialog.slint`,
`save-error-dialog.slint`, `question-dialog.slint` and
`takes-dialog.slint`), and `z: 200` in three of them; the takes dialog sits
at 205 and the question at 210, so the unsaved-changes question lands over
the takes dialog.
`about-dialog.slint` documents the duplication in a comment rather than
resolving it.

## 7. `modulation-shelf.slint`: one exported component for 1735 lines

`ModulatorShape`, `StepBank`, `ModuleTile`, `OutletChip`, `RouteRow` and
`LedToggle` are all private to the file. The step-column math is written
twice, once in `ModulatorShape`'s step preview and once in `StepBank`. The
shelf is the fifth-largest `.slint` in the tree and almost none of it is
reachable.

`SyncMiniKnob` and the `Divisions` vocabulary have moved out to
`controls.slint`, because the ML-P8's LFO was a second caller.

## 8. Adoption, not authorship

Some of these already exist and are simply not used:

- **`SectionLabel`** (`controls.slint`) is a `Theme.text-md`,
  `Theme.text-faint` caption whose height grows with the text size. It is
  used 63 times in 15 files. Type sizes are `Theme` tokens everywhere now, so
  a hand-rolled copy of it no longer stands out as a literal `9px`: it is a
  plain `Text` with the same two tokens.
- **`GainMath.format-db`** (`gain.slint`) is used at 20 sites in 14 files;
  `+ " dB"` is still appended by hand at **8 other sites**
  (`compressor-device.slint` ×2, `gate-device.slint` ×2,
  `master-comp.slint` ×2, `limiter-device.slint`, `device-displays.slint`).
  Each rounds through `GainMath.format-plain-db` and adds the unit itself.
- **`MenuField`** (`toolbar.slint`) and the std `ComboBox` both ship in
  this tree and do the same job differently.
- **Text entry** has three stacks: `NameField`, std `LineEdit`, and raw
  `TextInput` in a themed `Rectangle`.

## 9. `XYPad`

`DraggablePoint` (`controls.slint`) is interaction-only — it draws nothing.
Its four consumers, all in `device-displays.slint` (the filter response's
handle, the EQ's pass-filter corners and its bands, and the dynamics curve's
threshold), each re-derive the pixel↔normalised mapping and draw their own
handle chrome.

`EqResponseDisplay` (`device-displays.slint`) spreads overlapping points
with about twenty lines of unrolled comparisons, because a fixed seven-band
list has no component to iterate it. Its response curve is the one Rust
publishes, so it sums no bands itself.

## 10. `RoutingGrid` / `NetworkCell` — one call site, deliberately inline

`mlp8-device.slint` draws the ML-P8's oscillator network as a matrix: rows are
sources, columns are destinations, the diagonal is an oscillator on itself,
and each cell is a bipolar amount you drag. `NetworkCell` and `SyncChip` are
private to that file **on this list's own rule** — twenty cells is twenty
instantiations of one component in one device, not two devices sharing one.
A second device drawing a source-by-destination grid is when it becomes a
shared component.

The cell is a `ParameterKnob` with `show-dial: false` rather than a second
draggable control; `UI_DESIGN.md` > Synth faces says why.

## 11. Small, and already admitted in comments

- **`IconToggleButton`** — the layout toggles under `main.slint`'s
  `Status bar` banner say it outright: *"the pinned ToolButton style exposes
  no checkable state, so these are hand-rolled two-state chips"*. Four of
  them, drawn by one loop over `layout-chips`.
- **`Splitter`** — the lower dock's grip (`dock-grip :=`) and the browser
  sidebar grip; the comment on the second, under the `Browser sidebar`
  banner, says it is *"the same moving-origin drag integrator as the dock
  splitter"*.
  - `PlaylistLoopHandle` (`main.slint`, PLAYLIST banner) is a third thin
    grabbable edge, and it is **not** a third splitter. A splitter
    accumulates deltas to move a boundary; this reports an absolute pointer
    position the caller turns into a tick. What the three do share is the
    paint-narrower-than-target rule — 2px drawn in a 9px hit area — and the
    resize cursor that advertises it. If a fourth appears, that rule is the
    component to ask for, not the drag integrator.
- **`ListRow`**, **`TitledPanel`**, **`EmptyState`**, **`TabBar`** — each
  recurs, none is factored. `EmptyState` in particular is inconsistent:
  `audio-preferences.slint` and the automation lane's "No parameter
  selected" (`main.slint`, `NOTES`) phrase and style the same idea
  differently, and most surfaces that can be empty say nothing.

## 12. bug (MOO-509) — the mono and poly LFO glyphs ignore their own selector

The LFO glyphs in `mono-device.slint` and `poly-device.slint` are
byte-identical:

```slint
commands: "M 0 35 C 25 0 50 0 75 35 C 100 70 125 70 150 35 C 168 10 186 12 200 35";
```

A hardcoded cubic, sitting directly beneath the `lfo-wave` `SelectorBank`
that sets `root.lfo-wave`. Picking a saw or a square changes the DSP and
nothing on screen. It is on this list as well as in Linear because the
reason it is wrong twice is that there was no `ModulatorShape` to reach for —
`modulation-shelf.slint` has one, privately (see 7). Both faces are v1
devices on their way out (`docs/plans/archive/poly-v1-mono-mode/`), which is a reason
to fix this by adopting a shared component rather than by editing two
hardcoded paths.

---

## How to use this list

Adding a component is only worth it when it replaces call sites. Before
writing one, count the sites it would fold up; if the answer is one, write
the thing inline and add a row here instead.

New reusable widgets should be exported from their module, which puts them in
the mockup tool's UNCATALOGUED group the same day. Promoting one into the
palette is a row in `ui/mockup-catalog.slint` and a branch in
`MockupSpecimen`.
