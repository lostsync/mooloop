# 04 — no growing containers on the callback thread

Two retirement vectors can reallocate inside the audio callback. One of them
starts at zero capacity, so its **first** push allocates.

## The state today

`crates/mooloop-engine/src/render.rs:2768`, in `RenderState::new`:

```rust
preview_retired: Vec::new(),
```

and `render.rs:2841`, inside the preview mixing path, which runs in the
callback:

```rust
let voice = self.preview.take().expect("preview checked above");
self.preview_retired.push(voice.sample);
```

Audition a sample from the browser, let it play to the end, and the callback
allocates.

The executor's is better and still unbounded. `executor.rs:89` reserves
`MAX_RETIRED_PREVIEWS` (64) up front, and `executor.rs:228` drains into the
reclaim ring when there is room:

```rust
if self.reclaim_tx.slots() >= self.retired_previews.len() {
    for sample in self.retired_previews.drain(..) { ... }
}
```

But the pushes at `executor.rs:182` and `executor.rs:226` happen
unconditionally, *before* that check, and the drain is all-or-nothing on ring
space. A reclaim ring that stays full while previews keep retiring grows the
vector past 64 and reallocates.

Neither is likely to be audible. Both contradict the realtime contract in
`AUDIO_ARCHITECTURE.md` outright, and this is the class of fault that only
shows up when the system is already under load — which is to say, never in
testing and always in a session.

## What to do

The code is the easy part. **The decision is what overflow means**, and it
should be made once and written down rather than discovered per call site.

1. `preview_retired` gets `Vec::with_capacity(MAX_RETIRED_PREVIEWS)` at
   minimum. Better: both become the same fixed-capacity type, so there is one
   answer rather than two.
2. Enforce the bound on push. The options, in the order they are probably
   preferable:
   - **Hold the voice.** Refuse the retirement and let the preview sit one more
     block. Costs nothing, has no failure mode, and the next block almost
     certainly has room. This is likely the right answer.
   - **Leak deliberately.** Forget the `Arc` and let the sample outlive the
     session. Bounded by preview count, honest, and ugly.
   - Never: drop the `Arc` on the callback thread. That is a free on the
     audio thread, which is the thing the whole reclaim path exists to avoid.
3. Make the executor's drain partial rather than all-or-nothing — drain as many
   as the ring has slots for, instead of waiting for room for all of them. That
   alone makes the unbounded case much harder to reach.

## Acceptance — and the harness this shares with `buffer-implementation/`

A reading of the code is not acceptance for this one, and that is stated
elsewhere in the repo already: `buffer-implementation/`'s Stage 1 acceptance
test 8 — no allocations or locks in the callback — has been open since it was
written, and `FOCUS.md` records that it "needs an allocation-tracking harness
rather than a reading of the code".

**That harness would close both.** A counting global allocator, armed for the
duration of a callback, asserting zero allocations across a block that
retires a preview. It is more work than this step's fix and it is the only
thing in this directory that could reasonably become its own step.

So: take the fix, and either build the harness or say explicitly in the commit
that the fix is unverified and test 8 still is too. Do not close test 8 on the
strength of having read the diff.

The cheap partial check in the meantime is a debug-build counter on the two
pushes, run through a preview-heavy session — it proves the paths are reached,
which is worth having, and proves nothing about allocation.
