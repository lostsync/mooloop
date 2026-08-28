# Oscillator sync

Optional for v2. Poly's identity is complete after 05; this is character on
top of it. Do it if there is appetite, skip it without guilt, but do not do it
before 05.

## Why it belongs here

Hard sync is the first "advanced" oscillator feature that is specifically
Poly's. It pushes the instrument into Prophet-like hard, bright, clangy
territory — sync stabs and sweeping leads — and it does so without making Mono
more complicated. Mono's step 05 (`mono-synth-v2/05-the-acid-filter.md`) gets
its aggression from the filter; Poly gets a different aggression from the
oscillators.

## Do this

### 1. One fixed routing

**OSC 2 hard-syncs to OSC 1.** One pair, one direction, one toggle. No matrix,
no source/destination selectors. When OSC 1's phase wraps, OSC 2's phase
resets.

The musical move this enables is: sync on, OSC 2's Semi/Fine swept by hand or
by the filter envelope, producing the classic sync sweep. That means OSC 2's
pitch controls stay live and meaningful while its perceived pitch follows OSC
1 — which is the whole effect.

### 2. Implementation

`Osc` (`crates/mooloop-dsp/src/osc.rs`) advances phase internally and returns
a sample. Sync needs the master's wrap to be visible to the slave. Two
workable shapes:

- `next_sample` returns whether it wrapped this sample, and the caller resets
  the slave; or
- `Osc` exposes its phase and a `sync_to`/`hard_sync` entry point.

Prefer the first — it keeps the phase private and the sync logic in the voice,
where it is one visible line.

**Aliasing is the real work.** A naive phase reset produces a hard
discontinuity and broadband aliasing, and OSC 2 uses PolyBLEP for saw and
pulse (`crates/mooloop-dsp/src/osc.rs`) precisely to avoid that. The reset
edge needs the same treatment: a BLEP correction at the sync discontinuity,
scaled by the height of the step being introduced. Getting this wrong sounds
like the effect working plus a layer of fizz, so listen at high notes where
aliasing is worst.

Sync interacts with the per-voice phase offsets from step 02: the slave's
phase is being reset by the master anyway, so its drift offset stops mattering
while sync is on. That is correct and expected — the master's offset still
varies per voice, so voices remain non-identical.

### 3. Parameter and UI

| Field       | Kind | Default | ID |
|-------------|------|---------|----|
| `osc_sync`  | bool | false   | 45 |

UI: a `ToggleButton` on the OSC page, in or beside the OSC 2 strip, labelled
`SYNC` — it is unambiguous which oscillator is the slave if the control lives
on it. `OscillatorTrace` (`crates/mooloop-ui/ui/device-displays.slint`) draws
OSC 2's waveform; showing the synced shape is a nice-to-have, not a
requirement.

Note that `OscillatorDeviceStrip` is defined in both device faces — Mono's
step 07 extracts it to a shared component. If that has landed, the Sync
control has to be optional in the shared strip so Mono's face does not grow
it. If it has not landed, add it to Poly's copy and leave a note in Mono
step 07.

## Done when

- Sync on produces the characteristic bright, fixed-pitch-with-changing-timbre
  result: sweeping OSC 2's pitch changes harmonic content while the perceived
  fundamental follows OSC 1. Assert the fundamental is stable across an OSC 2
  pitch sweep.
- Aliasing is controlled. Assert that high-note sync output has no significant
  energy at frequencies that could only be aliases — the same kind of check
  the PolyBLEP oscillators should already justify.
- Sync off is bit-identical to the pre-step build.
- Toggling sync on a sounding voice does not click.
- Sync works across the whole voice pool with drift active, and the render
  stays deterministic.
