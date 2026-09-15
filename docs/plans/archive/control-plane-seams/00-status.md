# Control-plane seams status

What has landed, and what each step found that its own file could not have
known. `README.md` is the brief; this is the record.

## 01 — command delivery is observable, 2026-09-15

`send`, `send_structural`, `preview` and `add_channel` return whether the
command reached the ring, all four are `#[must_use]`, and the five reconcilers
advance their mirrors only on a delivery that happened.

**The step needed a seam it did not name, and that is its main finding.** Its
acceptance test is "fill the command ring, run a reconciler, assert the mirror
did not advance" — and there was no way to write it. An `EngineHandle` can only
be built by `Engine::new`, which opens JACK or Core Audio; nothing in the
workspace had ever constructed one in a test, which is why a defect this
mechanical survived in five places. So `send`/`send_structural`/`sample_rate`
moved onto a `CommandSink` trait that `EngineHandle` implements and the
reconcilers take generically. That is the whole of the abstraction: three
methods, one shipping implementor, and a test double in
`crates/mooloop-session/tests/delivery.rs` that can be full.

Two things came out of doing it rather than out of planning it.

- **`compensation_sent` could not stay a `CompiledLatency`.** A plan is
  compiled as a whole and is true as a whole; what the mirror has to record is
  sixteen plus eight independent deliveries, any subset of which the ring may
  have refused. `CompensationSent` replaces it — the two arrays, no `sends`
  and no `total`, because this reconciler never sends those. The step file
  said "the fix there is per-entry, not one bool at the bottom"; what it
  costs is a type, because the old mirror's shape made all-or-nothing the only
  expressible answer.
- **`#[must_use]` found one call site the plan had not counted.** `add_channel`
  sends structurally and swallowed the result, so the pump's `AddChannel`
  branch marked the document dirty for a channel the engine may never have
  been told about.

The refusal log is once per session, not once per refusal: the condition that
produces it — a burst of edits against a full ring — produces it many times in
a row, and a line per refusal would bury the first one. A reconciler's refusal
is recoverable by construction now, so what the line is for is the case where
it keeps happening.

## 02 — a sample and its slices as one fact, 2026-09-15

`ChannelAudioSnapshot` holds a channel's buffer and its slice map; the two
slot arrays became one; `Sampler::trigger` does one `load_full()` and reads
both fields off it. `load_sample`, `clear_sample`, `load_slices` and
`clear_slices` are gone — there is one `set_channel_audio`, and **no API that
sets a buffer without its markers**. That is the acceptance, and it is why the
step is worth more than the bug it closes: the half-updated state is not a
case that is now handled, it is a value that cannot be constructed.

The `mooloop-dsp` test that remains
(`a_slice_map_for_a_longer_buffer_is_clamped_rather_than_read_out_of_range`)
has to build the mismatched pair by hand, and it checks its own fixture first:
six of its eight markers are past the end of the buffer, asserted through the
same `SliceMap::span` call `trigger` makes. Without that assertion it would
pass on a fixture that was accidentally in range, which is the failure
`AGENTS.md` catalogues six times over.

Three things found on the way.

- **`EngineHandle::sample_snapshot` had no callers at all.** It was the only
  other reader of the sample slots, documented for "non-realtime display work
  such as waveform peak generation" — which the UI does from
  `Session::sample_snapshots`, a different function with a nearly identical
  name. Deleted rather than ported.
- **`RenderState::from_project` built its two slot arrays in two passes**, and
  the slice pass carried a comment recording that it had once been omitted
  entirely — "an exported mix was silent exactly where the app was not". The
  two passes are one now, so that omission is no longer expressible either.
- **The built-in sample reset published a buffer and left the markers
  standing.** A reset fires when a channel is added or switched to the
  sampler, so the markers it left were from the channel's previous life. It
  now publishes an empty snapshot, which is a fix the step did not go looking
  for and got for free from having one call instead of two.

And one thing the repository found rather than the step:
**`the_render_graph_costs_what_the_project_uses` failed**, because a
`ChannelStrip` got eight bytes *smaller* — the sampler held two slot pointers
and holds one. That test's own instruction is to check a new figure is one
worth paying before updating it, and this is the first entry in its list that
subtracts. Both pinned figures moved: 42,224 to 42,216 per strip, 60,664 to
60,656 per live channel.

The slot array is still shared across render generations. That is step 03, and
it is untouched here on purpose.

## 03 — a project installs with its samples, 2026-09-15

`install_project` takes the channel snapshots **by value** and builds its own
slots; `install_project_in_ui` composes them before it queues anything, so the
early return that already covered the project now covers the assets with no
second bail-out path.

**The decision the step said to price was a false choice.** It offered
"whether the handle keeps a bank at all, or whether per-channel publication
goes through a command", and suggested a plain `Vec<Option<..>>` without
atomics for the prepared generation. Neither survives point 4 of the same
step: loading a sample into an open song has to reach the live generation, and
a generation whose bank has no atomics cannot be published into at all. The
answer is a third thing — **a fresh bank per generation, still atomic** — and
what the handle holds is the bank of the most recently *prepared* generation,
not the live one. That is the detail that makes the window correct rather than
merely narrower: a publication between queueing and the switch belongs to the
incoming project, and lands where the incoming project will read it.

