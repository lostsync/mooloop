# Find the missing middle layer between `DelayLine` and whole devices

## The gap

The intended shape is a ladder: primitives compose into blocks, blocks
compose into devices. On the UI side that ladder already exists —
`theme.slint` → `controls.slint` (`ParameterKnob`, `MiniKnob`,
`ParameterFader`, `PeakMeter`, `SegmentedControl`, `SelectorBank`,
`LedIndicator`) → `device-rack.slint` (`DeviceFrame`, `DeviceHeader`,
`EffectDeviceShell`) → device faces. `filter-device.slint` is 79 lines and
is almost pure composition: the surface comes from the shell, the controls
are called in by name. That is the model working.

On the DSP side the ladder has a rung missing. There is `DelayLine` (raw
storage plus fractional read, `delayline.rs`) and there are entire devices
(`DelayEffect`, 550 lines; `ModulationEffect`, 299 lines), with nothing in
between. Both devices independently implement "read a modulated tap, apply
feedback, damp the feedback path, blend" — the concept that should be *one
callable block* and isn't.

This is the "make a 1-tap delay, then build the 4-tap out of it" layer.

## What to do (investigation only, no code changes)

1. Read `effects/delay.rs` and `effects/modulation.rs` side by side and
   write down what they actually share versus what genuinely differs. Known
   shared machinery: both own a `DelayLine`, both keep per-channel feedback
   state, both keep a one-pole damping/tone filter in the feedback or wet
   path (`delay.rs:41-42`, `modulation.rs` `tone_l/r`), both compute a
   fractional read offset per sample.
2. Identify the honest block boundary. A candidate is something like a
   "modulated tap": owns nothing but a read offset and its own feedback +
   damping state, borrows the shared `DelayLine`, and produces one wet
   sample pair per call. Under that shape:
   - a simple delay is one tap with a fixed offset and tempo-sync,
   - a chorus/flange is one tap with an LFO on the offset,
   - an ensemble is three taps at spread offsets (`modulation.rs` already
     does exactly this by hand for `ModulationMode::Ensemble`),
   - a ping-pong is two taps with crossed feedback,
   - a future multi-tap is N taps.
   Confirm this against the real code before committing to it. In
   particular check ownership: several taps must share one `DelayLine`
   (writing the input once, reading many times), so the block cannot own
   its line — it borrows. Verify `DelayLine`'s API supports that cleanly
   (`read(&self, offset)` at `delayline.rs:77` takes `&self`, which is a
   good sign).
3. Check whether `ReadHead` (`delayline.rs:129`) is already most of this.
   It holds an offset, has fade-on-jump machinery, and reads from a
   borrowed line. It may be that the block is "`ReadHead` + feedback +
   damping" and most of the work is a small wrapper rather than a new
   design. Establish that before writing anything.
4. Do the same read for the other obvious candidate cluster: `DriveEffect`
   and the drive stage inside `Sampler`/`MonoSynth`/`PolySynth` (all four
   call `apply_drive` or `shape`, but with their own gain-staging and
   compensation around it). Ask whether a "gain stage" block —
   trim in, saturate, compensate, trim out — is real or whether
   `shaper.rs`'s existing `shape` + `drive_compensation` is already the
   right granularity.

## Why this step is investigation only

The risk with a middle layer is inventing an abstraction that fits neither
existing caller and then having to bend both around it. The two devices in
question are 850 lines of working, tested audio code. The bar for extracting
a block is that both devices get *shorter and clearer*, not just
differently arranged. If reading them side by side doesn't produce an
obvious shared shape, that is a real finding — write it down and stop.

## Output

A short written answer to: what is the block, what does it own, what does it
borrow, and which two existing devices get simpler by using it. That answer
is the input to step 02.
