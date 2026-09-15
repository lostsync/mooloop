# 02 — Where they live, and how the session sees them

Read `01-the-device-preset-format.md` first. This step is
`mooloop-ui/src/settings.rs` and `mooloop-session`, still no window, still
sub-second to test.

## On disk

```
presets/generators/<device_kind>/     already exists
presets/channels/                     already exists
presets/effects/<effect_kind>/        this step
```

Add `effect_presets_dir(kind: EffectKind)` beside `generator_presets_dir` and
`channel_presets_dir` (`crates/mooloop-ui/src/settings.rs:528`), with a
`kind_slug` for `EffectKind` alongside the existing one for `DeviceKind`.

**Read the comment on the existing `kind_slug` before writing the new one.**
It records that `DeviceKind::MlM1` is deliberately spelled `ml1` on disk
because renaming it would orphan every preset already saved. Nothing is saved
under the new effect directories yet, so pick the names that will still be
right in a year — plain snake_case of the kind — and add the same warning that
they are frozen once anything ships against them.

One subdirectory per effect kind, rather than one flat `presets/effects/`,
because a filter preset loaded into a reverb row is nonsense and the directory
layout is the cheapest place to make that impossible.

## In the session

`Session` already carries `generator_presets` and `channel_presets` as
`Vec<PresetSummary>` (`crates/mooloop-session/src/session.rs:118`), and
`PresetSaveTarget` (`:36`) is the in-flight save-dialog marker.

- Add `effect_presets: Vec<PresetSummary>`.
- Add `PresetSaveTarget::Effect { slot: u8 }`. The slot is which rack row the
  save was started from, and it must be carried rather than re-derived: the
  rack can be reordered while a save dialog is open, and a save that lands on
  whichever row is now in position 3 is a bug that will be very hard to find
  later.
- Refresh `effect_presets` on the same path that refreshes the other two.

## The load path

Loading an effect preset replaces the `EffectSlotState` in one rack row. It
must be one undoable edit, like every other rack mutation — go through the
same `ProjectEdit`/snapshot machinery the rack already uses rather than
mutating the setup directly.

**Refuse a mismatched kind.** Loading a `Delay` preset into a `Filter` row is
not a coercion to attempt; it is an error to report. The directory layout
makes it unlikely and the check makes it impossible.

## Tests

`cargo test -p mooloop-session`, which is 87 tests in under a second and is
the reason this layer exists.

- Saving from rack row 2 and loading into row 0 produces an identical slot.
- A kind mismatch is refused and the rack is unchanged.
- Loading a preset is one undo step, and undo restores the previous slot
  exactly — including `bypassed` and the three trims.
- `PresetSaveTarget::Effect` survives a rack reorder in flight: start a save
  for row 1, reorder, and confirm the save still names the row it started on.
- An empty `presets/effects/` lists nothing rather than erroring, matching
  `list_presets`'s existing contract.

## Do not

- **Do not seed a factory bank of effect presets.** `00-status.md`'s problem 4
  is that first-run seeding is a mechanism that cannot update itself, and it
  should not be extended before it is fixed. Ship the format empty.
- **Do not migrate the ML-M1 bank.** `00-status.md` puts it out of scope and
  says leaving it alone is a valid answer.

## Done when

`cargo test -p mooloop-session` and `cargo test -p mooloop-project` are green,
an effect preset can be saved and loaded through the session with no window
involved, and undo puts the old slot back.
