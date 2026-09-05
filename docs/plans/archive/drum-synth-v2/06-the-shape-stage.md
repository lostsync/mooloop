# The shape stage and the output

Everything above sums into one shaper. This step decides what that shaper is,
and settles DS-01's gain contract.

v1 applies `apply_drive` and multiplies by a single `OUTPUT_REFERENCE`
constant of 0.26, chosen so the default kick lands near -12 dBFS. That is a
reasonable calibration and the wrong structure: the constant is doing the job
of a mix decision, a character control, and a safety bound at once.

## The controls

| Id | Control | Range | What it does |
| --- | --- | --- | --- |
| 90 | Drive | 0-1, default 0 | Amount into the nonlinearity |
| 91 | Character * | stepped | Which nonlinearity |
| 92 | Bias | 0-1, default 0 | Asymmetry. Adds even harmonics; at the top it gates and spits |
| 93 | Bits | 1-16, default 16 | Bit depth reduction, post-drive |
| 94 | Output HP | 5-2000 Hz, log, default 20 | Removes the DC that Bias creates, and thins a hit deliberately |

The device output level stays at id 1 in the global band, where step 02 put
it. It is a mix control, not part of the shaper.

**Character** is the one place DS-01 gets an opinion rather than a range.
Four, no more:

| Character | Behaviour |
| --- | --- |
| Soft | Symmetric soft clip. v1's `apply_drive`, so old-sounding patches remain reachable |
| Hard | Sharp knee. Squares off a kick, adds the click back |
| Fold | Wavefolder. Turns level into timbre; a folded sine kick becomes harmonically dense without getting louder |
| Crush | Rectification plus the bit reducer's character; the damaged one |

The taste brief asks for colour that is *playable* — that reacts to level,
timing, and the source rather than adding a fixed percentage of an effect.
Fold is the one that does that most literally: because folding is a function
of instantaneous amplitude, the shape of the hit changes across its own decay
for free. Bias does the same thing asymmetrically. These two are why the stage
is worth building instead of leaving `apply_drive` alone.

Character is structural and defaults to modulation-ineligible. Switching it
between hits must not click; switching it mid-hit is undefined and the UI does
not need to prevent it.

## Gain

Replace `OUTPUT_REFERENCE` with an explicit contract under
`docs/GAIN_STRUCTURE.md`:

- **One layer at its default level with Drive at 0 is the device reference.**
  Adding the noise or the body layer does not turn the tone layer down.
- The default patch at full velocity peaks within a dB of
  `gain::REFERENCE_PEAK_DBFS`, matching v1's calibration so a v1 kick and a
  DS-01 kick sit at the same place in a mix.
- Drive is compensated so that raising it changes timbre substantially more
  than it changes level, but it is **not** normalized to constant loudness:
  drive should be able to make a hit louder, because that is what drive does.
- The stage is bounded for every combination of every control, including under
  modulation of Drive, Bias, and all three layer levels at once. The bound is
  audible saturation, not a hidden limiter.

## Acceptance

- Peak output never exceeds full scale for any control combination, verified
  by a sweep rather than by inspection.
- No non-finite sample under any modulation of the shaper.
- Character = Soft with Bias 0 and Bits 16 reproduces v1's drive curve.
- Fold at a fixed Drive produces a spectrum that changes measurably across the
  decay of one hit, which is the property that makes it playable.
- Output HP removes the DC offset Bias introduces, tested at Bias = 1.
- The gain-structure tests grow a DS-01 case beside the existing drum one.
