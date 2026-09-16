# Loose Ends

Small known gaps that agents flagged when handing work back, gathered into one
place so they stop living in chat scrollback. The file and line named is where
to start.

Everything here as of **2026-09-06** was re-verified against the tree that
day, and a second pass on **2026-09-14** re-read about a third of the file --
enough to find six entries that had stopped being true, or had never been.
Entries added since carry their own date, and entries older than the sweep
that covered them have not been checked against the tree since — the spike
list below was still claiming thirty-nine unpushed commits on `main` a day
after `main` was pushed, which is what this paragraph is now careful about.

**An entry goes stale three ways and only one of them is loud.** Three of the
six on 2026-09-14 had been fixed by work that never came back to delete the
row: the preamp grew the per-band display that the entry beside it had
*designed*, `Project` grew `pattern_meta`, and `BUFFER_ENGINE.md` grew the
caveat the entry said it lacked. Two were never about the tree at all -- a
screenshot that had been retaken and a branch list that had moved -- and those
are the ones a reader has no way to doubt.

The sixth is the one worth reading the code for. It described a real
`continue` skipping a real publish, and the state it produces **cannot be
reached**: nothing can subscribe to a sampler channel, so a sampler channel
never enters that branch. An entry can be accurate about the source and wrong
about the program. So: **check the claim before you act on it, and delete the
row in the commit that makes it false.**

This is not a roadmap and not a bug list. Everything here was a deliberate
stopping point rather than an oversight, and none of it blocks the sequence in
`FOCUS.md`. It exists so that a thing already known does not get rediscovered
as a surprise, and so an idle half hour has somewhere to look.

Scope rule: an item belongs here if it is **small, specific, and true of the
code right now**. A gap large enough to need a plan belongs in `docs/plans/`;
a wish belongs in `ENHANCEMENTS.md`; a described behaviour gap belongs in
`CURRENT.md`. When an item is fixed, delete the row — do not annotate it.

---

## Wrong-looking UI over correct behaviour

**A shelf's Q knob stops steepening above 2 and the face does not say so.**
`Biquad::shelf_slope` clamps the slope to 0.1..2.0, where the cookbook's
radicand goes negative; a seven-band EQ band's Q descriptor runs to 18, because
the same id has to serve that band as a bell. So **the top 46% of the knob's
travel** does nothing while the band is a shelf -- measured 2026-09-15 and
pinned by `the_shelf_q_knob_saturates_a_little_past_half_its_travel`; this
entry said "four-fifths" until then, which was estimated and nearly twice the
truth. That is better than
2026-09-14, when *all* of it did nothing (`Biquad::shelf` took no Q at all),
and is still a control showing a number the filter is not using. The channel
strip avoids this by giving its two shelf-capable bands a narrower Q range, and
that answer is not available here: every band can be any kind, so the range
would depend on a *value*, and a descriptor is static per id. Same shape as the
rest of `eq-v2` -- a parameter model that cannot express a condition. The
response plot does not hide it any more: since 2026-09-15 the curve is the
bank's own coefficients evaluated, so a shelf's drawn slope stops moving at
the same place its sound does. Found 2026-09-14.

**`EqSlope`'s variant names are half the slope they name.** `Db6` runs one
`Biquad::pass` stage, which is a second-order section and therefore 12 dB per
octave, so the five variants are 12/24/36/48/72 and are spelled 6/12/18/24/36.
The *face* was corrected on 2026-09-15 -- `EqSlope::db_per_octave` is the
arithmetic and `eq_face.rs` holds the selector to it -- and the variants were
left alone on purpose: `serde` writes them (`"db6"`), so renaming them either
refuses every saved project or silently re-maps one slope to another, to
correct a spelling nothing reads. Rename them on the next `FORMAT_VERSION`
bump that happens for a reason worth having one, with serde aliases for the
old names. Found 2026-09-14.

**The automation destination menu now offers fifty rows for one EQ.** That is
what per-band addressing means and it is not a defect -- "EQ 1 / B3 Freq" is
the thing a lane should be able to name, and the channel strip has contributed
twenty-eight rows since 2026-09-11 without anyone minding. It does make the
menu long enough that finding a destination by scrolling stops being pleasant,
which is an argument for filtering it, not for fewer ids. Recorded so the next
person to open that popup knows it was foreseen. Found 2026-09-14.


**The oscillator Level knob works in dB; its descriptor is linear 0–1.**
`device-oscillator.slint:95` drives the knob through `GainMath.linear-to-db`,
while `generator.rs:75`'s `unit()` helper declares the parameter as a linear
`0.0..1.0`. A modulation depth is a fraction of the *descriptor's* range, so
the drawn excursion arc on that one knob is in dB-space and misrepresents its
own width. Assignment and the audible result are both correct. Fixing it
properly means giving Level a gain curve, which touches automation and project
files — which is why it was left.

**~~Reverse and ping-pong refuse to stretch, and slice mode does too.~~**
Closed 2026-09-08. `mooloop-dsp/src/sampler.rs:495` still gates
`stretch_is_active` on `!reverse`, `loop_mode != Pingpong` and
`play_mode != Slice` — the behaviour is unchanged and correct. What was
missing was the explanation: the stretch toggle names which of the three it
is, and says to commit, in the status bar. The recorded fix was a
`hover-hint` property threaded through `main.slint`; `StatusHint` made it a
line on the toggle instead.

---

**The drop gap cannot show which side of a container's edge it lands on.**
The rack opens a one-row gap where a dragged device will land, and the gap
falls inside a container's box when the landing is inside it -- except at a
run's last row, where "just inside the box" and "just after the box" are the
same index and the gap is drawn in the same place either way. Which one a drop
means is decided by `move_effect` in `mooloop-session`, and the rack does not
know that rule: `main.slint`'s cell works the box out from `depth` and
`children` alone. Showing the resolved answer needs the session to publish the
depth a drop at index N would produce -- one `int` property and a callback on
each target change, not a redesign. Everything else about the drag is visible;
this one case still asks for trust.

**Dragging a container opens a one-row gap, not a run-sized one.** The rack's
drop gap is the width of the row being dragged, and dragging a container
moves its whole run -- so the gap it opens is right for a leaf and too small
for a box, and the devices inside the box do not travel with its face while
the pointer is down. `move_effect` does the right thing on release; it is the
picture during the gesture that is wrong. The cell publishes its own width
into `RackDrag.source-width` (`main.slint`), and a run-sized figure would have
to come from the session, which is the only thing that knows where the run
ends.

## Wired but unreachable

**Spectrum subscriptions are still keyed by slot index; they are now
re-stated after every rack edit.** The orphan is gone. The walk that syncs
them said `true` or `false` for each device that *is* an analyzer, so a stage
an analyzer had moved off was never mentioned and kept its subscription --
the engine ran a Goertzel bank every hop for a display nobody drew, whatever
took that slot number drew a flat line behind a lit button, and the orphan
held one of the sixty-four `SPECTRUM_SLOTS` until the project was reloaded.
It now walks *stages* and states the answer for each, one past the end of the
chain, and `UiState::sync_effects` -- the one function every rack edit already
calls -- raises a flag the pump consumes on its next tick.

Two things about the fix worth knowing before touching it. **One past the end
is only enough because of an invariant**: nothing above a chain's length can
be subscribed when the sync returns, and a project install, the one edit that
shortens a chain by more than one, calls `DeviceTelemetry::clear_spectra`
first. And it deliberately does *not* clear before re-stating, because
re-stating a correct subscription is an early return in
`set_spectrum_enabled` while a clear zeroes the bins -- which would make every
open analyzer in the program blink on an unrelated device drag.

What is unchanged is the keying, which was the third and largest of the
options the entry named: a subscription is still `(target, slot)` rather than
the device's durable id. Nothing drifts now, because nothing outlives the
tick that renumbered it, but an id-keyed subscription would not need the
re-sync at all. `spectrum_subscription_plan` has a test; the flag and the
pump's consumption of it do not, and could not without driving the
application. Found 2026-09-13, fixed 2026-09-14.

