# Loose Ends

Small known gaps that agents flagged when handing work back, gathered into one
place so they stop living in chat scrollback. The file and line named is where
to start.

Everything here as of **2026-09-06** was re-verified against the tree that
day. Entries added since carry their own date, and entries older than that
sweep have not been checked against the tree since it — the spike list below
was still claiming thirty-nine unpushed commits on `main` a day after `main`
was pushed, which is what this paragraph is now careful about.

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

**Saving with embedded assets repoints the sampler's prev/next-sample arrows
into the song's own bundle.** The app writes the resolved paths back into the
live session after a save, and `selected_sample_target` lists
`sample_path.parent()` -- so a kick loaded from a fifty-file drum folder, once
saved with Embed Assets on (the default), has arrows that walk
`<song>-assets/samples/` instead. With four sampler channels embedded that
directory is `00-kick.wav, 01-snare.wav, 02-hat.wav, 03-clap.wav`, so "next
sample" on the kick loads the snare out of the bundle.
`can_previous_sample`/`can_next_sample` are computed only in
`apply_loaded_sample` and never recomputed on save, so the buttons stay lit
and lie about it. The fix is a second field -- the browse origin, which the
save's write-back must not overwrite -- rather than a change to what is
stored, because only `project_snapshot` needs the bundle-relative form. Found
2026-09-13.

**A preset's name appears on the device before the save is known to have
worked.** `set_effect_preset_name`/`set_source_preset_name` run synchronously
on confirm, while the write happens on a worker thread and can fail -- a
too-long name, a permission, a full disk. The error dialog opens and the rack
row goes on showing the name of a preset that was never written. The fix is to
move both calls into the `SavedPreset` arm, which means carrying the name on
that variant. Found 2026-09-13.

**The preamp face has no transfer-curve display, where Drive's has one.**
`preamp-device.slint` leaves the panel Drive fills with
`DriveTransferDisplay` empty. Drawing this stage's curve needs the
coefficients `HarmonicShaper::new` solves for, and reaching them means
widening `EffectSlotRow` in `main.slint` -- which is what the EQ, the
dynamics trio and the Buffer already do for `eq-spectrum-data`,
`gain-reduction-db` and `buffer-collisions`, so the path exists and is
ordinary. The alternative, computing them in markup from the voicing index,
would spell `harmonics.rs`'s profile numbers a second time, which is the
duplication `AGENTS.md` names as this codebase's characteristic fault.

**A transfer curve is probably the wrong display for it anyway.** Adam,
2026-09-10: the interesting question is *where on the spectrum* the stage is
distorting, which a curve cannot show and which is the whole point of the
tilt -- obvious on a kick, nearly clean on a hat. `SpectrumAnalyzer` already
produces exactly the right thing (48 log bands, a Goertzel bank rather than
an FFT, published once a hop and only while a display subscribes), and this
device is unusual in having both the dry and the wet signal in hand at the
same sample. Two analyzers and a per-band difference would make the tilt
visible on the same axis `colour_harmonics_vs_freq.csv` plots.

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

**Spectrum subscriptions are keyed by slot index and nothing re-keys them
when the chain changes.** `set_effect_spectrum_enabled` is called from the FFT
toggle (by the slot index at that moment) and from
`sync_effect_spectrum_subscriptions`, which runs only inside a project
install. Effect insert, removal, reorder and wrap do not go through one. So:
turn the FFT on for an EQ in slot 2, delete the device in slot 1, and the
analyzer draws a flat line behind a lit button until it is toggled twice or
the project reloaded. The orphaned stage keeps its pool entry, so enough
add/enable/remove cycles exhaust `SPECTRUM_SLOTS` and every later analyzer
silently draws zeros; and if a later device lands on that stage, the engine
runs a full Goertzel bank every hop for a display nobody is drawing. Not small
because the fix is a decision about where the subscription lives: re-sync
after every structural rack edit (cheapest, an O(channels x slots) walk on
every device drag, still keyed on a position); permute `spectrum_enabled`
alongside `MoveEffect`/`RemoveEffect` in the engine; or key the subscription
by the device's durable id, which cannot drift and is the largest change. Same
shape as the modulator-reorder entry above -- a permutation mirrored by index
rather than by identity. Found 2026-09-13.

**Two device faces draw a clip lamp that can never light.** The device rack's
IN and OUT rails and the bus face all instantiate `ChannelMeter`, which always
draws a `ClipIndicator`, and none of the three binds `clipping` or
`clip-reset` -- so every device row shows two faint red bars that cannot light
and do nothing when clicked, which is the "convincing but inert control" the
rack's own rule names. The bus face is the sharper case because the data is in
hand and thrown away: the same `MeterReading` the mixer strip uses carries
`held_db` and `clipping`, and `main.slint` declares only the two level
properties for that face -- so one track has two faces whose meters behave
differently. Fixing it is a `main.slint` contract change (three new
properties), so it belongs in the next batched cross per `AGENTS.md`'s "face
contract last". `ChannelMeter` could also grow a `show-clip` so a rail can opt
out honestly. Found 2026-09-13.

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

