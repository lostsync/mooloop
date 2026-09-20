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
