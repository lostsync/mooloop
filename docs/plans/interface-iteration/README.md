# Interface iteration

Adam, 2026-09-07, opening the week:

> we were supposed to go straight into making the 1.0 mockup a reality but im
> not sure i want to do that anymore. i think we should just keep iterating and
> let it take shape.
>
> that said, maybe today would be a good day for some ui and workflow work. we
> have a lot of new internals that we could expose or use.

That is a decision about *method*, and this directory is what it replaces the
old method with. `FOCUS.md` step 3 was "one push, mockup-driven," with
`reference/img/mooloop-1.0-mockup.png` as the argument for a layout that would
arrive all at once. It is now four independent steps that each end in
something usable, in an order that can be rearranged, with the mockup demoted
from target to reference.

**The mockup is not cancelled and it is not wrong.** A left channel sidebar, a
right-hand modulation panel and a browser that earns its panel are all still
where this is heading. What changed is that we stop treating the whole
rearrangement as one indivisible commitment, because nothing here needs the
others to be useful and the layout is likelier to be right if it is arrived at
than if it is declared.

## The rule

`FOCUS.md`'s rule for interface work still governs, and it is the reason this
plan is shaped the way it is:

> An interface change is judged by whether something that already exists
> becomes easier to reach, not by how much new surface it adds.

Every step below is downstream of that. Each one takes a mechanism that is
already built, already tested, and currently reachable only from a menu, a
rail button, or not at all, and gives it a place a musician would look for it.
**None of them adds a capability to the engine.** If a step starts wanting
one, it has left this plan.

## The steps

Worked in the order Adam set — presets first — but they do not depend on each
other and can be reordered.

| Step | What it exposes | The internal it spends |
| --- | --- | --- |
| `01-preset-browsing.md` | Presets in the browser panel, beside samples | `list_presets`, and the `category`/`tags` it already returns and nothing displays |
| `02-device-clipboard.md` | Copy, cut, paste and duplicate a device or a whole container run | `DeviceId` (2026-09-06) and `EffectRun` / `load_effect_run` (2026-09-07) |
| `03-channel-identity.md` | A channel's name and colour, and a bus's name | `MixerBus.name`, which saves and loads and has no setter |
| `04-the-keyboard-pass.md` | The action registry over the surfaces it never reached | `actions.rs`, now that focus is fixed |

## What was already done, before the plan was written

The focus fix landed first, on `fix/toolbutton-space`, because
`docs/CURRENT.md` and `FOCUS.md` both had its cause recorded wrongly and
because step 04 is unreachable without it. In short: Slint delivers a key to
the focused item and then walks *parent* items toward the window, and
`main.slint`'s root `FocusScope` was a **sibling** of the layout holding the
UI rather than its ancestor, so it only ever heard a key while it personally
held focus — which is why clicking a neutral background was the thing that
made shortcuts start working. Separately `ToolButton` accepted Space, and
every toggle, segmented control, pane tab and mute button is built from it.

The lesson worth carrying into the rest of this plan: **the recorded diagnosis
was more confident than it was correct, and it had been re-stated in three
documents.** Two of those restatements named `main.slint:1253`, a line number
that had drifted. Check the code before spending a step on what a document
says is wrong with it.

## Source questions this plan does not answer

- **Whether the modulation rack moves, and whether its modulator becomes a
  tracker.** `FOCUS.md` step 3 held both, `IDEAS.md` has held the tracker
  idea longer, and `MODULATOR_SYSTEM_SPEC.md` holds the contracts a move must
  not break. It is deliberately not a step here: it is the one piece of the
  mockup that is a genuine design question rather than a relocation, and
  Adam's own note says the first thing to settle is whether the tracker and
  the modulation panel are one design or two. Settle that before planning it.
- **Whether a named colour scheme is three seeds or a sixteen-colour ramp.**
  `ENHANCEMENTS.md` states the question and why base16 and pywal answer it.
  Step 03 adds a *channel* colour, which is content in the project file and
  independent of the palette question; it must not be allowed to decide it.
- **Layers.** `docs/plans/containers/06-layers-and-selectors.md` prices them
  and defers them to Adam after living with chain containers. Step 02 makes
  containers easier to move around, which is more time living with them, not
  a reason to revisit.
