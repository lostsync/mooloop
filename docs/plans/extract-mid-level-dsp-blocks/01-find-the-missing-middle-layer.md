# Find the missing middle layer between `DelayLine` and whole devices

> **Answered 2026-10-01 (MOO-149): neither block exists.** A "modulated tap"
> over `DelayLine` only looks shared, and the gain stage over `shaper.rs` is
> already there at the right granularity. Step 02 (MOO-150) is cancelled. The
> answer is the first section below; the question as it was set follows it.
> A first middle layer landed on the voice side instead on 2026-09-23
> (MOO-144: `VoiceCutoff`, `Glide`, the saturation stages in `shaper.rs`).

## The answer

Read against `bd3a8540`. The step's own line counts were stale: `delay.rs` is
887 lines (346 of code, the rest tests) and `modulation.rs` 1276 (624 of
code). Of the modulation code, the delay-tap path is `delay_sample`, about 50
lines; the rest is the phaser, its control-rate coefficients, width and
parameter plumbing. `DelayLine` has exactly these two users. `ReadHead` has
one, `DelayEffect`. Nothing else reads a `DelayLine`: the buffer device keeps
its own ring of absolute positions, the reverb and plate keep a mono linear
`Ring` on purpose (`reverb.rs` says why), and `mooloop-test-plugin` has a
private integer `DelayLine` of its own. ML-P8's finishing chorus is not a
third caller of a tap; it embeds the whole `ModulationEffect` through
`process_wet`, which is already a middle layer, one rung up.

### The modulated tap: it only looks like one block

Side by side, the two devices agree on the nouns (a line, a read, feedback, a
one-pole, a cross-feed) and disagree on every policy that connects them:

| | `DelayEffect` | `ModulationEffect` (Chorus, Flange, Ensemble, ADT) |
|---|---|---|
| The read | One `ReadHead`. It drifts per frame, and three modes steer it: Digital jumps with a 256-frame equal-power fade, Tape glides at most 0.05 frame a frame, Reverse drifts by 2 and wraps its window | Two or three bare `line.read`s a sample, at offsets recomputed absolutely from the LFO every sample. No fade and no drift, so `ReadHead` would add four fields a tap and a branch never taken |
| What goes back into the line | The one tap, after damping | The **mix** of the taps: Ensemble averages three reads, ADT two. Per-tap feedback would turn one comb into a sum of combs, a different sound |
| Damping | `OnePoleLp` **inside** the loop, so each repeat is darker than the last. 200 Hz to 20 kHz, coefficient smoothed directly | `OnePoleLp` on the **wet output**, outside the loop, so repeats do not darken. 350 Hz to 20 kHz, cutoff set only when Tone moves |
| Feedback range | 0 to 0.98 | -0.92 to 0.92. The negative half is the flanger's polarity |
| Cross-feed | `cross`, 0 to 1, of the damped tap | `spread * 0.35`, of the tap mix |
| Bounding the loop | A `tanh` knee **in** the loop, `feedback_saturate` (MOO-124) | A linear trim **after** the loop, `resonance_trim` (MOO-200). Its doc names the Delay's knee and rejects it: the knee would leave a quiet input its +22 dB and distort a loud one |
| A NaN | `feedback_saturate` zeroes a NaN repeat; it leaves after one trip (MOO-174) | `scrub_non_finite` on the block, then `clear_tail` (MOO-176) |
| Dry/wet | Mixed in the device | Wet only; the host blends |

A block that served both would have to take every row above as a setting:
where the damping sits, whether feedback reads one tap or a mix, which bound,
which NaN rule, whether the head fades. That is a struct of switches with two
callers that each set them one way, which is the "abstraction that fits
neither caller" the step warned about. The test the step itself set fails
outright: Ensemble cannot be written as three of these taps without changing
its loop.

What the two do share is already shared, in the right places:
`DelayLine` and `MIN_READ_OFFSET`, `max_read_offset` as the clamp,
`DelayLine::write_silence` in `skip_block`, `node::feedback_tail_frames` for
the tail, `Discontinuity::invalidates_tails` for the reset, `OnePoleLp`,
`Smoothed` and `process_param_split`. Those are the policies that would have
drifted if they were copied, and none of them is.

What is left duplicated is **arithmetic, not policy**:

- the line write, `input + fb * (a * (1 - c) + b * c)`, two lines in each
  device;
- milliseconds to frames, clamped to `MIN_READ_OFFSET..max_read_offset()`;
- `ring_frames`, `max_ms * sr / 1000 + 8`, once in each file. The `+ 8` is
  the one copied *number*: it is the interpolator's margin, and it belongs
  to `DelayLine`. It would only matter if the Hermite kernel ever widened,
  and both copies are generous for today's (`max_read_offset` needs 3).
- the exponential Tone law `MIN * (MAX / MIN)^tone`, with ranges that differ
  on purpose. `delay.rs`'s `tone_coeff` also re-derives
  `OnePoleLp::set_cutoff`'s coefficient without its `0.45 * sr` clamp. At
  44.1 kHz the top of Delay's Tone is 20 kHz instead of 19.85 kHz, a
  coefficient 0.14% different. That is copied arithmetic drifting, and it is
  harmless.

Taken together, a tap block would save perhaps ten lines between the two
devices and cost more than that before its tests. It does not meet step 02's
bar that both devices get shorter.

### The gain stage: it already exists

"Trim in, saturate, compensate" is already one call, in four shapes, and each
shape is a different anchoring policy with its own reason written down in
`shaper.rs`:

| Entry point | Anchor | Callers |
|---|---|---|
| `apply_drive(input, drive)`; `voice_drive_compensation(drive)` and `apply_drive_compensated(input, drive, comp)` to hoist the `tanh` | The operating level (`DRIVE_REFERENCE_LINEAR`), with a knee from the identity below `DRIVE_KNEE` | `monosynth`, `polysynth`, `drumsynth`, `sampler` (hoisted), `effects/filter`, DS-01's Soft character |
| `PreDrive::next_sample(input, drive, sr)` | An RMS follower, because a filter's input level is not known | ML-M1, ML-P8 |
| `shape(curve, x) * drive_compensation(curve, gain)` | Full scale | DS-01's Hard, Fold and Crush |
| `reference_drive_compensation(curve, drive)` with `Oversampler2x` and `shape_oversampled_slice` | The operating level, for every curve, at 2x | `DriveEffect` |

`soft_ceiling` is a bound rather than a tone stage. Only DS-01 and the Drive
effect assemble the steps themselves, and both have a reason the others do
not: DS-01 adds bias and quantization per character, and the Drive effect
runs a chunked, oversampled path with dry-latency alignment and a tilt. Output
trim and dry/wet are device policy in every caller. A `GainStage` type would
wrap these four in an enum and add nothing.

This search did find one copied **number**, and it is the kind that drifts.
`ds01.rs:123` sets `DRIVE_GAIN_RANGE = 15.0`, with a comment saying it is
"shared with `shaper::apply_drive`". It is not shared, it is a copy:
`shaper.rs` spells `15.0` inline in `voice_drive_compensation` and
`apply_drive_compensated`. DS-01's test holds its Soft character to
`apply_drive`, which covers only Soft, and Soft calls `apply_drive` directly.
If anyone changed the voice range, DS-01's Hard and Fold would stop reaching
as far as its Soft character, and no test reads both copies. Filed as MOO-482.

### When to reopen

When a third device needs a delay tap. A multi-tap or ping-pong delay is the
obvious one, and none is planned or filed. Before extracting anything, check
that its loop matches one of the two above. If it matches `DelayEffect`'s,
the block is `ReadHead` plus `DelayEffect`'s loop, lifted out of
`delay.rs`, and the work belongs to Effects. Then the third device is the
second real caller that `docs/COMPOSABLE_DEVICE_UNITS.md` (checklist item 7)
asks for.

## The question as set (2026-08-24)

The numbers below are the ones the step was written with, and they are stale;
see the answer above.

### The gap

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

### What to do (investigation only, no code changes)

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

### Why this step is investigation only

The risk with a middle layer is inventing an abstraction that fits neither
existing caller and then having to bend both around it. The two devices in
question are 850 lines of working, tested audio code. The bar for extracting
a block is that both devices get *shorter and clearer*, not just
differently arranged. If reading them side by side doesn't produce an
obvious shared shape, that is a real finding — write it down and stop.

### Output

A short written answer to: what is the block, what does it own, what does it
borrow, and which two existing devices get simpler by using it. That answer
is the input to step 02.
