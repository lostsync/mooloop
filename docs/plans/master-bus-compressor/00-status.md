# Master bus compressor plan status

Written 2026-09-23 for `FOCUS.md` step 3, from Adam's answers of that day on
MOO-13 and MOO-169. `README.md` has the decisions; the numbered files are the
steps.

Linear: project [Master bus compressor](https://linear.app/mooloop/project/master-bus-compressor-afe8168e32a5), umbrella MOO-13.

| Step | What | Issue | State |
| --- | --- | --- | --- |
| [01](01-the-laws.md) | The three laws, as data and as a detector | [MOO-206](https://linear.app/mooloop/issue/MOO-206) | **landed 2026-09-23** |
| [02](02-the-lookahead.md) | The safety limiter's lookahead, 0 bit-identical | [MOO-169](https://linear.app/mooloop/issue/MOO-169) | **landed 2026-09-23** (the ring; the box is 04) |
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

## Step 01 — the three laws

Landed 2026-09-23. `mooloop_core::strip::MasterSectionParams` (ids 44-56,
contiguous with the strip's, where 01 first said 48 -- the strip's table ends
at 43 and `StripSpec` finds a row by arithmetic on its position, so a gap
would have been a hole the face could fall into) and
`mooloop_dsp::strip::bus_comp`, the law table and the one detector. Every
marked position of every voicing measures within 10% of the rig by the rig's
procedure, or within the quarter-millisecond a 2 kHz carrier resolves.

What building it found:

- **An attack fitted with the wrong release measures wrong.** Tube's
  attacks were first fitted against a stand-in 100 ms release; with its own
  coupled release position 4 measured 7.85 ms against the unit's 7.10. A peak
  detector only charges on each crest, so how far it lets go between crests
  is part of the attack it measures. Tube's attacks are fitted against their
  own releases now; Grip's and Punch's against the rig's base release.
- **A steady tone does not sit exactly on the ratio line** at a slow attack:
  a peak detector charges on the crest and lets go between, and settles about
  half a decibel short at 10 ms. The unit does the same (it is where the SSL's
  60 Hz third harmonic comes from). The static-curve test runs at the fastest
  attack, and the curve itself is pinned exactly.
- **Programme dependence is measured the rig's way**: the fraction of the
  reduction left 500 ms after a 30 ms hit and after a 3 s passage, within 5%
  or half a percentage point where both have nearly finished. Every voicing
  and all six of Tube's positions pass.
- **Punch reads power**: 10 ms bursts every 50 ms get at least 3 dB less
  reduction than the tone at the same peak, where Grip and Tube are within
  1 dB. The rig: API 8.3 / 4.4, SSL 11.1 / 10.7, Fairchild 10.4 / 9.8.

Nothing runs it yet: step 03 puts it on the master.

## Step 02 — the lookahead

Landed 2026-09-23 with step 01. `OutputGuard::set_lookahead_ms`,
`latency_frames` and `is_at_rest`; the ring is allocated for 5 ms when the
guard is built. At 0 the guard takes the zero-latency branch it always had,
and a guard set to 3 ms and back is bit-identical to a fresh one on a signal
that goes over. Above 0, a burst train up to 25 dB over is ducked by the ramp
on every frame, with the final clamp never needed (the test reads the gain
each frame left under). Nothing sets it yet: step 03 reads it from the
master's strip, and step 04 draws the box.
