# Widget Inventory

Status: standing list. Last audited 2026-09-04 against `main` at
`c1765e9`.

UI patterns that exist in `crates/mooloop-ui/ui/` but have no reusable
component behind them. This is the complement of the mockup tool's
UNCATALOGUED group: that group lists widgets that exist and the tool cannot
place, this file lists widgets that do not exist and probably should.

Nothing here is scheduled. It is a menu, ordered by what it would pay back
first, and it is where the answer to "is there already a thing for this?"
should live. Every count below was measured against the tree at the audit
date above; treat them as of that date, not as invariants. Line numbers in
`main.slint` in particular drift with every edit to that file — prefer the
`// ===== ... =====` section banners it carries, which are named below.

Two entries are live bugs rather than duplication, marked **bug**. They are
here because the missing component is why they happened.

---

## 1. `PolylinePlot` — there is no plotting primitive

The highest-value gap. Slint's `Path` elements are static children rather
than a model, so nobody can express one path over a series; the workaround
is one element per adjacent sample pair, and that workaround is hand-rolled
**17 times across 8 files**:

| File | Plots |
| --- | --- |
| `device-displays.slint` | 8 (`:49`, `:77`, `:140`, `:166`, `:263`, `:306`, `:489`, `:498`) |
| `plate-device.slint` | 2 (`:59`, `:79`) |
| `modulation-device.slint` | 2 (`:17`, `:25`) — the file is 134 lines after the 2U rework, and both plots survived it |
| `eq-device.slint` | 1 (`:81`) |
| `modulation-shelf.slint` | 1 (`:231`) |
| `reverb-device.slint` | 1 (`:77`) |
| `sampler-device.slint` | 1 (`:489`) |
| `main.slint` | 1 (the automation lane, under the bottom-dock banner) |

**bug —** `DisplayPrefs.smooth-curves` is a user preference, and honouring it
means writing the loop twice: once as Rectangles, once as `Path` segments,
under a `!DisplayPrefs.smooth-curves` / `DisplayPrefs.smooth-curves` pair.
That pair is written out four times, **all four in `device-displays.slint`**
(`:139/:150`, `:165/:176`, `:262/:273`, `:497/:508`). The other thirteen
plots ignore the preference entirely — `eq-device.slint:81`,
`modulation-shelf.slint:231`, `plate-device.slint:59` and `:79`,
`modulation-device.slint:17` and `:25`, `reverb-device.slint:77`,
`sampler-device.slint:489` render only the hard-edged version, and the
automation lane in `main.slint` renders only the smooth one. Turning smooth
curves off changes four displays out of seventeen.

One `PolylinePlot { points: [float]; ... }` that owns the pair internally
fixes the bug and deletes about 200 lines. It is the one component on this
list that pays for itself immediately.

## 2. `GridLines` / `TimeRuler` / `FrequencyAxis`

Inline grid and ruler loops are everywhere; `MeterScale` (`meters.slint:20`)
is the only axis that was ever factored out, and it is dB-only.

`piano-grid.slint:269` and `main.slint:3902` are the same bar-division loop
twice, differing only in the property prefix (`snap-ticks` vs
`piano-snap-ticks`) and two opacity constants; the playlist writes a third
variant of it (`main.slint:2336`). The black-key row test is duplicated the
same way: `piano-grid.slint:261` and `main.slint:3528` share the identical
`mod(note-number, 12)` predicate with different colours, and they sit side by
side on screen — the `main.slint` copy is the keyboard gutter for the
`PianoGrid` instantiated 69 lines earlier at `:3597`. Two copies of one
predicate, adjacent, is one edit away from the gutter and the grid disagreeing
about which rows are black.

Neither log-frequency plot (`eq-device.slint`, the filter response in
`device-displays.slint`) has frequency labels at all, because there is no
component that would draw them.

## 3. `MenuPopup` / `ContextMenu`

The same shell — `PopupWindow` + `Theme.surface` + 1px border +
`Theme.radius-md` + `VerticalLayout { padding: 4px; }` — is written **10
times**: `main.slint` ×5, `device-rack.slint` ×2, and once each in
`menubar.slint`, `mixer.slint`, `toolbar.slint`.

Worse than the boilerplate: `EffectTypeMenu` (`device-rack.slint:49`) and
`AutomationLaneMenu` (`main.slint:215`) are two independent takes on the same
thing — a filterable picker over a fixed list — written by different hands
and behaving differently.

## 4. The editors trapped inside `main.slint`

`main.slint` is 4474 lines and exports exactly one component. Roughly 800 of
those lines are widgets that have nothing to do with the window. None of them
exists as a named component, which is the point — the names below are what
they would be called:

| Would-be widget | Where |
| --- | --- |
| `StepGrid` | under `// ===== Channel rack: one step-grid row per channel =====` |
| `PlaylistLane` | `playlist-canvas :=` |
| `PianoKeyboard` gutter | the `mod(note-number, 12)` black-key test, which duplicates the identical one in `piano-grid.slint` |
| `VelocityLane` / `AutomationLane` | under `// ===== Bottom dock: sampler and note editors =====`, around `lane-picker :=` |
| `BrowserTree` | under `// ===== Browser sidebar =====` |

