# 03 — Tell the node that time moved

Read `00-status.md` and step 02 first. This step closes the gap
`AUDIO_ARCHITECTURE.md` already admits to, in its DSP Node Contract:

> Latency and tail are implemented; "Rest And Tail" below states what they
> mean and what a host may do with them. **Reset and transport discontinuities
> are not.**

## The gap

A node cannot be told that time stopped being continuous. So when the executor
needs to say it, it says the only thing a node can hear: it synthesises
`Event::Choke` into every channel's event list (`render.rs:2880`).

That conflates two different statements. *Let go of these notes* is a
performance gesture. *Time is no longer continuous* is a fact about the host.
A node that hears only the first has to guess, and the guesses have already
diverged: `Choke` is `release_all()` in MonoSynth, PolySynth, ML-M1 and ML-P8,
and a hard fade in Sampler, DrumSynth and DS-01. Neither is wrong. They are
answers to a question nobody asked out loud.

Everything that is not a voice hears nothing at all. A delay line's tail, a
reverb's, a `Smoothed` parameter mid-ramp, a WSOLA reader's splice position:
all of them carry on across a seek as though the audio in them still belongs
to the position the transport is now at.

## The change

A defaulted method, following the precedent `tail_frames`, `is_at_rest` and
`skip_block` set — the default means "this changes nothing for me", so a node
that has not opted in behaves exactly as it does today:

```rust
fn on_discontinuity(&mut self, _kind: Discontinuity) {}
```

`Discontinuity` starts with the three the engine already distinguishes
internally: `Seek`, `Stop`, and `ProgramChange` (step 02's overhanging-note
case). The kind matters — a synth may want release on a program change and a
hard fade on a seek, and today it cannot express the difference because both
arrive as the same event.

Rules to write into the contract, all three already learned in this codebase:

- **Called before the block's events**, at a defined point relative to
  `process`, so a node that resets and then receives a note-on at offset 0
  behaves deterministically. `push_ordered` sorts a choke ahead of a note-on
  at the same offset today; the hook has to be at least as well specified.
- **No allocation, no lock.** It is the callback.
- **Free-running state keeps running.** The "Rest And Tail" section's second
  rule applies unchanged: an LFO that keeps its phase across silence must keep
  it across a seek too, or a bounce stops matching a take. A node that wants
  that behaviour gets it by *not* implementing the hook, which is the right
  default.
- **A node that cannot honour it declines in writing**, the way Aux In, the
  retained-audio buffer and ML-P8's chorus decline the rest-and-tail contract
  in their own comments rather than by omission.

## What it costs

This touches every `AudioNode` implementor, and AGENTS.md has a warning aimed
squarely at that shape: *"when a trait has one required method and a dozen
implementors, read all twelve before believing they differ."* That is how the
two hidden copies of the effect event-splitting loop were found — a byte-level
search missed them because somebody had renamed the loop variables on the way
past. So: read every `fn process` in `mooloop-dsp` before adding the method,
not after.

The payoff is that `release_all_voices` stops being the engine's only way of
saying anything, and step 02's overhanging-note case becomes expressible
instead of being rounded up to "choke everything".

## What it does not do

It does not remove `Event::Choke`. A choke is a real musical gesture — it is
what a choke group does (`inject_choke_events`, `render.rs:2845`) and that
usage is correct and stays. What changes is that the host stops borrowing it
to mean something else.

## What landed, 2026-09-20

`Discontinuity` (`Seek`, `Stop`, `ProgramChange`) and a defaulted
`AudioNode::on_discontinuity`, with the contract written into `node.rs` and
`AUDIO_ARCHITECTURE.md`. The engine says it at three sites: a seek or a loop
fold (`Seek`), `EngineCommand::Stop` (`Stop`), and a Pattern-mode switch under
a running transport (`ProgramChange`). The fan-out reaches every channel's
generator and effect chain and every bus's chain, **including sleeping and
bypassed slots** -- those are precisely the ones holding audio they would emit
on waking.

Four devices opted in, and the split between them is the point:

| Device | On a seek or stop | On a program change |
| --- | --- | --- |
| Delay | line cleared, read head re-seated, damping reset | nothing |
| Modulation | line, feedback, tone and phaser cleared -- **LFO phase kept** | nothing |
| Reverb (FDN) | pre-delay, diffusers and lines cleared, line modulation phase kept | nothing |
| Plate | pre-delay, combs and allpasses cleared | nothing |

Aux In and the retained-audio buffer decline in writing, as the plan asked.
The buffer's refusal is the one with teeth: its ring is a performance somebody
is playing, and clearing it on a seek would take a gesture away mid-flight --
the same thing `holds_frozen_audio` refuses a ring resize to prevent.

**The voice path was left alone.** `release_all_voices` still synthesises
`Event::Choke` for a seek, and the synths still answer it as they did. The
hook is what makes an alternative *expressible* -- which is how the plan
framed the payoff -- but migrating the voices onto it changes what a seek
sounds like, and that is a behaviour change to ask about rather than to slip
in beside a contract addition.

**One node is not reached.** The console channel strip is not an `AudioNode`,
so its EQ and compressor state still crosses a seek. It has a `reset` of its
own and closing the gap is small; it is recorded in `AUDIO_ARCHITECTURE.md`
rather than done, because nothing has reported hearing it.

**What a seek now sounds like, and it is a real change.** Reverb and delay
tails stop ringing across a seek instead of continuing over the new position.
That is what the plan asked for in as many words -- *"all of them carry on
across a seek as though the audio in them still belongs to the position the
transport is now at"* -- but it is audible, and if the ringing turns out to be
wanted, the kind check is where to say so.
