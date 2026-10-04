# Loose Ends

Small verified gaps recorded before Linear took over tracking (`AGENTS.md`,
*Tracking work: Linear*). This file takes no new rows. Check a row against the
code before acting on it, and delete it in the commit that fixes it or files
it as an issue.

---

## Wrong-looking UI over correct behaviour

**A mixer drag does not scroll the mixer, and a turned strip stays at its
seat.** Moving a track far takes a drag, a scroll and another drag; the
channel rack has the same limit. A strip's SENDS/STRIP page is private to the
strip instance and a `for` reuses instances by seat, so a track dragged off a
turned strip arrives on its fader face, and whichever track slides into the
old seat shows that page. Fixing that means moving the page into
`MixerStripRow`. The held strip is also drawn under the strip to its right
while it passes over it (the channel and device racks too); since Slint 1.18
(MOO-268) `z` may be a binding, so the held row could take a higher `z`.

**A shelf's Q knob stops steepening above 2 and the face does not say so.**
`Biquad::shelf_slope` clamps the slope to 0.1..2.0, where the cookbook's
radicand goes negative; a seven-band EQ band's Q descriptor runs to 18,
because the same id has to serve that band as a bell. So **the top 46% of the
knob's travel** does nothing while the band is a shelf, pinned by
`the_shelf_q_knob_saturates_a_little_past_half_its_travel`. A narrower range
for shelves is not available: every band can be any kind, and a descriptor is
static per id.

**`EqSlope`'s variant names are half the slope they name.** `Db6` runs one
`Biquad::pass` stage, which is a second-order section and therefore 12 dB per
octave, so the five variants are 12/24/36/48/72 and are spelled 6/12/18/24/36.
The face is right (`EqSlope::db_per_octave`, held by `eq_face.rs`); the
variants were left because `serde` writes them (`"db6"`). Rename them on the
next `FORMAT_VERSION` bump that happens for a reason worth having one, with
serde aliases for the old names.

**The oscillator Level knob works in dB; its descriptor is linear 0–1.**
`device-oscillator.slint` drives the knob through `GainMath.linear-to-db`,
while `generator.rs`'s `unit()` helper declares the parameter as a linear
`0.0..1.0`. A modulation depth is a fraction of the descriptor's range, so the
drawn excursion arc on that knob is in dB-space and misrepresents its own
width. Assignment and the audible result are both correct. Fixing it properly
means giving Level a gain curve, which touches automation and project files.

**The drop gap cannot show which side of a container's edge it lands on.** At
a run's last row, "just inside the box" and "just after the box" are the same
index and the gap is drawn in the same place either way. `move_effect` in
`mooloop-session` decides which; `main.slint`'s cell works the box out from
`depth` and `children` alone. Showing the answer needs the session to publish
the depth a drop at index N would produce -- one `int` property and a
callback on each target change.

**Dragging a container opens a one-row gap, not a run-sized one,** and the
devices inside the box do not travel with its face while the pointer is down.
`move_effect` does the right thing on release; only the picture during the
gesture is wrong. The cell publishes its own width into
`RackDrag.source-width` (`main.slint`); a run-sized figure has to come from
the session, which is the only thing that knows where the run ends.

## Wired but unreachable

**A generator's internal route amounts are a working automation destination
no picker can reach.** The engine resolves `ParamOwner::SourceRoute` lanes and
emits `Event::SourceRouteAmount` for them (`render.rs`), `restore_base_param`,
`integrity` and `Session::automation_descriptor` handle them, and
`removing_a_route_lane_restores_the_authored_depth` tests them. But
`automation_destinations` never produces one, so a file that already holds
such a lane plays and persists it while `refresh_automation` drops it from
view: it cannot be seen, edited or deleted. Either extend
`automation_destinations` to walk `base.internal_routes()` (each row needs a
label naming the route, from `route_descriptors()` and the route's durable
id), or delete the engine's route pass and the dead descriptor arm.

**A container's Mix is offered as an automation destination the engine
cannot read.** `automation_destinations` (`session.rs`) walks every slot's
`kind.descriptors()` with no filter, so "Chain 3 - Mix" takes a lane that
draws, persists and survives load and reorder. The container branch in
`EffectChain::process` clears `state.events` without calling
`control_events_for_slot`, and `close_run` takes the mix from
`state.base_params`, so the audio never moves. Modulation already leaves it
out: `ContainerDeviceFace` binds none of the `modulation-*` properties.
Filtering it from the picker silently deletes any lane already drawn
(`refresh_automation` drops targets not in `destinations`); making the engine
read it means a slot with no node carrying a resolved parameter (question 4 in
`docs/plans/archive/containers/README.md`).

