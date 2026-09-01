# Time-stretch spike results (issue #32)

Every number below is measured by `src/main.rs` on the remote build box
(8 cores, release profile, 48 kHz stereo) unless it is explicitly marked as
not measured. Section names in parentheses refer to the harness's CSV output.

Ratio throughout means **output frames per input frame**: `1.5` is longer and
slower, `0.75` is shorter and faster.

Preset names in the tables below are abbreviated by window size. Two WSOLA
presets share window 1024, so read the abbreviations as:

| Table label | CSV preset | Onset snapping |
| --- | --- | --- |
| wsola 512 | `wsola_fast` | off |
| wsola 768 | `wsola_break` | off |
| wsola 1024 | `wsola_nosnap` | off |
| wsola 2048 | `wsola_smooth` | on |

**Every headline WSOLA number below is `wsola_nosnap`**, i.e. snapping off,
which is what the recommendation actually proposes to ship -- snapping is
deferred to #33. The snapping preset (`wsola_music`, same 1024 window) is
reported only in the deferrals section, where it costs up to 255 cents on a
held bass note.

## Recommendation

**Own a WSOLA time-domain stretcher in `mooloop-dsp`. No external dependency,
no FFT, live per voice, 21.3 ms window by default.**

Roughly 250 lines of DSP. The whole thing is: advance a fractional analysis
pointer by `hop / ratio`, nudge each segment within `± search` frames to the
position whose leading half best correlates with the natural continuation of
the previous segment, and overlap-add under a Hann window that is COLA at 50%.
Alignment is decided on the mid channel and applied to both.

## The two candidates

| | Candidate A | Candidate B |
| --- | --- | --- |
| Family | WSOLA, time domain | STFT phase vocoder, frequency domain |
| Transient handling | similarity search; optional onset snapping | identity phase locking; optional phase reset on onsets |
| Presets measured | 512 / 768 / 1024 / 2048 window | 1024 / 2048, locked / plain / independent-channel |

Both were built against the same trait, fed identical material through one
region/loop mapping, and measured with the same code. The phase vocoder uses
`rustfft`, so it is not handicapped by a hand-rolled FFT.

## Why WSOLA won: transients

Drums are the primary material, and this is where the families genuinely
differ. From `click_train`, isolated well-separated transients where nothing
overlaps and every reading is unambiguous:

**Attack smearing** — 10-90% envelope rise time, output ÷ source. `1.0` is
preserved; `0.000` means no measurable rise remained at all, i.e. the attack
is gone.

| ratio | wsola 1024 | wsola 512 | pvoc locked | pvoc plain |
| --- | --- | --- | --- | --- |
| 0.50 | 3.33 | 2.47 | 8.51 | 15.28 |
| 0.75 | 2.63 | 1.77 | 6.81 | 12.61 |
| 0.90 | 1.70 | 1.40 | 8.00 | 1.16 |
| 1.25 | 0.93 | 0.84 | 7.14 | 0.000 |
| 1.50 | 0.95 | 0.95 | 0.000 | 0.000 |

**Rhythmic timing** — mean absolute onset placement error against the exact
expected position, in ms.

| ratio | wsola 1024 | wsola 768 | wsola 512 | pvoc locked | pvoc 1024 |
| --- | --- | --- | --- | --- | --- |
| 0.75 | 3.32 | 5.66 | 8.99 | 8.32 | 10.66 |
| 0.90 | 3.32 | 5.99 | 8.66 | 11.32 | 12.66 |
| 1.25 | 4.99 | 6.99 | 9.65 | 18.99 | 16.65 |
| 1.50 | 5.98 | 7.32 | 9.98 | 23.65 | 17.98 |
| 2.00 | 10.64 | 10.64 | 10.64 | 32.00 | 22.64 |

WSOLA's error is roughly flat in ratio. The phase vocoder's grows to 24-32 ms,
which at 138 BPM is longer than a 32nd note — audibly behind the grid.

**Punch** — crest factor in a 40 ms window at each hit, output minus source, dB.
Positive keeps or sharpens the transient.

| ratio | wsola 1024 | wsola 512 | pvoc locked | pvoc plain |
| --- | --- | --- | --- | --- |
| 1.25 | +0.53 | +0.15 | −0.49 | −2.49 |
| 1.50 | +0.35 | +0.12 | −1.06 | −2.70 |
| 2.00 | +0.03 | +0.03 | −1.56 | −1.19 |

The same ordering holds on the full `drum_break` fixture (rise ratio at 1.25:
0.74 wsola-1024 vs 3.37 pvoc; crest at 1.5: −0.35 vs −1.22 dB) and on
`mixed_loop` (LTAS distance from the source at 1.25: 0.08 dB wsola-1024 vs
0.46 dB pvoc).

