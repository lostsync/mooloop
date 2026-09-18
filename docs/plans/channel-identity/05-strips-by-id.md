# 05 — The engine keeps strips across an install

**Landed 2026-09-17.** The real fix `LOOSE_ENDS.md` names, for channels.

## What landed

A project install carries the live strip of any channel it did not change,
instead of building a new one and letting the old one leave with the retired
generation. A strip is where the voices, the effect nodes, the delay lines,
the reverb tails and the compressor envelopes are, so this is the whole of
what stops the dropout.

The decision is made on the **control thread** by `carry_plan`, which is the
only place both projects exist; the audio thread receives
`(outgoing index, incoming index)` pairs and swaps boxes. No comparison, no
reasoning and no allocation on the callback, which
`carrying_strips_allocates_nothing` measures rather than assumes.

## Simpler than the plan asked for

The plan proposed that a `ChannelStrip` record its `ChannelId`, that
`RenderState` keep an id-to-index map, that the two generations be matched by
*structure* (same device ids and kinds in the same order, same source kind),
and that a carried strip then receive the incoming project's parameter values
as events.

None of that is here. The match is `ChannelId` **and** `ChannelSetup`
equality, which collapses four mechanisms into one comparison:

- The id-to-index map is only needed if the *audio thread* does the matching.
  It does not.
- Structural equivalence plus parameter replay is a second mechanism to get
  wrong in exchange for nothing. If anything about a channel differs it is
  rebuilt -- and a channel that differs is, by definition, the one the user
  just edited, which is the one place a discontinuity is least surprising.
- Every case that makes a song audibly stutter still carries: a move, a
  paste, a delete, a track added, an undo. None of them change the setup of
  the channels they are not about.

The strip does not know its own `ChannelId`, and that is a real limitation
rather than a tidy-up left undone: an *incremental* channel edit -- the thing
that would remove the swap altogether -- would need it. Deferred with the
follow-up below.

## Two hazards that would have been silent

Both were found by asking what a carried strip holds that does **not** come
from its own channel's setup.

- **Compensation.** `install_compensation` derives each strip's delay from the
  *whole* project, so a channel that did not change can still be owed a
  different one because another channel altered the longest path into their
  shared bus. The freshly built strip's ring is swapped onto the carried
  strip; the stale one leaves with the discarded strip.
- **The audio slot**, which is the dangerous one. Every install builds a fresh
  `ChannelAudioBank`, and a strip binds to its slot when it is constructed --
  so a carried strip still holds the *retired* generation's slot while the
  handle publishes into the new one. It sounds correct at the moment of the
  swap, because the sample it is playing has not changed. What breaks is
  everything published afterwards: a sample loaded onto a reordered channel
  would land somewhere the strip never looks, and the channel would go on
  playing the old file indefinitely with nothing to say why.
  `a_carried_strip_reads_the_new_generations_audio_slot` asserts against slot
  identity rather than against audio, because audio would have passed.

The modulator rack travels with the strip too. A free-running LFO whose phase
restarted would step every destination it drives at the moment of an unrelated
edit, which is the same class of glitch in a different place.

## Test

Six, in `executor.rs`. The important one is
`an_install_keeps_the_voices_of_every_channel_it_did_not_change`, which is the
2026-09-17 measurement **inverted**: it used to assert that a null install
silenced the master, and now asserts it does not. It keeps the old behaviour
beside it as a control, so the comparison is against the same block rather
than against a memory.

Each of the two hazard fixes was verified failing with its fix disabled.

## What is still not done

**Tracks still take the whole-swap path.** A track added, removed or moved
rebuilds every strip in the song, because a track has no identity: this plan
deliberately left `TrackId` until step 05 existed to be copied, and now it
does. Until then, `LOOSE_ENDS.md` stays open for the track half.

More broadly, this removes the *destruction* rather than the swap. Adam's
question on 2026-09-17 -- why is there a whole-state swap at all for something
as cheap as adding a track, when adding a *channel* is already an incremental
`StructuralCommand` that misses no samples -- is not answered by this step,
and is the right next question. `06-the-remaining-cross-channel-addresses.md`
is unrelated; the incremental work wants a plan of its own.