**The channel strip's processing parameters are not automation or modulation
destinations.** Every one has a stable id (`mooloop_core::strip`), the engine
applies them by id, and a lane's target can be `ParamOwner::Strip`. But the
engine resolves only `STRIP_DESCRIPTORS` (the fader and pan,
`resolve_strip_segments`), and the picker lists only those. The processing ids
start at 16 so that the two tables can become one without renumbering
anything automation has persisted.

**Buffer MIDI mapping has no UI.** `EngineHandle::set_buffer_midi_map`
(`mooloop-engine/src/lib.rs`) is the only way to install one, and neither
`mooloop-ui` nor `mooloop-session` calls it.

**A mapped control carries no mark of its own.** The only place a binding is
visible is Preferences > MIDI. The mark needs a per-parameter `[bool]` on
every device face, beside the `modulation-route-counts` model: a line in fifty
markup files and a `main.slint` crossing for each face
(`docs/plans/archive/midi-control/04-interface.md`).

**`ParameterFader` cannot be learned, and neither can it be modulated.** The
learn gesture rides on `modulation-edit-started`, and the inline fader rows
have never carried that callback.

**There is no way to hear a track before its own fader.** Solo silences the
others rather than opening a monitor path, so a soloed track is heard through
its fader, its pan and its analog-sum switch, and a soloed channel through its
own volume, its pan and the track it feeds. `archive/MIXER_PLAN.md` records
the AFL tap as later work; it needs the tap points that pre-fader sends also
want.

---

## Focus

**Clicking away from a text field leaves the caret in it.** Enter or Escape
leaves a field (`every_editable_text_field_has_a_way_out` guards Escape), but
Slint has no click-outside for a focused input, so clicking on something that
is not a control leaves the caret where it was, and Space types a space until
Escape or Tab. Closing that means a focus-owning surface above the whole work
area.

**A name is renamed where its subject is edited, and nowhere nearer to it.**
A channel is renamed on the `DEVICES` toolbar (`main.slint`, the `NameField`
beside `CHANNEL PRESET`) and a track on its own device face. Double-clicking
the rack plate or the mixer strip was not built: the plate already carries a
press that selects, a drag that reorders and a right-click that opens a menu,
and a fourth gesture needs a decision about which one loses.

## Edits that do not undo

