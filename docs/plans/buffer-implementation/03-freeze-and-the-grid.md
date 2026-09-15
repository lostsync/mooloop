# Task: Buffer — Freeze, the Musical Grid, and a Face Worth Playing

`01-the-whole-thing.md` built the engine. `02-control-and-modulation.md`
wired it to automation and modulation. This document is the third step, and
it is the one that changes what the device *is* rather than what it is
connected to.

Read `docs/BUFFER_ENGINE.md` for the hypothesis and `docs/MODULATION.md` for
the destination rules. Where this document and `BUFFER_ENGINE.md` disagree,
this one wins — same relationship `01` had to it.

## The shape question is answered

`FOCUS.md` and `BUFFER_ENGINE.md` both carry a standing instruction not to
build the Buffer workflow until Adam settles whether the device should be an
ordinary insert at all — his 2026-08-30 position was that an insert was
"partly the wrong call" and that the intent was the end of a device rack with
its own sequencing lane.

**Settled 2026-09-15: Buffer stays a device.** Adam, on the realtime-sampler
framing below: *"it puts some of my doubts about a device-based implementation
to rest."* The rack-end placement and a dedicated sequencing lane are not
ruled out and are not this plan; nothing here forecloses them, because
everything here is published parameters, and a lane would drive exactly those.

Update the banner at the top of `BUFFER_ENGINE.md` and the "raise the shape"
paragraph in `FOCUS.md`'s step 2 when step 1 below lands.

## The user model

The device is not an effect with buffer-related controls. It is a small
realtime sampler lurking behind the live audio:

> Audio is always flowing through it and the last N bars are always being
> recorded. Hit FREEZE and that recent history becomes a sample. Manipulate
> the sample with control signals. Release FREEZE and return to live while
> the rolling buffer starts updating again.

Recording is invisible. There is no arm, no record gesture, no file dialog —
which is exactly the success test `BUFFER_ENGINE.md` already sets.

## What this breaks, and why Rate is not optional

`buffer_device.rs:50` states the head model plainly:

> A platter under the hand. The read head chases `target`, and the speed it
> closes the gap at *is* the playback rate. Position is the input; rate is
> derived. Driving rate directly instead would be a jog shuttle, not a
> turntable.

And `Scrub::offset_frames` recomputes the target as `write_head -
offset_frames` **every frame**. A held Offset plays forward at unity *because
the writer is moving and drags the target along with it*.

So: stop the writer and the target goes static, the head closes on it, the
speed falls to zero, and `SCRUB_MUTE_RATE` mutes it by design. **Freeze, added
naively, is a tape stop into silence.**

`01-the-whole-thing.md`'s "the writer never stops" and `02`'s "there is
deliberately no rate parameter — a rate would contradict a position" were both
correct, and they were correct *together*: the writer was the time base. Take
the writer away and something has to supply one explicitly.

Freeze and Rate are therefore one decision, not two. Do not take one without
the other.

## Position, Rate and Jump: the arbitration rule

The three would otherwise fight over head velocity. One rule settles it:

- **Rate is motion.** Signed free-run velocity in the buffer, `-4..+4`,
  `0` = held. It is what the head does when nothing else is talking to it.
- **Position is an edit.** Writing it arms a chase target; the head closes on
  the target at the existing turntable behaviour, and the target is *released
  once reached*, after which free-run at `Rate` resumes from there. A
  continuous stream of Position writes is therefore a scrub — the current
  behaviour, unchanged — and a static Position leaves `Rate` in charge without
  yanking the head back to a stale target.
- **Jump is an immediate relocation** to the current Position with no chase,
  crossfaded by `Xfade`. It is the hard edit, and it is what makes a
  sequencer's `Position: 0% / 50% / 25% / 75%` with a trigger per step
  deterministic while playback continues normally in between.

Carry the existing sub-frame guard across: a Position write moving the target
less than one frame arms nothing, mirroring `set_offset_beats`'s
`offset_frames < 1.0` case.

**State this budget honestly in the face's tooltips and anywhere it is
documented.** Control rate is 32 frames (~1.5 kHz at 48k; a do-not-relitigate
decision from `02`), the chase has a ~5 ms time constant
(`sample_rate / 200.0`), and `MAX_SCRUB_RATE` already clamps at 4.0. So
`ramp → Position` is a player, `triangle → Position` is ping-pong with rounded
turns, and `noise → Position` is granular scramble — but none of it is
sample-accurate, and the ±4x ceiling is the one already in the code.

