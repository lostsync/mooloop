# UI consistency pass status

Worked 2026-09-08 on `fix/ui-consistency-pass`, in the order `README.md` set.

## Step 01 — the clip latch latches

`clip-reset` was declared on `ChannelMeter` and `MasterMeter`, forwarded from
`ClipIndicator`, and bound nowhere. Every clip light in the application was a
dead click, and the reason nobody had noticed is that the latch released
itself after two seconds — so it went out on its own and looked like it had
obeyed. A latch with a timer on it is a lamp that is off by the time anyone
looks at the meter.

The mixer strips turned out to have neither half of the metering.
`MixerStripRow` carried only `left-db` and `right-db`, so `ChannelMeter` was
handed the level as its own held value — a peak marker pinned to the level it
exists to lag — and `clipping` was never passed at all. Both were already
computed per bus in the pump and dropped.

## Step 02 — the time controls

Three disagreements, one shape: a range written twice.

- The sampler's envelope readout was **wrong by 2.5x**, and its resting values
  disagreed with the table as well.
- **Nothing tapered.** Twenty-one envelope stages across five faces laid a
  1 ms–8 s ratio range across linear travel, so the useful part of every one
  was the first 2.5% of the knob.
- **The delay's `1/2` was an eighth note**, and the "Reverse Wash" factory
  preset has been playing eighth-note windows under a description that says
  half-note.

`EnvelopeEditor` was the structural cause of the second: it took seconds from
four callers and normalised values from a fifth and drew both as `0..1`, which
its own `time-curve` warp and the synth faces' `/2` and `*2` were both
workarounds for. It takes a range and a curve now.

`TimeDivisionKnob` is `SyncMiniKnob`'s lamp at a full knob's size, over the
shared twenty-one-entry grid, 92px where it was 230.

**Known limit, recorded rather than fixed:** the delay line is sized for
`DELAY_MAX_TIME_MS` (2 s), so a division asking for longer is clamped. The
label turns amber when it is, which is the honest version of what the audio
was already doing silently. Raising the ceiling means a larger ring per delay
instance and is a memory decision, not a UI one.

`slint_face_agreement.rs` (was `drum_slint_agreement.rs`) covers all
twenty-one stages.

## Step 03 — tooltips are labels

**Forty-six rendered tooltips were sentences, and one is left** — "Repeat the
marked section (L)", a label plus its shortcut, which is the one form Adam's
rule allows. The commit message says twenty-six: that was a recount taken
after the first twenty had already been shortened, and it undercounts.

The reason the rule was not kept is that explaining a control needed a
`hover-hint` property threaded up through its device face, and exactly one
face ever grew that plumbing.

`StatusHint` is a global on the pattern `Motion.gesture-active` established:
a control publishes its own sentence, the status bar reads it, and it works at
any depth including from inside a `PopupWindow`. Knobs, faders and
`SelectorBank` publish their existing `tooltip`, since they never rendered it;
button-shaped controls gained a separate `hint`.

It also closed a gap `CURRENT.md` was carrying — the stretch toggle that stays
lit while stretch is bypassed — whose recorded fix was the threading this
replaced.

**What it did not check, found 2026-09-08.** The sweep asked what each
`tooltip:` string *said*; it never asked whether the tooltip a widget renders
resolves to anything at all. `KnobStack` takes the formatted value for its own
field, turns the inner `ParameterKnob`'s readout off and never hands the
string back, so all 85 ML-P8 and DS-01 knobs rendered `@markdown("")` — an
empty bubble, and an empty `accessible-value` with it. It had been that way
since the component was written; the audit's own component list classified
`ParameterKnob` as a value-shower to be skipped, which is exactly the class
this lives in. It also missed `main.slint`'s step-grid prose, because that
`Tooltip` sits inline on a `TouchArea` rather than on one of the twelve
components the list enumerated.

Both are fixed on `fix/blank-value-tooltips`, along with the guard that was
missing: `tests/knob_value_text.rs` requires any component taking a
`display-text` to pass it on. **A re-run of `README.md`'s sweep should check
presence as well as content, and should walk `Tooltip` instantiations rather
than a list of components.**

## Step 04 — the toolbars