**A generator's internal route amounts are a working automation destination
no picker can reach.** The engine resolves `ParamOwner::SourceRoute` lanes per
control tick and emits `Event::SourceRouteAmount` for them
(`render.rs:4468`), `restore_base_param` has an arm for them, `integrity`
validates them, there is an engine test named for them
(`removing_a_route_lane_restores_the_authored_depth`), and
`Session::automation_descriptor` has an arm to turn their breakpoints into a
readout. `automation_destinations` never produces one -- it emits generator
and effect descriptors and nothing else -- and `open_automation_lane(index)`
indexes that list, which is the only path in from the app. So the descriptor
arm is unreachable code, and a file that *already* holds such a lane is worse
off than one that does not: `refresh_automation` clears a target it cannot
find in `destinations`, so the lane plays, persists, and cannot be seen,
edited or deleted. This is the exact mirror of the container-Mix item above --
there the picker offers what the engine cannot read; here the engine reads
what the picker does not offer. Not small because a route row is not a device
row: the descriptors come from `route_descriptors()` and each needs the
route's durable id, so the picker's rows need a label naming the *route*,
which is a naming decision on ML-P8's internal matrix rather than a loop over
a table. Either extend `automation_destinations` to walk
`base.internal_routes()` the way the engine does, or -- if route automation is
not wanted -- delete the engine's route pass and the dead descriptor arm,
rather than leaving a destination reachable only by hand-edited files. Found
2026-09-13.

**A container's Mix is offered as an automation destination the engine
cannot read.** `automation_destinations` (`session.rs:485`) walks every
slot's `kind.descriptors()` with no filter, and `EffectKind::Chain`'s table is
one entry, `Mix`. So "Chain 3 - Mix" lists in the lane picker, a curve draws
on it, it persists as an ordinary `ParamAddr` and it survives load and
reorder. The engine never reads it: the container branch in
`EffectChain::process` clears `state.events` and `continue`s without calling
`control_events_for_slot`, and `close_run` takes the mix from
`state.base_params`, which only `SetEffectParam`, `install` and `load` write.
The playhead crosses the curve and the audio does not move.

The contrast that makes it an oversight rather than a policy: **modulation is
not reachable the same way.** `ContainerDeviceFace` does not bind
`modulation-depths`, `modulation-allowed`, `modulation-offsets` or
`modulation-route-counts`, where every other face does -- so the Mix knob has
no depth ring and no drop target. Somebody decided that for modulation and
the automation picker never heard about it. Not small either way: filtering it
out matches the face but silently deletes any lane already drawn on one
(`refresh_automation` drops targets not in `destinations`), and making the
engine read it means a slot with no node has to carry a resolved parameter,
which today every device receives as a `ParamValue` event and a container has
nothing to hand one to. That is question 4 in
`docs/plans/containers/README.md`. Found 2026-09-13.

**Input trim and output trim on a container row are inert.** Both are read
only inside the leaf branch, past the container's `continue`, and both are
enabled unconditionally in markup -- including the output trim on the
*detached* tail rail (`main.slint:4544`), which is wired specifically to the
container, so somebody deliberately gave a box its own output trim and the
engine does not apply it. Unlike the shell's wet/dry, which was removed from
container rows on 2026-09-13 because a box has no node to be wet with, there
is nothing wrong with the idea: a gain going into a box and a gain coming out
of it are both meaningful, and the output one falls naturally out of
`close_run`. So this is apply-them or hide-them, not a deletion. If they are
applied, the input trim has to land *before* the dry copy is taken, or the
blend stops nulling at mix 0 and
`a_container_at_zero_mix_is_its_input_delayed_by_its_run` is what breaks.
Found 2026-09-13.

**The channel strip's parameters are not automation or modulation
destinations.** Every one has a stable id (`mooloop_core::strip`) and the
engine applies them by id, so the values are addressable; what is missing is
that a lane's target is an `EffectTarget` plus a *slot* and a strip is not a
slot. The ids start at 16 for this: `modulation::STRIP_PARAM_VOLUME` and
`STRIP_PARAM_PAN` are 0 and 1 of what is conceptually the same strip
(`ParamOwner::Strip`, already addressable by a route), so the two tables can
become one without renumbering anything automation has persisted. Recorded
2026-09-11 with step 03.

**Buffer MIDI mapping has no UI.** `EngineHandle::set_buffer_midi_map`
(`mooloop-engine/src/lib.rs:622`) is the only way to install one, and neither
`mooloop-ui` nor `mooloop-session` calls it. MIDI is decoded and routed; it is
just not reachable from the app.

**A mapped control carries no mark of its own.** As of 2026-09-15 the only
place a binding is visible is Preferences > MIDI: the knob it moves looks
exactly like an unmapped one. Drawing the mark is not hard, it is *wide* -- it
needs a per-parameter `[bool]` on every device face, beside the
`modulation-route-counts` model that already goes to all of them, which is a
line in fifty markup files and a `main.slint` crossing for each face that is
missed. `docs/plans/midi-control/04-interface.md` records the same thing as
the one piece of step 04 deliberately left out.

**`ParameterFader` cannot be learned, and neither can it be modulated.** The
learn gesture rides on `modulation-edit-started`, so its reach is exactly
modulation's reach, and the inline fader rows have never carried that
callback. Nothing is inconsistent between the two features; both simply stop
at the same place.

**Learning or removing a mapping is not undoable.** The control map is part
of the project and travels through `ProjectSnapshot` like everything else, so
the machinery is there; nothing records an entry. Ctrl+Z after a learn reaches
past it to the previous recorded edit, the way a channel or generator preset
already does. The document is marked dirty, so the mapping is at least saved.

**The record-arm and LEARN toolbar buttons have no action ids.** `ACTIONS.md`
says every operation a shortcut or menu row can perform is a named action in
`actions.rs`; these two are toolbar buttons only, so neither can be bound to a
key and neither appears on the Shortcuts page. Adding them means moving the
"63 actions in 11 categories" sentence and its test, which is why it was not
done on the way past.

**Nothing in the MIDI control layer has been run against a device, as of
2026-09-15.** Every layer has tests and the application compiles and draws its
mapping page, and no keyboard has been plugged into it. `scripts/mooloop-mcp`
and a controller are the check.

**The Core MIDI driver's port ids have never been compiled.**
`coreaudio_driver.rs` is behind `#[cfg(target_os = "macos")]` and the
2026-09-15 MIDI work was done on Linux, so the port threaded through
`MidiBytes`, the `Listening` struct that replaced the connection tuple, and the
narrowed `Ignore` flags are all unverified. Mechanical changes, but mechanical
is not checked. Build `mooloop-engine` on macOS.

**There is no way to hear a track before its own fader.** Solo is in place
as of 2026-09-11, which silences the others rather than opening a monitor
path, so a soloed track is still heard through its fader, its pan and its
analog-sum switch. `archive/MIXER_PLAN.md` records the AFL tap as later work and
names what it needs: the tap points that pre-fader sends also want.

**A channel cannot be soloed, only its track.** `MixerBus.solo` is per track,
which is the same scope the analog-sum switch has and for the same reason
(`docs/TERMINOLOGY.md`: a mixer strip is a track). Soloing one channel of
several on a track has not been asked for, and would need the per-channel
strip the sends work also wants — so it is one control on one face away, not
a design question.

---

## Focus

**A text field is left with Enter or Escape, and clicking away still leaves
the caret in it.** Escape is the exit, added 2026-09-14: the rename field, the
tempo entry and the knob's two numeric entries all take it, and
`every_editable_text_field_has_a_way_out` fails the next field that does not.
For the tempo and the numeric entries it is also a cancel, because those
commit only on `accepted`; for a rename it is not, because `NameField`
reports every keystroke as it happens and the application already has them.

What is unchanged is the click. Slint has no click-outside for a focused
input, so leaving one by clicking on something that is not a control still
leaves the caret where it was, and Space still types a space until Escape or
Tab. Closing that means a focus-owning surface above the whole work area,
which is a change to what takes focus rather than a handler on a field.

A `read-only` `TextInput` is not a field and has no exit: that is the
selectable label `save-error-dialog.slint` and the Developer page's log path
use so a reason or a path can be lifted out by hand. They sit in dialogs,
where the transport is not reachable anyway.

**A name is renamed where its subject is edited, and nowhere nearer to it.**
A channel is renamed on the `DEVICES` toolbar and a track on its own device
face, so renaming either means opening the view that owns it. The obvious
alternative — double-clicking the rack plate or the mixer strip — was not
built: the plate already carries a press that selects, a drag that reorders
and a right-click that opens a menu, and a fourth gesture on it needs a
decision about which one loses rather than an implementation. Nothing is
blocked by this; it is one more click than a user coming from FL will expect
(`main.slint`, the `NameField` beside `CHANNEL PRESET`).

## Edits that do not undo

