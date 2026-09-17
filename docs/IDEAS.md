# Ideas

Loose notes and a design conversation about them, kept because they may
become plans later. Nothing here is scheduled or decided. Last audited
September 2026.

---

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

For values, use the target parameter’s natural units, as already intended by [MODULATION.md](MODULATION.md): Hz, dB, buffer offset, window length, rate, etc., rather than a generic 0–1 column. The table becomes a very good manual-writing interface:

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

---

**2026-09-05, on the entry above.** Adam's 1.0 mockup
(`reference/img/mooloop-1.0-mockup.png`) draws the modulation panel's modulator
as a tracker — numbered rows, a value column, a per-row cursor, and
PATTERN/CONTROL/PLAYBACK/MAPPING tabs — which is the same notation this entry
proposed for automation events, arriving from the other direction. They are not
obviously the same feature: automation events are *persisted document data* on
a timeline, while a modulator is a *bounded realtime control signal* with no
document behind it, and this entry's own closing paragraph is the argument for
keeping those two apart. But they clearly want the same editor.

That is the thing to settle before either is built, and it is a real fork:
one tracker widget over two different backing models, or one of them borrowing
the other's look and nothing else. `FOCUS.md` parks the modulation-rack
redesign *on* this question: it is the first thing that redesign has to answer,
and it is why the move is not a step of `interface-iteration/`.

**2026-09-09, a third thing that wants the same answer.** Adam's morning list
asked for the playlist to work "more like DAW lanes", to zoom to fit the
patterns in a song, and to carry **song-level automation** -- automation
authored against the arrangement rather than against a pattern.

The first two are layout and are not this entry's business. The third is,
because it is the same fork one level up. Today automation is
`AutomationLane`s stored per channel per pattern, so a curve is a property of
a pattern and is replayed wherever that pattern is placed. Song-level
automation is a curve that belongs to the *timeline*, and it needs the same
three things the entry above is arguing about: a canonical event
representation, an editor notation, and a decision about whether the
tracker/curve editor is one widget over several backing models or several
widgets that resemble each other.

So the fork is not two-sided, it is three:

| | Backed by | Lives on |
| --- | --- | --- |
| Automation events | document data | a pattern's timeline |
| A modulator | a realtime control signal, no document | nothing; it is bounded and free-running |
| Song automation | document data | the arrangement's timeline |

Song automation is much closer to the first than to the second, which is
mildly good news: if the tracker is built for automation events, song
automation is the same widget over a longer timeline and a different address
space. It is recorded here rather than in a plan because it is *the same
question*, and answering it separately is how a project ends up with two
editors that nearly agree.

**Adam, 2026-09-17: for now there is no song-level automation at all.** A
ramp across two plays of a pattern is made by cloning the pattern and drawing
half the ramp in each, as in Impulse Tracker. He also expects song automation
to be how long-form audio is eventually handled, at least from the user's
side, now that audio recording goes into the sampler
(`plans/audio-recording/`). So when this question is answered, that is a
second use case for it.

Recorded while writing `docs/plans/archive/console/`, which is the mixer half of the
same morning list and which deliberately does not touch the playlist.

---

## Node-based patching in the device rack

Folded in from `IDEAS.md` on 2026-09-14, which was a 108-line document
whose own status line said *"not scheduled, and not a plan"* — which is this
file's job. Recorded direction, 2026-08-31. Nothing depends on it.

**Why it is here at all.** Adam likes how node-based systems look and work and
wants the option kept open. It was not arrived at by discovering a need. That
is a legitimate reason for a personal instrument, and it is exactly why it is a
direction rather than a plan: a want that has not met a workflow should not
reorder a roadmap.

**The corollary matters more than the idea.** Liking how node editing *looks*
is not the same as needing the graph architecture *underneath* it. Visible
connections and signal you can watch move are largely UI properties; the
expensive part is typed edges, buffer ownership, cycle policy and delay
compensation. A device that could merely *display* its internal signal flow,
read-only, would test the appetite at a fraction of the cost.

**The shape, and how it is not The Grid.** Bitwig's Grid is a blank canvas that
*replaces* the instrument. Here the devices would stay opinionated finished
instruments — externals — and the patching would happen *around* them in the
ordinary rack: note objects before a synth, control objects into a knob, audio
objects between devices. Same primitives as a modular environment, opposite
default: the Grid starts empty, this starts as a working instrument you unfold.
That is not a new position — `PRODUCT.md` already rules out Max/MSP-scale
patching as the ordinary workflow.

**Three domains, three very different prices** — the most useful thing the
original document had:

| Domain | Example | Cost |
| --- | --- | --- |
| Note / event | alternate velocities 64/127 before a synth | **cheap** — serial, in order, no latency, needs nothing new |
| Control | a modulator through user math into a knob | **medium** — sources, destination policy and rates exist; needs assemblable math objects |
| Audio | split, detune, mix back — a hand-built chorus | **expensive** — typed edges, buffer ownership, cycle policy, delay compensation |

The audio row is the catch, and that example is a parallel path.

**If it is ever proven, prove it in the note domain first.** A velocity
alternator needs no new graph shape, no compensation and no buffer ownership,
and it exercises the whole model end to end: an object in the rack with a
declared note-in/note-out boundary, saved as a fragment, dropped on an existing
channel. If that feels good the direction is real; if it feels like ceremony,
that was learned for the price of one small device.

**A declared boundary is what makes fragments saveable.** Without one a
fragment can only be stored as a whole channel, because nothing knows where
else it may legally go. With one, the browser question answers itself: an
insert point *is* a known boundary, so the browser offers only fragments whose
signature fits. That is the same problem the ML-M1 factory bank hit one level
down (`plans/archive/preset-system/00-status.md`).

**Deliberately undecided**: whether a node view is a separate editor, a rack
row expansion or a whole-channel view; whether users author objects or only
wire shipped ones; whether the graph is rewireable at runtime or compiled per
edit; and any visual design. None of it needs answering to keep the option
open. What keeps it open is the three habits in
`COMPOSABLE_DEVICE_UNITS.md`, which are worth following regardless.
