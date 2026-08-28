# Internal chorus

The Juno-flavoured lane, and the last step needed for Poly's identity to be
complete.

## The integration hazard — read this first

**The built-in chorus must process only the Poly synth's own signal.**

`PolySynth::process` writes into the shared `StereoBus` with `+=`
(`crates/mooloop-dsp/src/polysynth.rs:358`), because a generator sums into
whatever the channel already holds. A chorus that reads that bus, processes
it, and writes it back would process every other source on the channel too —
and it would do it silently, sounding like "the chorus is broken" rather than
like a routing bug.

So: the synth renders its voices into an internal, fixed-capacity scratch
buffer owned by `PolySynth`, runs the chorus over that, and only then sums the
result into `bus`. The scratch buffer is allocated once at construction, sized
to the engine's maximum block size, and never resized in `process()`. This is
the same lifecycle rule every other step in this plan follows; it is called
out here because this is the one place where getting it wrong still produces
audio.

## Do this

### 1. Four modes

```rust
pub enum PolyChorus { Off, One, Two, Ensemble }  // default Off
```

| Mode | Intent                    | Source                                     |
|------|---------------------------|--------------------------------------------|
| OFF  | Dry poly synth            | No processing at all — not a zero-mix path |
| I    | Light classic chorus      | Factory-tuned chorus policy                |
| II   | Deeper, wider chorus      | Factory-tuned chorus policy                |
| ENS  | Wide multi-voice ensemble | The existing Ensemble mode                 |

OFF is a genuine bypass: no delay lines ticking, no scratch-buffer copy beyond
what the voice sum needs. Not a wet/dry blend sitting at zero.

### 2. Reuse the existing DSP

`ModulationEffect` (`crates/mooloop-dsp/src/effects/modulation.rs:33`) already
implements chorus and ensemble on fractional stereo delay lines with an LFO,
and `ModulationMode` (`crates/mooloop-core/src/effect.rs:1114`) already has
`Chorus` and `Ensemble` variants. **Do not reimplement the algorithm.**

Instantiate an internal `ModulationEffect` inside `PolySynth`, configured from
a small fixed table mapping the four device modes to full `ModulationParams`.
The user sees a four-position switch; the rack effect's whole parameter set
stays hidden. Rate, depth, color, and spread for modes I and II are chosen by
ear — record the values here once tuned.

If `ModulationEffect`'s constructor or `set_params` allocates, that is a
prerequisite fix: it has to be safe to own inside a generator.

### 3. Amount

**Do not add an Amount/Mix control yet.** Start with the mode switch alone and
tune the fixed depths so each mode is useful as-is. Add Amount only if
listening tests show the fixed modes genuinely need it — and if they do, that
is evidence the mode presets are wrong first. Record the outcome here.

### 4. Mode switching

Switching mode mid-note must not click. Changing delay-line parameters
underneath a sounding signal is exactly what the modulation effect's own
smoothing exists for; verify it covers a mode change and not just a parameter
change. OFF → I is the awkward one, since the delay lines start empty and
their state has to be reset without a discontinuity — a short fade-in of the
wet path is acceptable and probably necessary.

### 5. Parameter and UI

| Field    | Kind                | Default | ID |
|----------|---------------------|---------|----|
| `chorus` | `PolyChorus` enum   | `Off`   | 44 |

OFF as the migration default: an old project must not suddenly acquire a
chorus.

UI: a `SelectorBank` in the VOICE page's Character section, beside Drift and
Spread — `OFF` / `I` / `II` / `ENS`.

## Done when

- I, II, and Ensemble are audibly distinct from each other and from OFF.
- **The chorus processes only Poly's output.** Test: render a Poly synth into
  a bus that already contains a known signal, with chorus on, and assert the
  pre-existing signal comes out unmodified.
- OFF is bit-identical to the pre-chorus build.
- No allocation in `process()` in any mode. The scratch buffer is allocated at
  construction.
- The scratch buffer handles the maximum block size, and a block larger than
  expected clamps rather than panicking or reallocating.
- Switching modes on a sounding chord produces no click.
- Transport stop and choke with chorus active do not leave a ringing tail
  after the voices are gone — decide whether the tail should flush on choke
  and record the choice.