**Removing a track does the silent orphan repair `archive/MIXER_PLAN.md` says must
not happen interactively.** The plan is explicit: deleting a non-master slot
"is an explicit structural operation", the confirmation "names them and offers
an explicit replacement destination (Master by default)", and **"There is no
invisible orphan repair in an interactive edit."** What happens is
`queue_track_remove` with no confirmation, no naming and no picker:
`rescope_tracks_after` silently re-points every inbound output to the master
and silently **drops** every send that named the track -- two different
repairs, neither reported. `CURRENT.md` mentions the first and not the send
drop. Undo does restore the snapshot, which is the half the plan asked for and
got. Not small because it needs a confirmation surface that counts and names
inbound routes and sends, plus a destination threaded into `remove_track`,
which today takes an index and hard-codes the master. Options: build it as
specified; or keep the silent repair and *report* it in the status bar, which
is much cheaper and shares the diagnostic channel `sanitize_bank`'s entry
already wants; or narrow the plan to what undo buys. Either of the last two
still needs the send-drop sentence adding to `CURRENT.md`. Found 2026-09-13.

**A song-mode clip boundary still leaves its destination stuck at the last
value the outgoing clip wrote.** The command half was fixed 2026-09-14:
`SetCurrentPattern`, `SetPlaybackMode` and `Seek` now walk the lanes of the
patterns covering the position they are *leaving* and hand back every
destination the incoming position does not cover. That closes the reachable
pattern-mode case -- pattern 1 sweeps a cutoff down to 200 Hz, pattern 2 has
no such lane, and the filter used to go on playing at 200 Hz while its knob
and its face both read 1 kHz.

A clip boundary in song mode is not a command. The playhead simply moves out
from under a lane, `has_automation_at` answers false on the next block,
`control_events_for_slot` takes its early return, and nothing ever writes
again. The same is true of the song wrap. Closing it needs what the original
entry named as the only complete answer: the engine carrying which
destinations had a curve last block and no longer do, which is per-channel
state across blocks on the audio thread, where `AutomationBlock` is
deliberately "a read-only view". The alternative is to declare that
automation latches and say so in `CURRENT.md` -- a defensible DAW convention,
which would then make `restore_base_param`'s five callers the inconsistency.

It bites only destinations that are automated and *not* modulated: a
modulated one takes `base_normalized = knob_normalized` when the curve is
`None` and so restores the knob every block by accident. Found 2026-09-13,
half-fixed 2026-09-14.

**Shortening a pattern hides automation points that still shape the sound --
the opposite of what it does to notes.** `refresh_automation_points` filters
the drawn points to `point.tick <= length_ticks`; the engine applies no such
filter, folding the position into `[0, length_ticks)` and interpolating
between whichever pair brackets it, which for a shortened pattern is the last
visible point and an invisible one past the end. Draw a ramp 0 to 1 across 16
steps and shorten to 8: the lane draws as a single point at 0 with a flat line
at 0.0, and plays a ramp from 0.0 to 0.5. Nothing on screen accounts for the
movement. For *notes*, past the new end means hidden **and silent** (the
stranded-NoteOff entry above); for automation points it means hidden and
**audible** -- the two banks in one clip answer the same question opposite
ways, and only one of them is written down. Not small because the three fixes
are three different products: ignore points past the end (matches the note
rule, but a lane briefly dragged short loses its tail on the way back); draw
them, which needs a way to show a point outside the roll's own width; or crop
on shorten, which destroys authored work and needs the same drag-release
gesture the `set_pattern_length` entry already needs. Found 2026-09-13.

**An undo of one edit silently destroys every unrecorded edit made after
it**, and whole surfaces are unrecorded. Undo installs `entry.before`, a full
snapshot taken when *that* edit happened, so anything done since that never
reached the history is discarded -- and redo cannot bring it back, because
`entry.after` predates it too. Not recorded: every step-grid edit (click,
right-click, velocity, paint), pattern length, add-pattern, playlist
placement add and remove, **every effect and generator parameter**, both
renames, and channel/generator preset loads from the browser.

