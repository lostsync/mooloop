# 03 — a project installs with its samples

Opening a song queues the new render graph through the command ring, then
immediately overwrites the shared sample bank out of band. Until the audio
thread consumes the structural command, the **old** project's graph is reading
the **new** project's samples — and mid-loop, a half-replaced bank.

This is the only step in the directory that changes what a structural command
carries. It is worth thinking about before starting.

## The state today

`install_project_in_ui` (`crates/mooloop-ui/src/lib.rs`) gets the hard part
right and says so:

```rust
// Queue the complete state first. If the bounded realtime queue is full,
// leave both the sample slots and visible project untouched.
if !handle.install_project(Arc::new(project.clone())) {
    return false;
}
```

Then, in the next sixteen iterations, it calls `handle.load_sample(index, ..)`
or `clear_sample(index)` per channel — and those are `ArcSwap` stores that take
effect *now*, not at the callback boundary the install will switch at.

The mechanism is visible in one line. `install_project` builds the replacement
renderer with

```rust
let mut render = RenderState::new(
    self.sample_rate,
    self.sample_slots.clone(),
    self.slice_slots.clone(),
);
```

— a **clone of the same `Arc`**. There is one bank, shared by every generation
that has ever existed. The prepared project is a complete, internally
consistent object in every respect except the assets it plays.

Reproducer: open a song while the previous one is playing. The window between
queueing and the block boundary is a block or two, and the loop over sixteen
channels runs inside it.

`AUDIO_ARCHITECTURE.md`'s control-plane list already claims the opposite —
"a structural edit produces a complete, internally consistent object", and the
audio side never observes "a node without its prepared buffers". A sample is
a prepared buffer. That document does not need changing; the code needs to
agree with it.

## What to do

The bank travels with the prepared generation and switches at the same
callback boundary.

1. `RenderState::new` takes an **owned** bank rather than a clone of the
   handle's — after step 02, that is a `Vec<Arc<ArcSwapOption<ChannelAudioSnapshot>>>`
   or, better here, a plain `Vec<Option<ChannelAudioSnapshot>>`, since a
   freshly prepared generation has no concurrent reader yet and does not need
   the atomics.
2. `install_project` takes the bank as an argument, built by the caller
   alongside the project. `install_project_in_ui` composes both *before* it
   queues anything, and queues them together — so the existing early return
   already covers the assets, with no second bail-out path.
3. The old generation's bank is reclaimed the way the old graph is, through
   the existing deferred-destruction route. Do not drop it on the callback
   thread.
4. Per-channel publication after install (`load_sample` / `set_channel_audio`
   for a sample the user loads while a project is open) keeps working as it
   does today, against the **live** generation's bank. That path is not the
   bug and should not get slower.

The decision worth making deliberately: whether the handle keeps a bank at all
once the generation owns one, or whether per-channel publication goes through
a command that addresses the live generation. Keeping the handle's bank is
less work and is probably right; owning it in the generation is cleaner and
touches the preview path. Price both before picking — `install_project`'s
meter-reconnection block right below the `RenderState::new` call is prior art
for what "reconnect everything the new renderer needs" already looks like.

## Also in scope, because it is the same seam

`install_project_in_ui` republishes channel audio after `replace_project`, for
a stated and correct reason — a channel whose stretch was committed plays the
re-rendered buffer. Once the bank is built up front, check whether that
republication is still needed or whether it is now a second write of a fact the
prepared generation already carries. If it is still needed, say why in the
comment; if it is not, delete it.

## Acceptance

A `mooloop-engine` test that prepares generation B with a distinct bank,
queues it, runs the executor for a block **without** letting it consume the
command, and asserts the still-live generation A reads A's samples. Today that
test fails; there is no way to write it at all against a shared `Arc`.

Then the ordinary one: install, run to the boundary, assert B's samples.

Rung 3 before handing over. This touches `install_project`, which the UI calls
from four places (`lib.rs:4984`, `:11750`, `:12031`, `:12217`) — so a
`mooloop-ui` build is owed once at the end, on the box.
