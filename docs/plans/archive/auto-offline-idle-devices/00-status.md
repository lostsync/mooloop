# Auto-offline idle devices plan status

**All three steps are in, 2026-09-05. The plan is finished and archives with
this entry.**

A thirty-two channel project with one channel playing costs about a tenth of
what it did per block. A thirty-two channel project with thirty-two channels
playing costs what it did, because nothing is skipped. What either renders is
unchanged, at any block size.

Measured on the laptop, release, 256-frame blocks, four effects a channel
(EQ, filter, delay, reverb), steady state:

| Project | Skipping off | Skipping on | |
| --- | --- | --- | --- |
| 32 channels, 1 sounding | 10,130 us/block | 1,057 us/block | **9.6x** |
| 32 channels, 4 sounding | 11,268 us/block | 2,109 us/block | **5.3x** |
| 8 channels, 1 sounding | 2,729 us/block | 554 us/block | **4.9x** |
| 32 channels, 32 sounding | 10,657 us/block | 10,690 us/block | 1.0x |

A 256-frame block is 5,333 us of wall clock, so the first row is the
difference between twice over budget and a fifth of it. The last row is the
one that had to be checked rather than assumed: the mechanism costs nothing
when there is nothing to skip.

## Step 01 landed: `AudioNode` can say it has nothing to do

`tail_frames` and `is_at_rest`, both defaulted to "never skip me", plus a
third method the plan did not anticipate (below). Twelve effect kinds and
seven generators implement them; three devices decline in writing.

`every_effect_kind_is_silent_once_it_says_it_can_be_skipped` and
`every_generator_is_silent_once_it_says_it_is_at_rest` drive the real device
past the point the host would have stopped calling it and fail if anything
comes out. That is what makes the numbers trustworthy rather than plausible,
and it caught a deliberately shortened reverb tail on the first try.

Four things the doing changed about the plan.

- **`is_at_rest` is about the node, not about the audio.** The plan suggested
  a memoryless effect could answer "have I seen non-silent input recently".
  It cannot: once asleep it sees nothing, so it could never say no again. The
  host counts input silence — it is the only side that can — and the node
  answers only for its own state. The two combine as
  `input silent && (state settled || tail elapsed)`, which is an `and` where
  the plan wrote an `or`; skipping on rest alone would sleep a bitcrusher
  under a full-scale signal, since a memoryless device's state is always
  settled.
- **A tail must cover any ring a parameter can move a head inside.** A
  sleeping node stops advancing its delay lines, so a delay time swept up
  afterwards would read audio from before the silence. Every tail is floored
  at the capacity of such a ring: the delay's two seconds, the reverb and
  plate pre-delays.
- **Bitcrush's dither makes noise out of nothing.** Its TPDF spans a whole
  quantiser step, so `quantize(0 + dither, step)` lands on plus or minus a
  step about half the time whatever the input was; at four bits that is
  -18 dBFS of hiss on a channel that has stopped playing. A dithering crusher
  never reports rest while its wet is heard, and
  `a_dithering_crusher_never_reports_rest_while_its_wet_is_heard` says so.
- **A one-pole ramp in `f32` does not reach its target.** It reaches a fixed
  point of its own recurrence and stops — for the default gate, a fiftieth of
  a dB short of full attenuation, so "close enough to shut" was never going to
  be true. The gate asks whether the next step *would be a step*. Everything
  else gates on `Smoothed::is_settled`, which is exact because the lag snaps,
  and on `EnvelopeFollower::is_at_rest`, which is exact for the same reason.

The threshold is `SILENCE_PEAK = 1e-7`, one bit below a 24-bit render's least
significant step. Nothing discarded could survive an export at the deepest
integer depth the application writes.

### The method the plan did not anticipate

**`skip_block`.** Freezing a sleeping device was the plan's whole model —
"waking must not reset state" — and it is right about input-driven state.
It is wrong about state that runs on the clock. A reverb's line modulation, a
chorus's LFO, a crusher's sample-and-hold counter and a poly synth's
free-running LFO all advance whether or not anything is sounding. Frozen, a
hall came back somewhere else; worse, *how far* depended on the host's buffer
size, which would have taken block-size-independent rendering with it — and
that is the property a bounce matching a take rests on.

So the host calls `skip_block` in place of `process` for a block it skips, and
implementations mirror the sample loop rather than closed-forming it: a phase
that arrived by a different route is a different phase.

One case inside that was worth the hour it took to find.
**`DelayLine::read` derives its interpolation fraction from `write - 1 -
offset`**, so where the write head sits is part of the answer, and an `f32`
holds fewer fractional bits at 2000 than at 200. A frozen ring came back
reading between different samples. The delay and the modulation stage keep
writing silence through theirs — two stores a frame against the Hermite reads
they save. `DelayLine` could take its fraction from the offset alone and stop
caring; that is a real improvement and it is *not* made here, because it would
move the output of the delay and the buffer device by a few bits for reasons
unrelated to this plan.

### Who declines, and why it is written down

