# The body resonator

This is the layer v1 has no equivalent of, and it is where most of the new
drum types come from. A tone oscillator and a filtered noise burst can make a
kick, a snare, and a hat. They cannot make a rim, a clave, a conga, a
cowbell, a bell, or a piece of struck metal, because those sounds are a short
excitation ringing through a resonant object, and the object is the sound.

## What it is

Three tuned resonators in parallel, each a high-Q band-pass with its own decay,
excited by a signal rather than triggered by an envelope.

```text
  excitation ──> resonator 1  (f)          ──┐
                 resonator 2  (f * r2)      ──┼──> body level
                 resonator 3  (f * r3)      ──┘
```

| Id | Control | Range | What it does |
| --- | --- | --- | --- |
| 30 | Body Level | 0-1, default 0 | The layer's contribution |
| 31 | Body Pitch | 20-8000 Hz, log | Fundamental. Tracks the note like the tone layer |
| 32 | Body Ratio | 0-1 | Harmonic at 0 (2, 3) to inharmonic at 1 (2.76, 5.40) |
| 33 | Body Decay | 5 ms - 8 s, log | Ring time of the resonators |
| 34 | Body Damping | 0-1 | High-frequency loss. Damps the upper modes faster than the fundamental |
| 35 | Body Excite | 0-1 | Impulse at 0, noise layer at 1, crossfaded |

**Ratio** is the whole design in one control. At 0 the modes are harmonically
related and the layer reads as a pitched drum — a tom, a conga, a tuned kick
body. Sweeping toward 1 detunes the upper modes to the ratios that make struck
metal, and the sound stops having a pitch and starts having a material. The
1 : 2.76 : 5.40 endpoint is the ideal circular membrane's mode set, which is
the reason a real drum head sounds like a drum head and not like a sine.

**Damping** is the difference between a bell and a woodblock and is the most
gestural control in the device. Modulating it from the Mod envelope gives a
strike that opens or closes over its own tail.

**Excite** decides what the resonators are hit with. At 0 it is the burst
impulse — a hard strike, which makes clave, rim, and woodblock. At 1 it is the
noise layer post-filter — a sustained excitation, which makes cymbal shimmer,
bowed metal, and the noise-driven ring under a snare. Between them is most of
what percussion actually is.

## Implementation notes

- Use the shared biquad from `mooloop-dsp/src/biquad.rs` in a band-pass
  configuration with Q derived from Body Decay and the mode frequency, so
  decay is a time in seconds at every pitch rather than a Q that means
  different things across the range.
- Decay and Damping are continuous per `01`, so a coefficient update path per
  control tick is required. Recompute all three modes' coefficients on the
  control tick, not per sample.
- Skip the whole layer when Body Level is zero **and** its smoother has
  settled — this is ML-P8's lesson about level-gated skipping written down in
  advance: a level reaching zero does not mean the smoother has arrived, and
  skipping early replaces a ramp with a step.
- Bound it. A high-Q resonator excited by a loud impulse at a low damping and
  a long decay is the most likely place in DS-01 to produce something enormous.
  The bound belongs here and in step 06's shaper, not in a master limiter.
- Modes above Nyquist are muted rather than folded, and the mute is smoothed.

## Acceptance

- Body Level at 0 costs approximately nothing measurable in the render loop.
- A body-only patch at Ratio 0 has a clear pitch that tracks the note; the
  same patch at Ratio 1 does not.
- Body Decay is a time: measure the -60 dB point at three pitches and show it
  is the same within tolerance.
- No control combination, including under modulation at both range extremes,
  produces a non-finite sample or exceeds full scale at the device output.
- Rim, clave, cowbell, and a tuned tom are reachable and go into step 09's kit.
