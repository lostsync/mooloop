# ML-P8 factory patches and listening pass

This step tunes the deliberately musical ranges and proves that ML-P8 is more
than a supersaw followed by chorus.

## The bank

| Patch | What it proves |
| --- | --- |
| Init Saw | One oscillator, eight clean voices, honest reference level |
| Crosswire Brass | Filter Env drives XMOD; velocity shapes filter and amp |
| Furnace Stab | Voice Feedback, drive, and LP24 make a hard percussive stab |
| Cold Metal | Bidirectional XMOD plus sync reaches stable inharmonic spectra |
| Sub Pressure | Derived sub remains solid beneath noise-modulated carriers |
| Servo Pad | Local LFO and per-voice envelopes move without channel routes |
| Broken Choir | Differently tuned oscillators interact, not merely stack |
| Wide Machine | Unison, Drift, Spread, and Chorus demonstrate the finishers |

At least the first seven patches use **Unison 1x** and **Chorus OFF**. At least
four use **Drift 0**. If those patches do not sound clearly distinct, fix the
network, modulation destinations, or ranges; do not turn on a duplicator.

Each patch must be reachable from Init Saw without hidden channel modulation
or insert effects. The saved patch includes all native routes it needs.

## Decisions to tune and record

Use the bank to settle these ranges, then replace the provisional language in
the corresponding step with measured values:

- maximum XMOD phase deviation and its knob curve;
- oscillator self-feedback and noise-mod scaling;
- Sub balance and Noise Color range;
- LP24 resonance distribution and cutoff compensation;
- positive/negative Voice Feedback bounds and drive compensation;
- ML-P8 LFO Warp, Slew, and Chaos behavior;
- Unison Detune maximum and Drift deviations;
- Chorus I/II fixed policies and whether a Mix control is genuinely needed.

The upper quarter of destructive controls should be wild but navigable. If a
one-percent knob move traverses all useful timbres, change the curve. If the
maximum is merely louder, change the topology or compensation.

## Automation and modulation abuse pass

The point of exposing these controls is to move them. Exercise full-range,
sample-timed automation on:

- every directed XMOD amount and the three self-feedback amounts;
- Noise Color and noise-to-oscillator amounts;
- Voice Feedback, drive, cutoff, resonance, and Filter Env Amount;
- internal route amounts, LFO Rate/Warp/Slew, and envelope times;
- Sub Level, Detune, Spread, and Drift.

Automate several together on an eight-note chord. The result may be abrasive;
it may not click accidentally, diverge, allocate, depend on oscillator
iteration order, or produce non-finite samples.

Structural automation gets separate transition tests for waveform, sync
source, filter mode, Sub source/octave, Unison, and Chorus mode. If a selector
cannot switch safely on a live voice, mark it non-automatable and document the
note-boundary behavior rather than pretending smoothing solves topology.

## Published-interface patches

Add two small routing fixtures outside the factory bank:

1. `ML-P8 / LFO` modulates a downstream delay or filter while continuing to
   modulate ML-P8 internally.
2. Gate resets a compatible downstream source, Trigger advances a step source,
   and the focus Filter Envelope modulates a downstream parameter with the
   documented one-block latency.

When typed audio edges are implemented, add a third fixture in which muted Osc
3 feeds a compatible audio consumer through its published pre-Level outlet.

## Whole-instrument checks

- Render the full bank twice in one process and once in a fresh process; all
  three renders are bit-identical.
- Measure eight-note worst cases under the gain contract. Honest summing may
  exceed 0 dBFS; the node must remain finite and must not normalize other
  voices or sources. Factory patches should still leave intentional headroom.
- Verify callback cost at the supported sample rates with eight active voices,
  the measured internal-route safety boundary, filter feedback, and outlet
  publication. No Cargo or live-audio procedure bypasses
  `docs/AGENT_OPERATIONS.md`.
- Transport stop and choke release envelopes and clear feedback/chorus tails
  according to their documented behavior.
- The original Poly device and old projects remain unchanged beside ML-P8.

## Done when

- All eight patches exist and reach their named territory.
- The seven non-finish patches remain convincing at Unison 1x and Chorus OFF.
- Crosswire Brass, Cold Metal, Furnace Stab, and Servo Pad use four materially
  different internal modulation relationships.
- Every provisional range above has a measured answer recorded in its step.
- ML-P8 plays eight ordinary notes, steals complete groups, remains
  deterministic, and meets the realtime callback budget.
- A musician can make a moving, velocity-responsive, feedback-heavy patch
  without opening the channel modulation shelf, then publish ML-P8's own
  signals to make the rest of the channel move with it.
