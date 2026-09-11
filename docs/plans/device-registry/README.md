# A device as one declaration

A survey, written 2026-09-11, of what it would take to register a device in
one place instead of nine. **Nothing here is built and no steps are written**,
because what to do about it is Adam's call and two of the three pieces are
worth having separately. It exists so the next person to add a device does not
re-derive the shape of the problem while in the middle of adding one.

Adam, 2026-09-10, after `EffectKind::Preamp` broke two tests that had nothing
to do with preamps:

> we should basically be loading these like plugins we get to have native
> conversations with

## What a kind costs today

Adding `EffectKind::Preamp` touched fourteen files: the nine below, the new
DSP node, a re-export in `mooloop-core/src/lib.rs`, and three tests. Ordered
by what the arm actually is:

| Where | Arms | What they say |
| --- | --- | --- |
| `mooloop-core/src/effect.rs` | ~14 | the variant, `ALL`, `label`, `latency_frames`, `descriptors`, `default_params`, the descriptor table, the params struct and its `Default`, the `EffectParams` variant, `kind()`, the typed accessor, `get`/`set` by id, the constructor |
| `mooloop-core/src/effect_factory.rs` | 2 | build from params, and the factory preset bank |
| `mooloop-dsp/src/effects/mod.rs` | 2 | the `pub use`, and params to node |
| `mooloop-engine/src/block_cost.rs` | 1 | the kind list the cost table walks |
| `mooloop-ui/src/lib.rs` | 2 | `effect_kind_index`, `effect_kind_units` |
| `mooloop-ui/src/settings.rs` | 1 | `effect_kind_slug`, the on-disk preset directory |
| `ui/device-rack.slint` | 1 | the insert-menu row |
| `ui/main.slint` | 2 | the face import, and a ~27-line face arm |
| `ui/<kind>-device.slint` | — | the face itself, which is the actual work |

Nine of those arms are one line each and say one fact: a name, a number, a
slug, a width. They are the registry, spread out. The fourteen in `effect.rs` are mostly
*typed* rather than tabular -- each kind has its own params struct, so they
cannot collapse into a row of a table without erasing the types that make the
DSP readable.

The three tests in that count are why this survey exists. Two of them broke on
the day and neither was about preamps: `source_snapshot.rs`, whose fixtures
were built from bare kind integers and silently started building preamps when
Chain moved from 12 to 13, and `the_insert_menu_offers_every_kind`, which
located the menu's last row by
`EffectKind::ALL.len() - 1` and so quietly required the menu to list kinds in
numbering order. Both are fixed (2026-09-11) and neither needed a registry:
the tests derive the numbers now, and
`the_menu_and_the_faces_cover_every_kind` checks the two markup lists against
`effect_kind_index` directly.

## What a registry could actually fold

**A table in `mooloop-ui`, keyed by kind, is straightforward and small.**
`effect_kind_index`, `effect_kind_units` and `effect_kind_slug` are three
matches over the same enum in two files that could be one `&[DeviceUi]` row
per kind: index, units, slug. It would make the numbering visibly dense (a
gap would be a hole in a table rather than an absence in a match) and it would
put the three facts a new kind needs in the UI where they can be added
together. It is also nearly cosmetic: the match arms are exhaustive, so
forgetting one is a compile error today.

