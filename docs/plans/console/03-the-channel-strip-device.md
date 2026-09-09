# 03 — The channel strip device

A composite `EffectKind`, hostable on a channel or a bus **today**, with no
mixer dependency at all. EQ + comp in one face, a mooloop voicing plus three
aimed at SSL / API / Neve.

## Reuse rather than rebuild

This device is mostly assembly, and every part it needs already exists or is
already queued to be extracted.

- **The shared `Biquad`.** `adopt-shared-biquad-in-eq/` is a queued plan to
  stop `eq.rs` declaring its own. Do that here rather than adding a third
  copy.
- **The shared detector and gain computer** the gate, compressor and limiter
  already share.
- **`DynamicsCurveDisplay`**, which already draws a gain-computer curve.
- **`EqResponseDisplay`**, which is currently **private to `eq-device.slint`**
  and would have to be extracted. That extraction is the one piece of face
  work this step has to do before it can start.

## Voicing is a mode, not four devices

A voicing selects curve shapes and time constants -- knee, ratio law, attack
and release curves, EQ band frequencies and Q -- over one signal path. Four
separate devices would be four faces, four descriptor tables and four sets of
presets for one idea. `COMPOSABLE_DEVICE_UNITS.md` is the contract for how the
shared units are addressed.

Adam's taste brief applies here more than anywhere else in this plan: a device
face is not a page of knob rows, it is a drawing of what makes the device
different. The channel strip's difference is that the EQ and the compressor
are looking at the same signal, so the face should say so.

## Order of work

Follow `AGENTS.md`'s device-work order, which exists because the three parts
of a device differ by four orders of magnitude in cost to see:

1. DSP against unit tests (1-4 s per iteration, laptop);
2. face iterated with `scripts/slint-sketch` (0.05 s);
3. cross into `main.slint` **once**, with every property and callback batched
   into that single pass (8.7 min).

## Free while it is out

`is_at_rest` / `skip_block` are what make an idle device cost nothing, and a
composite device has to implement them over its parts rather than inheriting
them. A channel strip with a flat EQ and a compressor below threshold should
be as cheap as an empty slot.

## Verification

`slint_face_agreement.rs` holds a face to its descriptor table. Extend it for
this device rather than adding a parallel check -- that test exists precisely
so a new device cannot ship a face that disagrees with what the engine reads.
