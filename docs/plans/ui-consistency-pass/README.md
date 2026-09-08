# UI consistency pass

Adam's standing list, 2026-09-08, in `~/Documents/laundry_list.md`. Its first
item is a sweep rather than a feature:

> check the ui for unwired controls, make sure all ms knobs taper
> appropriately in ms mode. if they should have sync mode make sure they do.
> all knobs using a ms/tempo knob should use the new style with a small led
> toggle for sync, ranged 4/1 - 1/64 with Ds and Ts. check metering for
> flicker, rendering issues. peak hold, clip latch. check all tooltips. make
> sure statusbar text is in the statusbar, not tooltips. no long tooltips -
> values or labels only.

This directory is the audit that ask produced and the order the findings are
worked in. It sits under `FOCUS.md` step 3 beside `interface-iteration/`,
which is about *reaching* what exists; this one is about the things that are
already reachable and wrong.

## The rule this pass runs on

**A control and the automation lane drawn against it are two views of one
value.** `ParamDescriptor` is the single source of truth for a range and its
curve (`effect.rs:180` says so outright), and every finding in section 1 below
is a face that states a different range, a different curve, or both. That is
the class of bug `drum_slint_agreement.rs` was written for — and the drum
synth is the only device that has such a test.

## The audit

Run against `main` at `f5d12df`. Method, so it can be re-run: every window
callback matched against `on_*` in `lib.rs`; every device callback matched
against its binding at the `main.slint` instantiation; every `tooltip:` site
attributed to the component that owns it, and that component checked for
whether it *renders* the string or only exposes it as `accessible-label`.

### 1. Faces that disagree with their own descriptors

| Where | The face says | The table says |
| --- | --- | --- |
| `sampler-device.slint` amp + filter A/D/R | `norm × 5000 ms`, linear | 1 ms .. 8 s, `Exponential` |
| `mono-device.slint`, `poly-device.slint`, `mlm1-device.slint` A/D/R, Glide | `0 .. 2 s`, linear | 1 ms .. 8 s, `Exponential` |
| `controls.slint` `TimeDivisionKnob` "1/2" | a half note | `DelayTimeDivision::Half.beats() == 0.5` — an eighth |

**The sampler readout is wrong by 2.5x.** `norm_to_time` is `v * MAX_TIME_S`
and `MAX_TIME_S` is 2.0 (`lib.rs:126`), so a fully open Attack is 2000 ms and
the face prints `5000 ms`. Six knobs — Attack, Decay, Release, and the same
three on the filter envelope — have printed a number the engine never had.

**The delay's `1/2` is an eighth note.** `DelayTimeDivision::beats()` returns
0.5 for `Half` while `Quarter` returns 1.0, so the labels and the durations
are off by a factor of four at one entry and correct at the other four.
`ModTimeDivision::beats()`, the 21-entry grid the modulation shelf uses, has
`Half => 2.0`. Two musical grids, one of them wrong.

**Nothing tapers in ms.** An envelope stage whose useful range is 1-50 ms is
laid across 0-2000 ms of linear travel: the first 2.5% of the knob is the part
anyone turns. The descriptors already say `Exponential`; only the faces are
linear. `ds01.rs:315` records the position this pass follows — the taper
belongs to the control surface — and the control surface has not taken it.

### 2. Unwired

- **Every clip indicator in the application is a dead click.** `clip-reset` is
  declared on `ChannelMeter` (`meters.slint:170`) and `MasterMeter` (`:215`),
  forwarded from `ClipIndicator`, and bound *nowhere* — not in `main.slint`,
  not in `mixer.slint`, not in `lib.rs`. The tooltip says "Clipped. Click to
  clear." and clicking does nothing. The latch clears itself after 2 s
  (`meter.rs:9`), which is why nobody noticed: it looks like it worked.
- `piano-focus-requested` (`main.slint:805`) is declared, never emitted, never
  handled.
- `save-error-dismissed` is emitted and unhandled, but harmless: the dialog
  closes itself.

Everything else is wired. 317 window callbacks, 438 window properties; the 28
properties Rust never touches are all view state that belongs to the view.

### 3. Tooltips that are prose

Only twelve components render their `tooltip` as a visible tooltip —
`ToolButton` and its heirs, `LedIndicator`, `StepperField`, `MenuField`,
`NameField`, `ToolModeButton`, `EffectTypeRow`, `BusPicker`, `SampleField`,
`HeldButton`. `ParameterKnob`, `ParameterFader`, `MiniKnob`, `MixerFader` and
`SelectorBank` show the *value* and keep `tooltip` for `accessible-label`, so
the hundred-odd long strings on those are correct where they are.

The ones that do show, and are sentences rather than labels:

| Site | Now |
| --- | --- |
| `main.slint:4176` | "Quantise edits to the grid. Hold the snap-override modifier to invert this for one drag." |
| `modulation-shelf.slint:586` | "Bipolar: swings around the base value" / "Unipolar: base value is the floor" |
| `sampler-device.slint:650` | "Replace the slices with equal divisions of the playback region" |
| `sampler-device.slint:674` | "Snap all four markers to zero crossings now" |
| `sampler-device.slint:908` | "Put the source buffer and its markers back" |
| `drum-device.slint:127`, `sampler-device.slint:692` | "Matching non-zero groups choke each other. 0 is off." |
| `main.slint:1897` | "Length of the selected pattern in 16th-note steps" |
| `main.slint:2381`, `:2386` | "Load a saved whole-channel preset (source + mixer)" |
| `device-rack.slint:238` | "Remove the container, keep what is inside it" |

`hover-hint` on the window is the status bar's live channel and takes priority
over `status-message` (`main.slint:5138`). `compressor-device.slint` is the
only device that threads it out; it is also the only device whose knobs say
the same sentence twice, once as a tooltip and once as a hint.

### 4. Metering

The ballistics are right and tested: instantaneous attack, IEC 60268-18's
20 dB in 1.7 s fall, a 1 s peak hold. No flicker mechanism found — the
segments are driven off a property, not a timer, and the peak-hold marker is
a segment index rather than a second overlay.

The one defect is the clip latch, and it is section 2's: a latch that releases
itself is a slow-blinking light, not a latch.

## The order

1. `01-the-clip-latch-latches.md` — wire `clip-reset`, stop auto-releasing.
2. `02-time-knobs-say-what-they-do.md` — the sampler's 2.5x, the envelope
   taper, and the delay's 21-entry grid on the LED-toggle style.
3. `03-tooltips-are-labels.md` — the twelve sentences above.
4. `04-toolbars.md` — laundry list item 9, which is its own argument.

Items 3, 5 and 7 of the laundry list — growing a pattern by four steps, a
configurable step highlight, and moving the playlist into the pattern pane —
are features rather than corrections and are not in this directory.
