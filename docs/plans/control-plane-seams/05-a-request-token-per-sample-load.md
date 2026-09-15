# 05 — a request token per sample load

Two decodes into the same channel are ordered by which one *finishes*. Load a
long file, then change your mind and load a short one, and the short one can
land first and be overwritten by the long one. The user clicks sample B and
hears sample A.

## The state today

`crates/mooloop-session/src/sample.rs:21`:

```rust
pub struct LoadResult {
    pub channel: usize,
    pub source_revision: u64,
    pub new_channel: bool,
    pub result: Option<Result<LoadedSample, String>>,
}
```

`source_revision` is a *project/source* revision, not a request identity. The
pump accepts any completion matching it:

```rust
let still_current = {
    let st = st.borrow();
    load.source_revision == st.session.source_revision
        && (load.new_channel && st.session.channels.len() < MAX_CHANNELS
            || !load.new_channel && st.session.channels.get(load.channel)
                .is_some_and(|channel| channel.kind == DeviceKind::Sampler))
};
```

Two in-flight loads for one channel both pass that check. Last completion
wins, and completion order is decode time, which is file size and page cache.

The awareness is already there and stops one granularity short. The same
function carries this comment:

> A `Vec`, not an `Option`: two "Load in New Channel" decodes can land in the
> same 60 Hz tick, and an `Option` silently kept the last — one sample gone and
> one channel created where two were asked for.

That is the *same race*, found and fixed for the new-channel case. The
existing-channel case has it still.

## What to do

A monotonically increasing token per channel, on the session.

1. `Session` holds `sample_request: [u64; MAX_CHANNELS]` (or a `Vec` alongside
   `channels` — whichever survives a channel removal more honestly; a removal
   shifts indices, which is the same hazard `install_project_in_ui`'s
   `bus_meters_stale` comment describes, so prefer keying by whatever durable
   channel identity exists rather than by index if one is available).
2. Every dispatch site bumps it and copies the new value into the spawn.
   There are four: `lib.rs:11548`, `:11559`, `:11570`, and
   `spawn_browser_sample_load` at `:13209`.
3. `LoadResult` carries `request: u64`.
4. `still_current` additionally requires `load.request == current token for
   that channel`. A stale completion is dropped silently — it is not an error,
   it is a superseded request, and logging it would be noise.

For `new_channel` loads the token is not per-channel, because the channel does
not exist yet. Those are already handled correctly by the `Vec` deferral, and
the cleanest thing is a separate monotonic counter for them or simply leaving
that path as it is. Do not force one mechanism over both if it makes the
new-channel path worse — the existing comment is there because somebody
already got that case wrong once.

## Acceptance

A `mooloop-session` or pump-level test that dispatches request 1 and request 2
for one channel, delivers 2 then 1, and asserts the channel holds 2's sample.
`lib.rs:14193` already asserts on a `LoadResult`'s `source_revision`, so
there is a place for it to go and a shape to follow.

Rung 2 on `mooloop-session`. The pump change means a `mooloop-ui` build on the
box before handing over.

## Note

This is the smallest step here and the one most likely to be noticed by a user
as "it just did the wrong thing once" rather than as a bug worth reporting.
Those are the ones that accumulate into an impression.