**The ninth automation lane draws, edits and saves, and never plays.** A file
carrying more than `MAX_AUTOMATION_LANES_PER_CHANNEL` lanes in one (pattern,
channel) is handled three ways: `integrity::check_lanes` calls `refuse`,
which records the issue and **repairs nothing**, so the project loads
unchanged; `Session::replace_project` clones them all, so the editor lists,
draws, edits and re-saves them; and `Pattern::set_lanes` takes the first
eight, so the engine has never heard of the rest. Not reachable from the app
now that the picker refuses past the ceiling (fixed 2026-09-13), so this is
hand-edited, foreign-build or future-version files -- which is what keeps it
out of the fixable list, not its severity. `refuse` was chosen because the
only correction discards authored work, which is the right instinct except
that `set_lanes` already discards it, silently and on one side only.
`PROJECT_FORMAT.md` does not state the cap at all. Options: make
`check_lanes` correct and truncate so both sides agree and the `Doctor` says
which lanes went; or truncate in `replace_project` too so the document matches
the engine; or raise the cap, since eight per channel per pattern is low for a
song automating a channel and two buses, and `CAPACITY_POLICY.md` says "it was
easier to preallocate" is not a sufficient reason. In every case the number
belongs in `PROJECT_FORMAT.md`. Found 2026-09-13.

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

**There is no way to hear a track before its own fader.** Solo is in place
as of 2026-09-11, which silences the others rather than opening a monitor
path, so a soloed track is still heard through its fader, its pan and its
analog-sum switch. `MIXER_PLAN.md` records the AFL tap as later work and
names what it needs: the tap points that pre-fader sends also want.

**A channel cannot be soloed, only its track.** `MixerBus.solo` is per track,
which is the same scope the analog-sum switch has and for the same reason
(`docs/TERMINOLOGY.md`: a mixer strip is a track). Soloing one channel of
several on a track has not been asked for, and would need the per-channel
strip the sends work also wants — so it is one control on one face away, not
a design question.

---

## Focus

**A text field is left with Enter, and by nothing else.** The toolbar's search
and rename fields and the knob/fader numeric entries all call `clear-focus()`
on `accepted` (`toolbar.slint:395`, `controls.slint:959`,
`controls.slint:2005`; the three line numbers this entry carried were all
stale by 2026-09-10) and have no Escape handler, so clicking into one and
then clicking away leaves the caret in it. While it is there, Space types a
space instead of starting the transport — which is correct for a field being
edited and wrong for a field nobody is editing. The 2026-09-07 focus fix made
every *control* transparent to shortcuts; text fields are the remaining case,
and they need a way out rather than a change to what they consume.

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

**Removing a track does the silent orphan repair `MIXER_PLAN.md` says must
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

**A lane that stops covering the playhead leaves its destination stuck at the
last value it wrote.** `restore_base_param` exists for exactly this and its
comment names the hazard -- "Removing a lane or a matrix route otherwise
leaves the device holding whatever the control signal last resolved, until
someone happens to touch that knob" -- but it is called only when a lane is
*deleted* or *cleared*, never when a lane stops covering the playhead.
Pattern 1 sweeps a cutoff down to 200 Hz, pattern 2 has no such lane; switch
to pattern 2 and `has_automation_at` answers false, `control_events_for_slot`
takes its early return, and the device never receives another `ParamValue`.
The filter plays at 200 Hz while its knob and its face both read 1 kHz, until
the knob is touched or the song reloaded. Same on a song-mode clip boundary
and on `SetPlaybackMode`. It bites only destinations that are automated and
*not* modulated -- a modulated one takes `base_normalized = knob_normalized`
when the curve is `None` and so restores the knob every block by accident.
Not small: a complete fix needs the engine to know which destinations had a
curve last block and no longer do, which is per-channel state across blocks on
the audio thread, and `AutomationBlock` is explicitly "a read-only view".
Options: restore on the *commands* only (`SetCurrentPattern`,
`SetPlaybackMode`, `Seek`), walking the outgoing pattern's lanes on the
command drain where `forget_device` already runs -- bounded, and fixes the
reachable pattern-mode half; or carry a "driven last block" set and diff it,
the only complete answer; or declare that automation latches and say so in
`CURRENT.md`, which is a defensible DAW convention but then makes
`restore_base_param`'s three existing callers the inconsistency. Found
2026-09-13.

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