Taking the snapshots by value is what carries the guarantee. There is no way
to hand `install_project` the live generation's slots, so the sharing is not
avoided by care — it is not expressible.

The acceptance test the step describes cannot be written: it wants
`install_project`, and an `EngineHandle` needs an audio driver. That is the
same wall step 01 hit. What is tested instead is the property install now
rests on — `a_generation_plays_the_bank_it_was_prepared_with` renders two
prepared generations whose buffers differ by 9:1 and holds the ratio, so
neither can be reading the other's bank — and the signature carries the rest.
The ratio, not the levels: what reaches the master has been through the
sampler's trim and the pan law.

### And the republication after the install turned out to be a defect

The step asked whether that loop was still needed. It is, but only for a
channel with a committed stretch: `replace_project` re-renders the commit on
the line above, and that buffer cannot have been in the bank because it did
not exist yet.

Running it over **every** sampler channel was wrong, and had been.
`published_sample()` is `committed_sample.or(sample_data)`, and
`replace_project` fills `sample_data` from `samples` alone — it does not apply
the legacy `SampleReference::Builtin` substitution that the bank-building loop
does. So opening a project old enough to carry a `Builtin` reference published
the default kick and then, four lines later, cleared it: the channel was
silent while its name, waveform and duration all described a kick. The loop is
guarded on `commit.is_some()` now, which a channel without a commit could
never have needed.

## 04 — no growing containers on the callback thread, 2026-09-15

Both retirement vectors became one `RetiredPreviews`, with
`MAX_RETIRED_PREVIEWS` moved where both halves can see it. A refused push
**hands the sample back**, which is the step's own first option and the only
one with no failure mode: the renderer leaves the finished voice in place and
retries next block, the executor defers the whole `Preview` command the way
`executor.rs` already defers an install. The reclaim drain is partial rather
than all-or-nothing.

### The harness was already in the crate

**`FOCUS.md` has recorded since `buffer-implementation/` Stage 1 that
acceptance test 8 — no allocations or locks in the callback — "needs an
allocation-tracking harness rather than a reading of the code", and the
harness was three lines on an allocator this crate has installed all along.**
`CountingAllocator` tracks `live()` bytes, which `block_cost` uses to ask what
a prepared project *holds*. A net byte figure cannot see an allocation paired
with a free in the same block, and that pair is exactly what a `Vec` growing
on the audio thread looks like. So it grew `allocations()`: a count of `alloc`
and `realloc` calls that never decreases.

Two details that are the difference between a harness and a decoration.

- **Per thread, not process-wide.** `block_cost`'s module doc already records
  what a shared global counter costs — it insists on `--test-threads=1`, and
  documents a run where three tests sharing a thread pool measured each other
  and produced a figure that looked like an answer. A thread-local `Cell`,
  `const`-initialised so touching it inside `alloc` cannot itself allocate and
  read through `try_with` so teardown cannot panic, needs no such flag.
- **`realloc` is counted explicitly.** The default `GlobalAlloc::realloc` is
  alloc-copy-dealloc and would have been seen, but `System` overrides it with
  `mremap` — so a growing `Vec`, the one thing this exists to catch, would
  have been invisible.

**The guard was validated against the defect**, which `AGENTS.md` requires and
which its own `one-sided-test` section exists because somebody once did not:
with `preview_retired` put back to `Vec::new()` and the bound removed,
`a_block_that_retires_a_preview_does_not_allocate` reports 1 allocation and
fails. With the fix it reports 0.

**What it does not close.** Test 8 claims no allocations *or locks* anywhere
in the callback. This measures allocations on one block, the one that retires
a preview, which is the path step 04 is about. The locks half is not measured
at all and no wider claim should be read into this.

## 05 — a request token per sample load, 2026-09-15

`Session::sample_request` is a `Vec<u64>` parallel to `channels`, minted by
`next_sample_request` at each of the five dispatch sites and checked by
`sample_request_is_current` in the pump's `still_current`. A superseded
completion is dropped silently — it is not an error, it is an answer to a
question the user has already changed their mind about.

**The step named four dispatch sites; there are five.** It counted
`spawn_browser_sample_load` as one, and it has two callers — "load into the
selected channel" and "load in new channel". The token is minted in the
callbacks, where the session is already borrowed, rather than inside the spawn
helper, so the helper stays a pure dispatcher.

The step asked for a decision on removal and offered "prefer keying by
whatever durable channel identity exists rather than by index if one is
available". **There is none.** `ChannelState` has no id; the durable
identities in this codebase are `DeviceId` on a rack row and a channel is
still a position. So the map is by index and is permuted in
`rescope_after`, which is the one place that already re-points every other
index-keyed thing a channel edit moves — the automation lane, the preset
names, the selected device. A completion whose seat changed hands fails the
comparison and is dropped, which is the conservative answer and the correct
one.

`new_channel` loads keep token 0 and are not checked, as the step directs.
That path is already correct for a different reason — the pump defers them
into a `Vec` and creates one channel per load — and its comment exists because
somebody got that case wrong once.