- **Buffer** plays back what it captured. It is the one device in the rack
  that makes sound out of a silent input by design.
- **Aux In**'s sound is another channel's, and it can start without an event
  of its own. Sleeping it on the strength of its own quiet would drop the
  first block of whatever it is subscribed to.
- **ML-P8 with its chorus switched on.** The finisher is a delay line and an
  LFO on the output of the voice sum; keeping them in step through a sleep
  would mean reproducing its state machine outside it. With the chorus off —
  `Off` is a true bypass that clears the line, and seven of the eight factory
  patches use it — the instrument sleeps like every other.

## Step 02 landed: effect slots with nothing passing through

`EffectChain::process` skips a slot whose input has been silent for longer
than its device's tail, or whose device says its state has settled, once the
slot's dry-path aligner has emptied.

**Silence detection is free**, which the plan asked for and which turned out to
be better than free. The chain already scanned each slot's bus for its input
meter; that scan moved to the top of the slot, before the input trim, and the
meter reads it scaled. Peak is linear in a non-negative gain, so it is the
same number — and the chain now does one scan a slot where it did two.

**Nothing is published and nothing is cleared.** The plan asked for zeroed
meters; they are not needed. `DeviceMeters` cells are peak-hold and the GUI
empties them as it reads, so not writing one *is* publishing silence. Queued
parameter events stay queued the way a bypassed slot's do, so a knob turned
while a channel was quiet lands when it wakes.

`silent_frames` lives in `EffectSlot`, not in a `[u32; MAX_EFFECTS_PER_CHANNEL]`
beside the nodes: that is what `EffectSlot` exists for, and a `u32` fits in its
existing padding, so the per-slot footprint did not move at all.

## Step 03 landed: whole idle channel strips

A strip is skipped when it has no events this block, its generator is at rest
and has been putting out silence, and every occupied slot in its chain would
be skipped. Bus strips are untouched, as the plan directed.

Two departures.

- **The event list has to be empty, not merely free of notes.** The plan said
  "no events this block" and the temptation was to read that as "no note
  events", since a `ParamValue` on an idle generator makes no sound. It cannot
  be: a generator splits its block at every event it is given, so a strip that
  slept through a modulated source parameter would advance its free-running
  state in one stride where a running one took several, and the two would not
  agree to the bit. A channel whose source is being driven keeps rendering,
  which is also the honest reading — something is still moving in it.
- **The generator's silence is measured at its output, not derived from a
  tail.** `source_peak` was already being read for the source device meter, so
  the strip counts there. That covers every reason a device might still be
  sounding after its voices retire — a finishing stage that outlives them,
  for one — without the host having to know such a stage exists.

Emptying the strip's bus and its compensation ring happens once, on the way
down, rather than every idle block: nothing writes either while the strip is
asleep. Playhead positions *are* written every block, because they are stored
rather than peak-held and would otherwise pin the last sounding voice's head
in the UI.

## What holds it honest

`RenderState::set_idle_skipping` exists so a render can be run twice and the
two compared sample for sample. A mechanism whose whole claim is that it
changes nothing has to be checkable against the thing it claims not to change.

- `a_project_renders_the_same_whether_or_not_idle_channels_are_skipped` —
  three channels with gaps, reverbs, a delay and long releases, twelve
  seconds, both ways. It also asserts that at least a quarter of the
  channel-blocks were actually skipped, because a skip that never fires would
  pass every test here.
- `skipping_renders_the_same_at_any_block_size` — exactly, at 128 and 1024.
  This is the one that fails when free-running state is frozen, and it is why
  `skip_block` exists.
- `a_channel_that_never_sounds_reaches_the_master_either_way` — bit-identical
  against the same project with the channel removed.
- `an_aux_in_channel_is_never_slept_out_of_its_producer` — the producer is
  muted, so every sample of that render arrives through the edge.
- `skipping_never_changes_what_a_tail_sounds_like` and
  `every_effect_kind_comes_back_where_it_would_have_been` — burst, sleep,
  burst, held frame by frame, for a four-device chain and for all twelve
  kinds.

The tolerance where it is not exact is `SILENCE_PEAK`, with room for the gain
of whatever stands after the device that fell asleep: a slot going to sleep
with its input sitting on the threshold hands the rest of the chain a signal
up to the threshold different, and an EQ band may put 24 dB on that. Sixteen
times the threshold is -116 dBFS, under one step of a 20-bit render. A
truncated tail would miss by five orders of magnitude, which is the distance
these tests are really measuring.

## Not done, and deliberately

**Bus strips still run.** The plan said to leave them, and the reasoning at
`render.rs`'s bus loop still holds: buses are few, shared, and a muted one
deliberately keeps processing so its tails decay. Their effect *slots* sleep
like everyone else's, which is most of what a bus costs anyway.

**The channel modulator tick pass still runs for every live channel.** It is
outside this plan and it is not obviously safe to skip: a modulator's phase is
supposed to keep moving so an unmuted channel's knobs do not jump. Worth
measuring separately if it turns up in a profile.