**Pattern names are wiped by every project edit and never reach disk.**
`replace_project` does `self.pattern_names = vec![String::new(); ...]`
(`session.rs:1146`), and every `ProjectEdit` runs through it -- so naming
three patterns "Verse", "Chorus", "Bridge" and then adding a channel, cloning
a pattern, or pressing Ctrl+Z blanks all three. `pattern_names` appears
nowhere in `mooloop-core`, `mooloop-project` or `PROJECT_FORMAT.md`, so they
do not survive a save either. `CURRENT.md` documents naming a pattern as a
peer of naming a channel or a track, with a paragraph on why a pattern may be
blank where the others may not, and says nothing about the name being
transient. Not small because a name has to survive `replace_project`: either
carry it in `Project` -- a persisted field, a migration, and length
validation in `integrity.rs`, which is also the only option that makes
`CURRENT.md` true and fixes persistence -- or keep it session-side and give
`ProjectEdit` a pattern-edit field beside `channel_edit`, since
`queue_pattern_clone` and `queue_pattern_remove` insert and remove pattern
indices in a cloned `Project` that the session's name vector knows nothing
about. Found 2026-09-13.

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
refuses to create.** `session/transport.rs:213` guards `add_playlist_placement`
against overlap, correctly and half-open, and it is the only place the
invariant exists: `set_pattern_length` (`:123`) rewrites the length with no
revalidation, `Sequencer::set_playlist_placement` has no overlap notion, and
`integrity::check_playlist` checks only the pattern index and the start tick.
Place pattern 0 at ticks 0 and 384 at 16 steps -- accepted, they abut exactly
-- then set it to 32 steps, and the first clip covers the second. Both are
scheduled: `instance_offset` differs, so the voice ids differ, and **every
note fires twice 384 ticks apart at doubled amplitude**. The view draws them
overlapping, and `placement_covering` uses `find` on a list sorted by
`(pattern, start_tick)`, so a click in the overlap always removes the earlier
clip and the buried one cannot be reached. Not a small fix because the
invariant has no owner: enforcing it in `set_pattern_length` means deciding
what a length increase does to the clips it now swallows, and the same
decision has to be made in `integrity` for files that already carry the
overlap -- a `Doctor` entry and a `PROJECT_FORMAT.md` change across two
crates. Options: clamp the length change to the largest value that keeps the
pattern's placements disjoint; or make the overlap legal everywhere and drop
the guard in `add_playlist_placement`; or drop the covered placements and
report them, which is the only one that also needs a repair path on load.
Found 2026-09-12.

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

**`MAX_CONTAINER_DEPTH` is enforced by nothing, reported by nothing, and
spelled a fifth time as bare numbers in the markup.** `structure.rs:38` says
it is "a limit on the *gesture*, not on the format: a deeper chain loads and
is reported by `integrity.rs` the way an over-long one is", and
`CAPACITY_POLICY.md:189` and `docs/plans/containers/02-...md:66` repeat the
same two sentences. Both are false. The constant appears in four places in the
workspace -- its definition, its re-export, and two uses in `render.rs` --
and neither `mooloop-session` nor `mooloop-ui` mentions it. No gesture checks
it: `wrap_in_container`, `insert_into_container` and
`move_effect_into_container` have no depth test, and `wrap-enabled` is
unconditional on every row. `integrity.rs` has no depth check either;
`span_problem`'s own doc says "Depth is deliberately not checked".

