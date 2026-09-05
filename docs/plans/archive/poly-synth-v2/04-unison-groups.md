# Native modulation

ML-P8 has its own modulation because a polysynth needs per-voice sources and a
saved patch must stand on its own. The channel modulation rack remains the way
to bring in other channel sources and to send ML-P8 outlets elsewhere.

## The ML-P8 LFO

The device owns one global, audio-rate LFO with:

- Wave: sine, triangle, ramp, pulse, sample-and-hold, chaos;
- Rate: free Hz or the shared musical-division vocabulary;
- Phase;
- Warp: phase asymmetry for periodic waves and probability bias for stepped
  waves;
- Slew: rounds discontinuities and turns random targets into wandering motion;
- Retrigger: Free / Chord / Note.

`Chord` retriggers only when a Note On arrives while no note gate is already
held. `Note` deliberately resets the global LFO on every Note On, including
inside a chord; status text must warn that later notes move modulation on
earlier ones. Free is the default.

Chaos is deterministic, bounded, and continuous. It must not call a runtime
RNG or merely rename sample-and-hold. A small fixed-state chaotic recurrence
or feedback oscillator is appropriate if its range and sample-rate behavior
are tested. Warp and Slew should make this LFO identifiable in motion without
requiring a novel waveform for novelty's sake.

The `ML-P8 / LFO` outlet in step 06 samples this same LFO state at its declared
control ticks. Do not run a second reduced or UI-only oscillator for the
outlet.

## Per-voice sources

The internal route system exposes:

| Source | Shape | Scope |
| --- | --- | --- |
| LFO | Bipolar | One global value applied to each voice |
| Amp Envelope | Unipolar | Current physical voice |
| Filter Envelope | Unipolar | Current physical voice |
| Velocity | Unipolar | Current note/group |
| Key | Bipolar around middle C | Current note/group |
| Gate | Gate | High while that note event is held |

A Trigger is a momentary event and belongs on trigger/reset inlets, not as a
continuously sampled modulation value. It is nevertheless published in step
06.

Oscillator and noise signals do not appear in this control-rate source list.
They already have the audio-rate XMOD paths from step 02. Collapsing them to a
slow value would create a misleading form of FM.

## Internal routes

An internal route is:

```text
source -> polarity/amount -> per-voice destination offset
```

The initial destination vocabulary includes:

- oscillator 1/2/3 pitch and pulse width;
- oscillator 1/2/3 source level;
- Sub level and Noise level/color;
- every XMOD, noise-mod, oscillator-feedback, and Voice Feedback amount;
- filter cutoff, resonance, env amount, and drive;
- VCA level and pan.

Only continuous destinations are legal. Waveform, sync source, filter mode,
Sub source/octave, Unison, and Chorus mode are structural and cannot be
flapped by an internal route.

Filter Env Amount, keytrack, Amp Velocity, and Filter Velocity remain dedicated
controls because they are fundamental playing behavior. The route list is for
additional relationships: Filter Envelope to XMOD, Velocity to feedback, LFO
to noise color, Amp Envelope to oscillator balance, and so on.

Resolve each destination as authored base plus the sum of internal route
offsets, then clamp through the destination's descriptor mapping. Channel
automation/modulation resolves the authored base before ML-P8 applies its
per-voice offsets. This preserves one understandable center value.

## Timing and prepared topology

Envelope and LFO values are already available per sample, so internal routes
are evaluated at audio rate. Route topology is compiled on the control thread
into flat fixed-capacity operations grouped by destination. The audio callback
does not search descriptors, allocate rows, or match strings.

Persist a dynamic list with durable route IDs. The first realtime compiler may
have an explicit measured safety boundary, initially proposed as 16 active
routes. That number is a callback-work limit, not a panel with sixteen empty
slots or a permanent product promise. The UI shows authored routes plus **Add
route**. If the compiler rejects an over-capacity patch, it reports the reason
and preserves the authored route rather than silently dropping it.

Route source and destination changes are structural edits prepared off the
audio thread. Signed amount is an ordinary automatable parameter with a stable
address tied to the durable route identity. An internal route cannot target
its own amount; that prevents an undeclared control feedback cycle.

## UI

The MOD page has a compact ML-P8 LFO editor followed by the route list. Adding
a route creates one row with Source, Destination, polarity, signed Amount, and
remove. Selecting a source and touching a destination may be added as a direct
assignment gesture if it edits this same route data; no second hidden routing
model is allowed.

The heading must say **ML-P8 MOD** or equivalent. The common frame continues to
say **MOD** for the channel shelf. This small naming distinction prevents a
saved instrument route from being mistaken for a channel route.

Reserve ML-P8 IDs 55-63 for LFO controls. Route amount addresses use their
durable route IDs through an explicit internal-route owner/address form rather
than consuming an arbitrary permanent block of generator parameter IDs.

## Done when

- A patch using only ML-P8 state can route its Filter Envelope to oscillator
  XMOD, Velocity to Voice Feedback, and LFO to filter cutoff simultaneously.
- Two notes with different velocities and envelope phases receive different
  per-voice results; neither is collapsed to a last-note channel value.
- Free, Chord, and Note retrigger policies differ exactly at documented Note On
  boundaries.
- Chaos, random, and every periodic mode render bit-identically twice and stay
  bounded for long offline renders.
- Internal base-plus-offset resolution agrees with descriptor ranges and with
  a simultaneously channel-modulated base.
- Route amount automation is sample-timed through the ordinary event path and
  does not rebuild topology.
- Structural edits prepare outside the callback, swap safely, and produce no
  allocation or descriptor lookup in `process()`.
- A complete moving patch works with the channel MOD shelf empty.
