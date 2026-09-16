# 02 — The session follows a track edit

Step 01 renumbers everything saved with the song. This step renumbers the
state that is not saved: what the session holds by track position. It also
makes the device rack stay on the track that moved.

## Carry the edit

`ProjectEdit` (`crates/mooloop-session/src/project.rs`) has
`channel_edit: Option<ChannelEdit>`. A track edit needs the same field.
Rather than add a second `Option` beside it, replace the field with one enum:

```rust
pub enum ListEdit {
    Channel(ChannelEdit),
    Track(TrackEdit),
}
```

`edit: Option<ListEdit>` cannot say "both at once". That is correct, because
no single queued edit moves a channel and a track. `queue_structural_edit`
takes the enum. `queue_track_remove` starts passing
`Some(ListEdit::Track(TrackEdit::Removed(..)))` and goes through
`queue_structural_edit` instead of `queue_project_edit`, which fixes the
removal gap `00-status.md` records.

The pump arm in `lib.rs` (the `if let Some(edit) = edit.channel_edit` block)
dispatches on the variant.

## `Session::rescope_after_track(TrackEdit)`

`Session::rescope_after`'s twin, in `session.rs` beside it. It covers the
four referrers that can hold an `EffectTarget::Bus`:

- `selected_device`
- `automation_target`, through `TrackEdit::address`
- `pending_preset_save`
- `effect_preset_names`

`selected_source` and `source_preset_names` are channel-only and are not
included. `sample_request` is too.

The inner `moved` helper in `rescope_after` maps a `Bus` target to itself,
and the new one maps a `Channel` target to itself. **Write it once:** give
both enums a `fn target(self, EffectTarget) -> Option<EffectTarget>`, or add a
small private trait, so each walk is the same code over a different edit.
Two hand-written copies of a four-field walk are how one of them later
misses a fifth field.

## The rack stays on the moved track

`Session::replace_project` ends with
`effect_target = EffectTarget::Channel(project.selected_channel)`. Every
install therefore sends the rack back to a channel, including the one a
track move triggers. That is tolerable for an add or a remove. For a move it
is wrong: somebody who drags the track they are editing expects to still be
editing it.

In `rescope_after_track`, if the target *before* the install was
`Bus(from)`, set `effect_target` to `Bus(edit.track(from))` after it. The
pre-install target is gone by the time the pump arm runs, so capture it
there before `install_project_in_ui` and pass it in. Do not change
`replace_project`'s reset. An undo, a load and a paste should still land on
a channel.

Selecting the moved track on drop, as the channel reorder does with
`project.selected_channel = to`, is step 03's call. It needs the same
mechanism, so decide it here: **a drop selects the track it dropped**,
matching the rack. `queue_track_move` then passes `Some(to)` as the target
whatever was selected before.

## Tests

In `crates/mooloop-session/tests/structure.rs`, next to the channel ones:

- Label a device on track 3, select it, open a lane on it, then apply
  `TrackEdit::Moved { from: 3, to: 1 }`. All four referrers now name track 1.
- The same after `Removed(1)`. Everything that named track 3 names track 2,
  and a label that was on track 1 is gone.
- A channel edit leaves every `Bus` referrer alone, and a track edit leaves
  every `Channel` referrer alone. This checks the shared helper in both
  directions.

## Verification

`cargo test -p mooloop-session`. `cargo check -p mooloop-ui` for the pump
arm and the `queue_*` signatures. That is a check, not a build, and the
check cost is the whole cost here.