So five clicks reach it with no warning: wrap a device, then wrap the box four
more times. Past the cap the render branch `continue`s, so the innermost box's
**Mix does nothing at any value**, and `main.slint`'s `for level in [1, 2, 3,
4]` -- `MAX_CONTAINER_DEPTH` written out as four literals, read by no test --
has no level 5, so it draws no chrome either. It is inert and invisible at the
same time.

Its **bypass** used to be dead too, because the depth `continue` sat above the
bypass branch; that half was fixed 2026-09-13 by moving the check below it,
which needs no decision -- being too deep to blend is no reason for a bypass
button to lie. The rest is a decision, because `CAPACITY_POLICY.md:255` has a
standing rule for a cap on a user-created collection (document it beside the
type *and in the persisted-format validation*, make the UI communicate it
honestly, test the boundary) and one of the four is done. Options: make the
three claims true -- refuse the gesture, add an `effect.container.depth` check
to `check_spans`, and derive the Slint level list rather than spelling it; or
drop the cap, since all it buys is one `StereoBus` per level in a `Box`
allocated off the audio thread, and 8 or 16 costs a few hundred KB on chains
that hold a box at all; or cap the gesture only and delete the integrity
sentence from all three documents. Found 2026-09-13.

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

**Device meters are drained only for the chain currently on screen.** The
pump takes `take_device_peak`/`take_device_dynamics` for one `device_target`,
and every other channel's and bus's stage cells are `fetch_max` holds that
nothing ever empties -- so switching the rack to a channel last viewed ten
minutes ago draws that ten-minute maximum for one 8 ms tick before the next
read clears it. Two lines in the bus loop used to be an attempt at this and
could never have worked: both `publish` and `publish_input` are `fetch_max`,
so writing zero cannot lower a cell. They are gone (2026-09-13) along with the
comment claiming they cleared the meter. Not small because draining every
target every tick is `(MAX_CHANNELS + MAX_BUSES) x (MAX_EFFECTS+1) x 6` atomic
swaps at 125 Hz, which is the cost the spectrum pool exists to avoid in the
analogous case. Options: drain the *previous* target once when `device_target`
changes, which needs one `last_device_target` local in the pump and is
correct; or drain the whole array on a slow secondary timer; or accept the
one-frame flash and write it down. Found 2026-09-13.

**A bus clip latch outlives the track it belongs to.** Removing a track shifts
every later one down an index, and nothing resets the per-bus
`MeterBallistics` the pump owns -- whose clip latch never self-clears, by
design. Delete track 3 with its clip lamp lit and old track 4, now track 3,
opens with a latched clip it never earned, permanently, until somebody clicks
it. Peak hold and decay transfer too but wash out in under two seconds. Not
small because the ballistics are `move`d locals inside the pump closure with
no outside handle, so clearing them on a track edit needs the same flag
handoff `master_clip_clear`/`bus_clip_clear` already use. Options: a
`meters_reset` flag raised by `install_project_in_ui` (bluntest, also covers
reorder); reset the removed index and shift the rest, which needs the edit's
shape rather than "something changed"; or rule that a clip latch belongs to
the strip position rather than the track. Found 2026-09-13.

**The master is metered twice, through two transports, with two clip
latches.** `executor.rs` pushes `EngineEvent::Metering` onto the bounded event
ring every block and `render.rs` publishes the same numbers into `BusMeters`
cell 0. The transport bar reads the event; the mixer's master strip reads the
cell. The event push is `let _ = evt_tx.push(..)`, so under ring pressure the
**always-visible toolbar meter** is the lossy one while the atomic cell cannot
drop a block -- the two meters for one signal can disagree. And there are two
independent clip latches for the master: clicking the toolbar's does not clear
the mixer strip's, or the reverse. Options: drop `EngineEvent::Metering` and
read bus 0 for both, which unifies the latch for free and removes a per-block
ring push; or keep both and share one `MeterBallistics` pair. Found
2026-09-13.

**A muted channel that something taps meters silent and freezes its playhead
while its audio flows.** There are two mute paths for a channel. The one where
nobody taps it skips the render, so a frozen playhead is honest. The other --
muted, but an Aux In reads this channel -- runs `strip.process`, so voices
advance and the audio *is* heard through the Aux In, and then `continue`s
before both the device-meter publish and the playhead publish. So the source
rail reads silent, and the sampler playhead stops at the mute and never moves
again or clears, because `source_silent_frames` is reset every block and the
sleep branch's zero-publish can never run. Adjacent to the solo entry above
but the opposite sign: there a silenced track meters live, here an audible one
meters dead. Options: publish both before the `continue`, matching the comment
that already says "a muted producer publishes"; or publish only the playhead,
since a frozen line over a moving voice is indefensible under any reading; or
rule that the Aux In's own channel is where that signal should be metered.
Whoever rules on the solo entry should rule on this one too. Found 2026-09-13.

**A track silenced by someone else's solo still meters, and can still latch
its clip lamp.** `render.rs` computes `let muted = strip.output.muted ||
strip.solo_silenced` and governs the sends, the summing and the send emission
with it -- then twenty-four lines later the meter alone re-reads
`strip.output.muted`. So under a solo, a silenced track's strip meter, its
peak hold and its clip latch all report a signal nobody can hear, while the
comment directly above that line says the meter shows "what is heard rather
than what is running" and `MIXER_PLAN.md` says the strip shows **audible**
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

**The project is 4/4 end to end.** `render.rs:1709` hardcodes
`const BEATS_PER_BAR: f32 = 4.0`, and `integrity.rs:480` rewrites any other
meter back to 4/4 on load with a doctor message. `Project.beats_per_bar`
exists and is settable in memory, so the field promises more than the engine
delivers. Not a silent bug any more — but a 3/4 project is not a thing.

**`position_ticks` is an accumulator, not derived from `frames_played`**
(`mooloop-engine/src/transport.rs:24`). `ARCHITECTURE_REVIEW.md`'s action
table calls this out and says to fix it *with* the tempo map, not before.
Recorded so the deferral stays deliberate.

---

## Decisions whose reason expired

**A song's embedded samples can never be turned back into references, and
unticking the box is discarded in silence.** `keep_owned` is true whenever an
embedded sample already lives inside the bundle, and it skips the
`AssetMode::Referenced` branch entirely -- so after the first embedded save,
every later save keeps them embedded whatever the user asked for. The guard
itself is **necessary**: without it `replace_song_file` would delete the
sidecar the new reference points at, destroying the only copy. What is wrong
is everything around it. Unticking "Embed assets" and saving produces no
warning, no status message and no change. The manifest records `asset_mode =
"referenced"` beside `embedded = true` on every sample. And reopening sets the
checkbox from the document-level `asset_mode`, so the box shows *unticked* on
a bundle whose samples are all embedded, and the state never converges.
`CURRENT.md`'s "embedded and referenced asset policies are available per save"
is true only of a song that has never been embedded.

Not small because un-embedding has to mean something -- copy out to where, and
under whose name. Options: refuse honestly, reporting "N samples stay in the
bundle; there is no other copy" in the save report's warnings and leaving the
box showing embedded; make the checkbox follow the *per-sample* flags rather
than the document-level mode so it at least tells the truth; or implement a
real un-embed that copies bundle-owned samples out to a chosen folder first,
which is a new user-facing gesture. The first two together are cheap and
remove the lie. Found 2026-09-13.

**The limiter still has no lookahead, and the code's stated reason is now
false.** `mooloop-dsp/src/effects/dynamics.rs:391` says "Add lookahead when
the engine can compensate for it, not before." The mixer became latency
compensated on 2026-09-05, so the condition is met. `CURRENT.md` already
records this as an open decision rather than a settled no; the source comment
does not.

**`BUFFER_ENGINE.md` still specifies Buffer as an ordinary insert** "at the
useful point in a chain" (lines 12, 45, 66). Adam has since said that framing
is partly wrong — the device belongs at the end of a rack with its own lane.
Nothing in the repository captures the rethink; the doc reads as settled.

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

**The modulator plan's acceptance test 8 — RT hygiene, no allocations or
locks in the audio callback — still has no harness that can express it.**
There are two `#[global_allocator]`s in the tree and neither one does this
job: `spikes/time-stretch/src/main.rs:52` is outside the workspace, and
`mooloop-session/src/lib.rs:55` is `#[cfg(test)]`, counts *live bytes* to
measure undo-history footprint, and lives in the session crate rather than
in engine or DSP where the callback actually runs. Counting a steady-state
total is not the same as trapping an allocation on the audio thread. Until
something is, the test is satisfiable only by reading code — which is the
thing it exists to replace.

