# Latency compensation for the existing mixer tree

`AUDIO_ARCHITECTURE.md`'s migration sequence, step 5. Its own header says
steps 6 and 7 depend on it, and step 6 is the typed audio and dependency edge
work `FOCUS.md` step 1 is waiting on.

## Why this is first

`FOCUS.md` and `AUDIO_ARCHITECTURE.md` disagreed about the order. FOCUS listed
typed audio edges as the prerequisite and parked compensation under
"deliberately not now"; the architecture document lists compensation as step 5
and says step 6 waits on it. Adam settled it on 2026-09-05: if compensation is
on the path either way, building the edge type against an uncompensated tree
means building it twice.

That is also the honest reading of the code. `CompiledBusGraph`
(`mooloop-core/src/mixer.rs`) carries destinations and a render order and
**no latency at all**. `DryAlign` (`mooloop-dsp/src/align.rs`) aligns one
effect container's dry branch against its own node, which is migration step 4
and is inside a single slot; nothing aligns one channel against another.

## What is wrong today, concretely

Two channels feeding the master. One has a Drive, which oversamples and
reports 32 frames of latency; the other does not. They sum at the master
32 frames apart. Nothing detects it, nothing corrects it, and the result is
comb filtering that gets worse the more correlated the two channels are — a
kick doubled onto two channels is the worst case and the most common one.

It is currently near-invisible because Drive is the only device that reports
latency, and 32 frames is 0.7 ms. It stops being invisible the moment
limiter lookahead, plugin hosting, or a parallel send exists, and every one of
those is on the roadmap.

## The rule, from the architecture document

> Each node reports integer latency frames initially. The graph compiler sums
> serial latency and inserts compensation on shorter inputs at every summing or
> dependency point. Compensation storage is allocated before activation.

And the reason the tree case is worth doing on its own:

> The current one-destination mixer is cheap to compensate because each node
> has one downstream audio edge.

Each channel feeds exactly one bus and each bus feeds exactly one bus, so
"delay every shorter input by the difference" collapses to one delay per
producer, sized to its own deficit. There is no DAG to solve yet. Building it
now is what makes step 6's general rule an extension rather than a rewrite.

## What this plan is not

Not parallel sends, not sidechains, not plugin delay compensation for hosted
plugins, and not the typed edges themselves. Those are steps 6 and 7 and the
product tasks that pull them. This plan ends when the tree the application
already has is time-aligned and says so.

Bypass deliberately keeps its node's declared latency, per the same document:
toggling a bypass must not move the channel in time.