## Why WSOLA won: stereo image

`stereo_wide` is a decorrelated noise bed with hard-panned hits. Deltas from the
source; smaller is better.

| | inter-channel correlation Δ | side energy Δ (dB) |
| --- | --- | --- |
| wsola, any preset | +0.004 … +0.038 | −0.04 … −0.33 |
| pvoc, shared mid phase | +0.062 … +0.124 | −0.52 … −1.11 |

The phase vocoder here is already the *good* stereo implementation: one phase
trajectory propagated from the mid signal and both channels rotated by the same
correction, which preserves inter-channel phase exactly in principle. It still
loses 3-9x more image than WSOLA, because the phase locking is applied per
spectral peak and the peak structure of the two channels is not identical.
A naive two-mono-instance port would be worse on correlated material; the
`pvoc_indep` ablation does not show that because the fixture's bed is already
decorrelated, so this particular comparison is weaker than it looks.

## What the phase vocoder won, and why it was not enough

Pitch accuracy on sustained tonal material is exact by construction for a phase
vocoder and window-dependent for WSOLA. On a sustained 55 Hz sawtooth
(`bass_note`), measured error in cents:

| window | 0.75 | 0.90 | 1.25 | 1.50 |
| --- | --- | --- | --- | --- |
| wsola 512 | +498 | +183 | −386 | −705 |
| wsola 768 | +9.8 | +102 | +11.1 | +0.4 |
| **wsola 1024** | **+0.4** | **+0.2** | **+0.1** | **−0.0** |
| pvoc (any) | +0.2 | +0.2 | +0.2 | +0.2 |

This is the single most important design constraint the spike produced:

> **The WSOLA window must span at least ~1.2 periods of the lowest fundamental
> to be preserved.** 1024 frames at 48 kHz is 21.3 ms, which is 1.17x the
> 18.2 ms period of A1 (55 Hz). At 512 frames the search locks onto the wrong
> period and the fundamental is destroyed.

At window 1024 WSOLA matches the phase vocoder to within half a cent, so the
family's usual pitch objection does not survive contact with a correctly sized
window. On a pure 440 Hz tone every candidate is within 0.7 cents. The phase
vocoder does hold slightly more energy in the fundamental on harmonic-rich bass
(−1.0…−1.7 dB vs −1.8…−2.4 dB), which is a real but small tonal advantage.

The phase vocoder is also genuinely cheaper — 25 µs vs 66 µs mean per 128-frame
block. That matters less than it sounds: WSOLA at 2.5% of one core per voice was
never the scarce resource. The phase vocoder wins CPU and loses transients, and
transients are what breaks are made of.

## Non-differentiators (hypotheses the measurements killed)

Two things I expected to separate the families did not.

- **Loop seams.** On stationary loop material, curvature at the wrap is 0.2 dB
  above a control point for every WSOLA preset and 0.3-1.0 dB for every phase
  vocoder preset. Neither clicks. Loop-boundary quality remains #31's problem,
  not the stretcher's.
- **Live ratio changes.** Switching 1.0→1.5, 1.0→0.75, 1.25→1.26 and 0.5→2.0
  mid-render produced ≤0.4 dB excess curvature for every candidate. No declick
  or crossfade machinery is needed on either side.

One small real difference: WSOLA reaches full level in 0.00 ms at note-on; the
phase vocoder takes 1-2 ms even after its startup overlap ramp is divided out.

## Realtime properties (both families passed)

- **Allocation**: 0 allocations across 200 `render` calls, `reset`, and
  `set_ratio`, measured with a counting global allocator. All state is
  preallocated in `new`.
- **Block-size determinism**: bit-exact. Max absolute sample difference between
  a one-shot render and a blocked render is **0.0** across blocks
  32/64/128/256/480/512/1024 and ratios 0.75/1.0/1.37/2.0, for all ten presets.
  Realtime and offline agree by construction.
- **Duration**: exact. The analysis pointer is fractional, so error never
  accumulates; the caller asks for `round(len * ratio)` frames and gets them.
- **Latency: zero.** Output frame 0 corresponds to input frame `start`. Because
  the sample is fully resident, the window and search are lookahead *into a
  buffer*, not delay in time. This is a property of the pull-from-resident-sample
  architecture rather than of either algorithm, and it is why an external
  streaming stretcher with a declared `analysisLatency()` would be a regression.

## Live per voice, not prepared and cached

Recommend **live**. Measured basis:

- Live costs 2.5% of one core (mean) per voice at block 128. That is affordable.
- A prepared cache costs 1.8 MB per (sample × ratio) for a 3.9 s stereo loop,
  multiplied by every distinct ratio and eventually every slice.
- Re-rendering that loop takes ~119 ms, so dragging a tempo control would
  re-render per knob position or lag behind it.
