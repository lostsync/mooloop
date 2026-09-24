# Master bus compressor plan status

Written 2026-09-23 for `FOCUS.md` step 3, from Adam's answers of that day on
MOO-13 and MOO-169. `README.md` has the decisions; the numbered files are the
steps.

Linear: project [Master bus compressor](https://linear.app/mooloop/project/master-bus-compressor-afe8168e32a5), umbrella MOO-13.

| Step | What | Issue | State |
| --- | --- | --- | --- |
| [01](01-the-laws.md) | The three laws, as data and as a detector | [MOO-206](https://linear.app/mooloop/issue/MOO-206) | in progress |
| [02](02-the-lookahead.md) | The safety limiter's lookahead, 0 bit-identical | [MOO-169](https://linear.app/mooloop/issue/MOO-169) | not started |
| [03](03-the-master-section-runs.md) | Runs on the master, saved, metered, takes and exports aligned | [MOO-207](https://linear.app/mooloop/issue/MOO-207) | not started |
| [04](04-the-face-and-the-meter.md) | The per-voicing face, the meter, the lookahead box | [MOO-208](https://linear.app/mooloop/issue/MOO-208) | not started |

**The listening pass** is Adam's, and is the acceptance case for *"turning it
on should feel special"*. Nothing in this plan claims it. Step 03 adds the
render to `FOCUS.md`'s list.

## What the fitting found before step 01

The rig's timings are for a 2 kHz tone. Simulating the laws the way the rig
measured them (`spikes/preamp-measure/RESULTS.md` §4, the same step and the
same 63% crossing) and fitting each position's internal time constant to it
gave three things worth knowing before building:

- **Release in the dB domain is the measurement.** A one-pole on the
  reduction releases to 37% in its own time constant, so every release in the
  table is its measured t63 to within 0.1%. Punch's RMS window adds 4-5 ms.
- **Attack is not.** A peak detector on a 2 kHz tone only sees a new peak
  every 0.25 ms, so Grip's fastest markings resolve in quarter-milliseconds:
  0.1 ms fits to 0.19 ms and 0.3 ms to 0.56 ms. The rig says the same of
  itself (an attack under 1 ms is a ceiling), so those two are held to "at
  most 1 ms" rather than to a figure.
- **Punch's 3.8 ms floor is a 5.3 ms RMS window.** With it, the 0.03 ms
  marking measures 3.83 ms and the 0.1 ms one 3.98 ms, which is the unit's
  own pair (3.81, 3.98).
- **Tube's positions 5 and 6** are two releases summed, a fast one of 495 ms
  shared by both and a slow one of a third of the published time (3.3 s and
  8.3 s), weighted 0.40 and 0.60 to land t63 on 1760 and 1426 ms. Position 5's
  t90 then lands on the measured 6.0 s; position 6's is predicted at 11.5 s,
  past what the rig recorded.
- **Tube's static curve is used as measured.** Its ratio rises from about
  2:1 at onset to 8:1 thirty decibels over; no knee formula reaches that
  shape, and a table of eleven points does.