**One fact has no Rust home at all, and should get one first.** A *generator*
face's width is only in markup: `main.slint` says `root.source-kind == 7 ? 2 :
(root.source-kind == 6 || root.source-kind == 5 ? 4 : 3)`, where an effect's
width is `effect_kind_units` in Rust and comes down the model. So nothing
outside the markup can say how wide a synth is, and `rack_keyboard.rs` opens
with `SOURCE_WIDTH = 220.0 * 3.0 + ...` as a constant it cannot derive. A
`device_kind_units` beside `device_kind_to_int` would close that, and is the
smallest useful piece of any of this.

**A table spanning crates is not straightforward, and layering is why.**
`mooloop-dsp` depends on `mooloop-core`, so a single row that names both the
params type and the node constructor cannot live in `core` -- the constructor
is not nameable there. Either the table lives in `dsp` and `core` loses the
ability to answer `default_params`, or it splits in two and the "one place"
claim goes with it. The nearest honest version is three small tables, one per
layer, each checked against `EffectKind::ALL` by a test.

**The typed params are not a defect and should not be erased.** `EffectParams`
is an enum of distinct structs, and `params.eq()` returning `Option<&EqParams>`
is what lets the EQ's spectrum code be written at all. A registry of
`fn(&EffectParams, u32) -> Option<f32>` function pointers would make dispatch
uniform and make every reader of a device's parameters worse off.

## The Slint side is the real constraint

**Slint has no dynamic component instantiation.** There is no
`create(component_name)`, and a component is not a value. The only way to draw
one of fourteen faces is fourteen conditional elements:

```slint
if slot.kind == 12 : PreampDeviceFace { ... }
```

So `main.slint` will hold one arm per kind under any design short of
generating the markup from Rust in `build.rs` -- which is possible, since
`build.rs` already runs `slint-build` over `ui/main.slint` and could compile
an emitted file instead, and which would cost the thing that makes face work
bearable: `scripts/slint-sketch` type-checks and
screenshots a real `.slint` file in 0.05 s, and a generated one is no longer a
file anybody edits. That trade is not obviously worth it for fourteen arms.

**But the arms are 55% boilerplate, and that part is reachable.** The fourteen
arms are 444 lines of `main.slint`, and 245 of them are the same eleven
bindings repeated: the four modulation models, the three modulation callbacks,
the geometry, `slot-index`, `slot-count`, `bypassed`, `preset-name`,
`reorder-requested`, `select-requested`. The genuinely per-device part is the
positional mapping -- `drive: slot.p0`, `voicing: round(slot.p1 * 3)`, and a
`*-changed` callback per parameter -- which is the descriptor table's own
order written out a second time in markup.

Two moves, neither of which needs dynamic instantiation:

1. **A face host.** Put the eleven shared bindings in one component that takes
   the row and the index, and let the arm bind only what is the device's own.
   An arm becomes roughly eight lines from twenty-seven.
2. **A uniform face interface.** If a face took `in property <[float]>
   params;` and emitted `param-changed(int, float)`, the per-device mapping
   would move inside the face, where the knob and its readout already are, and
   an arm would become three lines. The named properties exist for
   readability, and `slint_face_agreement.rs` parses the *knob* declarations
   rather than these bindings, so the test that holds a face to its table
   would survive the change.

After both, adding a kind means: a face file, a three-line arm, a menu row, a
number, and the model work in `core`. The arm and the import are the
irreducible residue of "no dynamic instantiation", and they are two lines.

## The other thing a plugin would have that a kind does not

`EffectSlotRow` is a flat struct with `p0..p9` and a growing tail of
per-device extras -- `eq_band_data`, `eq_spectrum_data`, `gain_reduction_db`,
`buffer_collisions`, `detector_db`. Every device pays the struct size of every
other device's display data, and a device that needs a new *shape* of data
widens a struct `main.slint` and `lib.rs` both know. Ten positional parameters
is already a declared ceiling, of which eight are addressable
(`EFFECT_ROW_DESCRIPTOR_PARAMS`; the last two are reserved for the tempo-sync
flag and division that the delay and the modulation effect both need), and the
modulation effect's eight descriptors fill every one of the eight. The next
device with nine parameters widens the struct.
Nothing here is urgent, and it is the part of "like a plugin" that a
registry does not touch: a real plugin host hands the device an opaque buffer
and lets it draw itself, which is the same wall as dynamic instantiation seen
from the data side.

## What is worth doing, if anything

In order of value per hour, in this surveyor's opinion:

1. The face host (1 above). It is a pure markup refactor, it needs no Rust
   change, it removes 245 duplicated lines, and it makes every future arm
   shorter. `scripts/slint-sketch` can check it without a build.
2. The `mooloop-ui` table for index/units/slug. Half an hour, small win.
3. The uniform face interface (2 above). Larger, touches every face file, and
   wants doing at the same time as anything else that edits all fourteen.

Generating `main.slint` from a Rust table: recorded as possible, and not
recommended, for the reason above.
