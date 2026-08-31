# Published signals

ML-P8 is a complete instrument and a composable device. This step publishes
the useful parts of its internal behavior with explicit signal types, timing,
and reduction rules. It does not expose every implementation field.

## Control outlets

Publish stable named outlets:

| Outlet | Type/range | Reduction |
| --- | --- | --- |
| `LFO` | bipolar control, -1..1 | exact device-global LFO value |
| `Amp Envelope` | unipolar control, 0..1 | mean across the focus unison group |
| `Filter Envelope` | unipolar control, 0..1 | focus-group mean |
| `Velocity` | unipolar control, 0..1 | focus note velocity |
| `Note` | unipolar control, MIDI 0..127 normalized | focus note number |
| `Gate` | gate | high while any scheduled ML-P8 note is held |
| `Trigger` | trigger | one declared control tick for every Note On |

The **focus group** is the group created by the most recent Note On. It remains
the focus through its release so envelope outlets have a coherent tail. When
it becomes idle, its envelope, Velocity, and Note outlets return to zero; they
do not jump backward to an older held chord note. A new Note On immediately
becomes focus. Voice stealing follows the same rule because the stealing Note
On is the new focus event.

Gate is intentionally not the focus note's gate. `Gate = any held note` is the
useful channel-level contract and does not fall low when the newest note is
released while an older one remains held. Gate follows scheduled Note On/Off,
not the VCA release tail. Trigger and Gate therefore communicate different
facts.

Control outlets enter the prepared per-channel control table. Cross-device
consumers see the documented one-block latency from
`MODULATOR_SYSTEM_SPEC.md`; ML-P8's own native routes read their sources
directly with no such delay. Publication is never sampled from display
telemetry.

## Audio outlets

Publish stereo audio-rate port metadata and prepared taps for:

- `Osc 1`, `Osc 2`, `Osc 3`: sums of the pre-Level modulation taps across
  active voices, with voice pan applied;
- `Sub` and `Noise`: their pre-Level source taps, with voice pan applied;
- `Pre-Filter Mix`: source levels, voice feedback, and drive applied, before
  the filter;
- `Filter`: filter output before VCA.

Pre-Level oscillator/source outlets deliberately continue to signal when that
source is muted in ML-P8's own mix. This makes a silent internal modulator
available to an external subscriber. The port status text must say `pre-level`
so the behavior is not surprising.

These are audio ports, not control outlets. The current one-block control table
cannot carry them, and downsampling them would destroy their purpose. Delivery
requires the typed auxiliary audio-edge/process-buffer work described by
`AUDIO_ARCHITECTURE.md`. Implement this step in two honest slices if needed:

1. land control outlets plus stable audio-port descriptors and the internal
   taps without claiming the audio ports are connectable;
2. make the audio outlets subscribable when prepared typed audio edges exist,
   with declared latency and cycle policy.

Do not add a private bus pointer or same-block callback escape hatch to make an
ML-P8 demo work. External audio feedback cycles need the graph compiler's
general delay/cycle policy. The internal oscillator and voice feedback paths
remain zero-graph, explicitly delayed ML-P8 topology.

## Discoverability and identity

Every outlet has a stable ID, user-facing name, direction, type/domain, range
or level convention, update rate, and latency. IDs belong to the ML-P8 device
interface and cannot be casually renumbered after a project saves a route.

The channel modulation source picker groups control outlets under `ML-P8`.
Audio outlets appear only in a surface that can create a compatible audio
edge. A control destination never accepts `Osc 1` merely because both are
numeric samples; an explicit follower or other adapter is required.

## Done when

- The seven control outlets are discoverable without special-casing ML-P8 in
  the channel modulation UI.
- LFO publication is the same signal used internally, and focus-envelope
  reduction matches a tested unison group sample for sample before the
  declared cross-device latency.
- Gate stays high across overlapping held notes; Trigger fires once per Note
  On; envelope outlets continue through the focus group's release.
- Offline and live outlet timing agree exactly, including one-block
  cross-device latency.
- Audio-port descriptors distinguish audio from control and clearly declare
  their pre-Level/pre-VCA taps.
- Once typed audio edges are available, a muted Osc 3 can feed another
  compatible unit while remaining absent from ML-P8's main mix.
- Incompatible routes are rejected visibly and retained as inspectable orphan
  state when a project cannot resolve them.
- Publication adds no allocation, locks, I/O, telemetry dependency, or hidden
  same-block feedback to the callback.