So: draw a note (recorded), click eight steps, turn a filter's cutoff, press
Ctrl+Z -- and the eight steps and the cutoff are gone with no redo path. The
boundary was deliberate in one direction (pattern clone/remove/clear and the
channel clipboard verbs were given the `ProjectEdit` path on purpose, "reuse
the same whole-project undo pipeline") and nobody wrote down what it costs in
the other.

Not one patch per callback. A knob reports on every move frame, so making
parameters undoable needs a gesture token per control, and only the piano
roll and the slice editor have a `drag-started`/`finished` pair today -- so it
is a `main.slint` contract change across every face, the eight-minute build
`AGENTS.md` says to batch. `NameField.edited` fires on every *keystroke*, so
a naively recorded rename is one undo step per character. Options: give the
UI a general `gesture-begin`/`gesture-end` pair and route these through
`record_project_history`; or declare the boundary and leave the destruction,
which `CURRENT.md` now at least describes honestly; or make undo refuse to
run over unrecorded state, which needs a "changed since the last entry"
marker `record` has no way to set today. Found 2026-09-13.

**Two preset producers mutate the live session before queueing, so a refused
install leaves the document and the engine disagreeing.**
`on_effect_preset_selected` (`lib.rs:8076`) and `append_effect_preset`
(`lib.rs:12205`) apply the preset to the live session, take `after` from it,
and only then queue -- where every sibling (`queue_channel_insert`,
`queue_pattern_clone`) clones `before.project`, mutates the clone, and leaves
the session untouched until the pump installs it. When `install_project_in_ui`
returns false because the 1024-slot realtime queue is full, the pump prints
"Channel edit is waiting for audio" and **drops the edit** -- nothing retries
-- so the rack draws the new device while the engine keeps playing the old
one, and the edit is in no history entry. Not small because
`load_effect_preset` is a `Session` method that also writes
`effect_preset_names`, so it cannot run against a detached `Project` clone
without splitting the name-setting out. Options: split it and follow the
`queue_channel_insert` shape; or have the pump's failure branch roll the
session back, which needs the pre-edit snapshot it currently discards; or make
the failure retriable by parking the edit and re-sending next tick -- which
also makes the status message true, since "waiting for audio" describes
behaviour nothing implements. Found 2026-09-13.

**Shortening a pattern hides a sounding note rather than releasing it.**
`sequencer.rs:698` (and `:761` in song mode) filters notes by
`note.start_tick < pattern_ticks`, which is what implements "a shortened
pattern keeps its notes" -- but the filter is applied to *both* edges. A note
already sounding whose start is now past the new end stops being scheduled at
all, so its NoteOff never arrives and the voice holds until Stop. Same root as
the pattern-switch bug fixed 2026-09-12, and the one-line fix there does not
transfer: `Session::set_pattern_length` returns `Some` on every value a drag
crosses, so reusing the seek release would fire a full-rack choke twelve times
on a drag from 16 steps to 4. Three options. Release only on a *decrease*, and
only for channels with a note in the vacated range -- which needs the
sequencer to answer "was anything sounding past here", and it cannot today.
Or make the length command fire once on drag release, which changes what the
control means during the drag. Or keep scheduling the off edge for filtered
notes while suppressing their on edge, which strands nothing and chokes
nothing but needs a decision about what a hidden note's NoteOff means on the
second pass. Found 2026-09-12.

**A pattern-length change can create the overlapping placements the editor
refuses to create.** `session/transport.rs:213` guards
`add_playlist_placement` against overlap, correctly and half-open, and it is
the only place the invariant exists: `set_pattern_length` rewrites the length
with no revalidation, `Sequencer::set_playlist_placement` has no overlap
notion, and `integrity::check_playlist` checks only the pattern index and the
start tick. Place pattern 0 at ticks 0 and 384 at 16 steps -- accepted, they
abut exactly -- then set it to 32 steps, and the first clip covers the second.
Both are scheduled: `instance_offset` differs, so the voice ids differ, and
**every note fires twice 384 ticks apart at doubled amplitude**.

**The clip is at least reachable now.** `placement_covering` took the first
match on a list sorted by `(pattern, start_tick)`, so a click in the overlap
always answered with the earlier clip and the buried one could not be removed,
moved or undone by any gesture. It takes the **latest-starting** cover as of
2026-09-14, which is the rule `Sequencer::automation_lane_at` already states
one layer down for lanes. So the state is escapable rather than permanent.

What is still open is whether the overlap should exist. The invariant has no
owner: enforcing it in `set_pattern_length` means deciding what a length
increase does to the clips it now swallows, and the same decision has to be
made in `integrity` for files that already carry the overlap -- a `Doctor`
entry and a `PROJECT_FORMAT.md` change across two crates. Options: clamp the
length change to the largest value that keeps the pattern's placements
disjoint; or make the overlap legal everywhere, drop the guard in
`add_playlist_placement`, and say in `CURRENT.md` that a doubled clip is a
layer; or drop the covered placements and report them, which is the only one
that also needs a repair path on load. Found 2026-09-12, made escapable
2026-09-14.

**Snap-all-markers and the four trim/loop markers are not undoable**, where
the five slice verbs beside them now are. `add_slice`, `move_slice`,
`remove_slice`, `divide_slices` and `clear_slices` record from the *caller*
(`lib.rs:8840`-`8922`, with a gesture token for drags), which is why this
entry used to say none of them did: it looked for history in `sampler.rs` and
the snapshot is taken one level up. `snap_all_markers` (`lib.rs:8545`) and
`wire_marker_param!` (`lib.rs:8410`-`8456`) are still on the cheap path.
Corrected 2026-09-13.

**Send edits are not undoable, because routing never was.** `add_send`,
`remove_send`, `set_send_level`, `set_send_tap` and `set_send_enabled` in
`mooloop-session/src/mixer.rs` mark the document dirty and return an
`EngineCommand`, the same shape `set_bus_output` beside them has always had.
A track *add* is undoable, because it goes through the project-edit path — so
one mixer face now has both behaviours on it, which is the part worth fixing.
Unifying them means routing joining `ProjectEdit`, not a per-callback patch.

---

## Ceilings and one-shots

**A chain nested past `MAX_CONTAINER_DEPTH` still loads unreported, and the
integrity pass has no way to say so.** The gesture half was fixed 2026-09-14:
`can_wrap`, `can_insert_into_container` and `can_move_into_container` refuse a
wrap, an insert and a drag that would put a box past the cap, the rack's wrap
button asks the same function rather than comparing a depth of its own, and
`the_rack_draws_a_band_for_every_level_the_engine_blends` holds `main.slint`'s
four literal chrome levels to the constant. Before that, five clicks reached a
box whose Mix does nothing at any value and which the rack draws no chrome
for -- inert and invisible at the same time.

The format half is not a missing check, which is why it did not land with the
rest. **`Doctor` has two severities and this needs a third.** `correct`
repairs, and every other method -- `refuse`, `block` -- sets `repaired: false`,
which `Issue::is_blocking` reads as "do not open this document". So reporting
a deep chain the way an over-long one is reported would stop a song opening
that opens today, which is the brick this file has already been asked about
once, under a different name. And there is no safe repair to reach for
instead: unwrapping looks free, since a box past the cap contributes no blend,
but its **bypass still works** (fixed 2026-09-13), so removing it would unmute
whatever it was muting.

So the options are: give `Doctor` a tolerated severity -- an issue worth
telling the user about that stops nothing, which the report, the status bar
count and `Diagnosis::blocking` all have to learn; or accept that the format
does not check depth and say so, which `structure.rs`, `CAPACITY_POLICY.md`
and `docs/plans/containers/02-...md` now do rather than claiming otherwise.
Found 2026-09-13, gesture half fixed 2026-09-14.

**`MAX_AUTOMATION_LANES_PER_CHANNEL` is 8, and raising it is not free.** The
ninth lane no longer draws and plays nothing -- `check_lanes` truncates and
reports as of 2026-09-14, so the document, the editor and the engine all keep
the same eight, and `PROJECT_FORMAT.md` states the cap. What was not decided
is whether eight is the right number. It is low for a song automating a
channel and two buses, and `CAPACITY_POLICY.md` says "it was easier to
preallocate" is not a sufficient reason -- but the preallocation is real here
in a way it is not for most caps. `Pattern::with_steps` builds
`MAX_CHANNELS` channel patterns each holding a
`Vec::with_capacity(MAX_AUTOMATION_LANES_PER_CHANNEL)`, and the sequencer
builds `MAX_PATTERNS` of those, so the cap is multiplied by 65,536 before it
is paid. Measure that before changing it. Found 2026-09-13, half-closed
2026-09-14.

**`MAX_MOD_ROUTES_PER_CHANNEL` is 16** (`modulation.rs:1290`) — two routes per
module across eight slots. It was left there deliberately, to be raised once
the modulator grid has been lived in. The price of raising it is now measured
and linear, so this is a one-line decision when the answer is known.

**Factory banks self-seed once and can never update.**
`mooloop-project/src/factory.rs` writes `.factory-v1` (and `.ml1-factory-v1`)
markers on first run and never rewrites the directory. Editing a factory patch
in `effect_factory.rs`, `ds01_factory.rs`, `mlm1_factory.rs` or
`mlp8_factory.rs` will not reach a machine that has already seeded unless the
marker or the directory under `presets/` is deleted by hand.

**Per-slice loop points do not exist.** `loop_mode` sits on `SamplerParams`
(`sampler.rs:552`), so looping is all slices or none. Explicitly deferred.

---

## Meter and time

**Device meters are drained only for the chain currently on screen.** Fixed
2026-09-14 by the first of the three options this entry named, which it
already called the correct one: one `last_device_target` local in the pump,
and `DeviceMeters::clear_target` emptying whatever the rack has just moved
off. Before that, every other channel's and bus's stage cells were `fetch_max`
holds that nothing ever emptied, so switching the rack to a channel last
viewed ten minutes ago drew that ten-minute maximum for one 8 ms tick before
the next read cleared it.

What is *not* done is the same thing for the two cells the rack does not read
at all. Nothing here drains a target the rack has never been pointed at, so
the first tick after opening a chain for the first time still shows whatever
that chain's loudest block was -- which is a shorter window than before and
the same shape. Draining every target every tick is
`(MAX_CHANNELS + MAX_BUSES) x (MAX_EFFECTS+1) x 6` atomic swaps at 125 Hz,
which is the cost the spectrum pool exists to avoid in the analogous case; a
slow secondary timer is the option left on the table. Found 2026-09-13,
fixed for the reachable case 2026-09-14.

**A bus clip latch is cleared by any project edit, which is broader than the
problem it fixes.** Removing a track shifts every later one down an index and
the per-bus `MeterBallistics` are keyed by that index, so old track 4 would
open as track 3 wearing track 3's latched clip -- permanently, because a
latch has no timer and only a click releases one. That is fixed: a project
install raises `UiState::bus_meters_stale` and the pump resets every
non-master pair on its next tick, which is the bluntest of the three options
the entry named and the only one that also handles undo, redo and reorder
without being told the edit's shape.

What it costs is a false *clear*: a legitimately lit lamp on a track nobody
touched goes out when the user adds a channel or clones a pattern. That was
chosen deliberately -- for an alarm, losing one is an inconvenience and
showing one nobody earned is the meter lying -- but it is broader than it
needs to be, and the narrow version is still available. The bank's shape is
in hand at the install, so resetting only when the track count changed, or
only the indices at or after a removal, would both work. Neither was done
because a project install is already a rare, deliberate act and the extra
machinery would need the edit's shape threaded to a place that currently
knows only "something was installed". The master is exempt: it is always
track 0, so its meter never reads somebody else's audio.

Peak hold and decay are reset by the same call, which is right for the same
reason and invisible anyway -- they wash out in under two seconds.
`MeterBallistics::reset` has a test; the wiring that calls it does not, and
could not without driving the application. Found 2026-09-13, fixed
2026-09-14.

**`EngineEvent::Metering` is a per-block ring push that only
`engine-selftest` reads.** The master used to be metered *twice*, through two
transports and with two clip latches: `executor.rs` pushes the event every
block and `render.rs` publishes the same two numbers into `BusMeters` cell 0,
the toolbar read the event and the mixer's master strip read the cell. The
event push is `let _ = evt_tx.push(..)`, so under ring pressure the
always-visible meter was the lossy one while the atomic cell cannot drop a
block, and clicking one clip lamp did not clear the other.

Fixed 2026-09-14 by the second of the two options this entry named: both faces
read bus 0 through one `MeterBallistics` pair, so the latch is shared and the
toolbar is no longer the lossy reader. The first option -- dropping the event
outright -- was **not** taken because `engine-selftest` is built on counting
it, and it is the only thing that reports whether the *callback* produced
audio: a held cell says the loudest it ever was, which cannot tell silence
from a callback that never ran.

So what is left is a per-block push on the audio thread serving one
diagnostic. Cheap, and worth knowing it is not free: dropping it means giving
`engine-selftest` another way to ask the same question, not deleting a
duplicate. Found 2026-09-13, unified 2026-09-14.

**A muted channel that something taps meters silent while its audio flows.**
There are two mute paths for a channel. The one where nobody taps it skips the
render, so silence is honest. The other -- muted, but an Aux In reads this
channel -- runs `strip.process`, so the audio *is* heard through the Aux In,
and then `continue`s before the device-meter publish. The source rail reads
silent for a signal that is reaching the master. Adjacent to the solo entry
above but the opposite sign: there a silenced track meters live, here an
audible one meters dead. Whoever rules on the solo entry should rule on this
one too.

**The playhead half of this entry was wrong and the reason is worth keeping.**
It said the sampler playhead stopped at the mute and never moved again,
because the `continue` skips that publish too. It does -- and the state is
unreachable. `AudioGraph::produces` is "does any tap name this channel", a tap
names an *audio outlet*, and the sampler publishes **no outlets at all**
(`outlet.rs` asserts exactly that for Sampler, DrumSynth, MonoSynth,
PolySynth and ML-M1). So nothing can subscribe to a sampler channel, a sampler
channel never reaches this branch, and the only channels that do -- ML-P8 and
DS-01 -- have an idle `strip.sampler` whose `voice_positions()` are all `NaN`,
which `PlayheadMeters::read` filters out. Publishing it there is a no-op, and
it was written and then reverted on 2026-09-14 rather than shipped as one.
Found 2026-09-13, half of it withdrawn 2026-09-14.

**A track silenced by someone else's solo still meters, and can still latch
its clip lamp.** `render.rs` computes `let muted = strip.output.muted ||
strip.solo_silenced` and governs the sends, the summing and the send emission
with it -- then twenty-four lines later the meter alone re-reads
`strip.output.muted`. So under a solo, a silenced track's strip meter, its
peak hold and its clip latch all report a signal nobody can hear, while the
comment directly above that line says the meter shows "what is heard rather
than what is running" and `archive/MIXER_PLAN.md` says the strip shows **audible**
post-fader output.

There is a real argument on the other side, which is why this is a note rather
than a one-word fix: `MixerStripRow.solo_silenced` exists so a silenced
strip's name plate *dims* rather than looking muted -- "it is set up and
waiting" -- and a live meter under a dimmed name is arguably the same
statement. What is not defensible is the current split, where `muted` means
one thing for audio and another for the meter in the block that computes it.
Options: meter it silent, matching the two documents; or keep it live and
change both documents, which then needs a separate ruling on the clip latch,
since latching a clip on a track that produced no audible sample is the
hardest part to defend either way; or publish it as a third state and draw it
in the dimmed treatment the name already uses. Found 2026-09-13.

**The project is 4/4 end to end.** Still true, and since 2026-09-15 it is
true in one place: `time::BEATS_PER_BAR`, where it used to be nine anonymous
fours across six crates (`docs/plans/archive/musical-time/`). `integrity.rs` still
rewrites any other meter back on load with a doctor message, and
`Project.beats_per_bar` is still a persisted field the audio thread never
sees — its doc comment says so now, and `scripts/dupe-audit bar-arithmetic`
fails the day a tenth spelling appears. **A 3/4 project is still not a
thing**, and what stands between here and one is threading a signature to the
audio thread, not finding the constants.

**`position_ticks` is an accumulator, not derived from `frames_played`**
(`mooloop-engine/src/transport.rs:24`). `archive/ARCHITECTURE_REVIEW.md`'s action
table calls this out and says to fix it *with* the tempo map, not before.
Recorded so the deferral stays deliberate.

---

## Decisions whose reason expired

**A song's embedded samples can never be turned back into references.** The
guard is **necessary** and is not the problem: without it `replace_song_file`
would delete the sidecar the new reference points at, destroying the only
copy. What was wrong was everything around it, and the cheap half of that was
fixed 2026-09-14. Unticking "Embed assets" and saving now produces a per-sample
warning -- "sample stays embedded: the bundle holds the only copy of it" --
instead of no warning, no status message and no change; and the checkbox
follows the **per-sample flags** rather than the document-level `asset_mode`,
so a bundle whose samples are all embedded no longer reopens showing the box
unticked, which is what stopped the state ever converging.

What is left is that un-embedding does not exist. Doing it for real means
copying the bundle-owned samples out to a folder the user chooses first, which
is a new user-facing gesture and a new dialog. Until then the manifest still
records `asset_mode = "referenced"` beside `embedded = true` on every sample,
which is now merely redundant rather than a lie nobody is told about, and
`CURRENT.md` says what a referenced save of an embedded song actually does.
Found 2026-09-13, made honest 2026-09-14.

**The limiter still has no lookahead, and the code's stated reason is now
false.** `mooloop-dsp/src/effects/dynamics.rs:391` says "Add lookahead when
the engine can compensate for it, not before." The mixer became latency
compensated on 2026-09-05, so the condition is met. `CURRENT.md` already
records this as an open decision rather than a settled no; the source comment
does not.

---

## Cannot currently be tested

**A muted track's frozen send rings could not be shown failing.** Fixed
2026-09-13 -- `SendBank::reset` now drains them on every path that skips
`emit` -- but the fix went in on symmetry (with `emit`'s own reset for a
disabled send, and the track's own compensation ring, which resets on the same
mute) rather than against a failing test, which is not the standard the rest
of this sweep held to.

The obstacle is worth writing down. A muted track also goes to *sleep*, so the
frozen ring contributes nothing at the destination until that track wakes
again -- an attempt that muted, idled for forty-eight blocks and unmuted
measures exactly zero with the fix and without it. Observing it needs a second
note after the unmute so the producer wakes, and then a differential render
against an unmuted control to tell the stale frames from the new ones, since
both arrive in the same block. Worth building if the send path is touched
again; the setup also has to put the latency-declaring device in the
*project*, because `RenderState::from_project` computes the compensation plan
and an effect installed afterwards leaves the send with no ring at all --
which is how the first attempt at this test came to pass without the fix.

**Acceptance test 8 — RT hygiene, no allocations or locks in the audio
callback — has a harness now, and it covers one block.** Amended 2026-09-15 by
`control-plane-seams/04`; what this entry said before was that no harness in
the tree could express it, and the reason given was right about every
allocator it named.

The instrument turned out to be a small addition to `mooloop-engine`'s own
`#[cfg(test)]` `CountingAllocator`, which the entry above had overlooked
because it only named the session crate's. It counts *live bytes*, and a net
byte figure cannot see an allocation paired with a free inside one block —
which is exactly what a `Vec` growing on the callback thread looks like. It
now also carries `allocations()`: a count of `alloc` and `realloc` calls that
never decreases, held in a `const`-initialised thread-local so that reading it
from inside `alloc` cannot itself allocate, and so that two tests running in
parallel do not measure each other the way `block_cost`'s module doc records
happening with `live()`. `realloc` is counted explicitly, because `System`
implements it with `mremap` rather than alloc-copy-dealloc.

`a_block_that_retires_a_preview_does_not_allocate` uses it, and was validated
against the defect it was written for: with `preview_retired` put back to a
`Vec::new()` it reports one allocation and fails.

**What is still open is the coverage, not the instrument.** Test 8 claims no
allocations *or locks* in the callback under every Buffer operation the plan
lists. One block on the preview path is a floor. Extending it is now writing
tests rather than building a harness, and the locks half is not measured at
all.

---

## Consistency questions, not bugs

**Song-mode swing follows pattern phase, and only a test name says so.**
`swing_offset_ticks` (`sequencer.rs:867`) takes the offbeat parity from a
note's position *inside its pattern*, so a clip placed at an odd number of
steps swings on the opposite sixteenths from every other clip.
`song_swing_uses_pattern_phase_not_playlist_position` (`sequencer.rs:993`)
asserts exactly this, so it is deliberate -- but the editor makes the
conflicting case easy to reach, because `main.slint:820`'s snap table bottoms
out at 6 ticks and will place a clip at tick 6, 12, 18 or 24. At 66% swing two
copies of one pattern a step apart play their offbeats 16 ticks apart in
opposite directions, about 40 ms at 120 BPM. Whether swing belongs to the
pattern or to the song grid is a decision rather than a defect; if it stays as
it is, it belongs in `CURRENT.md` where a user would find it and not only in a
test name. Found 2026-09-12.

**Automation does not follow a fold inside the block that contains it.**
Raised unconfirmed 2026-09-12 and **confirmed 2026-09-13**, with a condition
the first pass did not have and a second trigger it did not know about.

`AutomationBlock` is built once per block from `spans[0].start_tick`
(`render.rs:4315`), `curve_for` resolves the lane at that pre-fold position,
and `value_at` advances linearly across the whole block, folding only on
`curve.length_ticks` -- the *pattern's* length. The transport meanwhile cuts
the block into spans at the fold and schedules notes span by span. For a
frame past the split the computed tick exceeds the true one by exactly the
loop length `L`, so the lane is read at `(true_local + L) mod P`:

> **The read is correct iff `L` is an exact multiple of the covering
> pattern's length `P`.** That is the whole condition.

Worked case. 48 kHz, 120 BPM, PPQ 96, one 16-step pattern (384 ticks) at song
tick 0, `LoopRange { 0, 192 }` -- two beats, on the editor's own snap grid --
a lane ramping 0 to 1 across the pattern, 512-frame period. The block starting
at tick 191.5 splits at frame 125, and frames 128-511 read the lane **192
ticks ahead of the playhead, half the pattern**: the cutoff sits at 0.50
normalized where it should be 0.0. That is 8 ms on every pass, twice a second,
and up to 170 ms at the 8192-frame maximum. The next block resolves fresh, so
it is a periodic blip rather than a drift.

Two things the first pass missed. **A loop range is not required** -- in song
mode the song itself repeats by `wrap_tick(song_tick, song_length_ticks())`,
which is bar-rounded, so the block straddling the song repeat reads the
outgoing clip's lane too. And **notes are handled correctly**:
`schedule_note_edge` walks `absolute_tick += period` across the whole block,
so automation's linear advance is the only reader of a folded position that
does not fold. What bounds it: `process_once_block` passes `looping = false`,
so **an offline export is correct and only realtime monitoring is wrong** --
which is exactly when someone is looping a section.

Not small, because the once-per-block hoisting is load-bearing:
`has_automation_at` exists to be asked once, and `curve_for` walks every
active channel's lanes for every descriptor of every device. Options, cheapest
first. **(1) One line, strictly better than today:** when `span_count > 1`,
clamp `AutomationBlock::ticks` to the control ticks inside span 0, so the
destination *holds* across the fold instead of jumping half a pattern away --
stale for at most one block, and the audible blip is gone. **(2)** Give
`AutomationBlock` the span list and map a control tick's frame to its span
before computing the position, keeping one resolved curve; fixes the position
error and the common case, but not the case where the fold lands in a
*different* placement. **(3)** Build one `AutomationBlock` per span and
re-resolve per span -- complete, and a fold is rare enough that the amortised
cost is near zero; the cost is structural rather than cyclic.

Whichever is taken, `render.rs:543`'s comment must go with it: it claims this
case is handled, and recomputing per tick is what makes the *pattern* wrap
correct while saying nothing about the loop or the song wrap.

**A departed producer and a departed device are handled oppositely.** Aux In
sends a subscription whose source channel was deleted to `DEPARTED_SOURCE`
(`aux_in.rs:147`), keeping it inert and inspectable. The modulation rack drops
routes whose device is gone (`modulation.rs:1875`) while keeping *illegal*
routes inert (`modulation.rs:1932`). Both behaviours were chosen on purpose in
their own passes; nobody has decided whether they should match.

---

## One name, two policies

**An embedded sample that is a symlink escapes the bundle, is read, and is
copied into the next bundle saved from it.** `embedded_bundle_path` is purely
lexical and `resolve_setup_asset` then does `is_file()` and reads, so a shared
`.mooloop-channel` or kit bundle can make the app read an arbitrary local file
as audio -- and, the part that matters, **re-saving that preset stages the
symlink target's bytes into the new bundle**, which the user may then share.
`PROJECT_FORMAT.md` says embedded paths "must remain below the document's
`samples/` directory", which reads as a containment guarantee the check does
not provide; the doc overstates and the code under-delivers. Not small because
the fix is a decision about how much the loader may touch the filesystem:
canonicalising and re-checking containment costs a syscall per sample and
changes behaviour for anyone deliberately symlinking a shared sample library
into a bundle. Options: canonicalise both sides and require containment for
embedded references only; refuse a symlink under `samples/` outright; or
accept it and correct `PROJECT_FORMAT.md` to say the check is lexical. Found
2026-09-13.

**The compensation plan is still derived twice, in two crates, but the
policy in it is not.** The divergence the row here described is fixed: the
send half's guard -- a bank whose routing does not sort has no compensable
sends, because a send compiled against an order that is not the one being
walked arrives a block late -- was in `RenderState::install_compensation` and
not in `Session::latency_plan`, so the session would have handed over a full
`SendBank` compiled against a default order's arrival numbers. It is now
`mixer::sends_are_compensable` and `mixer::compensable_send_edges`, which both
call sites read, and `mooloop-core` and `mooloop-session` each have a test on
it.

What is left is the *shape*: both sides still walk their own channels and
buses to build `channel_latency`, `channel_bus` and `bus_latency` before
calling `compile_latency`. They cannot share that walk as it stands, because
the engine reads `ProjectChannel.setup` and the session reads its own channel
type -- the arithmetic is identical and the iteration is not. Extracting it
means a shared input type or a trait, which is a bigger change than the one
the drift called for. The same shape one size down still holds for
`Session::console_plan` against `RenderState::install_console`, where nothing
has diverged and nothing is checked.

`an_offline_render_compiles_the_same_compensation_as_a_live_one` still does
not read the session's derivation -- both sides of that comparison go through
`install_compensation` -- so the two new tests are what hold the policy, not
that one. Found 2026-09-13, half-fixed 2026-09-14.

**Load silently deletes authored modulation the spec says to keep as an
orphan, and the mechanism built for keeping it is unreachable.**
`ModRack::deserialize` drops four things with no diagnostic: a slot whose
index is past `MAX_MODULATORS_PER_CHANNEL` (`modulation.rs:1510`), a route
whose source id names no surviving slot (`:1543`) -- which is exactly what a
capacity truncation produces -- a route whose `to_local_slot` fails (`:1536`),
and every route past `MAX_MOD_ROUTES_PER_CHANNEL`, because `apply_route`'s
`None` is discarded into `let _` (`:1548`). `MODULATION.md` says
the opposite: "project persistence retains it as an inspectable orphan rather
than silently deleting authored work." The truncation itself is the accepted
design -- `AGENTS.md` records both constants as engine constants rather than
format fields -- and it is *coherent*, in that a route naming a module that
did not survive is dropped rather than re-aimed at a survivor. The silence is
the divergence.

`UNRESOLVED_SLOT` was built for this and **no reachable path parks anything
on it.** Its own doc comment (`modulation.rs:1386`) describes behaviour
nothing implements: `add_route` stamps from an occupied slot, `apply_route`
refuses an unheld id, `clear` removes routes both ways, `move_module` and
`swap_slots` never drop a module, and the deserializer drops rather than
parks. The only producer is an out-of-range generator outlet, and that is
refused at the door. Two tests were asserting this correctly while their own
comments claimed the spec's behaviour; the comments were corrected 2026-09-13
and the behaviour was left alone, because changing it is this entry.

Options: implement the spec -- park un-sourced routes and report the
truncation through `mooloop-project`'s `Doctor`, so the user is told rather
than surprised; or keep the deletion and have the `Doctor` report *that*,
which is the cheap honest half; or amend the spec to say load-time truncation
deletes. Whoever decides this should also decide the departed-producer versus
departed-device inconsistency above, which is the same question at a
different site. Found 2026-09-13.

**Reordering the modulator grid used to restart or cross-wire every moved
module's running state.** Fixed 2026-09-14 by the first of the three options
this entry named: `ModRack::move_module_mapped` returns the permutation it
applied, `ModulatorRack::permute` carries each module's running state through
it, and `MoveModulator` has its own handler ahead of the params diff.

Two things about the fix worth knowing before touching it. The permutation is
**not enough on its own**: `retarget` may rewrite a Math module's
`input_slot`, which lives in its params, so the handler still runs a params
pass afterwards -- against the *permuted* previous params, which is what keeps
it to the Math modules. A `MathSource` is its params and rebuilds for nothing,
where rebuilding an LFO is the whole defect. And `permute` clears before it
writes, so a slot the permutation does not name is emptied rather than left
holding a module that has moved away; an in-place swap would lose that
silently, which is why there is a test for it.

The option **not** taken was keying the DSP rack by `ModSourceId` rather than
by slot, which removes the class rather than this instance of it. The diff by
slot number is what makes every other narrow command cheap, and a reorder is
the one edit that owes it a permutation instead -- but a second edit that a
diff cannot see would be the argument for the bigger change. Found
2026-09-13, fixed 2026-09-14.

**A unipolar route from a *fading-in* LFO rises from half the module's depth
rather than from the floor.** The steady-state half of this was fixed
2026-09-14: `offset_for`'s lift now stands on `ModRack::wire_span`, the span
the module's params say it reaches, so an LFO at half depth rests on the base
instead of a quarter of the route's depth above it, and one at depth zero
contributes nothing instead of half a depth of silent offset. What the span
cannot see is `fade`, because that is DSP state in `Lfo` rather than a
parameter, and reading it per control tick means a second
`[[f32; 8]; 256]` table per channel beside `ControlOutputs` -- 8 KB a live
channel, doubling the control capture, for a transient on a parameter that
defaults to zero seconds. So during a fade the lift is anchored on the
unfaded depth and the route rises from `depth * 0.5` to its floor. Exact from
the moment the fade completes. Found 2026-09-13, mostly fixed 2026-09-14.

**A Math module's `input_slot` follows a reorder but not a removal.**
`retarget` (`modulation.rs:1767`) goes out of its way to carry the input
across a `move_module`, and `clear` (`:1678`) does not touch it -- although
`clear`'s own comment is explicit that "a destination left on an emptied slot
would be inherited by whatever module is installed there next", which is
precisely what happens here. Remove the LFO in slot 0 that a Math in slot 1
reads, and the Math correctly reads 0.0; install anything else, `free_slot`
returns 0, and the Math is silently multiplying the new module. The rack has
`a_new_module_never_inherits_a_removed_ones_routes` for this hazard on the
route side and the math input is outside it. Compaction has the same hole:
`move_module` fills `remap` only for occupied slots, so an input pointing at
an empty slot is left alone while a different module compacts into that
number. Rated lower confidence than the rest because `input_slot` is half
visible -- it is `MATH_PARAM_INPUT_SLOT`, a persisted stepped parameter drawn
as the shelf's INPUT selector showing a slot *number*, so "it points at slot 1
and slot 1 changed" is a defensible reading. Options: give Math a
`ModSourceId` input the way a route has one, which is what the code's own
comment (`:952`) anticipates but changes a persisted field's meaning; or park
a dependent input out of range on `clear` and map empty-slot references in
`move_module`, which is small but leaves the INPUT selector showing a position
it cannot represent; or decide the slot number is the contract and delete the
`retarget` remap so the three behaviours at least agree. Found 2026-09-13.

**The bus-bank repair is logged, not reported, and `load_bundle` on its own
still hands back an unsanitised bank.** Two of the four consequences this
entry described are fixed as of 2026-09-14. The cycle branch no longer clears
`sends` on every track: `break_cycles` removes only edges that are genuinely
on a loop, sends before outputs, so one bad `output` edge in a hand-edited
file costs that edge rather than a whole bank's aux routing. And
`sanitize_bank` returns a `BankRepair` per correction, which
`Session::replace_project` writes to the log.

What is left is where those repairs *go*. They are not in
`LoadReport::repairs`, so the status bar's count, the repair log's load line
and the copyable `Diagnosis::report()` still omit them -- and the sanitized
bank is still what the next save writes, so a repair is an edit to the user's
file that only the log mentions. Folding them into the report means moving the
sanitise into `integrity::check_buses`, which needs `mooloop-project` to call
`compile_bus_graph` itself and changes *when* it runs relative to what
`Session::replace_project` assumes about the bank it is handed -- and that
function runs on every project install, an undo included, not only on a load.
The narrow alternative is a second entry point used only by the load path.

Separately, `load_bundle` on its own returns an unsanitised bank, so anything
driving the loader directly -- an offline render, a headless measurement loop
-- gets a graph `compile_bus_graph` will refuse. Found 2026-09-12, half-fixed
2026-09-14.

**`from_index` answers out-of-range input two different ways depending on
which enum you ask, and nothing currently reaches it.** Forty-five enums
convert a selector index to a variant under one name, in two conventions: the
`Self::ALL.get(index.clamp(0, len - 1))` body, thirteen times, clamps to the
nearest end; a hand-written `match` with a `_ =>` arm, thirty-one times,
falls through to the default variant. `NotePriority::from_index(99)` is
variant 0 where an ML-P8 enum's is its last.

**This was recorded as more dangerous than it is, earlier the same day, and
the correction is the useful part.** Every parameter path runs through a
`set` that calls `descriptor.clamp_natural` first, so an out-of-range index
never reaches `from_index` from automation, modulation, a project file or the
UI. All thirty-one hand-written pairs were checked and every one round-trips.
The divergence is real and unreachable, which makes unifying the forty-five
bodies churn rather than a fix -- and is why the pass that found it wrote a
test instead.

What the two conventions do cost is a reader: the same call means two things
depending on the enum, and neither says so. A sentence on each, or one shared
trait, would settle it whenever one of these files is open anyway.

**The five generators each split their own block at note events, and this was
read and left alone.** `mlm1.rs:572`, `monosynth.rs:314`, `polysynth.rs:393`,
`drumsynth.rs:437` and `mlp8.rs:2494` carry the loop the twelve effects carried
until `effects::process_param_split` replaced it. The entry that recorded this
said it needed all five read before anyone decided; they were, on 2026-09-12,
and the answer is **no**:

- **mlm1, monosynth and polysynth are byte-identical.** Three real copies.
- **drumsynth shares the loop and not the handler.** Note-on triggers without
  an id, note-off ends nothing because drums are one-shot, and a stopped
  transport chokes rather than releasing. Those are the device, not an
  oversight.
- **mlp8 cannot participate at all.** Its `render_range` also takes `ctx.bpm`
  and `&mut AudioTaps`, so it cannot match a trait method shaped like the
  others, and the alternative -- holding the taps in a field across the call --
  is what `AUDIO_ARCHITECTURE.md` forbids ("a node must not retain a borrowed
  bus reference received at construction").

So it is four copies with two principled exceptions, where the effects case was
twelve copies of a loop whose handler was *identical*. Unifying these needs a
trait plus a per-device `handle_event`, which is net-neutral in lines and adds a
hop to follow. The two clamps the loop carries are now tested once, in
`effects::mod`, and they are the same two here -- so if this is ever revisited,
the reason to do it is sharing those tests, not the line count.

## Numbers nothing is watching

**Two spellings of the meter floor are kept as literals on purpose, and the
reason is a guard that wants them that way.** The floor moved into
`GainMath.min-db` on 2026-09-13 and fifty literal `-60`s across ten `.slint`
files became references to it. `device-displays.slint`'s `threshold-min-db`
and `floor-db` did not, because `strip_face.rs` holds them to
`gain::MIN_DB` by *parsing the number out of the declaration* -- so replacing
the number with a property reference takes the guard off rather than improves
it. `no_face_spells_the_floor_for_itself` skips that one file and says why.

Fixing it properly means teaching `strip_face.rs` to resolve
`GainMath.min-db` instead of reading a literal, which is a second parser for
one file's two lines. Left as a note rather than done, because the copy is
currently the safer of the two arrangements: it is the only one anything
checks.

**Nothing outside `modulation.rs` read the five modulator descriptor tables
until 2026-09-14.** `LFO_`, `ENVELOPE_`, `STEP_`, `RANDOM_` and
`MATH_DESCRIPTORS` are all `pub`, all exported from `mooloop-core`'s root, and
`grep` found no reader anywhere else -- so the shelf's twenty-one ranges and
forty-two parameter ids were mirrored by hand against tables the program never
consulted. `shelf_agreement.rs` is their first reader and now holds all three
mirrors: the ids, the ranges, and the two scales.

What that had cost was one real disagreement, found the day the check was
written. The LFO's and the Random module's Rate knobs are drawn
`ValueScale.logarithmic` and their descriptors said `ParamCurve::Linear` --
the same law from the two ends, disagreeing. Fixed to `Exponential`, and the
check reproduces it against the tree as it stood the day before.

The remaining question is whether the shelf should read these tables rather
than be checked against them, as DS-01's face reads its defaults at run time.
That is a bigger change than a test, and the test is what was missing.

**One device face still spells a number the descriptor table already
states.** `scripts/dupe-audit unchecked-face` names it. The count was eight
faces and twenty-three numbers when the check was written on 2026-09-12; it is
`bus-device.slint` and two numbers now. The test's parser became block-based
-- which is what the entry here said had to come first -- and its list then
grew to take `modulation-device`, `device-oscillator`, `eq-device`,
`filter-device`, `buffer-device` and `container-device`. `aux-in-device`
followed on 2026-09-13, and needed a test of its own rather than a longer
list, because Aux In is not an `EffectKind`. DS-01 is still absent and still
correctly so: its paged face reads the table at run time
(`default-value: root.defaults[root.param]`), which is a copy of nothing.

What is left needs the parser widened first, and may not be worth it.
`bus-device`'s two numbers are a `MiniKnob`'s pan range (`-1..1`, resting at
`0`) and a `MixerFader`'s `default-value: 1.0`, whose `maximum` already reads
`GainMath.fader-db[0]` rather than spelling one. `face_knobs` walks
`ParameterKnob` blocks and nothing else, so neither is reachable -- and
neither is descriptor-backed, so there is no table entry for a widened parser
to compare them against. Unity and centre are the kind of literal that has
nowhere else to live.

## Housekeeping

**Two EQ listening passes are owed, and the plan they belonged to has
closed.** `eq-v2/` archived on 2026-09-15 when Adam declined step 04, so these
are now owed against shipped code rather than against a pending decision, and
they are recorded here so the archiving does not bury them. Neither is a
defect; both are a change nobody has yet confirmed by ear.

- **Step 03, the shelf slope.** Measured rather than guessed, so the listen is
  narrow: at the Q both shelves rest at (0.707) the change peaks at **0.45 dB**
  an octave from the corner, it pivots about the corner rather than moving the
  shelf, and it is gone three octaves out. What actually changed is the Q knob,
  which did nothing at all before -- **up to 1.9 dB at Q 0.15**. So the patches
  affected are the ones where somebody tried to use that knob and gave up. Listen
  to shelves with a Q away from 0.707, an octave either side of the corner.
  `archive/eq-v2/00-status.md` has the table. See the shelf-Q entry above for
  why the top 46% of that knob still does nothing.
- **Step 02, the response plot.** Changed nothing audible and changed what the
  picture *claims*: the curve is the bank's own coefficients evaluated rather
  than a shape drawn to resemble them, and the pass filters appear in it at
  last. That wants a look with a patch moving under it, not a listen.

**`mooloop-ui` had never been linted, and two things had ridden in on that.**
Fixed 2026-09-07, recorded because the *shape* of it will recur: `cargo
clippy` walks the dependency graph, `mooloop-core` had been failing since
`df52933`, and a run that dies there never reaches the crate you were asking
about. The two it was hiding were a redundant rebinding and — the one that
mattered — a `#[test]` attribute that had come adrift from its function, so
`effect_rack_scrolls_horizontally_to_reach_a_long_chain` had stopped being a
test. **A disabled test does not fail; it stops existing**, and `dead_code`
was the only thing that could have said so. When clippy is red anywhere,
nothing downstream of it is being checked at all.

**The division list is spelled three times, and all three are now checked.**
`main.slint:820`'s `snap-ticks(index)` gives eleven divisions in ticks,
`mooloop-ui`'s `MUSICAL_DIVISIONS` gives the same eleven with their names, and
`controls.slint`'s `Divisions` gives twenty-one in beats, mirroring
`ModTimeDivision::beats`. Every one of those numbers is a division of
`TICKS_PER_STEP` or of a beat, so a single source is imaginable.

What made this worth recording was that **none of the mirrors was held to
anything** -- the test named for the snap table compared it with a literal copy
of itself, and the beats table had no test at all. Both were fixed on
2026-09-12 and both were mutation-checked, so what is left is tidiness rather
than drift risk: three spellings that cannot part without a test naming which
one moved.

The remaining question is whether they should be one, and it is a real design
question rather than a missing constant. The snap list is the *roll's* eleven,
`ModTimeDivision` is the *modulator's* twenty-one, and they are different
vocabularies that happen to overlap -- the roll offers no `1/2D` and the
modulator offers no `1 Bar` under that name. Collapsing them means deciding
whether the roll's picker should grow to twenty-one entries, which is a question
about the interface and not about duplication. Worth leaving alone until
somebody wants dotted snaps.

**A rack unit is two different widths.** A device's total width -- face plus
both rails -- is computed twice and not the same way. An effect slot uses
`unit-width * units + half-gap * (units - 1) + rail-width * 2`
(`main.slint:3806`); the source device uses `unit-width * units + half-gap +
rail-width * 2` (`main.slint:3283`), with the gap term not multiplied. They
agree only at two units. A three-unit source is 724px where a three-unit
effect is 728px, and a four-unit source is 944px against 952px -- so
"3U" on a sampler and "3U" on a delay are not the same measurement.

Nothing is visibly misaligned: the two sit side by side rather than stacked,
and the drag hit-tests each row from its own `absolute-position + width`
(`main.slint:3838`), which is what `CURRENT.md` means by "measured from that
row's own bounds". The cost is only that the unit is not a unit.

Which one is wrong is a design call rather than a reading of the code, which
is why this is a note. Two of the three sites that size a face use
`* (units - 1)`, so it has the majority. But `JOURNAL.md` records the
three-unit source face at its inner 664px -- exactly what the source formula
gives -- as measured and deliberate, and ML-P8 moved to four units on the
finding that three had "no slack anywhere". Correcting the source formula
widens every three-unit source face by 4px and every four-unit one by 8px,
against faces that were sized by eye and signed off. Found 2026-09-10.

**The desktop entry describes an app that cannot be launched from a file
manager.** The window's app id is set as of 2026-09-15 (`AppUi::new`), which
was the loud half -- Hyprland reported `class: ""` until then, so no window
rule, taskbar grouping or icon lookup could match it. What is left is quieter
and mostly one shape: `packaging/mooloop.desktop` claims less than it could,
and one of its omissions is not the desktop file's fault.

- **`.mooloop` has no MIME type and `Exec=mooloop` has no `%F`**, so
  double-clicking a song opens nothing and a song file has no icon. The
  prerequisite is the real item: `crates/mooloop-app/src/main.rs` parses no
  arguments at all, so the binary cannot open a path it is handed. Registering
  the type before that is worse than not registering it -- the file manager
  would hand mooloop a song and mooloop would open empty.
- **No AppStream metainfo**, so KDE Discover and GNOME Software show a name
  and nothing else, and Flathub would refuse the submission.
- **No `StartupNotify`, deliberately.** winit consumes an activation token
  only when asked (`with_activation_token`), and Slint's winit backend never
  asks -- there is no mention of it in `i-slint-backend-winit 1.17.1`. Setting
  the key advertises support that is not there, which costs a launcher
  spinner that never resolves. This one wants the backend to grow the feature,
  not the desktop file to grow a line.
- **One 256x256 icon**, already flagged as a placeholder wordmark in
  `packaging/README.md`. Worth reading that note before adding sizes: a
  scalable entry and the small hicolor sizes are what a 24px taskbar wants,
  and the wordmark will not survive the downscale whatever sizes exist.

**Nothing inhibits idle while the transport is playing**, so the screen can
blank and the lock screen can take the display in the middle of a take.
`hyprctl clients` reports `inhibitingIdle: false` for the mooloop window and
no crate mentions idle inhibition. The policy question is what counts as busy:
transport running is the obvious answer, and an armed recording or a held note
is the one that would actually annoy somebody if it were missed. Found
2026-09-15.

**Four unmerged spikes**, re-counted 2026-09-14. `spike/slint-split-build`
(5 commits), `spike/egui-view-layer` (3), `spike/pattern-bank-cost` (1) and
`spike/song-from-scratch` (1) are answers rather than candidates — none is
waiting to land. Adam's call whether any goes anywhere. A fifth,
`spike/measure-charts`, is fully merged and its branch can be deleted.

There is also `claude/device-identity-rack-addressing-99yt4o` on the remote,
one commit that is not in `origin/main` and has no local branch. Nobody has
said whether it is wanted.

---

## Closed since being raised

Kept briefly so the same thing is not re-reported. Delete freely once stale.

- Buffer had no parameter descriptors and could not be automated — it has
  `BUFFER_DESCRIPTORS` now (`effect.rs:119`).
- The oscillator Semis descriptor disagreed with its knob's travel — both are
  `-48..48`, with a comment saying why (`generator.rs:428`).
- Clear Pattern and Select All were disabled menu rows — both are wired
  (`main.slint:1485`, `main.slint:1508`).
- The v1 mono synth shipped alongside "Mono 2" — the picker shows one Mono
  (`main.slint:2555`).
- The last recorded listening pass was stale at ML-M1 / 2026-08-31 —
  `FOCUS.md:337` now records DS-01 and ML-P8.
- Whether `BufferParams` references audio, which would have forced effect
  presets down the asset-collection path — it is three scalars
  (`effect.rs:1785`), so it does not.
- `scripts/antibox` refilling with caches for checkouts that no longer exist —
  `--prune` and `--prune-age` exist.
- Stray remote-tracking refs and empty `mooloop-worktrees/` directories — gone.
