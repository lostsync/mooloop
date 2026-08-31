# Time-stretch spike (issue #32)

Throwaway comparison harness. Nothing in here is meant to ship, and no
production crate depends on it.

```sh
scripts/antibox cargo run --release -p mooloop-spike-time-stretch
```

Prints every measurement section as CSV to stdout and writes 64 comparison
renders to `$STRETCH_SPIKE_OUT` (default `/tmp/stretch-spike`, about 81 MB).
Pull a render back for listening with:

```sh
scripts/antibox --pull /tmp/stretch-spike/drum_break__wsola_nosnap__r1.25.wav \
  cargo run --release -p mooloop-spike-time-stretch
```

Fixtures are generated from a seeded PRNG rather than committed as audio, so
runs are byte-reproducible and the repository stays free of binaries.

## What is in here

| File | Contents |
| --- | --- |
| `src/fixtures.rs` | Synthetic break, one-shot, click train, bass, tone, mixed loop, decorrelated stereo |
| `src/wsola.rs` | Candidate A: WSOLA, five presets, optional onset snapping |
| `src/pvoc.rs` | Candidate B: STFT phase vocoder with identity phase locking, five presets |
| `src/metrics.rs` | Onset detection and scoring, attack/crest, pitch, LTAS, stereo, glitch probes |
| `src/main.rs` | The run matrix, the allocation gate, and the CPU/polyphony benchmarks |

`RESULTS.md` holds the conclusions and the numbers behind them.

Both candidates implement the same `Stretcher` trait, which is deliberately
shaped like the sampler's real situation: an immutable resident sample, a
region, an optional loop, and a pull-style `render` that fills whatever block
the executor asked for. That shape is itself part of the finding — see
`RESULTS.md` on latency.
