# 02 — publish a sample and its slices as one fact

A channel's audio buffer and its slice map are documented as one fact and
published as two. A note-on landing between the two stores plays the new
buffer against the old markers.

## The state today

`crates/mooloop-session/src/engine.rs:174` says it plainly:

> Both are always sent together because they are one fact: after a commit the
> published buffer and the map that indexes it change at the same instant, and
> delivering one without the other would leave the voice reading markers that
> name frames in a buffer it no longer holds.

And the transport honours it: `ChannelAudio` carries both fields through one
`mpsc` send. The contract is lost one layer later, in three places.

- The pump unpacks it into two calls (`crates/mooloop-ui/src/lib.rs`, the
  `ChannelAudio` drain — `handle.load_sample(...)` then `load_slices(...)` /
  `clear_slices(...)`).
- Those land in two independent stores: `sample_slots` and `slice_slots`,
  declared separately at `crates/mooloop-engine/src/lib.rs:472`.
- The sampler reads them independently too — `sample_slot.load_full()` at
  `crates/mooloop-dsp/src/sampler.rs:634`, `slice_slot.load_full()` at
  `sampler.rs:656` — with voice selection and rate resolution in between.

So there are two windows, not one: between the two stores, and between the two
loads. A single atomic snapshot closes both.

## Why it bites hardest after a stretch commit

A stretch commit changes the buffer's **length**. A slice marker from the old
map is then a frame index into a buffer of a different size — past the end, or
landing a bar early. That is an audible wrong note or a silent voice, arriving
once, on a note that happened to fall in a few-microsecond window. It will be
blamed on the stretch code.

Memory: extreme stretch artifacts are a target sound here, not a failure mode,
which makes this *harder* to notice — a slice playing the wrong material does
not obviously read as a bug in a project built on mangling.

## What to do

Replace the two slot arrays with one array of snapshots.

```rust
/// What a channel's voices read. Replaced whole; never edited in place.
pub struct ChannelAudioSnapshot {
    pub sample: Option<Arc<SampleData>>,
    pub slices: Option<Arc<SliceMap>>,
}
```

- `EngineHandle` holds `Arc<Vec<Arc<ArcSwapOption<ChannelAudioSnapshot>>>>`.
- `load_sample` / `clear_sample` / `load_slices` / `clear_slices`
  (`lib.rs:580`–`602`) collapse into one `set_channel_audio(channel,
  snapshot)`. Keeping a `load_slices` that reads the current snapshot and
  stores a modified copy would reintroduce exactly the race, so **do not** keep
  the four as conveniences.
- `Sampler::trigger` does one `load_full()` and reads both fields off it.
- `publish_channel_audio` builds the snapshot; `ChannelAudio` the message type
  can become that snapshot plus the channel index, or go away.

Check the other readers before assuming two call sites: `lib.rs:617` and
`lib.rs:792` also reach into the slots, and the preview path has its own
relationship with `sample_slots`.

## Acceptance

The strong form is that the bug becomes unrepresentable — there is no API that
sets one without the other, so no test can construct the half-updated state.
Say that in the commit message; it is the reason to prefer the snapshot over
a sequence-number check.

Alongside it, one `mooloop-dsp` test that triggers a voice against a snapshot
whose slice map indexes a *shorter* buffer than the one published, and asserts
the voice is silent or clamped rather than reading out of range — the failure
mode the old comment describes, now reachable only by constructing it
deliberately.

`cargo test -p mooloop-dsp -p mooloop-engine -p mooloop-session`, then rung 3.

## Leave for step 03

The slot array is *shared across render generations* — `RenderState::new` takes
a clone of it (`lib.rs:641`) — which is a second, separate race. Do not try to
fix it here. Step 03 is about exactly that, and it is easier once this step has
given it one type to move.
