# 05 — Move engine command emission behind a session interface

Read `00-status.md` first. Step 04 must be substantially in.

## What is wrong today

The route from a UI gesture to the audio engine is currently spread across the
UI crate:

- Six newtype senders (`lib.rs:147-206`) — `EngineCommandSender`,
  `StructuralCommandSender`, `ProjectEditSender`, `AudioActionSender`,
  `TelemetryActionSender`, `PreviewSender` — all wrapping one
  `mpsc::Sender<PendingEngineMessage>` so ordering is preserved.
- A pump, driven by a Slint `Timer`, which exclusively owns the `EngineHandle`,
  drains that channel, applies engine events, and refreshes meters, playheads
  and modulator outputs.
- Side channels for things published out of band: `sample_reset_rx`,
  `channel_audio_rx`, the document result channel, browser load results.

The senders and the channel are toolkit-free. The pump is not — it is a Slint
timer and it writes Slint models.

## What to do

Move the senders, `PendingEngineMessage`, `TelemetryAction`, `AudioAction`,
`ChannelAudio` and the side channels into `mooloop-session`, and give the
session a method that drains and applies them, owning the `EngineHandle`:

```
impl Session {
    /// Drain queued commands to the engine and engine events back.
    /// Returns what the view needs to redraw. Called on a timer by whatever
    /// view layer is current.
    pub fn tick(&mut self) -> TickReport { ... }
}
```

`TickReport` carries plain data — meter values, playhead positions, modulator
outputs, document results, whatever the current `sync_*` calls need — and the
view decides what to do with it.

**The timer stays in the view.** Slint's `Timer` today, egui's frame loop later.
The session should be driven, not self-driving; that is what keeps it testable
and what makes the eventual view swap a matter of calling `tick` from a
different place.

## Why this is its own step rather than part of 04

Because the pump is the one place where the direction of travel reverses.
Everything in step 04 is UI gesture flowing down to the engine; the pump is
engine state flowing back up. It has different failure modes — a missed refresh
rather than a wrong edit — and it should be verified on its own.

It also has a constraint worth stating explicitly: the engine's side of this is
already correct and must not be disturbed. Meters and playheads come from
wait-free snapshots (`crates/mooloop-engine/src/meters.rs`) polled on a timer,
which is what `docs/reference/ANATOMY_OF_A_DAW.md` prescribes and what keeps a
missed frame invisible instead of audible. This step reorganises who calls the
poll, and nothing about the poll.

## Definition of done

- `Session` owns the `EngineHandle` and exposes `tick`.
- `mooloop-ui`'s timer callback is a few lines: call `tick`, apply the report.
- Meter, playhead and modulator-output refresh behave identically, at the same
  rate.

## Verification

Full build, then watch meters and the sampler playhead under playback. The
failure mode here is a refresh that stops happening rather than one that
happens wrongly, so it needs eyes on a running application, not a snapshot.

`docs/plans/archive/reduce-ui-pump-overhead/` is worth reading before starting —
the pump's cost has been tuned once already, and this step must not undo that.
