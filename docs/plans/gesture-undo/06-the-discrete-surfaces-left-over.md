# 06 · The discrete surfaces left over

Everything on `LOOSE_ENDS.md`'s unrecorded list that never needed a bracket at
all. These are single, discrete edits: they were left out because the
surrounding work was about continuous controls, and nobody came back.

Take the list from step 01's check, not from here — but this is what is
expected to be on it:

- **Step-grid edits**: click, right-click, velocity, paint. Paint is the one
  exception in this step: it is a drag, so it takes a gesture like a fader,
  and the grid is a good second proof of the mechanism outside a knob.
- **Pattern length**, and **add-pattern**.
- **Playlist placement** add and remove. Painting a range sends one per cell
  (`Sequencer::set_playlist_placement`), so a painted range is one gesture,
  not one entry per cell.
- **Preset loads** — channel and generator, from the browser.
- **The MIDI IN and AUDIO input picks**, and the sampler Record page's CLIP
  and LENGTH (`on_midi_input_picked`, `on_audio_input_picked`,
  `on_sampler_record_clip_changed`, `on_sampler_record_bars_changed`).
- **Swapping a channel's device kind** (`on_channel_source_changed`).

## The one that is not like the others

The device-kind swap deserves reading before it is recorded. `LOOSE_ENDS.md`
calls it the worst of the unrecorded set, for a reason that recording it does
not automatically fix: **the swap destroys the outgoing device's state
outright**, so an undo restores a snapshot in which the old device's
parameters exist only if the snapshot was taken before the swap. Recording it
here gives it that snapshot, which is exactly the fix — but check that the
restored channel comes back with its old device's state and not an empty one
of the right kind. If it comes back empty, the swap is discarding state
before the snapshot is taken, and that ordering is the actual bug.

MOO-29 gave the toolbar's Add Channel its own history entry and deliberately
left the swap alone, because a swap is a replace rather than a structure
change. This step is where that deferral is paid.

## Done when

- [ ] Every surface on step 01's list either records, or is on the exemption
      list with a reason.
- [ ] A painted step run and a painted playlist range are each one undo
      entry.
- [ ] Undoing a device-kind swap restores the outgoing device **with its
      parameters**, and there is a test that would fail if the state were
      discarded before the snapshot.