---

## Consistency questions, not bugs

**Duplicating the last device in a container puts the copy outside the
box.** `duplicate_device` is `copy_device` then `paste_device` at the same
slot, and `paste_device` inserts at `run_of(slot).end` -- where a run's end
boundary counts as *outside* the container, which is the documented and
tested rule for paste
(`pasting_onto_a_containers_last_child_lands_outside_the_box`). For duplicate
it produces a boundary inconsistency rather than a rule: in `[Chain(2),
Filter, Drive]`, duplicating the Filter inserts at 2, inside the span, and the
box grows to 3; duplicating the Drive inserts at 3, the span's exclusive end,
and the copy lands outside. Same gesture, same box, two answers depending on
which child was clicked -- and the ejecting one is the case a user reaches for
most, duplicating the thing at the end of the run they just built. Not a
one-liner because the ambiguity is the one `insert_into_container` exists to
resolve: the index after a run's last row means both "still inside" and "just
after", and only the gesture can say which. Either give `duplicate_device` its
own landing rule -- grow the parent explicitly when the insertion point is its
span's end -- or state in `CURRENT.md` that a duplicate lands beside the
original at the original's own depth. Found 2026-09-13.

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

**Renaming a song file makes it permanently unopenable, even when the assets
sidecar is renamed with it.** `safe_embedded_path` requires a stored relative
path to begin with *this song's exact file name* followed by `-assets`. Rename
the pair the only sane way -- in a file manager, both together -- and the song
refuses to open with "channel 0 has unsafe embedded path
Untitled.mooloop-assets/samples/00-kick.wav", about a path that is present,
relative, traversal-free and sitting right beside the file. It is
`Error::Invalid`, so the whole song is refused rather than one sample warned
about, and recovery means hand-editing TOML. (Copying the song *without* its
sidecar is handled correctly: the name matches, the file is missing, you get a
warning.)

Not small because it is a question about what the check is for. The
`Component::Normal | CurDir` filter above it already guarantees the path
cannot escape the song's directory; the name equality only adds "and it is
*this* song's sidecar", which is exactly what a rename breaks. Options: drop
the name equality, keeping "first component ends in `-assets`, second is
`samples`" -- two lines, every traversal property kept, but the stored
component still names the *old* sidecar so the song then opens with the
samples missing, turning a brick into a silent loss (pair it with the asset
warnings now being logged); or repoint on load, substituting the bundle's
actual assets directory name, so a rename self-repairs -- the behaviour a user
expects, and the option that makes the product right, but it makes the loader
a path rewriter and that needs a ruling; or downgrade the mismatch to a
warning, which is least code and worst outcome. Found 2026-09-13.

**An embedded sample that is a symlink escapes the bundle, is read, and is
copied into the next bundle saved from it.** `safe_embedded_path` is purely
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

**The compensation plan is derived twice, in two crates, and the copies have
already diverged.** `RenderState::install_compensation` guards the send half
on whether the bank sorts -- `let sorts = compile_bus_graph(..).is_some()`,
with a comment giving the reason: a send compiled against an order that is not
the one being walked would arrive a block late, so a bank running the
everything-to-master repair gets no sends either. `Session::latency_plan`
calls `send_edges` unconditionally and `send_specs` iterates every bus
unconditionally, so on a bank that does not sort the session would hand the
engine a full `SendBank` compiled against arrival numbers derived from a
default order.

Unreachable today, because the session's bank is sanitized on load and both
`set_bus_output` and `add_send` refuse cycles. It is the characteristic fault
in its pure form: two copies of one policy, one corrected and one not, with
nothing able to notice. `an_offline_render_compiles_the_same_compensation_as_a
_live_one` looks like the test that holds them together and does not -- both
sides of that comparison go through `install_compensation`, and the session's
derivation is never compared against anything. The same shape one size down
holds for `Session::console_plan` against `RenderState::install_console`.
Options: extract the shared derivation into `mooloop-core` so both call sites
become three lines, which removes the copy permanently; or add a test that
renders one `Project` through both and asserts the plans match, which is
cheaper and is at least a test that reads both copies; or, minimum, port the
`sorts` guard across so the two agree today, which does not stop the next
divergence. Found 2026-09-13.

