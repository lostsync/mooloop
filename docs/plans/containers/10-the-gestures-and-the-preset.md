# 10 — The gestures, and a layer is a preset

The step that makes a layer reachable without a debug control, and the one
that closes the plan.

Most of this is already built for chains and needs to learn a second kind
rather than a second implementation — which is `07`'s predicate doing its job.
If this step is large, `07` was done wrong.

## The gestures

Rewritten 2026-09-23 with 09, whose list face (Adam's answer: copy Bitwig's FX
Layer) takes over two of the gestures this step used to have to invent.

`04` found that two gestures were enough for containers and a third already
worked. 09 makes **a Chain the unit of a branch**, because a branch's Level,
Mute and Solo are a container's parameters, so every gesture that makes a
branch makes a Chain:

| Gesture | Chain today | Layer |
| --- | --- | --- |
| **Wrap** | left-rail button puts a box around the clicked row's *run* | the same button offers a choice of kind. Wrapping in a layer makes a layer of **one branch**, a Chain holding the run, so the branch has its controls from the start |
| **Unwrap** | right-rail button, leaves the contents where they are | reads the predicate; no change |
| **Add a branch** | `insert_effect_into_container` is the container rail's `+` | **the list's `+`**: an empty Chain appended as the layer's *last* branch, and selected, so the rack shows it ready to fill from its own `+`. **Built in 09** (`Session::add_layer_branch`) |
| **Remove a branch** | the rail's remove, a box going with its contents | **a row's context menu** in the list: *Remove* takes the branch head and its run (`remove_effect` already does exactly that) |
| **Drop into a run** | already worked -- `move_effect` decides what the landing index falls inside | a drop between two branches makes a leaf branch, which 09 records as legal and control-less. Wrapping it in a Chain as it lands is the fix, if it turns out to matter in use |

**Remove takes the whole branch**: the branch head and its run, as one
removal and one undo step, because `remove_effect` already takes a box with
its contents. Nothing is left orphaned or re-parented into a sibling branch.
**The last branch can be removed too**, leaving an empty layer. An empty layer
is already legal (it caps itself and passes its input), its list says "no
branches", and its `+` is right there. Refusing would make the list's Remove
behave differently for one row out of many.

So the new interface objects are **choosing which box to wrap in** and **the
list row's context menu**. The wrap choice is a menu on the existing button
rather than a second rail button, because a menu extends to a third kind and a
button row does not. Both are menus, and a menu comes with a known trap.

**`scripts/dupe-audit popup-close-order`.** A `PopupWindow` tears down the
repeater item whose handler is still running, so a callback written after
`close()` never lands: the menu opens, draws, and does nothing. That has been
found four separate times — `BusPicker`, the rack's preset menu, `PickerChip`'s
`MenuField`, and the add-channel menu that shipped as MOO-53 — each diagnosed
from scratch. Call the callback, then close. The check exists precisely so
this is the fifth time it is *not* rediscovered.

## Every edit here records one

`archive/gesture-undo/` landed 2026-09-21 and its rule is not optional:
**undo installs a whole-project snapshot, so an edit that never reaches the
history is destroyed by the next undo, silently and with no redo path.**
Wrapping in a layer, adding a branch and removing one are all document edits.

`scripts/dupe-audit unrecorded-edit` is the check, and its own history is the
warning: it reported a clean tree **twice** for resolution bugs before it
worked, and then passed validation while reading a third of the program,
because more callbacks in this file are wired by a `wire_*!` macro than by a
`window.on_`. A clean run from it is evidence, not proof — trace the press.

## A layer is a preset

`05-a-container-is-a-preset.md` made a container's preset carry its run:
`take_preset_save` strips identity on the way out, `load_effect_preset` keeps
the live row's id on the way in, and `EffectRunLoaded`
(`session/src/effects.rs:88`) tells the engine the removal and the insertion
that mirror it.

A layer's preset is a run too, so this is a predicate change and a bank. What
it needs beyond that is **content**: a layer preset that is two branches of
nothing proves the mechanism and teaches nobody what the device is for.
`FOCUS.md`'s rule — a device ships presets to prove its architecture reaches
its range from the controls — means three or four, not a curated bank:

- parallel drum compression (clean ‖ compressed, mix to taste)
- clean ‖ distorted, the reason `06` was filed
- a three-way split with a filter on each, which is the layer as a crossover

## Acceptance

- A musician can build a two-branch layer from the rack with no keyboard
  shortcut and no debug control, hear it, save it, reopen it, and render it
  offline to the same result.
- Undo puts every one of those gestures back, and redo puts it forward.
- `dupe-audit unrecorded-edit` and `dupe-audit popup-close-order` are clean,
  and clean for the right reason.
- The layer's preset round-trips through the browser like a chain's.

## Then the plan closes

`06` says it closes when `00-status.md` records that chain containers have
been lived in and Adam has ruled on layers either way. He ruled. When `10`
lands, move `docs/plans/containers/` to `archive/` — an active directory
should always hold live work.

**Selectors stay unbuilt.** `06` prices a selector as nearly free once layers
exist, and that is still true and still not a reason to build one. It goes on
the record here so the next reader finds the price rather than the omission.