Adam: *"reorganize the toolbars. they're sort of a mess. that one I was just
talking about has no reason to exist."*

- **The 26px pane strip is gone.** Its switcher leads the toolbar row whose
  contents it decides, and its permanent sentence of chrome is that
  switcher's status-bar hint.
- **That row follows the pane.** With the mixer up, the pattern selector, the
  step tools and the pattern length are not drawn, because they do not act on
  anything visible.
- **Snap was three controls for one setting**, in two halves of the window:
  a `MenuField` in the toolbar that asked `editor-page` which of two indices
  it meant, a toggle-and-`ComboBox` in the piano roll, and a
  label-and-`ComboBox` in the playlist. It is now one control in each
  editor's own header, both spelled the same way.
- **The source switcher is a `PickerChip`.** Adam: *"a toolbar button per
  instrument doesn't really make sense past prototyping."* Eight segments cost
  384px and grew by 48 with every instrument.
- Both pane switchers are `SegmentedControl`s, which the lower dock's three
  `FlatButton`s were not.

## Step 05 — two of the small features, since the toolbar was open

- **Pattern length moves a beat at a time.** Adam: *"maybe shift click moves
  by 4 or something, or a hotkey to grow it by 4."* Both: `StepperField`
  gained a `coarse-step` that Shift takes on its arrows and its wheel, and
  `pattern.length-grow` / `pattern.length-shrink` are in the registry, so the
  chord is rebindable like every other. `STEPS_PER_BEAT` is in
  `mooloop-core` rather than a four written in two places.
- **The rack grid's accent is a setting.** Adam: *"its 1 bright 3 dim
  static."* It was `mod(i, 4) == 0`, which is a 4/4 sixteenth grid and
  nothing else; GROUP in the work-surface toolbar offers 2, 3, 4, 6, 8, 12
  and 16. View state, not persisted, which is how the two snap indices beside
  it are already treated.

## Step 06 — the last control that wanted sync and did not have it

**The modulation effect's Rate.** Chorus, flange, phaser, ensemble and ADT
all run off one LFO at 0.02-12 Hz, and it was free-running only. Every other
candidate was checked and does not want sync: a compressor attack, a gate
hold, a limiter release, a reverb predelay and a buffer crossfade are all
absolute times, and the sampler's stretch and both modulator racks already
have it.

Three things had to move, and the shape of each is the interesting part:

- **`update_tempo_synced_delay_times` stopped being about delays.** It is
  `update_tempo_synced_effects` and returns the *parameter id* alongside the
  value, because two kinds of effect answer to a tempo change now and a
  caller that assumed one id would have written a rate into a delay time.
- **`EFFECT_ROW_PARAMS` went from eight to ten**, and the last two are
  reserved rather than free. `EFFECT_ROW_DESCRIPTOR_PARAMS` is what a table
  may fill; `p8` and `p9` carry what a device keeps *beside* its parameters
  — a sync flag and a division, neither of which is continuous or
  addressable. The delay's pair fitted inside its six descriptors and had
  been living at `p6`/`p7`; the modulation effect has eight of its own and
  could not. The reservation makes that a rule rather than an accident.
- **The clamp lives in `mooloop-core`.** `synced_rate_hz` resolves a division
  against `MODULATION_MIN_RATE_HZ`/`MAX`, which the descriptor now reads too,
  so the ceiling is written once. A 64th triplet at 120 BPM asks for 48 Hz
  against a 12 Hz device.

`SyncLamp` came out of it: the "O." gesture had two implementations and was
about to have three, so it is one component with three callers — the
modulator rack's sync knobs, the delay's time, and this.

## Not in this pass

**Laundry-list item 7 — moving the playlist into the pattern pane.** A
feature rather than a correction, and the one that wants a plan of its own
before any of it is built. In Adam's words: keep the current single-pane
mode, but with the playlist up there too, *or* split the pane so a pattern
and the playlist can be read at the same time with the piano roll open
below. That is a layout question, and `interface-iteration/` is the right
home for it.

Step 04 of this pass is a down payment on it and should be read first: the
work surface's toolbar already switches on which pane is showing, which is
the mechanism a third pane would join rather than a thing it would have to
invent.