**Load silently deletes authored modulation the spec says to keep as an
orphan, and the mechanism built for keeping it is unreachable.**
`ModRack::deserialize` drops four things with no diagnostic: a slot whose
index is past `MAX_MODULATORS_PER_CHANNEL` (`modulation.rs:1510`), a route
whose source id names no surviving slot (`:1543`) -- which is exactly what a
capacity truncation produces -- a route whose `to_local_slot` fails (`:1536`),
and every route past `MAX_MOD_ROUTES_PER_CHANNEL`, because `apply_route`'s
`None` is discarded into `let _` (`:1548`). `MODULATOR_SYSTEM_SPEC.md` says
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

**Reordering the modulator grid restarts or cross-wires every moved module's
running state.** `EngineCommand::MoveModulator` goes through
`edit_modulation` (`render.rs:3168`), whose only mirror into the DSP rack is a
params diff *by slot number*. A reorder is a permutation, so every moved
position reads as "the params changed" and gets `set_slot`, which knows about
a slot being reconfigured and not about a module having moved. Different kinds
swap and both are rebuilt from scratch: an envelope dragged to the front
restarts at level 0 stage `Idle`, so a held note's contour drops to zero
mid-sustain and will not re-arm until the next Note On; a Random module is
reseeded, changing the sequence that is supposed to be identical between
realtime and offline. Same kinds **cross-wire**: two LFOs dragged past each
other keep their own phase, smoothing state and fade position and take the
other's params, so both jump and nothing shows why. The session layer's own
comment (`session/modulation.rs:120`) claims "both racks run the same
permutation", which is true of the data and not of the running state. Not a
small fix because `edit_modulation` is deliberately generic over
`FnOnce(&mut ModRack)` and cannot tell a reorder from any other edit -- the
diff is what makes narrow commands cheap. Options: give `ModulatorRack` a
`move_slot(from, to)` that permutes `slots` and `outputs` together and give
`MoveModulator` its own handler ahead of the diff, which then correctly finds
nothing changed; or key the DSP rack by `ModSourceId` rather than by slot,
which removes the class; or accept the restart and say so in the spec -- but
the same-kind cross-wiring is not defensible under any reading. Found
2026-09-13.

**A unipolar route only rests at its base when the module's own Amount is
exactly 1.** `offset_for`'s lift is `(output + 1.0) * 0.5`
(`modulation.rs:1997`), which assumes the source spans the full `-1..1`. An
Envelope and a unipolar Random do; an LFO does not, because its output is
`raw * depth * fade`, so the lift's minimum is `(1 - depth)/2` rather than 0.
`mlm1_factory.rs:272` ships an LFO to pulse-width route at `Unipolar` with the
module's Amount at 1.0, which is correct today -- turn that AMOUNT to 0.5 and
the destination does not modulate less around the same floor, it shrinks its
swing *and rises*. At Amount 0 the module contributes a constant `+0.5 *
depth` offset while visibly producing no movement: "the modulator is off" and
"the destination is parked half a depth up" become the same knob position.
The spec names this exact hazard for outlets (lines 305-309) and does not
address it one level down, where it says flatly "A unipolar route maps that
output to `0..1`, making the base the floor." Not a small fix because it is a
question about what `Unipolar` means and the answer changes resting values in
saved projects: map the source's *declared* range onto `0..1`, which needs
`ModSourceDescriptor.signal` carried into the realtime path where it is not
today; or accept it on the grounds that reducing a bipolar source's amount
legitimately collapses it to its midpoint -- defensible, but then the spec's
polarity paragraph has to say so. Found 2026-09-13.

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

**Half the bus-bank repair happens in `mooloop-core` and reports nothing, and
one branch of it can delete every send in the project.** `PROJECT_FORMAT.md`
attributes three repairs to the loader; only the out-of-range destination is
there. `mixer::sanitize_bank` (`mooloop-core/src/mixer.rs:305`) does the other
two -- drop a send whose target is gone, flatten a cycle to
everything-to-master -- and it is called from `Session::load_project`
(`session.rs:1140`), after the integrity pass, with no `Doctor` in reach.

Four consequences, in order of how much they cost a user. The repair is
**unreported**: `LoadReport::repairs` is empty for it, so the status bar, the
repair log and the copyable `Diagnosis::report()` all omit it. It is then
**persisted** -- the sanitized bank becomes `self.buses`, and the next save
writes it -- so the sends are gone from the file and not just from the running
document. The cycle branch clears `sends` on **every** track rather than the
ones in the cycle, so one bad `output` edge in a hand-edited or foreign-build
file costs a whole bank's aux routing silently. And `load_bundle` on its own
returns an unsanitised bank, so anything driving the loader directly -- an
offline render, a headless measurement loop -- gets a graph `compile_bus_graph`
will refuse.

The fix is not small because the repair needs the whole graph: moving it into
`integrity::check_buses` means `mooloop-project` calling `compile_bus_graph`
itself, and it changes *when* the sanitising runs relative to what
`Session::load_project` assumes about the bank it is handed. Three options,
and the third is worth doing whichever of the first two wins: move it into the
integrity pass and report each dropped send and the cycle flatten through
`Doctor`; or leave it where it is and have it return its issues for `Session`
to fold into the `LoadReport`; or narrow the cycle branch to break the smallest
set of edges that makes the graph compile, instead of clearing everything.
`PROJECT_FORMAT.md`'s limits section now says where these actually happen
rather than claiming the loader does all three. Found 2026-09-12.

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