## Position is a coordinate change, not a rename

`Offset` is *beats behind a moving writer*: relative, live-anchored, `0` =
live, range `0..16`. `Position` is *where in retained memory*: absolute while
frozen.

If one id changes meaning with freeze state, every `AutomationLane` — a
normalized `0..1` curve against a `ParamAddr` — means two different things
depending on a control the lane cannot see. That is the `eq-v2` step 01 bug
in a new costume: one automatable id standing for two settings of different
semantics.

The definition that works in both states:

> **Position is normalized `0..1` over the buffer length, where `0` is the
> oldest retained sample and `1` is now. Freeze latches what "now" means.**

Live, it tracks the writer, so it is the current Offset in new units. Frozen,
"now" is the freeze instant, so it is absolute. Same units, same lane
semantics, no mode. The direction is inverted relative to `Offset` — `0` is
the old end — which is the right way round, because a rising ramp is then
forward playback.

`Offset` (id 0) is retired rather than redefined: drop it from the descriptor
table and migrate saved lanes and automation to id 2 with `pos = 1 - beats /
history_beats`, following the `DelayTimeDivision` → `ModTimeDivision`
migration in `mooloop-project/src/lib.rs:2698` as the shape to copy. The
never-renumber rule is why the id is retired and not reused.

## The grid already exists — use it, and do not add a D/T toggle

Adam asked for a quantize grid of 2 bars, 1 bar, 1/2, 1/4, 1/8, 1/16 with
dotted and triplet variants, "maybe a toggle off to the side for D/T."

**`ModTimeDivision` in `mooloop-core` is already that grid**, and more: 21
entries from `4/1` down to `1/64T`, with dotted and triplet interleaved in
pitch order, plus `beats()`, `seconds()`, `time_ms()` and `from_index()`. The
Slint side is spelled once in the `Divisions` global (`controls.slint:1804`),
whose own comment says why: *"none of them gets to invent a second
spelling."* `TimeDivisionKnob`, `SyncMiniKnob` and `SyncLamp` are the widgets.

So the recommendation is **no D/T toggle**. A base-division knob plus a
dotted/triplet modifier would be a second spelling of a grid the codebase
deliberately unified after a delay and an LFO disagreed about what "1/2" was
worth by a factor of four. The cost of the interleaved grid is that sweeping
from `1/4` to `1/16` passes through five intermediate steps; on a stepped knob
that is a feature, not a tax.

*Adam: this is the one place the plan argues against what you asked for.
Overrule it and the fallback is a 7-entry base knob plus a tri-state
straight/dot/triplet control that composes into a `ModTimeDivision` index —
never a parallel enum.*

The requested range is a subrange of the existing one: `2/1` is two bars,
`1/1` is one bar. Keep the house labels on the knob (`1/1`, not `1 BAR`) and
let the BBT readout below it carry the bar-thinking.

## Bars:Beats:Ticks

Adam asked for BBT throughout, and the pieces are half-built: `Ppq(96)` and
`Ticks` in `mooloop-core/src/time.rs`, a `PositionReadout` assembled by hand
in `mooloop-session/src/engine.rs:712`, and a padded `bar:beat:tick` formatter
living in `toolbar.slint:483`. That is a value stated in two places, which is
what `docs/workflows/rust-slint-boundary/` exists for.

Consolidate: a `BarsBeatsTicks` type in `time.rs` that both `PositionReadout`
and the Buffer face feed from, and one Slint formatting component lifted out
of `toolbar.slint` into `controls.slint`.

**Positions are one-based, durations are zero-based.** `engine.rs:755` already
asserts the one-based readout for transport position. A one-bar Length must
read `1:0:0`, not `2:1:0`, so the duration path is a distinct constructor and
wants its own test. Getting this wrong is the obvious way to ship a confusing
face.

## Quantization

Two different things, and only one of them is optional.

**Freeze quantization is on/off with a grid**, as asked, defaulting to on at
`1/1`. It exists because "pressing Freeze sounds like almost nothing happened"
is only true when it is true: at the freeze instant the head sits at the write
position, so continuing forward wraps immediately into the *oldest* samples —
you hear N bars ago, not now. That is seamless exactly when the material is
periodic at the buffer length, which is what freezing on the bar line of a
loop gives you. Quantized freeze is not a nicety; it is what makes the
headline behaviour true. Unfreeze quantizes the same way.

