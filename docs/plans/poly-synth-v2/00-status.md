# ML-P8 plan status

Not started.

This plan now defines **ML-P8**, a new eight-voice polysynth. It replaces the
earlier Poly v2 design, whose identity depended too heavily on three stacked
oscillators, per-voice drift, unison, and chorus. Those can all make a sound
wider; none makes the oscillator section more programmable.

The original Poly synth remains as its own device. ML-P8 is not a rename or an
in-place migration of it, and old Poly projects continue to load unchanged.

Read 01 first, then work 02 through 07 in order:

1. `01-what-poly-is.md` is the product and DSP contract.
2. `02-per-voice-drift.md` builds the oscillator network, sub, and noise.
3. `03-the-multimode-filter.md` adds the two envelopes, multimode filter, and
   per-voice feedback loop.
4. `04-unison-groups.md` adds ML-P8's native LFO and internal modulation
   routes.
5. `05-internal-chorus.md` finishes allocation, optional drift, unison, and
   chorus without making duplication the instrument's identity.
6. `06-oscillator-sync.md` publishes the instrument's typed control and audio
   outlets.
7. `07-poly-factory-patches.md` is the listening, range-tuning, and identity
   pass.

The filenames are retained so existing references to this plan do not break;
their headings describe their new scope.

The separate filter ADSR and keytracking that used to be an unnamed
prerequisite are now part of step 03. The device's own modulation is part of
step 04. The channel modulation rack remains useful for reaching other devices
and for adding channel-level sources, but ML-P8 must make complete patches with
no channel routes at all.