**`render_blocks` is written about seven times.** `audio_edge_tests.rs`,
`container_tests.rs`, `ds01_tests.rs`, `idle_skip_tests.rs`,
`console_tests.rs`, `gain_structure_tests.rs` and `strip_tests.rs` each
declare their own "render N seconds in blocks of M and collect the output".
They are all `#[cfg(test)]` modules inside `mooloop-engine/src`, so unlike
the `mooloop-ui` integration tests they can share a plain module without any
`tests/common/` arrangement. The cheapest of the duplication items here.

**The Slint testing backend is set up eighteen times.** Fifteen
`mooloop-ui/tests/*.rs` files spell out the same
`TestingBackend::new(TestingBackendOptions { mock_time, threading,
renderer_name: "software" })`, `source_snapshot.rs` eleven times on its own.
This is the same shape as the piano-roll grid constants below, wants the same
`tests/common/` module, and would be worth doing in the same pass.

## Numbers nothing is watching

**`gain::MIN_DB` is spelled twenty-six times in markup and the test that
checks its two siblings does not check it.**
`slint_meter_thresholds_match_the_rust_constants` holds `meter-warning-db`,
`meter-hot-db` and `reference-peak-dbfs` to their Rust constants. It does not
hold the meter *floor*, because `GainMath` has no property for it:
`gain.slint` spells `-60.0` inline inside `db-to-linear`/`linear-to-db`, and
`meters.slint` and `controls.slint` spell `-60` twenty-two more times across
`minimum-db` defaults and resting values. The Rust side is clean -- everything
goes through `METER_FLOOR_DB`. So this is the characteristic question
answering *no* for the floor of every meter in the application while answering
*yes* for the two colour thresholds beside it, and the `declares()` helper the
test would need already exists. Not a one-liner because the four `minimum-db`
properties are scale *declarations* (legitimately overridable per meter) and
the rest are resting *values*, so one shared `GainMath.min-db` has to be
threaded through both kinds across three files.

Two smaller instances worth folding into that pass: the Rust repaint throttle
passes a segment count of 14 for the mixer strip and 12 for the device rails,
mirroring `MixerMetrics.meter-segments: 14` and `segments: 12` in the markup.
They agree today; if the markup's count is raised, the throttle would suppress
repaints that change a visible segment -- which is exactly the "peak marker one
segment behind where the audio put it" failure its own comment names. Found
2026-09-13.

**The modulation shelf spells twenty-one ranges by hand and nothing checks
any of them.** `modulation-shelf.slint:1208-1658` declares
`minimum`/`maximum`/`default-value`/`free-*` for the LFO, Envelope, Step,
Random and Math modules. All twenty-one were compared against `LFO_`,
`ENVELOPE_`, `STEP_`, `RANDOM_` and `MATH_DESCRIPTORS` on 2026-09-13 and every
one agrees -- including the non-obvious `QUANT 0..16` (a 17-position selector)
and `LENGTH 1..16`. So this is drift risk rather than present drift, the same
shape as the eight device faces above. It is *not* reachable by extending
`slint_face_agreement.rs`'s list: that test works from a list of device faces
and the shelf is not a device face, so covering it is a new check rather than
a longer list.

One curiosity for whoever writes it. The shelf draws the LFO and Random RATE
knobs with `ValueScale.logarithmic` while `LFO_PARAM_RATE_HZ` and
`RANDOM_PARAM_RATE_HZ` declare `ParamCurve::Linear`. Nothing reads those
curves on the modulator path today -- the shelf sends natural values straight
to `ModulatorParams::set`, and modulator parameters are neither automation nor
modulation destinations -- so it is inert, and a naive agreement test would
trip over it on the first run. The correct resolution is to fix the Rust
curve, not the markup.

**`default_band_position()` returns the middle position for two of the four
strip EQ bands and one step low for the other two, and no test reads it.**
`mooloop-core/src/strip.rs:147` returns a bare `2`. `STRIP_BAND_POSITIONS` is
`[5, 7, 7, 5]`, so the middle of a seven-position mid band is 3 --
`DEFAULT_POSITIONS` says `[2, 3, 3, 2]` and the four descriptors read it. Four
things state what the middle is; the one serde reaches is wrong for the two
mids, and its own doc comment and `PROJECT_FORMAT.md` both claim the band
"loads centred". `grep -rn default_band_position crates/` returns two hits:
the attribute and the definition. This is `AGENTS.md`'s question -- *does
anything read the copy the test checks?* -- answering no, and it is the same
shape as item 6 in that list.

It bites exactly one class of file: a song saved on 2026-09-11, the day
`StripBand` carried `frequency_hz` instead of `position`, which is the only
reason the default exists. `kind`, `gain_db` and `q` have no default, so such a
band decodes with those and takes `2` for its position -- under `MOO_EQ` band 1
opens at 2 kHz where the file meant 3 kHz, while the face and the descriptor's
double-click-to-default both say 3.