While quantization is pending, the device is still live and the face must show
that a freeze is armed. A second press before the boundary cancels it.

**Length quantization is not optional.** Adam: *"quantized length should be
all of the time, pretty much."* Take the "pretty much" as no: a free length
and a grid length behind one automatable id is the `eq-v2` arity bug again,
and there is no `SyncLamp` on this knob. Length is a `ModTimeDivision` index,
always, defaulting to `1/1` — one bar.

That constraint pays for itself: it also answers what modulating Length even
means. A stepped index makes `envelope → Length` sweep `1/4 → 1/8 → 1/16`,
which is the gesture anyone wants, where a continuous length in beats would be
a smear.

## Parameter set

Descriptor-addressed, therefore automatable and modulatable, per
`MODULATION.md`. Ids are stable and never renumbered.

| id | Name | Unit | Range | Default | Notes |
|---|---|---|---|---|---|
| 0 | *(retired: Offset)* | — | — | — | Out of the table; migrated to id 2 |
| 1 | Crossfade | ms | 0.5..50 exp | 2.5 | Unchanged |
| 2 | Position | % | 0..1 lin | 1.0 | 0 = oldest retained, 1 = now/freeze point |
| 3 | Rate | x | -4..4 lin | 1.0 | Signed; 0 = held, and gain follows speed down as it already does |
| 4 | Length | grid | 0..20 step | 2 (`1/1`) | `ModTimeDivision` index |
| 5 | Loop | bool | 0..1 | 0 | Wrap within the active window |
| 6 | Freeze | bool | 0..1 | 0 | Hysteresis ±0.05 around 0.5, or a modulator resting near the threshold chatters the writer |
| 7 | Jump | trigger | 0..1 | 0 | Rising edge; samples Position |
| 8 | Quantize | bool | 0..1 | 1 | Applies to freeze and unfreeze |
| 9 | Quant Grid | grid | 0..20 step | 2 (`1/1`) | `ModTimeDivision` index |

`bars` stays out, for the reason already written at `effect.rs:2400`: resizing
the ring reallocates.

**Window anchoring carries forward from `02`'s hard-won gotcha:** the active
window extends *forward* from the anchor for a forward head and *backward* for
a reverse one. Pointing a reverse window at frames the writer has not reached
plays silence, and it is easy to reintroduce.

## The face

Ten parameters do not fit 1U, and `UI_DESIGN.md` is explicit that a face which
outgrows its width takes another unit and does not compress. **Buffer becomes
2U** in `effect_kind_units` (`mooloop-ui/src/lib.rs`).

```text
┌──────────────────────── BUFFER ────────────────────────┐
│  LIVE ●    HISTORY [ 2 BARS ]              [ FREEZE ]  │
│  ┌─────────────────────────────────────────────────┐   │
│  │████████████ waveform ███████████████████████████│   │
│  │        |<------ active window ------>|    ▲     │   │
│  └─────────────────────────────────────────────────┘   │
│  POSITION    LENGTH     RATE    XFADE   QUANT  ◉ 1/1   │
│  1:2:48       1:0:0     +1.00   2.5ms                  │
│  [ LOOP ]    [ JUMP ]   [ REV ]   [ STUT ]             │
└────────────────────────────────────────────────────────┘
```

The waveform earns its place here in a way it does not on a synth, because the
memory *is* the instrument. `sampler-device.slint:448` is already a full
waveform editor with ruler and region dimming (`WIDGET_INVENTORY.md` §5), so
the widget is not new work; the telemetry payload is. The collision counter
publishes a scalar into the telemetry bank, and a downsampled peak array is a
new shape on that lock-free path. Peaks are cheap to accumulate per bin on the
audio thread; publishing them without allocating is the actual task.

Display gestures set published parameters and nothing else — click sets
Position, drag sets the window, dragging the head while frozen scrubs. No
UI-only operations.

`COLLISIONS` comes off the face. It is telemetry occupying the most valuable
real estate, and Freeze removes most of what it was reporting: no writer, no
collision. It survives as a flash at the forced-return point on the waveform,
which is live-mode information anyway.

**The buttons become macros over published parameters.** `REV` multiplies
Rate by −1. `STUT` is Loop on + Length + a retrigger, released back. Nothing
the mouse can reach should be unreachable from a lane or a modulator — the
face's own comment already calls these three a debug surface standing in for
the real control layer.

