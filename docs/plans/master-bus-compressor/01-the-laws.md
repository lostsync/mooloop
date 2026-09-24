# 01 · The three laws

**Crates:** `mooloop-core` (`src/strip.rs`), `mooloop-dsp` (a new
`src/strip/bus_comp.rs`, a child module of `strip.rs`). Nothing outside
Mixer & Routing's files. Rungs 1-2 on those two crates.

## What it builds

- `mooloop_core::strip::MasterSectionParams`: `comp_in`, `voicing`
  (`BusCompVoicing::{Grip, Punch, Tube}`), `threshold_db`, `makeup_db`, `mix`,
  and per voicing `grip_ratio`, `grip_attack`, `grip_release`, `punch_ratio`,
  `punch_attack`, `punch_release`, `tube_time`, each a **position** on the
  unit's own switch, plus `lookahead_ms` for step 02's guard. Its descriptor
  rows continue `StripParams`'s table from id 44; ids are frozen from
  the day they land. Not yet a field of `StripParams` -- that is step 03,
  which is where it becomes a saved field.
- `mooloop_dsp::strip::bus_comp::BusComp`: one stereo-linked compressor that
  runs whichever law its voicing selects. No allocation, `Copy`-sized state,
  exactly transparent while out.

## The laws

Measured by the rig in `spikes/preamp-measure/` (`RESULTS.md` §4,
`out/comp/*.json`), summarised in `SCOPE.md` §2.1.

| | Grip (SSL G bus) | Punch (API-2500) | Tube (Fairchild 670) |
| --- | --- | --- | --- |
| Detector | peak, linked | **RMS**, linked (a sine reads its peak) | peak, linked |
| Static curve | soft knee, 8.4 dB wide | near-hard knee, 3.4 dB | **the measured curve**: ratio rises with level, about 2:1 at onset to 8:1 at 30 dB over |
| Ratio | 2 / 4 / 10 | 1.5 / 2 / 3 / 4 / 6 / 10 | none: the curve is the ratio |
| Attack | 0.1 / 0.3 / 1 / 3 / 10 / 30 ms, measured 0.31 ... 21.4 ms | 0.03 ... 30 ms, **floor 3.8 ms** | coupled to release in six TIME positions |
| Release | 0.1 / 0.3 / 0.6 / 1.2 s / Auto, measured 77 ms ... 3.4 s | 0.05 ... 2 s, measured 44 ms ... 1.7 s | 104 ms ... 1.8 s; positions 5 and 6 two-stage |
| Programme dependence | none | none | none (5 and 6 are a fixed two-stage release, not a memory of how long the loud part lasted) |

The static curve and ballistics run in the dB domain: detector level, then the
curve gives a target reduction, then attack and release smooth the reduction.
The internal time constants are fitted so that the **rig's own measurement**
-- a 2 kHz tone stepped from -40 to -12 dBFS with about 10 dB of reduction,
time to 63% of the settled reduction, and to 37% on release -- lands on the
measured figure. The table holds both numbers: the marking the knob shows,
the measured t63 that is the law, and the constant that produces it.

## Tests that pin it

- Every position of every voicing measures within 10% of the rig's t63, by
  the rig's procedure (attack t63 under 1 ms is a ceiling, as the rig says of
  itself, and is only held to "at most").
- Punch's attack floor: its two fastest markings measure within 5% of each
  other.
- Detector shape: a tone and 20%-duty bursts of the same peak. Grip and Tube
  reduce both within 1.5 dB of each other; Punch reduces the bursts at least
  3 dB less than the tone (the rig: 8.3 against 4.4).
- **No programme dependence**: reduction left 500 ms after a 30 ms hold and
  after a 3 s hold agree within 5% for every voicing and position (the rig:
  1.0x on all three units, where the channel strip's opto/FET laws are
  2.4-3.6x).
- The static curve: Grip and Punch sit on their ratio line past the knee;
  Tube's reduction at each measured input level is within 0.5 dB of the rig's.
- Out is out: a `BusComp` that is out, or in with no reduction and mix 1 and
  makeup 0, passes every sample bit for bit.
- Every descriptor round-trips through `set`/`get`, strangers are refused, and
  a stepped id holds exactly its switch's positions.

## Not in this step

The engine, persistence, meters and the face. The unit's harmonic colour
(the SSL's h3, the 670's h2): `SCOPE.md` §2.1 ranks it last, and the three
laws are distinct without it. Side-chain filters.