Why it is not a one-character fix: serde's `default = "fn"` gets no array
index, so no single `u8` is right and changing `2` to `3` just moves the error
to the outer bands. Three options. Decode `bands` through a wire type with
`position: Option<u8>` filled from `DEFAULT_POSITIONS[i]` (~25 lines, contained
to `strip.rs`, makes the code match both claims) -- this is the fix if that
day's files are still meant to open. Or accept `2` and correct the function's
comment and `PROJECT_FORMAT.md`, which writes drift down as intent. Or drop
the `#[serde(default)]` and the doc paragraph together, so a band without a
position fails to load like its three siblings -- cleanest format, but it is a
decision about whether one day's files still have to open. Found 2026-09-12;
the load-time range check added the same day would have caught this class of
thing, and now does for everything else on the strip.

**Eight device faces spell a number the descriptor table already states, and
`slint_face_agreement.rs` reads none of them.** `scripts/dupe-audit
unchecked-face` lists them; it was written for this and the count was
twenty-three the day it was added, 2026-09-12. The faces are
`modulation-device` (7), `device-oscillator` (4), `eq-device` (3),
`filter-device` (3), `buffer-device` (2), `bus-device` (2), `aux-in-device` (1)
and `container-device` (1).

Two things this is *not*, both worth knowing before spending an afternoon on
it. DS-01 is absent and correctly so: its paged face reads the table at run
time (`default-value: root.defaults[root.param]`), which is a copy of nothing
and is the shape the rest could move to. And the modulation face was checked by
hand when the list was made -- all seven of its defaults agree with
`MODULATION_DESCRIPTORS`, including the two that are not obvious
(`Feedback` 0.5 for a bipolar -0.92..0.92, `Stages` 0.5 for a stepped 4..12).
So this is drift risk, not present drift.

**Ten of the twenty-three cannot be added to the test as it stands.** The
agreement test finds a knob by looking for one line carrying both the property
binding and `default-value:`, and says why in a comment: a graphical editor
binds the same property on a line of its own, so matching the binding alone
finds the wrong line. Faces written with the binding and the default on
separate lines are therefore structurally unreachable to it --
`filter-device`, `buffer-device`, `bus-device`, `aux-in-device` and
`container-device` are all that shape. Widening the parser is the first step,
not the face lists.

**And the faces that are covered are covered for their ranges, not their
resting positions.** The two idioms differ: a covered face declares
`minimum`/`maximum` in natural units, where `modulation-device` declares no
range at all and a *normalized* `default-value` -- the position the knob rests
at and what a double-click returns to. Those have to equal
`descriptor.to_normalized(descriptor.default)`, which is a different assertion
from the one the test makes. Covering both idioms means the test grows a second
comparison, not just a longer list.

## Housekeeping

**The piano roll's grid geometry is a constant in two test files and nothing
holds them together.** `piano_drag.rs:32` and `piano_tools.rs:16` each declare
`GRID_ORIGIN_X` / `GRID_TOP_Y` / `ROW_HEIGHT` / `STEP_WIDTH` / `HIGH_NOTE`,
measured off a software render of the 960x760 window, and `piano_tools.rs`
says "matching `piano_drag.rs`" in a comment that nothing enforces. Both moved
on 2026-09-08 when the dock's two toolbars merged; fixing the first and
running the suite reported the second as nineteen fresh failures, which is how
the copy was found. The cost is one wasted five-minute remote run per toolbar
change, so it is small — but it is exactly the shape `slint_face_agreement.rs`
exists to prevent for faces, and a shared `tests/common/` module or one
element-derived origin would end it. `rack_tools.rs:28` has a third
`GRID_ORIGIN_X` for the step grid; that one is genuinely a different grid.

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

**The README hero screenshot predates effects.** `mooloop-screenshot.png`,
captioned "channel rack and Mono Synth" — accurate, but no longer showing the
most interesting part of the app. A fresh one can be rendered headlessly.

**`CURRENT.md` has two bullets spliced into one line.** At line 554 the
limiter-lookahead sentence runs straight into "Each kind publishes a static
`ParamDescriptor` table", which belongs to a separate bullet that lost its
list marker in the 2026-09-05 edit.

**Six dB readouts still round for themselves.** `GainMath.format-db` now
covers every readout that is a *gain*, but six sites spell their own number
because they are not gains and the shared formatter's signed, `-inf`-floored
output would misreport them: a `+` on a knee width or a gate range is wrong,
and `±0.0 dB` on a limiter ceiling parked at full scale reads oddly.
`compressor-device.slint:70,142`, `gate-device.slint:68,140`,
`limiter-device.slint:62`, `device-displays.slint:545`. Three of them use
`round(x * 10) / 10`, which drops the tenth on a whole value — the same
width-jitter this pass took out of `format-db` itself. What is missing is an
unsigned one-decimal formatter to sit beside `format-db`; that is a decision
about the dB vocabulary rather than a typo, which is why it was left.

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

**Three unmerged spikes.** `spike/egui-view-layer` (3 commits),
`spike/slint-split-build` (5) and `spike/pattern-bank-cost` (1) are answers
rather than candidates — none is waiting to land. Adam's call whether any
goes anywhere.

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