Two things that dissolve with them, and need replacing rather than dropping:

- **`BufferEvent` stays the internal transport.** Its atomicity is
  load-bearing: offset, rate, window, repeat, duration and crossfade land
  together so a sequenced slice is exact. Macros must *compose one event*, not
  emit three parameter writes that happen to share a control tick.
- **"A running gesture outranks the offset" is the current precedence rule**
  (`buffer_device.rs:265`). Once gestures are parameter writes it has nothing
  to rank. Replace it with last-writer-wins and say so in the doc comment,
  or a held STUT and a drawn Position lane will fight silently.

## Build order

Each step is audible on its own and leaves `main` playable.

### 1. Rate, and a head that runs without a writer

DSP only, no UI beyond a debug knob. Add signed free-run velocity to the head
alongside the chase, implement the Position/Rate/Jump arbitration above, and
keep the existing scrub behaviour bit-identical when Rate is 1.0 and the
writer is running.

Done when: a detached head plays at a commanded rate with the writer stopped
in a test, `Rate = 0` holds silently rather than repeating a DC sample, and
every existing buffer test still passes unchanged.

### 2. Freeze

Gate the writer, latch the Position reference at the freeze instant, crossfade
both transitions by `Xfade`. Lock `HISTORY` while frozen — `bars` resizes
off-thread and would destroy the thing being played. A gesture running at the
moment of freeze keeps running against the frozen ring.

Done when: freezing a running loop and doing nothing else is inaudible at a
bar boundary, unfreezing returns to live without a click, and collisions stay
at zero for the whole frozen span.

### 3. Position replaces Offset

Descriptor table change, the id-2 addition, id-0 retirement, and the project
migration. A project saved before this step must load with its lanes pointing
at the same audio.

Done when: an old project's Offset lane sounds the same after load, and
`integrity.rs` has the round-trip test.

### 4. Length, Loop and Jump on the shared grid

`ModTimeDivision` for Length. Window anchoring per the gotcha above.

Done when: a `1/16` Loop at Position 50% stutters in time, and a sequenced
Position + Jump pattern slices deterministically.

### 5. Quantized freeze, and BBT everywhere

The `Quantize`/`Quant Grid` pair, the armed-and-pending indication, and the
`BarsBeatsTicks` consolidation — including moving `PositionReadout` and the
`toolbar.slint` formatter onto it.

Done when: freeze fires on the boundary rather than the mouse, a one-bar
Length reads `1:0:0`, transport position still reads one-based, and the
duration/position distinction has a test.

### 6. The 2U face

Waveform telemetry, the head and window overlay, drag gestures, buttons as
macros, `COLLISIONS` demoted.

Done when: `FOCUS.md`'s step 2 test runs — generate or load sound, capture it,
sequence an audible transformation, *see what the head is doing*, save,
reload, and render offline to the same result.

### Alongside: close acceptance test 8

Steps 1 and 2 both touch the realtime path, and the harness this has been
waiting for since Stage 1 now exists — `CountingAllocator::allocations()` in
`mooloop-engine`, from `control-plane-seams/04`. Cover the Buffer operations
with it here rather than leaving the last Stage 1 acceptance item open through
a third plan.

## Out of scope

- The rack-end placement and a dedicated Buffer sequencing lane. Not ruled
  out; every parameter here would be what such a lane drives.
- Persisting frozen *content*. `BUFFER_ENGINE.md` specifies a project-owned
  WAV snapshot, and Freeze makes that worth doing — frozen audio is now
  material rather than working buffer. It is its own step.
- Write feedback, overwrite, resampling the device output. Still excluded, as
  `01` excluded them.
- Multiple read heads.
- `FREEZE MODE: Continue / Restart` as a visible control. Quantized freeze
  covers the musical case; keep the compact face compact.

## Open questions

1. **The D/T toggle.** Recommended against above, with the fallback spelled
   out. Adam's call.
2. **Does `Quant Grid` need to be automatable at all**, or is it a
   control-plane setting like `bars`? It is cheap either way; listed as
   automatable because it costs nothing, but a smaller table is a smaller
   picker.
3. **Where the waveform's peak array lives in the telemetry bank.** The bank
   holds scalars today. An array per Buffer instance has a capacity question
   that `docs/CAPACITY_POLICY.md` should answer before step 6.