**Removing a track does the silent orphan repair `archive/MIXER_PLAN.md` says
must not happen interactively** ("There is no invisible orphan repair in an
interactive edit"). `queue_track_remove` has no confirmation, and
`rescope_tracks_after` silently re-points every inbound output to the master
and silently drops every send that named the track. Undo restores the
snapshot. Options: build the specified confirmation, which counts and names
inbound routes and sends and threads a destination into `remove_track` (today
it hard-codes the master); or keep the silent repair and report it in the
status bar; or narrow the plan to what undo buys. Either of the last two still
needs the send drop adding to `CURRENT.md`.

**A song-mode clip boundary leaves its destination stuck at the last value
the outgoing clip wrote.** `SetCurrentPattern`, `SetPlaybackMode` and `Seek`
hand back every destination the new position does not cover, but a clip
boundary or the song wrap is not a command: `has_automation_at` answers false,
`control_events_for_slot` takes its early return, and nothing writes again.
It bites only destinations that are automated and not modulated. Fix: the
engine carries which destinations had a curve last block and no longer do
(per-channel state on the audio thread, where `AutomationBlock` is "a
read-only view"); or declare that automation latches, say so in `CURRENT.md`,
and reconcile `restore_base_param`'s callers with that.

**Shortening a pattern hides automation points that still shape the sound.**
`refresh_automation_points` draws only points with `point.tick <=
length_ticks`; the engine folds the position into `[0, length_ticks)` and
interpolates towards the invisible point past the end. A ramp 0 to 1 across 16
steps, shortened to 8, draws flat at 0.0 and plays 0.0 to 0.5. Notes past the
end are hidden and silent, so the two banks in one clip disagree. Options:
ignore points past the end (a lane briefly dragged short loses its tail on the
way back); draw them outside the roll's own width; or crop on shorten, which
needs the same drag-release gesture as the sounding-note row below.

**The gesture global has no owner, so two controls can close each other's
gesture.** `Gesture.begin()`/`end()` (`controls.slint`) carries no identity. A
name field brackets its editing on focus and a knob press steals that focus,
so if the field's focus-loss arrives after the knob's `begin()`, the field
closes the knob's gesture and the drag records one history entry per pointer
frame. Fix: an id on the pair, so an `end()` naming a replaced gesture is
ignored.

**Nothing checks that `apply_engine_message` still reads
`EngineCommand::edits_document`.** The four `edits_document_tests` call the
predicate directly. No test pushes a `PendingEngineMessage::Command` through
`Session::apply_engine_message` (`mooloop-session/src/engine.rs`) and asserts
`session.dirty`, so the arm reverting to an inline `!matches!(command, Play |
Pause | Stop)` would pass the whole suite. The function takes a concrete
`&mut EngineHandle`; `CommandSink` covers only three of its ten arms.

**Add Channel frees one allocation on the audio thread:** the seat's
`ModRack`, replaced by `set_channel_modulation(channel, ModRack::default())`
in the `AddChannel` arm (`render.rs`) and dropped where it stands. The fix is
to send it through the reclaim ring. `adding_a_channel_frees_no_lane_on_the_callback`
measures a difference rather than zero for this reason, and will start failing
usefully the day the rack goes through the ring.

**Opening a lane still allocates when the pool is empty.** A slot that has
never held a lane takes a spare point vector from `LanePool`, which
`Sequencer::load_project` refills to 32 off the audio thread; the 33rd new
lane between two installs allocates on the callback. Preallocating per slot
is 6 GiB (`CAPACITY_POLICY.md`), and refusing would be a user-facing cap. The
answer is for the session to send the storage with the command, the way
`StructuralCommand` sends a routing table, and take the vacated one back
through the reclaim ring. `EngineCommand` is `Copy` in a POD ring, so lane
opening moves to the structural channel and the two queues need an agreed
order. Adam's call: do it with automation recording, not before.

**Shortening a pattern hides a sounding note rather than releasing it.** The
sequencer filters notes by `note.start_tick < pattern_ticks` on both edges
(`sequencer.rs`), so a note already sounding whose start is now past the new
end never gets its NoteOff and holds until Stop. Releasing on every change
does not work: `Session::set_pattern_length` returns `Some` on every value a
drag crosses, so a drag from 16 steps to 4 would choke the rack twelve times.
Options: release only on a decrease, for channels with a note in the vacated
range (the sequencer cannot answer that today); make the length command fire
once on drag release; or keep scheduling the off edge for filtered notes while
suppressing their on edge.

**A pattern-length change can create the overlapping placements the editor
refuses to create.** `add_playlist_placement` (`session/transport.rs`) guards
against overlap and is the only place the invariant exists: `set_pattern_length`
does not revalidate, `Sequencer::set_playlist_placement` has no overlap
notion, and `integrity::check_playlist` checks only the pattern index and the
start tick. Two abutting 16-step clips of one pattern, set to 32 steps,
overlap, and every note fires twice at doubled amplitude. `placement_covering`
takes the latest-starting cover, so the buried clip can still be reached.
Options: clamp the length change to keep the pattern's placements disjoint; or
make the overlap legal everywhere, drop the guard and say in `CURRENT.md` that
a doubled clip is a layer; or drop the covered placements and report them,
which also needs a repair path on load (`Doctor`, `PROJECT_FORMAT.md`).

**Send edits and the bus output pick do not go through `ProjectEdit`.**
`add_send`, `remove_send`, `set_send_level`, `set_send_tap`,
`set_send_enabled` and `set_bus_output` in `mooloop-session/src/mixer.rs` mark
the document dirty and return an `EngineCommand`. All of them record an undo
entry, so the behaviour is uniform; the path is two mechanisms where one
would do.

---

## Ceilings and one-shots

**A chain nested past `MAX_CONTAINER_DEPTH` still loads unreported.** Every
gesture and index primitive refuses to go past the cap (MOO-363), but
`integrity` has no depth check. **`Doctor` has two severities and this needs
a third**: `correct` repairs, and `refuse` and `block` set `repaired: false`,
which `Issue::is_blocking` reads as "do not open this document", so reporting
a deep chain would stop a song opening that opens today. Unwrapping is not a
safe repair, because a box past the cap still bypasses and removing it would
unmute whatever it was muting. Options: give `Doctor` a tolerated severity,
which the report, the status bar count and `Diagnosis::blocking` all have to
learn; or accept that the format does not check depth, which `structure.rs`,
`CAPACITY_POLICY.md` and `docs/plans/archive/containers/02-...md` already say.

**`MAX_AUTOMATION_LANES_PER_CHANNEL` is 8, and raising it is not free.**
`check_lanes` truncates and reports past eight, and `PROJECT_FORMAT.md` states
the cap; whether eight is right was not decided. It is low for a song
automating a channel and two buses, but `Pattern::with_steps` builds
`MAX_CHANNELS` channel patterns each reserving the cap, and the sequencer
builds `MAX_PATTERNS` of those, so the cap is multiplied by 65,536 before it
is paid. Measure that before changing it.

**`MAX_MOD_ROUTES_PER_CHANNEL` is 16** (`modulation.rs`) — two routes per
module across eight slots, left there to be raised once the modulator grid has
been lived in. The price of raising it is measured and linear, so this is a
one-line decision when the answer is known.

**Factory banks self-seed once and can never update.**
`mooloop-project/src/factory.rs` writes `.factory-v1` (and `.ml1-factory-v1`)
markers on first run and never rewrites the directory. Editing a factory patch
in `effect_factory.rs`, `ds01_factory.rs`, `mlm1_factory.rs` or
`mlp8_factory.rs` will not reach a machine that has already seeded unless the
marker or the directory under `presets/` is deleted by hand.

**Per-slice loop points do not exist.** `loop_mode` sits on `SamplerParams`
(`sampler.rs`), so looping is all slices or none. Explicitly deferred.

---

## Meter and time

**Device meters are not drained for a chain the rack has never shown.** The
pump reads the chain on screen and `DeviceMeters::clear_target` empties the
one the rack has just left (`last_device_target`), but the first tick after
opening a chain for the first time shows that chain's loudest block so far.
Draining every target every tick is `(MAX_CHANNELS + MAX_BUSES) x
(MAX_EFFECTS+1) x 6` atomic swaps at 125 Hz; a slow secondary timer is the
option left.

**A bus clip latch is cleared by any project edit, which is broader than the
problem it fixes.** A project install raises `UiState::bus_meters_stale` and
the pump resets every non-master `MeterBallistics` pair (they are keyed by
track index, which a removal shifts). So a legitimately lit lamp on a track
nobody touched goes out when the user adds a channel or clones a pattern.
Resetting only when the track count changed, or only the indices at or after
a removal, would both work, and need the edit's shape threaded to a place
that currently knows only "something was installed".

**`EngineEvent::Metering` is a per-block ring push that only
`engine-selftest` reads.** Both master meters read bus 0 through one
`MeterBallistics` pair, so what is left is a per-block push on the audio
thread serving one diagnostic. Dropping it means giving `engine-selftest`
another way to ask whether the callback produced audio: a held meter cell
cannot tell silence from a callback that never ran.

**A muted channel that something taps meters silent while its audio flows.**
Muted, but read by an Aux In, the channel runs `strip.process`, so the audio
is heard through the Aux In, and then `continue`s before the device-meter
publish, so the source rail reads silent. The opposite sign of the solo row
below; whoever rules on that one should rule on this one too.

**A track silenced by someone else's solo still meters, and can still latch
its clip lamp.** `render.rs` computes `let muted = strip.output.muted ||
strip.solo_silenced` and governs the sends and the summing with it, then the
meter alone re-reads `strip.output.muted` -- against the comment above it
(the meter shows "what is heard rather than what is running") and
`archive/MIXER_PLAN.md` (the strip shows audible post-fader output). On the
other side, `MixerStripRow.solo_silenced` dims a silenced strip's name rather
than showing it muted, and a live meter under a dimmed name is arguably the
same statement. Options: meter it silent, matching the two documents; or keep
it live and change both documents, with a separate ruling on the clip latch;
or publish a third state drawn in the dimmed treatment. A channel silenced by
another channel's solo already meters silent.

**The project is 4/4 end to end.** The meter is `time::BEATS_PER_BAR`;
`integrity.rs` rewrites any other meter on load with a doctor message, and
`Project.beats_per_bar` is a persisted field the audio thread never sees. A
3/4 project needs a signature threaded to the audio thread.

**`position_ticks` is an accumulator, not derived from `frames_played`**
(`mooloop-engine/src/transport.rs`). `archive/ARCHITECTURE_REVIEW.md`'s action
table says to fix it *with* the tempo map, not before.

---

## Decisions whose reason expired

**A song's embedded samples can never be turned back into references.** The
guard in `replace_song_file` is necessary: without it the sidecar the new
reference points at would be deleted. Unticking "Embed assets" warns per
sample, and the checkbox follows the per-sample flags. What is missing is
un-embedding itself: copying the bundle-owned samples out to a folder the user
chooses, which is a new gesture and a new dialog. Until then the manifest
records `asset_mode = "referenced"` beside `embedded = true` on every sample.

---

## Cannot currently be tested

**The automation lane menu closes before it reports, and nobody has clicked
it.** `scripts/dupe-audit popup-close-order` lists its three handlers in
`main.slint`: each does `lane-menu.close()` and *then* calls the window, the
sequence that made the add-channel menu add nothing (MOO-53). The failure
needs a repeater item to tear down, and the lane menu's rows are repeated one
component inwards -- the `for` is in `AutomationLaneMenu` and the `close()`
in `MainWindow`'s handler for its callback -- which none of the known cases
had. `scripts/mooloop-mcp` cannot click inside a `PopupWindow`; moving the
menu out of `MainWindow` into a component a harness can import, as was done
for `channel-rack.slint` and `tests/add_source_menu.rs`, would answer it.
Until then: **report first, close second**, in any popup row you touch.

**The muted-track send-ring fix has no failing test.** `SendBank::reset`
(`render.rs`) drains a track's send rings on every path that skips `emit`, but
went in on symmetry rather than against a failing test. A muted track also
goes to sleep, so a test that mutes, idles and unmutes measures zero with the
fix and without it. Observing it needs a second note after the unmute, a
differential render against an unmuted control, and the latency-declaring
device in the *project*, because `RenderState::from_project` computes the
compensation plan and an effect installed afterwards leaves the send with no
ring.

---

## Consistency questions, not bugs

**The ML-M1's Acid filter is really quiet.** Adam, 2026-09-18: *"cutoff is
fine but for some reason that filter is really quiet."* Heard, not yet
measured. Acid's cutoff compensation constant is load-bearing (0.41x nominal
against the other two models' 0.65-0.68x), so check first whether the level
drop comes from the same place -- a model voiced by moving its corner that
never had its output level matched to the other two.

**Two things left over when Buffer closed.**

- **A quantized Buffer press starts two frames behind the newest frame.**
  A press landing on a frame's end reads `now` after that frame is written and
  before the writer advances;
  `a_frozen_buffer_played_in_reverse_is_the_ring_backward` measures the lag as
  2. Inaudible, and unfixed.
- **`BufferMidiMap` on `ParamAddr` is a second source-to-destination system**
  beside the general control map (`02-control-and-modulation.md`, step 5).
  Two ways of saying "this note drives that parameter".

**Song-mode swing follows pattern phase, and only a test name says so.**
`swing_offset_ticks` (`sequencer.rs`) takes the offbeat parity from a note's
position *inside its pattern*, so a clip placed at an odd number of steps
swings on the opposite sixteenths from every other clip;
`song_swing_uses_pattern_phase_not_playlist_position` asserts it. The
playlist snap places clips at 6-tick steps, so at 66% swing two copies of one
pattern a step apart play their offbeats 16 ticks apart in opposite
directions. Whether swing belongs to the pattern or the song grid is a
decision; if it stays, it belongs in `CURRENT.md`.

**Automation does not follow a fold inside the block that contains it.**
`AutomationBlock` is built once per block from the block's start tick
(`render.rs`), and `value_at` advances linearly across the block, folding only
on the pattern's length, while the transport cuts the block into spans at a
loop or song wrap. Past the split the lane is read at `(true_local + L) mod
P`, which is correct only when the loop length `L` is a multiple of the
covering pattern's length `P`. A two-beat loop over a 16-step pattern reads
half a pattern ahead for the rest of the block, up to 170 ms at 8192 frames,
on every pass. Notes are scheduled correctly, and an offline export
(`process_once_block` passes `looping = false`) is unaffected. Options,
cheapest first: **(1)** when `span_count > 1`, clamp `AutomationBlock::ticks`
to span 0 so the destination holds across the fold; **(2)** give
`AutomationBlock` the span list and map each control tick to its span (misses
a fold into a different placement); **(3)** one `AutomationBlock` per span,
re-resolved. The `render.rs` comment that claims this case is handled must go
with the fix.

**A departed producer and a departed device are handled oppositely.** Aux In
sends a subscription whose source channel was deleted to `DEPARTED_SOURCE`
(`aux_in.rs`), keeping it inert and inspectable. The modulation rack drops
routes whose device is gone (`modulation.rs`) while keeping *illegal* routes
inert. Nobody has decided whether they should match.

---

## One name, two policies

**An embedded sample that is a symlink escapes the bundle, is read, and is
copied into the next bundle saved from it.** `embedded_bundle_path`
(`mooloop-project/src/lib.rs`) is purely lexical and `resolve_setup_asset`
then reads the file, so a shared `.mooloop-channel` or kit bundle can make the
app read an arbitrary local file as audio, and **re-saving that preset stages
the symlink target's bytes into the new bundle**, which the user may then
share. Options: canonicalise both sides and require containment for embedded
references (a syscall per sample, and it changes behaviour for anyone
deliberately symlinking a shared sample library into a bundle); or refuse a
symlink under `samples/` outright.

**The compensation plan's inputs are still derived twice, in two crates.**
The policy is shared (`mixer::sends_are_compensable`,
`mixer::compensable_send_edges`), but `RenderState::install_compensation` and
`Session::latency_plan` each walk their own channels and buses to build
`channel_latency`, `channel_bus` and `bus_latency` before `compile_latency`,
because the engine reads `ProjectChannel.setup` and the session its own
channel type. Sharing the walk means a shared input type or a trait.
`Session::console_plan` against `RenderState::install_console` is the same
shape, and nothing checks it. `an_offline_render_compiles_the_same_compensation_as_a_live_one`
does not read the session's derivation.

**Load silently deletes authored modulation the spec says to keep as an
orphan, and the mechanism built for keeping it is unreachable.**
`ModRack::deserialize` (`modulation.rs`) drops four things with no
diagnostic: a slot past `MAX_MODULATORS_PER_CHANNEL`, a route whose source id
names no surviving slot, a route whose `to_local_slot` fails, and every route
past `MAX_MOD_ROUTES_PER_CHANNEL` (`apply_route`'s `None` is discarded into
`let _`). `MODULATION.md` says the opposite: persistence "retains it as an
inspectable orphan rather than silently deleting authored work."
`UNRESOLVED_SLOT`'s doc comment describes parking that no reachable path
does. Options: implement the spec (park un-sourced routes and report the
truncation through `mooloop-project`'s `Doctor`); or keep the deletion and
have the `Doctor` report it; or amend the spec to say load-time truncation
deletes. Decide the departed-producer row above with it.

**A unipolar route from a *fading-in* LFO rises from half the module's depth
rather than from the floor.** `offset_for`'s lift stands on
`ModRack::wire_span`, the span the module's params say it reaches, which
cannot see `fade`, DSP state in `Lfo`. Reading it per control tick means a
second `[[f32; 8]; 256]` table per channel beside `ControlOutputs` -- 8 KB a
live channel -- for a parameter that defaults to zero seconds. Exact from the
moment the fade completes.

**A Math module's `input_slot` follows a reorder but not a removal.**
`retarget` (`modulation.rs`) carries the input across a `move_module`, and
`clear` does not touch it, although `clear`'s own comment warns that a
destination left on an emptied slot is inherited by the next module installed
there. Remove the LFO a Math reads, install anything else, and the Math is
silently multiplying the new module. `move_module`'s compaction has the same
hole: `remap` is filled only for occupied slots.
`a_new_module_never_inherits_a_removed_ones_routes` covers routes, not this.
Lower confidence: `input_slot` is `MATH_PARAM_INPUT_SLOT`, a persisted stepped
parameter drawn as a slot *number*, so "it points at slot 1 and slot 1
changed" is a defensible reading. Options: give Math a `ModSourceId` input the
way a route has one (changes a persisted field's meaning); or park a dependent
input out of range on `clear` and map empty-slot references in `move_module`;
or decide the slot number is the contract and delete the `retarget` remap.

**The bus-bank repair is logged, not reported, and `load_bundle` on its own
still hands back an unsanitised bank.** `sanitize_bank` returns a `BankRepair`
per correction, which `Session::replace_project` writes only to the log: they
are not in `LoadReport::repairs`, so the status bar's count, the repair log's
load line and `Diagnosis::report()` omit them, though the next save writes the
sanitised bank. Moving the sanitise into `integrity::check_buses` needs
`mooloop-project` to call `compile_bus_graph` itself and changes when it runs
relative to `Session::replace_project`, which runs on every install, an undo
included; the narrow alternative is a second entry point used only by the
load path. Separately, anything driving `load_bundle` directly -- an offline
render, a headless measurement loop -- gets a graph `compile_bus_graph` will
refuse.

**`from_index` answers out-of-range input two different ways depending on
which enum you ask.** Forty-five enums convert a selector index to a variant
under one name: the `Self::ALL.get(index.clamp(0, len - 1))` body clamps to
the nearest end, and a hand-written `match` with a `_ =>` arm falls through to
the default variant (`NotePriority::from_index(99)` is variant 0 where an
ML-P8 enum's is its last). Unreachable, because every parameter path runs
through `descriptor.clamp_natural` first, so unifying the bodies is churn. A
sentence on each, or one shared trait, would settle it whenever one of these
files is open anyway.

## Numbers nothing is watching

**One device face still spells numbers the check cannot reach.**
`scripts/dupe-audit unchecked-face` names `bus-device.slint`'s four: a
`MiniKnob`'s pan range (`-1..1`, resting at `0`) and a `MixerFader`'s
`default-value: 1.0`. `face_knobs` walks `ParameterKnob` blocks and nothing
else, and neither is descriptor-backed, so there is no table entry to compare
them against. It may not be worth fixing.

## Housekeeping

**Tempo and swing live only in the Slint window, and a few flags are read
back from it.** `Session` has no field for BPM or swing:
`Session::project_snapshot` takes them as arguments, and `lib.rs` reads
`get_bpm` and `get_swing_percent` throughout. Smaller, and the same shape:
`editing-bus` and `editing-bus-index` duplicate `Session::effect_target` (the
track move and solo/mute shortcuts read the window copy), and `playing`,
`pattern-length` and `selected-channel` are read back rather than asked of the
session. Moving tempo and swing onto `Session` is the one worth doing before
any view rewrite. The full list is in
`docs/plans/archive/egui-view-layer/00-status.md`.

**A rack unit is two different widths.** An effect slot is `unit-width *
units + half-gap * (units - 1) + rail-width * 2`; the source device is
`unit-width * units + half-gap + rail-width * 2` (both in `main.slint`), with
the gap term not multiplied. They agree only at two units: a three-unit source
is 724px where a three-unit effect is 728px, and a four-unit source is 944px
against 952px. Nothing is visibly misaligned. Which is wrong is a design call:
the majority use `* (units - 1)`, but `JOURNAL.md` records the three-unit
source face at its inner 664px as measured and deliberate, so correcting the
source formula widens signed-off faces by 4px or 8px.

**The desktop entry claims less than it could** (`packaging/mooloop.desktop`).

- **`.mooloop` has no MIME type**, so double-clicking a song opens nothing and
  a song file has no icon.
- **No `StartupNotify`, deliberately.** winit consumes an activation token
  only when asked (`with_activation_token`), and Slint's winit backend never
  asks (`i-slint-backend-winit 1.17.1`). Setting the key costs a launcher
  spinner that never resolves; this wants the backend to grow the feature.
- **One 256x256 icon**, flagged as a placeholder wordmark in
  `packaging/README.md`. Read that note before adding sizes.

**Nothing inhibits idle while the transport is playing**, so the screen can
blank and the lock screen can take the display in the middle of a take; no
crate mentions idle inhibition. The policy question is what counts as busy:
transport running is the obvious answer, and an armed recording or a held note
is the one that would annoy somebody if it were missed.

There is `claude/device-identity-rack-addressing-99yt4o` on the remote, one
commit that is not in `origin/main` and has no local branch. Nobody has said
whether it is wanted.
