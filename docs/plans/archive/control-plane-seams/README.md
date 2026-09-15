# Control-plane seams

An outside architectural review on 2026-09-15 read mooloop as a living system
rather than as a diff: document → session → structural command → audio thread →
UI feedback, then the subsystems that stress those seams. Its verdict on the
layering was good and is not what this plan is about. Its verdict on the
boundary was:

> The main weakness is the control-plane boundary, especially the UI pump:
> several logically atomic operations are expressed as sequences of independent
> mutations and fallible queue writes. That is where most future "the UI says
> one thing, the engine does another" defects will originate.

Five concrete defects came out of it. Each was checked against the source on
2026-09-15 and each is real. This directory is the five of them, plus the sixth
item — which is a direction and not a step, and is at the bottom of this file
rather than in a numbered one.

`docs/archive/ARCHITECTURE_REVIEW.md` is the precedent for a plan arriving this
way, and is worth reading beside this one.

## The shape all five share

**The code knew the right rule and applied it one layer too shallow.** That is
worth stating before the steps, because it changes what the fix is: none of
these needs a design decision, and none of them is a case of nobody having
thought about it.

- `ChannelAudio`'s doc comment states the atomicity contract outright — "both
  are always sent together because they are one fact" — and the pump then
  unpacks the one fact into two stores.
- `EngineHandle::install_project` already returns a delivery result, and
  `install_project_in_ui` already checks it and bails. The other sends on the
  same handle discard theirs.
- The load pump already reasons about a same-tick race, in a comment, for the
  `new_channel` case — and stops one granularity short of the same race on an
  existing channel.
- `AUDIO_ARCHITECTURE.md` already says queue overflow must be observable to the
  sender and that silent divergence is not an acceptable contract. Five
  reconcilers advance their mirrors as though every send landed.

So these are not five separate omissions. They are one habit — writing the
contract down next to the code instead of into it — and the steps are mostly a
matter of moving each contract one layer inward, where the compiler or a test
can hold it.

This is the same fault class `FOCUS.md` names for 2026-09-10 to 2026-09-12: *a
claim the source no longer supported.* The difference is that those were claims
a face made about a value, and these are claims a comment makes about an
ordering.

## The steps

| Step | Seam | Size |
| --- | --- | --- |
| [01](01-make-command-delivery-observable.md) | Session mirrors advance on sends that were dropped | Small |
| [02](02-publish-sample-and-slices-as-one-fact.md) | A note can pair a new buffer with old slice markers | Small |
| [03](03-install-a-project-with-its-samples.md) | The old render graph can read the new project's samples | Medium |
| [04](04-no-growing-containers-on-the-callback-thread.md) | Preview retirement allocates in the audio callback | Small |
| [05](05-a-request-token-per-sample-load.md) | An older decode can overwrite a newer one | Small |

01, 02, 04 and 05 are an afternoon each or less. 03 is the only one that
changes what a structural command carries, and the only one worth thinking
about before starting.

**Order matters only between 02 and 03**, which touch the same slots: do 02
first, because the snapshot type it introduces is what 03 then moves into the
prepared generation. Everything else is independent.

## What is audible, and what is only wrong

Worth being honest about, because three of these have never been reported and
one probably never will be.

- **02 is the one a user will hit.** After a stretch commit the buffer's
  *length* changes, so a slice marker from the old map points past the end or
  into the wrong bar. Intermittent, unreproducible, and exactly the kind of
  thing that gets blamed on the stretch code.
- **03 is reachable by opening a song while the previous one plays**, which is
  an ordinary thing to do.
- **01 needs the command ring full**, which needs a burst of edits. It has
  probably never happened. When it does it produces a routing or compensation
  state that is wrong *and stays wrong*, because the diff that would resend it
  has already been satisfied.
- **04 is a contract violation that is unlikely to be audible.** The first
  preview retirement allocates; the allocator is fast and the block has slack.
  It matters because it is the class of thing that only bites under load.
- **05 needs two decodes into one channel finishing out of order**, which
  wants a long file then a short one. The user clicks sample B and hears
  sample A.

## Verification

All five are below `mooloop-ui`'s test binaries except where the pump itself
changes, so rung 2 (`cargo test -p mooloop-engine`, `-p mooloop-session`)
covers most of the work and rung 3 closes each step. `AGENTS.md`'s ladder
applies as written; none of this needs a workspace run to iterate.

Each step names its own acceptance test. Four of the five are testable
without audio: a full ring and a mirror that did not advance, a snapshot that
cannot be half-applied because the type does not permit it, and a stale token
that is refused.

**04 is the exception and it is the one already on the books.**
`buffer-implementation/`'s Stage 1 acceptance test 8 — no allocations or locks
in the callback — has been unverified since it was written, and `FOCUS.md`
says it needs an allocation-tracking harness rather than a reading of the
code. That harness would settle 04 and test 8 together. Building it is more
work than 04 itself, and it is the only thing here that could reasonably grow
into its own step.

## The sixth item is a direction, not a step

The review's last priority was to break the UI/session seam into cohesive
adapters: `Session` exposes a very large mutable surface, `UiState` mirrors
much of it into Slint models, and `refresh_editor` hand-projects hundreds of
device properties. Nothing there is a defect. What it costs is that every new
callback site has to remember six things — mutate, mark dirty, push history,
send to the engine, update telemetry subscriptions, refresh the right scope —
and one place to forget one of them is the drift `LOOSE_ENDS.md` already
catalogues.

The suggested shape is sound: a session edit returns an explicit effects value
(engine messages, history entry, refresh scope) and one applier consumes it.

**It has no numbered step here on purpose**, for the reason `device-registry/`
has none: whether it is worth doing is a `FOCUS.md` question, and writing steps
would presume the answer. Two things are true about it either way. It is the
natural endpoint of step 01, which already forces delivery results back through
those call sites — so 01 is a down payment on it whether or not it is ever
taken. And it is a direction new work can adopt one device family at a time,
while somebody is already in the file, which is a different and much cheaper
thing than a migration branch. `session-layer-extraction/00-status.md` is the
prior art and already records that `UiState::new` stayed long and that the
pump's meter polling stayed in the view deliberately.

## Not in scope

- Anything about how the UI is *drawn*. That is `egui-view-layer/`.
- The reclaim ring's own sizing. It has a defined overflow path already
  (`executor.rs:188` defers the command rather than dropping it) and the review
  did not fault it.
- Plugin-facing parameter identity. `SCOPE.md` has it.