- Live ratio changes measured click-free, so the thing a prepared path would buy
  (a clean transition) is not needed.
- Same algorithm either way, so there is no quality argument for preparing.

## Contract values for #13

| Item | Value |
| --- | --- |
| Algorithm | Owned WSOLA in `mooloop-dsp`. No new dependency, no FFT. |
| Default window / hop / search | 1024 / 512 / ±512 frames at 48 kHz. Scale with sample rate to hold ~21.3 ms / ~10.7 ms. |
| Correlation | Mid channel only, decimated by 2, applied identically to both channels. |
| Supported ratio range | **0.5 – 1.5**. Clamp hard at 0.25 – 4.0. |
| Latency | 0 frames. `latency_frames()` returns 0; nothing to compensate. |
| Sample lookahead | `window + search` = 1536 frames past the nominal analysis position. A region bound, not latency. |
| Tail | None beyond the voice's own amp envelope. No flush at note-off. |
| Ratio change | Takes effect at the next OLA hop, not the next block: `set_ratio` only updates the value `produce_frame` reads, so a change waits up to one hop (~10.7 ms at the default window). No crossfade, no declick, no re-preparation. |
| Live vs prepared | Live per voice. Prepared/cached assets explicitly out of scope. |
| Quality modes | Two. `Music` (1024) is the default. `Drums` (512) extends usable ratios to 2.0 on a break but must be labelled percussion-only. |
| CPU per voice | 65.9 µs mean / 298 µs worst per 128-frame block (2.5% / 11.2% of the 2667 µs budget). |
| Polyphony cap | **4 simultaneously stretching voices** (9.9% mean, 43.0% worst). |
| Memory per voice | 24,580 bytes at window 1024; 12,292 at window 512. Fully preallocated in `new`. |
| Reset cost | One `window`-sized memset. Allocation- and drop-free. |

### Ratio range, justified

`drum_break` with the 1024 window, of 19 placed onsets:

| ratio | missed | spurious |
| --- | --- | --- |
| 0.25 | 7 | 0 |
| 0.50 | 1 | 0 |
| 0.75 – 1.50 | 1 | 0 |
| 2.00 | 10 | 10 |
| 4.00 | 11 | 14 |

0.5–1.5 is clean. 2.0 falls apart with the musical window and survives with the
512 window (1 missed, 0 spurious), which is what the `Drums` mode is for. 4.0
is not usable in either mode. Note the phase vocoder degrades earlier: it loses
its measurable attack rise by ratio 1.5, inside the supported range.

### Polyphony, justified

Block 128, ratio 1.25, window 1024, one core:

| voices | mean µs | worst µs | mean % | worst % |
| --- | --- | --- | --- | --- |
| 1 | 65.9 | 298 | 2.5 | 11.2 |
| 4 | 264 | 1146 | 9.9 | 43.0 |
| 8 | 527 | 2249 | 19.7 | 84.3 |
| 16 | 1054 | 4488 | 39.5 | **168.3** |

The binding constraint is the worst block, not the mean, and worst is a
structural 4.3x the mean: a whole 1024-frame overlap-add is produced inside one
128-frame block, so one block in four does all the work. Staggering the start
position does not fix it — the harness already staggers by 977 frames and the
burst persists.

**#13 should flatten this**, by amortizing each frame's correlation search
across the hop or by explicitly staggering per-voice frame phase at note-on.
With the burst flattened the cap rises toward the mean-limited figure. Until
then, 4 is the honest number, and it must leave headroom for the rest of the
mixer. These are build-box timings; the laptop will be slower.

### Explicit v1 deferrals

- **Transient snapping / onset-driven splice placement** → #33. With a correct
  onset table it helps an isolated one-shot (crest +1.2 dB at ratio 1.5 vs
  −0.9 dB without). With a bad table it destroys tonal pitch — up to 255 cents
  on a held bass note. Not shippable before the detector is trustworthy.
- **Reverse and ping-pong while stretching.** Not implemented, not measured. The
  UI must disable stretch for these rather than silently falling back.
- **Formant preservation.** Out of scope, as #20 already anticipated.
- **Prepared/cached stretched assets.** Rejected above.
- **Per-slice ratios**, and the slice-marker interactions in #15/#35/#36.
- **Pitch shift via stretch composed with #11's resampler.** Plausible and
  cheap, but not measured here.
- **Ratios outside 0.25–4.0.**

## Rejected: the phase vocoder

Rejected on transient smearing (3-8x the attack rise time), rhythmic timing
(19-32 ms onset error at ratios 1.25-2.0 vs 5-11 ms), punch (0.5-2.9 dB crest
loss where WSOLA gains), stereo image (3-9x more damage), and memory (132 KB
per voice vs 24 KB). It wins pitch exactness on tonal material and 2.6x less
CPU. For an instrument whose primary material is breaks, that is the wrong
trade.