None of them can be placed in the mockup tool, snapshot-tested in isolation,
or reused, and the file is too large to navigate. Extracting them is
mechanical; it is only large.

## 5. `WaveformView`

`sampler-device.slint:448` is a full waveform editor — ruler, region dimming,
loop band, playheads, slice markers — sharing nothing with `SampleTrace`
(`device-displays.slint:4`), which is the read-only version of the same
picture. There is no overview or minimap widget at all, so anything else that
wants to show a buffer starts from zero.

## 6. `DialogShell`

Five copies of scrim + card + title + footer. `#00000099` is hardcoded in all
five (`about-dialog.slint:10`, `export-dialog.slint:13`,
`appearance-dialog.slint:684`, `save-preset-dialog.slint:14`,
`save-error-dialog.slint:31`), and `z: 200` in three of them.
`about-dialog.slint` documents the duplication in a comment rather than
resolving it.

## 7. `modulation-shelf.slint`: one export for 1640 lines

`ModulatorShape`, `StepBank`, `ModuleTile`, `RouteRow`, `SyncMiniKnob` and
`LedToggle` are all private to the file. `StepBank`'s column math is
duplicated inside it (`:293` and `:363`). The shelf is the second-largest
`.slint` in the tree and none of it is reachable.

## 8. Adoption, not authorship

Some of these already exist and are simply not used:

- **`SectionLabel`** (`controls.slint:215`) is a 9px `Theme.text-faint`
  caption. It is used in 8 files; a bare 9px `Text` appears **61 times across
  20 files**, and 8px captions another 65 times across 16. Most of those are
  the same caption, hand-rolled.
- **`GainMath.format-db`** (`gain.slint`) is used 5 times; `+ " dB"` is
  assembled by hand at **9 other sites** (`compressor-device.slint` ×3,
  `gate-device.slint` ×2, `limiter-device.slint` ×2, `eq-device.slint`,
  `device-displays.slint`), each rounding for itself.
- **`MenuField`** (`toolbar.slint:172`) and the std `ComboBox` both ship in
  this tree and do the same job differently.
- **Text entry** has three stacks: `NameField`, std `LineEdit`, and raw
  `TextInput` in a themed `Rectangle`.

## 9. `XYPad`

`DraggablePoint` (`controls.slint:9`) is interaction-only — it draws nothing.
Its three consumers (`device-displays.slint:189`, `device-displays.slint:580`,
`eq-device.slint:90`) each re-derive the pixel↔normalised mapping and draw
their own handle chrome.

`EqResponseDisplay`'s point-overlap spreading is 15 lines of unrolled
comparisons (`eq-device.slint:39-53`), sitting next to another 8 unrolled
lines of band summing (`:54-61`), because a fixed seven-band list has no
component to iterate it.

## 10. `RoutingGrid` / `NetworkCell` — one call site, deliberately inline

`mlp8-device.slint` draws the ML-P8's oscillator network as a matrix: rows are
sources, columns are destinations, the diagonal is an oscillator on itself,
and each cell is a bipolar amount you drag. `NetworkCell` and `SyncChip` are
private to that file **on this list's own rule** — twelve cells is twelve
instantiations of one component in one device, not two devices sharing one.

It is recorded because there is a plausible second caller. Step 04 of
`docs/plans/poly-synth-v2/` adds ML-P8's internal modulation routes, and a
source-by-destination grid is the same picture with a different vocabulary;
if that step reaches for this, it becomes a shared component then rather
than speculatively now.

Worth keeping whichever way it goes: the cell is a `ParameterKnob` with
`show-dial: false` rather than a second draggable control. Arming a
modulation source changes what every gesture *means*, and a hand-rolled cell
would have been a second implementation of that contract. `show-dial` joins
`show-label` and `show-value-text`, which existed for the same reason.

## 11. Small, and already admitted in comments

- **`IconToggleButton`** — `main.slint:4324` says it outright: *"the pinned
  ToolButton style exposes no checkable state, so these are hand-rolled
  two-state chips"*. Two of them, in the browser sidebar.
- **`Splitter`** — the lower dock's grip (`dock-grip`, under the bottom-dock
  banner) and the browser sidebar grip; the comment at `main.slint:4023` says
  the second is *"the same moving-origin drag integrator as the dock
  splitter"*.
- **`ListRow`**, **`TitledPanel`**, **`EmptyState`**, **`TabBar`** — each
  recurs, none is factored. `EmptyState` in particular is inconsistent:
  `audio-preferences.slint:74` and `main.slint:3896` phrase and style the
  same idea differently, and most surfaces that can be empty say nothing.

## 12. bug — the mono and poly LFO glyphs ignore their own selector

`mono-device.slint:188` and `poly-device.slint:192` are byte-identical:

```slint
commands: "M 0 35 C 25 0 50 0 75 35 C 100 70 125 70 150 35 C 168 10 186 12 200 35";
```

A hardcoded cubic, sitting directly beneath the `lfo-wave` `SelectorBank`
that sets `root.lfo-wave`. Picking a saw or a square changes the DSP and
nothing on screen. It is here rather than in an issue because the reason it
is wrong twice is that there was no `ModulatorShape` to reach for —
`modulation-shelf.slint:231` has one, privately (see 7). Both faces are v1
devices on their way out (`docs/plans/poly-v1-mono-mode/`), which is a reason
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
