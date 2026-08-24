Arduino/ESP32 adapter - kinda like what SimHub does for controlling LEDs, but for MIDI events

---

automation events - if we x,y them to beat, we could provide a second value to offset x. this could be a way to have both arbitrary, unsnapped positioning of events, and the necessary data to accurately display automation events on a tracker lane. this doesnt need to be a full .it editor or whatever - i just would like a nice way to manually type in what you want the automation to be doing, so basically we're just setting values and curve shapes if appropriate. there could be some other special cases in our own devices - specifically the buffer thing seems like it could benefit from this a lot. if we go this far with it, we might also be able to say that we can do some basic math or generative/algo mutations in that interface? maybe that starts getting to far into shell land...hmmm...but what if our automation engine is basically script powered? that opens up the console idea and feels like it moves toward node based devices, which would just be little gui blocks of code like that basically, or dsp. hmm.

idea review: I’ll ground this against the current product and buffer-engine direction, then separate the immediate tracker/automation design from the larger scripting and node-system implications.

The repository already makes the key commitment: parameter events are sample-timed, stored in natural units, and resolved as base value plus modulation offsets. The missing design decision is therefore the authored-event representation and its editor, not a new automation engine.

Yes. I think the right center is a typed automation event list, with the graphical lane as a view of it, not its source of truth.

Store one canonical exact position in PPQ ticks, but present it in tracker form as:

```text
BAR.BEAT   +TICKS   VALUE     SHAPE
02.03      +11      0.740     linear
02.04      -3       0.120     hold
```

`+TICKS` is the useful second X value. It makes the row readable and editable on a musical grid while allowing any position the engine can represent. Crucially, it should be derived from and round-trip to absolute ticks; it should not become a second timing system. Changing the visible row resolution then cannot move events.

For values, use the target parameter’s natural units, as already intended by [MODULATION_PLAN.md](/home/adam/projects/mooloop/docs/MODULATION_PLAN.md:45): Hz, dB, buffer offset, window length, rate, etc., rather than a generic 0–1 column. The table becomes a very good manual-writing interface:

```text
01.01  +00  follow       hold
01.02  +09  380 ms ago   jump
01.02  +21  -1.00x       linear
01.03  +00  return-live  instant
```

That is particularly strong for the buffer device. Some targets should be ordinary continuous parameters; others should be discrete actions. `Return Live`, `Freeze`, `Jump`, and loop-mode changes should be typed as actions, not represented as magic numeric values. The buffer then feels composed rather than merely “automated.”

I would model each automation record as:

```text
time: absolute_tick
value: natural parameter value or typed action
shape_to_next: hold | linear | exponential | smooth
```

The shape belongs to the outgoing segment, which makes a tracker table and a curve lane describe the same thing cleanly. Start with `hold` and `linear`; add exponential/smooth only where the target has a musical reason for it. Internally, curve segments can be compiled into the existing sample-timed parameter stream at a declared control resolution, while exact event boundaries remain sample-accurate.

On scripting: I would keep it out of the realtime automation engine initially. The promising route is a deterministic, offline “event transform” layer:

```text
selected events -> expression / transform -> preview diff -> undoable event changes
```

That supports math, probability, Euclidean-ish operations, scale/offset, reverse, quantize, density, and generative mutation without introducing an audio-thread interpreter, a console as the main workflow, or an accidental Max/MSP clone. It also fits the current focus: selection plus undo are prerequisites for usable mutation.

Later, that transform language could grow into a constrained control-script device, but I would make two boundaries explicit:

- **Automation scripts** generate or transform persisted event data.
- **Modulators/devices** produce bounded realtime control signals through the existing parameter/modulation path.

Those are related artistically, but they have very different determinism, safety, save/load, and UI requirements. Keeping them separate lets Mooloop become expressive without becoming a shell first.

The current product direction already supports this: “one automation language,” sample-timed `ParamValue` events, and the buffer’s sequenced read-head behavior are all aligned. The next design work is an automation-event document that defines the event schema, curve compilation, tracker notation, and typed buffer actions.