## Rejected: external crates

Surveyed on crates.io on 2026-08-30. None was benchmarked — all were rejected on
licence, architecture, or maintenance grounds, and that limitation is stated
rather than papered over.

- **`signalsmith-stretch` 0.1.3** (MIT, 78k downloads, last release 2025-09-18).
  The strongest external option, and the underlying Signalsmith Stretch library
  is also MIT, which is compatible with mooloop's GPL-3.0-or-later. Rejected on
  four counts:
  1. It is STFT/phase-vocoder family — the family that lost here on transients.
     Its own transient handling is better than this spike's textbook phase
     vocoder, so my numbers are *not* its numbers, but the family's
     characteristic failure is the one that matters most for breaks.
  2. `build.rs` runs `cc` and `bindgen`, adding a C++14 compiler and libclang to
     every build machine, the CI runner, and the `cargo deb` /
     `cargo generate-rpm` release workflow.
  3. It declares `analysisLatency()` and `synthesisLatency()`. Real latency to
     compensate, where the owned design measured zero.
  4. It is a streaming push/pull API (`writeInput` / `moveInput`). The sampler
     needs random access: region bounds, loop wrap, slice jumps, and the live
     retrigger / beat-repeat / jump / flip gestures in #38. Each of those means
     re-priming a streaming stretcher.

  And the decisive realtime point: mooloop's allocation detector is a Rust
  global allocator. **It cannot see C++ `new`.** Adopting a C++ stretcher would
  put the audio thread's most complex component permanently outside the
  project's own realtime-safety test.
- **`ssstretch` 0.1.0** (MIT). Same C++ library, one release, 2025-03-01.
  Strictly worse than the above.
- **`soundtouch` 0.5.4 / `soundtouch-ffi` 0.4.1** (LGPL-2.1). The licence works
  — LGPL-2.1 §3 permits relicensing under the GPL and mooloop is
  GPL-3.0-or-later. But it is the same SOLA/WSOLA family we would be
  implementing, in C++, with the same invisible-allocation problem, and with
  less control over window and search than owning 250 lines gives.
- **`bungee-rs` 0.2.0** (MPL-2.0). **Yanked** on crates.io. Not viable.
- **`timestretch` 0.14.0** (MIT, pure Rust). Fine on licence and toolchain, but
  8.3k downloads and a release four days before this spike is no track record,
  and "optimized for EDM" is not a realtime-safety statement.
- **`wsola` 0.1.0** (MIT) and **`rodio-wsola` 0.2.0** (Apache-2.0). Both are
  15-40 KB single-purpose crates extracted from other applications (a podcast
  player; a `rodio` Source adapter). Neither offers a random-access or
  preallocated realtime interface.
- **Rubber Band.** No maintained Rust binding exists on crates.io. The C++
  library is GPL-2.0-or-later or commercial; the licence would work, the binding
  does not exist.
- **`rustfft`** (MIT OR Apache-2.0) is used in this spike for the rejected
  candidate only. The recommendation needs no FFT, so nothing new enters the
  production dependency tree.

## What could not be measured

Stated plainly, because a narrow real result beats a broad estimated one.

1. **Listening quality.** I cannot hear the renders. Every conclusion above is
   objective metrics on synthetic fixtures. The 64 renders exist precisely so
   this can be checked by ear — start with `drum_break__wsola_nosnap__r1.25.wav`
   against `drum_break__pvoc_locked__r1.25.wav`.
2. **Real recorded material.** Fixtures are synthesized. A real Amen break has
   room tone, cymbal wash, and bleed that synthetic hits under-represent, and
   dense cymbal wash is exactly where WSOLA's own artifact (a phasey shimmer)
   would show up.
3. **Polyphonic tonal warble.** WSOLA's known weakness on chords is a
   periodicity-search failure producing warble or stutter. LTAS and crest do not
   detect it. `mixed_loop` contains a three-note pad but no metric targets this.
4. **Signalsmith Stretch and SoundTouch quality.** Rejected on architecture,
   licence, and build grounds; never benchmarked.
5. **Transient snapping's real ceiling.** The throwaway spectral-flux detector
   over-triggers badly on sustained low-frequency material — 56 onsets on a
   single held 55 Hz note even after high-pass weighting. The snapping presets'
   poor tonal scores measure my detector, not the idea. #33 owns this.
6. **Formant behaviour.** Out of scope.
7. **Laptop CPU.** All timings come from the 8-core build box under the release
   profile.

One metric artifact to be aware of when reading the raw CSV: at ratio 4.0 the
mean onset error can read `0.00` because nothing matched at all, not because
timing was perfect. Read it together with the missed and spurious columns.
