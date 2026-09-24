# The master bus compressor

`SCOPE.md` item 13 (MOO-13), built together with the safety limiter's
lookahead control (MOO-169). `FOCUS.md` step 3. Written 2026-09-23, from
Adam's answers of the same day.

## What Adam asked for

2026-09-14: *"I'm totally cool with us using the master strip to house the bus
comp -- that's only natural -- but it needs to be more prominently displayed in
the rack, with a nice meter, and it should be running its own algos, aimed at
like SSL, 2500, and idk Massive Passive or something. Turning it on should feel
special."* The third unit was settled the same day as vari-mu (the Fairchild
670), because a Massive Passive is an EQ.

2026-09-23, on MOO-13: **the face is per voicing.** SSL and API show attack
and release; vari-mu swaps both for one six-position TIME selector, as the
unit has.

2026-09-23, on MOO-169: *"make it a knob, defaults to 0.0"*, *"on the device
face. its usually just a small knob or scrollable number box."* At 0 the
limiter is what it is today, bit for bit. Above 0 the added latency is
reported like any other, so take alignment and monitoring account for it.

## The decisions this plan makes

1. **It lives in the master's strip.** The parameters are a
   `MasterSectionParams` inside `StripParams`, ids appended to the strip's own
   table from 44, so they cross to the engine as `SetStripParam` and load with
   the strip: no new `EngineCommand`, no new install path. Every track's strip
   carries the struct (a few dozen bytes, like the strip itself) and only the
   master's runs; the session refuses the ids on any other track.
2. **It runs after the master's inserts and before its fader.** A mix-bus
   compressor sits on the insert point, and a fade-out on the master fader must
   not pull the mix out of compression on its way down.
3. **Its own law table, not new values in the channel strip's.** Three
   voicings, each a detector shape, a static curve and a timing law, and none
   with programme dependence (`SCOPE.md` §2.1). `mooloop_dsp::strip`'s
   `process_comp` is not touched.
4. **Each voicing has its own controls, and each keeps its own settings.**
   Grip's ratio, attack and release, Punch's, and Tube's TIME are separate ids,
   so switching voicing is swapping a unit in the rack rather than
   reinterpreting three knobs. The shared controls are In, Voicing, Threshold,
   Makeup and Mix. No control shows a range its voicing does not have.
5. **A knob reads the unit's marking, and the law is the measurement.** The
   rig measured each marked position (`spikes/preamp-measure/RESULTS.md` §4);
   the law table holds both, so an SSL release marked 0.3 s recovers to 63% in
   114 ms, as the unit does, and the status bar says so.
6. **Ratios are honest.** Both VCA units delivered about 3.5:1 when set to 4:1
   on the static curve. A knob reading 4:1 here delivers 4:1; the difference is
   recorded and not modelled.
7. **The safety limiter's lookahead is a guard setting, stored with the
   master section.** Default 0 ms, range 0-5 ms. A lookahead of L delays
   everything leaving the master by L, so a take from the hardware input waits
   L longer before it starts, the graph's reported latency grows by L, and an
   export trims L from its head so a file still starts on the bar line.

The voicing names follow the channel strip's convention of names that are not
trademarks: **Grip** (the SSL G-bus law, as the strip's Grip is the SSL
channel), **Punch** (API-2500, as the strip's Punch is the API), and **Tube**
(vari-mu). They are persisted by name, so renaming one later is a
serde alias, not a migration.

## Steps

| Step | What |
| --- | --- |
| [01](01-the-laws.md) | The three laws, as data and as a detector, against unit tests |
| [02](02-the-lookahead.md) | The safety limiter looks ahead when asked, and is bit-identical when not |
| [03](03-the-master-section-runs.md) | It runs on the master, is saved, meters, and keeps takes and exports aligned |
| [04](04-the-face-and-the-meter.md) | The per-voicing face, the gain-reduction meter, and the lookahead box |

`00-status.md` records what each step found. Linear holds the state: project
*Master bus compressor*.
