# Reverb

Status: feedback delay network implemented, August 2026. Supersedes the
generated-room convolution player documented here through August 2026.

The reverb is an eight-line feedback delay network. Mono-summed input passes a
pre-delay, a one-pole low cut, and four Schroeder allpass diffusers
before it is injected into the network; each line's return is lowpass-damped
and attenuated to a per-line gain solved for the target RT60, then remixed
through a normalized Hadamard matrix. Two orthogonal taps across the lines
form the stereo output, which a mid/side `width` control then narrows.

## Realtime contract

- No latency. The device reports zero for both `latency_frames` and
  `dry_path_latency_frames`, so the host neither aligns nor delays around it.
  Pre-delay is a musical control, not reported latency.
- Cost is a fixed number of taps, one-poles, and multiplies per sample. It
  does not vary with `decay_s`, `size`, or anything else: measured at a
  64-frame period and 48 kHz, a 0.5 s tail and a 20 s tail both run ~13.6 us
  mean and under 60 us worst case, about 3-4% of the block budget.
- Nothing allocates, locks, or reallocates in `process`. The rings are sized
  at construction for the longest `size` plus the modulation excursion, so a
  size change moves read heads inside buffers that already exist.
- Every parameter arrives as `Event::ParamValue` in natural units and is
  applied at its sample offset, like every other effect. There is no prepared
  resource, no fingerprint, and no node swap.

## Why not convolution

The previous device generated a room impulse response from explicit geometry
(width, depth, height, a capture point, a shape and a material) and played it
back with uniformly-partitioned FFT convolution. It was replaced rather than
optimized, for three reasons:

- **Its cost was a spike, not a load.** All partitions were accumulated in the
  single `process` call where the 512-sample input window filled. At a
  64-frame period a two-second tail measured ~1400 us in that one block out of
  eight against a 1333 us budget — an xrun — while the mean was an affordable
  54 us. `docs/plans/amortize-reverb-partition-cost/` proposed spreading that
  work across the intervening blocks. An FDN removes the window instead of
  redistributing it.
- **It could not be modulated.** A convolution node cannot accept a parameter
  change; the response has to be regenerated and re-partitioned off-thread and
  swapped in whole. The node ignored `events_in` outright, so a modulation
  route aimed at a reverb knob was silently inert even though the destination
  metadata declared it legal. `docs/MODULATION_PLAN.md` requires "no effect
  changes to support modulation, ever" — the convolution player was the one
  device that could not honour it.
- **It sounded static.** A finite image-source set plus a filtered noise tail
  is geometrically defensible and completely still. Nothing in the response
  moved, so long settings rang rather than bloomed.

## Parameters

`REVERB_DESCRIPTORS` ids start at 8. Ids 0..=7 belonged to the room
generator's controls and are permanently retired: a shipped id may never
change meaning, so a route or automation lane saved against the old "Mic X"
resolves to no descriptor and stays inert rather than quietly becoming
Diffusion.

| Control | Range | What it does |
| --- | --- | --- |
| Size | 0..1 | Scales every delay and diffuser length, 0.4x to 2.5x |
| Decay | 0.2..20 s | Mid-band RT60 the per-line feedback gains are solved for |
| Damp | 0..1 | High-frequency loss per trip around a line |
| Pre | 1..200 ms | Silence before the network sees the input |
| Diffuse | 0..1 | Input allpass gain, discrete echoes to a wash |
| Width | 0..1 | Mid/side spread of the two output taps |
| Mod | 0..1 | Slow independent drift of the delay lines |
| Low Cut | 20..500 Hz | Highpass on the input, before diffusion |

Damping compounds once per trip, so a heavily damped tail measures shorter
than `decay_s`. That is what a real room does and is not a calibration error.

`Low Cut` filters the input and not the feedback loop, for the same reason
stated in reverse: a highpass inside the loop would compound the way damping
does, and any corner high enough to control mud in a single pass would strip
the bass out of the tail entirely over sixty of them. One pass on the way in
is the control that actually behaves like a low cut.

`Size` glides rather than jumping. Moving a read head straight to a new offset
lands it on uncorrelated history, which is a click; the lengths are one-poled
toward their target over 50 ms so the control survives being swept or
modulated. The plate takes the opposite trade and clears its buffers, because
its size is not a modulation destination.

## Level

`OUTPUT_REFERENCE` in `reverb.rs` pins the network's absolute output. A
feedback network has no natural unity — its steady-state level depends on
decay, size, and where the input's energy sits against the network's modes —
so the constant is measured, not derived. It is enforced by
`steady_state_wet_path_is_level_matched` in `gain_structure_tests.rs`, which
holds a note and reads 1..2 s in, past the buildup and before the release.

## Measured IRs

There is no IR player in the tree any more. A convolution reverb remains a
reasonable *separate* device if measured-space loading is ever wanted: it
would decode and resample off the audio thread, and would need the
prepared-resource path (`StructuralCommand::ReplaceEffect` with a resource
key, which the retained-audio buffer still uses) to install the result. It
should not come back as a mode of this device — its parameter model is
fundamentally different, and mixing the two is what produced a reverb whose
knobs could not be modulated.
