//! Control surfaces: how a knob on a desk reaches a knob in the project.
//!
//! One binding is *what a surface sends* → *what it moves* → *how the
//! movement is read*. Only the first of those three is a MIDI idea, and the
//! split is deliberate: an OSC surface adds a variant to [`ControlSource`] and
//! a decoder that produces a [`ControlValue`], and reuses every line below it
//! unchanged — takeover, relative encodings, toggles, transport, and the whole
//! of [`ControlMap`]. `docs/CONTROL_SURFACES.md` states that seam and what an
//! OSC surface owes it.
//!
//! ## Where this runs, and why not on the audio thread
//!
//! Notes are realtime and are routed inside the renderer. Control is not: a
//! binding resolves on the control thread, and its result is the *same*
//! `EngineCommand` and the same project edit that moving the on-screen control
//! produces. That is the point. A CC that moved a parameter by a private
//! realtime path would leave the project holding the old value, the interface
//! drawing the old value, and undo unable to see the move — and it would be a
//! second implementation of every parameter edit, which is the fault
//! `AGENTS.md` opens with. The cost is one pump of latency on a knob turn,
//! which is the right trade for a mapping layer; a performance subset that
//! needs tighter timing can be given a realtime fast path later, against these
//! same types.

use crate::midi::{MidiChannelFilter, MidiPortFilter, MidiPortInfo, MidiPortMatch, MidiPortId};
use crate::midi::{MidiKind, MidiMessage, RelativeEncoding};
use crate::ParamKey;

/// What a surface did, with the protocol taken off.
///
/// This is the type an OSC decoder produces too, which is most of why the
/// pipeline below is worth writing once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlValue {
    /// A position, already normalized to `0..=1`. A 7-bit CC divided by 127,
    /// a pitch bend recentred, or an OSC float.
    Absolute(f32),
    /// Movement since the last message, in detents.
    Relative(i32),
    /// A press, with a strength in `0..=1` — velocity, for a pad.
    Press(f32),
    /// The press ended.
    Release,
}

/// Which physical control a binding listens to.
///
/// **OSC lands here**, as `Osc { address: String }`, and needs nothing else in
/// this module: it produces a [`ControlValue`] like every variant below and
/// the rest of the pipeline is already protocol-free.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSource {
    /// A control change. The ordinary case: a knob or a fader on a desk.
    Cc {
        #[serde(default)]
        port: MidiPortFilter,
        #[serde(default)]
        channel: MidiChannelFilter,
        controller: u8,
    },
    /// A key or a pad, by note number. Velocity arrives as the press
    /// strength, so a pad can drive a parameter as well as fire an action.
    Note {
        #[serde(default)]
        port: MidiPortFilter,
        #[serde(default)]
        channel: MidiChannelFilter,
        note: u8,
    },
    /// The pitch wheel, normalized so that centre is 0.5 rather than 0 —
    /// a binding's job is to move a control, and a control's range starts at
    /// one end.
    PitchBend {
        #[serde(default)]
        port: MidiPortFilter,
        #[serde(default)]
        channel: MidiChannelFilter,
    },
}

/// Whether two port filters could both accept one message. `Any` overlaps
/// everything, including a named port that is not plugged in -- a binding on a
/// missing device is inert *today*, and it is still the binding that owns that
/// knob when the device comes back.
fn ports_overlap(a: &MidiPortFilter, b: &MidiPortFilter) -> bool {
    match (a, b) {
        (MidiPortFilter::Any, _) | (_, MidiPortFilter::Any) => true,
        (MidiPortFilter::Named(a), MidiPortFilter::Named(b)) => a == b,
    }
}

/// Whether two channel filters could both accept one message.
fn channels_overlap(a: MidiChannelFilter, b: MidiChannelFilter) -> bool {
    match (a, b) {
        (MidiChannelFilter::Omni, _) | (_, MidiChannelFilter::Omni) => true,
        (MidiChannelFilter::One(a), MidiChannelFilter::One(b)) => a == b,
    }
}

impl ControlSource {
    /// Which port this source listens to. Public because the mapping editor
    /// asks the same question the resolver does, and a second reading of the
    /// variants is a second place for a new one to be forgotten.
    pub fn port(&self) -> &MidiPortFilter {
        match self {
            Self::Cc { port, .. } | Self::Note { port, .. } | Self::PitchBend { port, .. } => port,
        }
    }

    pub fn midi_channel(&self) -> MidiChannelFilter {
        match self {
            Self::Cc { channel, .. }
            | Self::Note { channel, .. }
            | Self::PitchBend { channel, .. } => *channel,
        }
    }

    /// What this source hears in `message`, if anything. `port` is the
    /// already-resolved form of this source's port filter; resolving it per
    /// message would mean a string comparison per message.
    pub fn read(&self, message: &MidiMessage, port: MidiPortMatch) -> Option<ControlValue> {
        if !port.accepts(message.port) || !self.midi_channel().accepts(message.channel) {
            return None;
        }
        match (self, message.kind) {
            (Self::Cc { controller, .. }, MidiKind::ControlChange { controller: c, value })
                if *controller == c =>
            {
                Some(ControlValue::Absolute(f32::from(value) / 127.0))
            }
            (Self::Note { note, .. }, MidiKind::NoteOn { note: n, velocity }) if *note == n => {
                Some(ControlValue::Press(f32::from(velocity) / 127.0))
            }
            (Self::Note { note, .. }, MidiKind::NoteOff { note: n }) if *note == n => {
                Some(ControlValue::Release)
            }
            (Self::PitchBend { .. }, MidiKind::PitchBend { value }) => {
                // -8192..=8191 onto 0..=1, with centre exactly half.
                Some(ControlValue::Absolute(
                    ((f32::from(value) + 8192.0) / 16383.0).clamp(0.0, 1.0),
                ))
            }
            _ => None,
        }
    }

    /// Whether these two sources could both fire on one message — the same
    /// control, with port and channel filters that overlap.
    ///
    /// Not equality, which is what this used to be, and the difference is the
    /// whole of what a learn gesture means. Learning CC 7 on channel 1 over a
    /// binding of CC 7 on *every* channel left both in the map: they were not
    /// equal, so neither displaced the other, and one knob then drove two
    /// things with no way to tell from the desk. Overlap is the question a
    /// user is actually asking.
    pub fn conflicts_with(&self, other: &Self) -> bool {
        let same_control = match (self, other) {
            (Self::Cc { controller: a, .. }, Self::Cc { controller: b, .. }) => a == b,
            (Self::Note { note: a, .. }, Self::Note { note: b, .. }) => a == b,
            (Self::PitchBend { .. }, Self::PitchBend { .. }) => true,
            _ => false,
        };
        same_control
            && ports_overlap(self.port(), other.port())
            && channels_overlap(self.midi_channel(), other.midi_channel())
    }

    /// How this source reads in the interface: "CC 74", "Note 36", "Bend".
    pub fn label(&self) -> String {
        match self {
            Self::Cc { controller, .. } => format!("CC {controller}"),
            Self::Note { note, .. } => format!("Note {note}"),
            Self::PitchBend { .. } => "Bend".to_owned(),
        }
    }

    /// The same control with its two filters spelled out, for a mapping list
    /// where two rows can otherwise read identically: "CC 74 · ch 3 ·
    /// Launchkey MK3".
    ///
    /// Both filters are always shown, including when they are the permissive
    /// default. A row that said only "CC 74" would leave a reader unable to
    /// tell a binding that listens to one keyboard from one that listens to
    /// every keyboard, and that difference is exactly what `bind_port`
    /// decides.
    pub fn detail_label(&self) -> String {
        let channel = match self.midi_channel() {
            MidiChannelFilter::Omni => "omni".to_owned(),
            filter => format!("ch {}", filter.label()),
        };
        let port = match self.port() {
            MidiPortFilter::Any => "any port".to_owned(),
            MidiPortFilter::Named(name) => name.clone(),
        };
        format!("{} · {channel} · {port}", self.label())
    }
}

/// What a control moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlTarget {
    /// Any parameter in the project, by the durable name of its scope.
    /// A fader, a knob, a stepped switch — the descriptor says which, and the
    /// binding does not have to know.
    ///
    /// A [`ParamKey`] rather than the [`ParamAddr`] automation and modulation
    /// use, because a binding outlives the edits those do not: a lane lives
    /// *on* its channel and travels with it, where a desk fader learned onto
    /// channel 3's volume is stored once for the whole project and used to
    /// start moving channel 4's the moment channel 1 was deleted. Nothing
    /// renumbers a binding now; resolving one is [`ParamKey::resolve`], and a
    /// binding that resolves to nothing is inert rather than dropped.
    ///
    /// [`ParamAddr`]: crate::ParamAddr
    Param(ParamKey),
    /// The transport.
    Transport(TransportControl),
}

/// A transport gesture, however it was asked for — a mapped pad, a MIDI Start
/// message, a keyboard shortcut, or an OSC message.
///
/// Every one of these is something the interface can already do; naming them
/// here is what lets a surface ask for one without the surface knowing how the
/// transport is driven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportControl {
    /// Start from the current position.
    Play,
    /// Stop and return to the start.
    Stop,
    /// Hold position.
    Pause,
    /// Play if stopped, pause if playing. What a single transport button on a
    /// desk does.
    PlayPause,
    /// Move the playhead to the start without changing whether it is running.
    ReturnToStart,
    /// Arm or disarm MIDI recording.
    ToggleRecord,
    /// Turn the loop range on or off.
    ToggleLoop,
}

impl TransportControl {
    pub fn label(self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::Stop => "Stop",
            Self::Pause => "Pause",
            Self::PlayPause => "Play/Pause",
            Self::ReturnToStart => "Return to Start",
            Self::ToggleRecord => "Record Arm",
            Self::ToggleLoop => "Loop",
        }
    }

    /// Every gesture, in the order a mapping menu lists them. Written out
    /// rather than derived so that adding one means writing down what it is
    /// called, which is how `DeviceKind` keeps its own list honest.
    pub const ALL: [Self; 7] = [
        Self::Play,
        Self::Stop,
        Self::Pause,
        Self::PlayPause,
        Self::ReturnToStart,
        Self::ToggleRecord,
        Self::ToggleLoop,
    ];
}

/// What happens when an absolute control's position disagrees with the value
/// it is bound to — which it always does, the first time a knob is touched
/// after loading a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Takeover {
    /// The parameter jumps to the control's position. Right for a motorised
    /// or endless control, and for anything where the surface is the truth.
    Jump,
    /// The control does nothing until its position passes the parameter's,
    /// and takes over from there. Right for an ordinary potentiometer, which
    /// is why it is the default: a filter sweep should not begin with a jump
    /// to wherever the knob was left.
    #[default]
    Pickup,
}

/// How a control's movement becomes a parameter change.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
    /// The control's position is the value.
    Absolute {
        #[serde(default)]
        takeover: Takeover,
    },
    /// Movement adds to the value. `step` is one detent as a fraction of the
    /// parameter's whole range, so 1/128 is a full turn of a 128-detent
    /// encoder across the range.
    Relative {
        #[serde(default)]
        encoding: RelativeEncoding,
        step: f32,
    },
    /// A press flips the value between the ends of its range; for a transport
    /// target, a press fires it.
    Toggle,
    /// A press goes to the top of the range, a release back to the bottom.
    /// For a transport target it fires on press, like [`Self::Toggle`].
    Momentary,
}

impl Default for ControlMode {
    fn default() -> Self {
        Self::Absolute {
            takeover: Takeover::default(),
        }
    }
}

impl ControlMode {
    /// How this mode reads in a mapping list. Takeover is folded in rather
    /// than given a column of its own: it is the only thing an absolute
    /// binding has to say about itself, and it means nothing for the other
    /// three.
    pub fn label(self) -> &'static str {
        match self {
            Self::Absolute {
                takeover: Takeover::Pickup,
            } => "Absolute, pickup",
            Self::Absolute {
                takeover: Takeover::Jump,
            } => "Absolute, jump",
            Self::Relative { .. } => "Relative",
            Self::Toggle => "Toggle",
            Self::Momentary => "Momentary",
        }
    }

    /// The takeover this mode runs, for a mode that has one.
    pub fn takeover(self) -> Option<Takeover> {
        match self {
            Self::Absolute { takeover } => Some(takeover),
            _ => None,
        }
    }
}

/// One mapping: a control, what it moves, and how its movement is read.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ControlBinding {
    pub source: ControlSource,
    pub target: ControlTarget,
    #[serde(default)]
    pub mode: ControlMode,
    /// Where the bound range starts and ends, normalized. Inverting them
    /// inverts the control, which is how a fader is made to close a filter as
    /// it rises without a mode for it.
    #[serde(default = "unit_low")]
    pub min: f32,
    #[serde(default = "unit_high")]
    pub max: f32,
}

fn unit_low() -> f32 {
    0.0
}

fn unit_high() -> f32 {
    1.0
}

impl ControlBinding {
    /// A binding with the defaults a learn gesture produces: the whole range,
    /// absolute, with pickup.
    pub fn new(source: ControlSource, target: ControlTarget) -> Self {
        Self {
            source,
            target,
            mode: ControlMode::default(),
            min: 0.0,
            max: 1.0,
        }
    }

    /// Whether the bound range runs backwards, which is how a fader is made
    /// to close a filter as it rises. There is no mode for it: the range's
    /// ends carry it, and this is the question the editor asks of them.
    pub fn inverted(&self) -> bool {
        self.min > self.max
    }

    /// Point the range one way or the other, keeping whatever span it covers.
    /// An editor's invert switch is this, rather than a write of 1 and 0: a
    /// binding narrowed to a third of a knob's travel should stay narrowed.
    pub fn set_inverted(&mut self, inverted: bool) {
        if self.inverted() == inverted {
            return;
        }
        std::mem::swap(&mut self.min, &mut self.max);
    }

    /// Map a normalized control position onto this binding's range.
    fn scale(&self, position: f32) -> f32 {
        (self.min + (self.max - self.min) * position.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }
}

/// What a binding did with one message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ControlOutcome {
    /// The parameter should move to this normalized value.
    Set(f32),
    /// The transport gesture should fire.
    Fire,
}

/// A binding's memory between messages: what pickup is waiting for.
///
/// Held beside the map rather than inside it, because the map is a persisted
/// document and this is the state of one performance. Reset when a project
/// loads or a target's value is changed from anywhere else, which is what
/// makes pickup re-arm rather than fire on a stale comparison.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PickupState {
    /// Which side of the parameter the control was last seen on, or `None`
    /// before it has been seen at all.
    below: Option<bool>,
    /// Once the control has caught the parameter it stays caught, until
    /// something else moves the parameter.
    caught: bool,
    /// Where this binding left the parameter, as the parameter itself then
    /// read — see [`Self::wrote`].
    left_at: Option<f32>,
}

impl PickupState {
    /// Forget where the control was. Called when the parameter moved for a
    /// reason other than this control — the on-screen knob, a preset, undo.
    pub fn release(&mut self) {
        *self = Self::default();
    }

    /// Record where the parameter ended up after this binding moved it.
    ///
    /// **This is what lets a caught control notice that something else has
    /// taken the parameter off it**, without every on-screen control, preset
    /// recall and undo step having to remember to say so. A caught binding
    /// otherwise writes freely forever: a knob that had taken over pulled the
    /// parameter back to its own position on the next message it sent,
    /// undoing whatever the mouse had just done.
    ///
    /// The value recorded is the one the parameter *reads back* rather than
    /// the one that was asked for, because a stepped parameter quantizes and
    /// a clamped one clips. Comparing against the request would then see a
    /// difference on every message and release a control that nothing had
    /// touched.
    pub fn wrote(&mut self, observed: f32) {
        self.left_at = Some(observed);
    }

    /// Whether the parameter has moved since this binding last wrote it.
    /// `None` for a binding that has not written one, which cannot have been
    /// overtaken.
    fn overtaken(&self, current: f32) -> bool {
        self.left_at
            .is_some_and(|left| (left - current).abs() > PICKUP_MOVED_EPSILON)
    }
}

/// How far a parameter has to move from where a binding left it before the
/// binding counts as overtaken. A round trip through one descriptor is exact,
/// so this only has to clear the noise of one float comparison.
const PICKUP_MOVED_EPSILON: f32 = 1e-6;

/// Resolve one message against one binding.
///
/// `current` is the target's present normalized value, which pickup and
/// relative movement both need and a transport target ignores.
pub fn apply(
    binding: &ControlBinding,
    value: ControlValue,
    current: f32,
    pickup: &mut PickupState,
) -> Option<ControlOutcome> {
    let transport = matches!(binding.target, ControlTarget::Transport(_));
    match (binding.mode, value) {
        // A press fires a transport gesture whatever the mode says; there is
        // no such thing as a transport button held half-down.
        (_, ControlValue::Press(_)) if transport => Some(ControlOutcome::Fire),
        (ControlMode::Toggle | ControlMode::Momentary, ControlValue::Release) if transport => None,
        // An absolute control crossing the halfway point fires a transport
        // gesture once, on the way up. A desk whose transport row sends CCs
        // rather than notes is ordinary.
        (_, ControlValue::Absolute(position)) if transport => {
            let high = position >= 0.5;
            let was_high = pickup.caught;
            pickup.caught = high;
            (high && !was_high).then_some(ControlOutcome::Fire)
        }
        (_, ControlValue::Relative(_) | ControlValue::Release) if transport => None,

        (ControlMode::Toggle, ControlValue::Press(_)) => {
            let midpoint = (binding.min + binding.max) / 2.0;
            let high = if binding.min <= binding.max {
                current >= midpoint
            } else {
                current <= midpoint
            };
            Some(ControlOutcome::Set(if high {
                binding.min
            } else {
                binding.max
            }))
        }
        (ControlMode::Toggle, _) => None,
        (ControlMode::Momentary, ControlValue::Press(strength)) => {
            // Velocity is the strength of the press, so a pad can play a
            // parameter as well as switch it. A controller that sends a fixed
            // velocity simply always lands on the top of the range.
            Some(ControlOutcome::Set(binding.scale(strength)))
        }
        (ControlMode::Momentary, ControlValue::Release) => {
            Some(ControlOutcome::Set(binding.scale(0.0)))
        }
        (ControlMode::Momentary, _) => None,

        (ControlMode::Absolute { takeover }, ControlValue::Absolute(position)) => {
            let wanted = binding.scale(position);
            match takeover {
                Takeover::Jump => Some(ControlOutcome::Set(wanted)),
                Takeover::Pickup => {
                    if pickup.caught {
                        // Still caught only while the parameter is where this
                        // binding left it. Anything else moving it -- the
                        // on-screen knob, undo, a preset, another binding --
                        // hands the catching back to the control.
                        if !pickup.overtaken(current) {
                            return Some(ControlOutcome::Set(wanted));
                        }
                        pickup.release();
                    }
                    // Equality counts as caught, so a control already sitting
                    // exactly on the value takes over on its first message
                    // rather than on its second.
                    let below = wanted < current;
                    let crossed = pickup.below.is_some_and(|was| was != below);
                    if crossed || (wanted - current).abs() < f32::EPSILON {
                        pickup.caught = true;
                        pickup.below = Some(below);
                        return Some(ControlOutcome::Set(wanted));
                    }
                    pickup.below = Some(below);
                    None
                }
            }
        }
        (ControlMode::Absolute { .. }, _) => None,

        (ControlMode::Relative { step, .. }, ControlValue::Relative(detents)) => {
            let moved = current + step * detents as f32;
            Some(ControlOutcome::Set(moved.clamp(
                binding.min.min(binding.max),
                binding.min.max(binding.max),
            )))
        }
        // A relative binding reading an absolute message decodes the message
        // as the encoder convention the binding was configured with. This is
        // the whole reason `RelativeEncoding` is configuration: an endless
        // encoder sends ordinary CC bytes and only the setting says which of
        // three mutually unintelligible conventions they are in.
        (ControlMode::Relative { encoding, step }, ControlValue::Absolute(position)) => {
            let byte = (position * 127.0).round().clamp(0.0, 127.0) as u8;
            let detents = encoding.delta(byte);
            if detents == 0 {
                return None;
            }
            let moved = current + step * f32::from(detents);
            Some(ControlOutcome::Set(moved.clamp(
                binding.min.min(binding.max),
                binding.min.max(binding.max),
            )))
        }
        (ControlMode::Relative { .. }, _) => None,
    }
}

/// Every mapping in the project, and the resolved port for each.
///
/// Bindings live with the project because their targets do: a [`ParamAddr`]
/// names a channel in *this* song. A surface template that outlives one song —
/// a desk's transport row, say — is a separate document that stamps bindings
/// into a project, and is not this type.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ControlMap {
    #[serde(default)]
    pub bindings: Vec<ControlBinding>,
}

/// A `ControlMap` with each binding's port resolved and its pickup state
/// alongside — what the control thread actually runs a message through.
#[derive(Debug, Clone, Default)]
pub struct ControlMapState {
    ports: Vec<MidiPortMatch>,
    pickup: Vec<PickupState>,
}

impl ControlMapState {
    /// Re-resolve every binding's port against the ports that exist now.
    /// Called when the map changes and when the port list does; a binding
    /// whose device was unplugged goes inert rather than promiscuous.
    pub fn resolve(&mut self, map: &ControlMap, ports: &[MidiPortInfo]) {
        self.ports.clear();
        self.ports
            .extend(map.bindings.iter().map(|b| b.source.port().resolve(ports)));
        self.pickup.resize(map.bindings.len(), PickupState::default());
        self.pickup.truncate(map.bindings.len());
    }

    /// Forget pickup for every binding on `target`. The caller does this when
    /// the parameter moved for a reason other than its bound control, so the
    /// control has to catch the new value rather than snapping it back.
    pub fn release(&mut self, map: &ControlMap, target: &ControlTarget) {
        for (index, binding) in map.bindings.iter().enumerate() {
            if &binding.target == target {
                if let Some(state) = self.pickup.get_mut(index) {
                    state.release();
                }
            }
        }
    }

    /// Forget every binding's pickup. For a project load or an undo, where
    /// everything may have moved.
    pub fn release_all(&mut self) {
        for state in &mut self.pickup {
            state.release();
        }
    }

    /// What this message does, binding by binding.
    ///
    /// `current` is asked for a binding's target only when a binding actually
    /// heard the message, because resolving a `ParamAddr` to its present value
    /// is a walk through the project and most messages match nothing.
    pub fn apply<'m>(
        &'m mut self,
        map: &'m ControlMap,
        message: &'m MidiMessage,
        mut current: impl FnMut(&ControlTarget) -> f32 + 'm,
    ) -> Vec<(usize, ControlOutcome)> {
        let mut outcomes = Vec::new();
        for (index, binding) in map.bindings.iter().enumerate() {
            // A binding this state has not resolved does nothing.
            //
            // It used to fall back to `MidiPortMatch::default()`, which is
            // `Any` -- so a state that had not been resolved against the port
            // list made every binding *promiscuous*, listening to every
            // keyboard, which is the one thing `Missing` exists to prevent.
            // That is exactly the state a project load leaves behind until
            // somebody remembers to call `Session::resolve_control_map`, and
            // "forgot to resolve" should be a binding that does nothing
            // rather than a binding that does everything.
            let Some(port) = self.ports.get(index).copied() else {
                continue;
            };
            let Some(value) = binding.source.read(message, port) else {
                continue;
            };
            let Some(state) = self.pickup.get_mut(index) else {
                continue;
            };
            if let Some(outcome) = apply(binding, value, current(&binding.target), state) {
                outcomes.push((index, outcome));
            }
        }
        outcomes
    }

    /// Tell one binding where the parameter ended up after its outcome was
    /// applied. The caller does this because only the caller can read the
    /// value back: see [`PickupState::wrote`] for why it matters.
    pub fn wrote(&mut self, index: usize, observed: f32) {
        if let Some(state) = self.pickup.get_mut(index) {
            state.wrote(observed);
        }
    }

    /// Which bindings would hear this message at all, whatever they would do
    /// with it. The interface uses this to light a mapped control while its
    /// knob is being turned.
    pub fn listening(&self, map: &ControlMap, message: &MidiMessage) -> Vec<usize> {
        map.bindings
            .iter()
            .enumerate()
            .filter(|(index, binding)| {
                // Unresolved is inert here too; see `apply`.
                self.ports
                    .get(*index)
                    .copied()
                    .is_some_and(|port| binding.source.read(message, port).is_some())
            })
            .map(|(index, _)| index)
            .collect()
    }
}

impl ControlMap {
    /// Add a binding, replacing any that the same control could already fire.
    ///
    /// One physical knob drives one thing. A learn gesture that silently
    /// stacked a second target onto a knob already in use would be
    /// indistinguishable, from the desk, from a knob that had gone wrong.
    /// Conflict is *overlap* rather than equality -- see
    /// [`ControlSource::conflicts_with`] -- so learning a knob on one channel
    /// displaces the same knob bound across all of them. Returns the bindings
    /// that were displaced.
    pub fn bind(&mut self, binding: ControlBinding) -> Vec<ControlBinding> {
        let mut displaced = Vec::new();
        let mut index = 0;
        while index < self.bindings.len() {
            if self.bindings[index].source.conflicts_with(&binding.source) {
                displaced.push(self.bindings.remove(index));
            } else {
                index += 1;
            }
        }
        self.bindings.push(binding);
        displaced
    }

    /// Remove every binding onto `target`.
    pub fn unbind(&mut self, target: &ControlTarget) -> usize {
        let before = self.bindings.len();
        self.bindings.retain(|binding| &binding.target != target);
        before - self.bindings.len()
    }

    /// Re-scope every parameter binding after a **track** edit, dropping
    /// those whose track is gone. Returns whether anything changed.
    ///
    /// There is no channel twin, and its absence is the point. A binding
    /// named its channel by position until 2026-09-18 and was renumbered here
    /// exactly as a lane is; a [`ChannelId`] means a channel edit moves no
    /// binding at all, so `Project::rescope_after` no longer calls into this
    /// file. A track is still a seat, so this half remains.
    ///
    /// A binding whose track was removed is **dropped**, where one whose
    /// channel is deleted now survives inert. That is not an inconsistency:
    /// the removed track has no identity to come back as, so the binding has
    /// nothing left to name, while the deleted channel's id is exactly what
    /// undo restores it under.
    ///
    /// [`ChannelId`]: crate::ChannelId
    pub fn rescope_tracks(&mut self, edit: crate::structure::TrackEdit) -> bool {
        let mut changed = false;
        self.bindings.retain_mut(|binding| {
            let ControlTarget::Param(old) = binding.target else {
                return true;
            };
            match old.after_track(edit) {
                Some(new) => {
                    changed |= new != old;
                    binding.target = ControlTarget::Param(new);
                    true
                }
                None => {
                    changed = true;
                    false
                }
            }
        });
        changed
    }

    /// The binding onto `target`, for an interface that wants to draw what a
    /// control is mapped to.
    pub fn binding_for(&self, target: &ControlTarget) -> Option<&ControlBinding> {
        self.bindings
            .iter()
            .find(|binding| &binding.target == target)
    }

    /// Bindings whose port names no current port carries, by index. The
    /// interface warns about these rather than the map dropping them: a
    /// controller that is not plugged in right now is not a mistake.
    pub fn unresolved(&self, ports: &[MidiPortInfo]) -> Vec<usize> {
        self.bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| {
                matches!(binding.source.port().resolve(ports), MidiPortMatch::Missing)
            })
            .map(|(index, _)| index)
            .collect()
    }
}

/// A learn gesture in progress: one target waiting for a control to touch.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlLearn {
    pub target: ControlTarget,
    /// Whether to bind the port the message arrives on, rather than any port.
    /// On for a studio with several controllers, off for one.
    pub bind_port: bool,
}

impl ControlLearn {
    /// The binding this message completes the gesture with, or `None` for a
    /// message that names no control. A note-*off* does not complete a learn:
    /// the gesture is "touch the control", and a key's release would otherwise
    /// bind the key twice over.
    pub fn resolve(&self, message: &MidiMessage, ports: &[MidiPortInfo]) -> Option<ControlBinding> {
        let port = match (self.bind_port, port_name(message.port, ports)) {
            (true, Some(name)) => MidiPortFilter::Named(name.to_owned()),
            _ => MidiPortFilter::Any,
        };
        let channel = MidiChannelFilter::One(message.channel);
        let source = match message.kind {
            MidiKind::ControlChange { controller, .. } => ControlSource::Cc {
                port,
                channel,
                controller,
            },
            MidiKind::NoteOn { note, .. } => ControlSource::Note {
                port,
                channel,
                note,
            },
            MidiKind::PitchBend { .. } => ControlSource::PitchBend { port, channel },
            _ => return None,
        };
        let mut binding = ControlBinding::new(source, self.target);
        // A pad learned onto a parameter is a switch, not a fader; a pad
        // learned onto the transport fires. Either way a note wants Toggle,
        // and it is a worse default for neither.
        if matches!(binding.source, ControlSource::Note { .. }) {
            binding.mode = ControlMode::Toggle;
        }
        Some(binding)
    }
}

fn port_name(port: MidiPortId, ports: &[MidiPortInfo]) -> Option<&str> {
    ports
        .iter()
        .find(|info| info.id == port)
        .map(|info| info.name.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{MidiPortId, SYSTEM_CHANNEL};
    use crate::{ChainKey, DeviceId};

    fn ports() -> Vec<MidiPortInfo> {
        vec![
            MidiPortInfo {
                id: MidiPortId(0),
                name: "Launchkey MK3".to_owned(),
            },
            MidiPortInfo {
                id: MidiPortId(1),
                name: "Faderfox".to_owned(),
            },
        ]
    }

    /// A map that has not been resolved against the port list does nothing.
    ///
    /// The fallback used to be `MidiPortMatch::default()`, which is `Any`, so
    /// an unresolved state was not inert -- it was *promiscuous*, and every
    /// binding that names one keyboard listened to all of them. A project
    /// load leaves exactly that state behind until the caller resolves it.
    #[test]
    fn an_unresolved_map_hears_nothing() {
        let mut map = ControlMap::default();
        map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Named("Faderfox".to_owned()),
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Transport(TransportControl::Play),
        ));
        let mut state = ControlMapState::default();
        let message = cc(1, 0, 7, 127);

        assert!(state.apply(&map, &message, |_| 0.0).is_empty());
        assert!(state.listening(&map, &message).is_empty());

        // Resolved, the same message reaches it.
        state.resolve(&map, &ports());
        assert_eq!(state.listening(&map, &message), vec![0]);
    }

    /// Inverting a binding points its range the other way and keeps the span
    /// it covers, so a knob mapped to a third of a parameter stays mapped to
    /// a third of it.
    #[test]
    fn inverting_a_binding_keeps_the_span_it_covers() {
        let mut binding = ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 7,
            },
            ControlTarget::Transport(TransportControl::Play),
        );
        binding.min = 0.2;
        binding.max = 0.5;
        assert!(!binding.inverted());

        binding.set_inverted(true);
        assert!(binding.inverted());
        assert_eq!((binding.min, binding.max), (0.5, 0.2));
        assert_eq!(binding.scale(0.0), 0.5);
        assert!((binding.scale(1.0) - 0.2).abs() < 1e-6);

        // Asking for what it already is changes nothing rather than swapping
        // the ends back.
        binding.set_inverted(true);
        assert_eq!((binding.min, binding.max), (0.5, 0.2));
        binding.set_inverted(false);
        assert_eq!((binding.min, binding.max), (0.2, 0.5));
    }

    fn cc(port: u16, channel: u8, controller: u8, value: u8) -> MidiMessage {
        MidiMessage {
            offset: 0,
            port: MidiPortId(port),
            channel,
            kind: MidiKind::ControlChange { controller, value },
        }
    }

    fn note_on(port: u16, channel: u8, note: u8, velocity: u8) -> MidiMessage {
        MidiMessage {
            offset: 0,
            port: MidiPortId(port),
            channel,
            kind: MidiKind::NoteOn { note, velocity },
        }
    }

    const CUTOFF: ControlTarget = ControlTarget::Param(ParamKey::effect(
        ChainKey::Channel(crate::ChannelId(0)),
        DeviceId(0),
        3,
    ));

    fn param_binding(controller: u8) -> ControlBinding {
        ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller,
            },
            CUTOFF,
        )
    }

    /// The default takeover is pickup, and pickup means the parameter does not
    /// move until the knob reaches it. A filter left open, with the knob left
    /// closed, must not slam shut on the first message.
    #[test]
    fn pickup_waits_for_the_control_to_reach_the_value() {
        let binding = param_binding(74);
        let mut state = PickupState::default();
        // The knob is at the bottom, the parameter is at 0.8.
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.0), 0.8, &mut state),
            None
        );
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.5), 0.8, &mut state),
            None
        );
        // Crossing takes over, and everything after it moves.
        let caught = apply(&binding, ControlValue::Absolute(0.9), 0.8, &mut state);
        assert_eq!(caught, Some(ControlOutcome::Set(0.9)));
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.2), 0.9, &mut state),
            Some(ControlOutcome::Set(0.2))
        );
        // Until something else moves the parameter, at which point the knob
        // has to catch it again.
        state.release();
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.2), 0.9, &mut state),
            None
        );
    }

    #[test]
    fn jump_takeover_does_not_wait() {
        let mut binding = param_binding(74);
        binding.mode = ControlMode::Absolute {
            takeover: Takeover::Jump,
        };
        let mut state = PickupState::default();
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.0), 0.8, &mut state),
            Some(ControlOutcome::Set(0.0))
        );
    }

    /// Inverting the ends inverts the control, which is the whole mechanism
    /// for a fader that closes a filter as it rises.
    #[test]
    fn an_inverted_range_inverts_the_control() {
        let mut binding = param_binding(74);
        binding.mode = ControlMode::Absolute {
            takeover: Takeover::Jump,
        };
        binding.min = 1.0;
        binding.max = 0.0;
        let mut state = PickupState::default();
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.0), 0.5, &mut state),
            Some(ControlOutcome::Set(1.0))
        );
        assert_eq!(
            apply(&binding, ControlValue::Absolute(1.0), 0.5, &mut state),
            Some(ControlOutcome::Set(0.0))
        );
    }

    /// An endless encoder sends ordinary CC bytes, and only the binding's
    /// configured encoding says which direction 65 means. Getting this wrong
    /// is not a small error: it is the opposite direction.
    #[test]
    fn a_relative_binding_reads_cc_bytes_as_movement() {
        let mut binding = param_binding(74);
        binding.mode = ControlMode::Relative {
            encoding: RelativeEncoding::BinaryOffset,
            step: 0.01,
        };
        let mut state = PickupState::default();
        // 65 is one detent up under binary offset.
        let up = apply(&binding, ControlValue::Absolute(65.0 / 127.0), 0.5, &mut state);
        assert_eq!(up, Some(ControlOutcome::Set(0.51)));
        let down = apply(&binding, ControlValue::Absolute(63.0 / 127.0), 0.5, &mut state);
        assert_eq!(down, Some(ControlOutcome::Set(0.49)));
        // 64 is no movement, and produces no edit rather than a no-op one.
        assert_eq!(
            apply(&binding, ControlValue::Absolute(64.0 / 127.0), 0.5, &mut state),
            None
        );
        // The same byte under a different convention moves the other way --
        // sixty-three detents down rather than one up, which the range clamps
        // at the bottom.
        binding.mode = ControlMode::Relative {
            encoding: RelativeEncoding::TwosComplement,
            step: 0.01,
        };
        let other = apply(&binding, ControlValue::Absolute(65.0 / 127.0), 0.5, &mut state);
        assert_eq!(other, Some(ControlOutcome::Set(0.0)));
        let one_up = apply(&binding, ControlValue::Absolute(1.0 / 127.0), 0.5, &mut state);
        assert_eq!(one_up, Some(ControlOutcome::Set(0.51)));
    }

    #[test]
    fn relative_movement_stays_inside_the_bound_range() {
        let mut binding = param_binding(74);
        binding.mode = ControlMode::Relative {
            encoding: RelativeEncoding::BinaryOffset,
            step: 0.5,
        };
        let mut state = PickupState::default();
        assert_eq!(
            apply(&binding, ControlValue::Relative(8), 0.9, &mut state),
            Some(ControlOutcome::Set(1.0))
        );
        assert_eq!(
            apply(&binding, ControlValue::Relative(-8), 0.1, &mut state),
            Some(ControlOutcome::Set(0.0))
        );
    }

    /// A pad bound to a parameter flips it between the ends of its range, and
    /// the flip is decided by which end the value is nearer — not by the
    /// binding remembering, which would desynchronise the moment the on-screen
    /// control was touched.
    #[test]
    fn a_toggle_flips_between_the_ends_of_the_bound_range() {
        let mut binding = param_binding(74);
        binding.mode = ControlMode::Toggle;
        let mut state = PickupState::default();
        assert_eq!(
            apply(&binding, ControlValue::Press(1.0), 0.0, &mut state),
            Some(ControlOutcome::Set(1.0))
        );
        assert_eq!(
            apply(&binding, ControlValue::Press(1.0), 1.0, &mut state),
            Some(ControlOutcome::Set(0.0))
        );
        // A release does nothing: the flip already happened.
        assert_eq!(
            apply(&binding, ControlValue::Release, 1.0, &mut state),
            None
        );
    }

    #[test]
    fn a_momentary_pad_plays_the_parameter_with_velocity() {
        let mut binding = param_binding(74);
        binding.mode = ControlMode::Momentary;
        let mut state = PickupState::default();
        assert_eq!(
            apply(&binding, ControlValue::Press(0.5), 0.0, &mut state),
            Some(ControlOutcome::Set(0.5))
        );
        assert_eq!(
            apply(&binding, ControlValue::Release, 0.5, &mut state),
            Some(ControlOutcome::Set(0.0))
        );
    }

    /// A transport gesture fires on a press, and on an absolute control only
    /// as it rises past the middle -- once, not on every message above it.
    /// A desk whose transport row sends CCs is ordinary, and a held button
    /// sending a stream of 127s must not restart playback sixty times a
    /// second.
    #[test]
    fn transport_fires_once_on_the_way_up() {
        let binding = ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 118,
            },
            ControlTarget::Transport(TransportControl::PlayPause),
        );
        let mut state = PickupState::default();
        assert_eq!(
            apply(&binding, ControlValue::Absolute(1.0), 0.0, &mut state),
            Some(ControlOutcome::Fire)
        );
        assert_eq!(
            apply(&binding, ControlValue::Absolute(1.0), 0.0, &mut state),
            None
        );
        assert_eq!(
            apply(&binding, ControlValue::Absolute(0.0), 0.0, &mut state),
            None
        );
        assert_eq!(
            apply(&binding, ControlValue::Absolute(1.0), 0.0, &mut state),
            Some(ControlOutcome::Fire)
        );
        // A pad fires on press and does nothing on release.
        assert_eq!(
            apply(&binding, ControlValue::Press(0.1), 0.0, &mut state),
            Some(ControlOutcome::Fire)
        );
        assert_eq!(
            apply(&binding, ControlValue::Release, 0.0, &mut state),
            None
        );
    }

    /// A binding on one port hears that port only. This is the difference
    /// between a two-controller studio working and every knob on every desk
    /// fighting over the same parameter.
    #[test]
    fn a_binding_hears_its_own_port_and_channel() {
        let source = ControlSource::Cc {
            port: MidiPortFilter::Named("Faderfox".to_owned()),
            channel: MidiChannelFilter::One(2),
            controller: 21,
        };
        let resolved = source.port().resolve(&ports());
        assert_eq!(resolved, MidiPortMatch::Port(MidiPortId(1)));
        assert_eq!(
            source.read(&cc(1, 2, 21, 127), resolved),
            Some(ControlValue::Absolute(1.0))
        );
        // Wrong port, wrong channel, wrong controller.
        assert_eq!(source.read(&cc(0, 2, 21, 127), resolved), None);
        assert_eq!(source.read(&cc(1, 3, 21, 127), resolved), None);
        assert_eq!(source.read(&cc(1, 2, 22, 127), resolved), None);
        // A port that is not plugged in goes inert, not promiscuous.
        let missing = ControlSource::Cc {
            port: MidiPortFilter::Named("A desk nobody owns".to_owned()),
            channel: MidiChannelFilter::Omni,
            controller: 21,
        };
        let resolved = missing.port().resolve(&ports());
        assert_eq!(resolved, MidiPortMatch::Missing);
        assert_eq!(missing.read(&cc(0, 0, 21, 127), resolved), None);
        assert_eq!(missing.read(&cc(1, 0, 21, 127), resolved), None);
    }

    #[test]
    fn pitch_bend_centres_on_half_a_range() {
        let source = ControlSource::PitchBend {
            port: MidiPortFilter::Any,
            channel: MidiChannelFilter::Omni,
        };
        let bend = |value| MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: 0,
            kind: MidiKind::PitchBend { value },
        };
        let Some(ControlValue::Absolute(centre)) = source.read(&bend(0), MidiPortMatch::Any) else {
            panic!("a bend reads as a position");
        };
        assert!((centre - 0.5).abs() < 0.001, "centre was {centre}");
        assert_eq!(
            source.read(&bend(-8192), MidiPortMatch::Any),
            Some(ControlValue::Absolute(0.0))
        );
        assert_eq!(
            source.read(&bend(8191), MidiPortMatch::Any),
            Some(ControlValue::Absolute(1.0))
        );
    }

    /// One knob drives one thing. Learning a knob that is already in use
    /// displaces what it drove rather than stacking a second target on it.
    #[test]
    fn learning_a_knob_twice_displaces_the_first_binding() {
        let mut map = ControlMap::default();
        assert!(map.bind(param_binding(74)).is_empty());
        let volume =
            ControlTarget::Param(ParamKey::strip(ChainKey::Channel(crate::ChannelId(1)), 0));
        let mut second = param_binding(74);
        second.target = volume;
        let displaced = map.bind(second);
        assert_eq!(displaced.len(), 1, "the first binding was displaced");
        assert_eq!(displaced[0].target, CUTOFF);
        assert_eq!(map.bindings.len(), 1);
        assert_eq!(map.binding_for(&volume).map(|b| b.source.label()), Some("CC 74".to_owned()));
        assert_eq!(map.unbind(&volume), 1);
        assert!(map.bindings.is_empty());
    }

    /// A learn gesture binds the control that was touched, on the channel it
    /// was touched on, and a pad comes out as a switch rather than a fader.
    #[test]
    fn learning_binds_the_control_that_was_touched() {
        let learn = ControlLearn {
            target: CUTOFF,
            bind_port: true,
        };
        let binding = learn
            .resolve(&cc(1, 5, 21, 64), &ports())
            .expect("a CC completes a learn");
        assert_eq!(
            binding.source,
            ControlSource::Cc {
                port: MidiPortFilter::Named("Faderfox".to_owned()),
                channel: MidiChannelFilter::One(5),
                controller: 21,
            }
        );
        assert_eq!(binding.mode, ControlMode::default());

        let pad = learn
            .resolve(&note_on(0, 9, 36, 100), &ports())
            .expect("a pad completes a learn");
        assert_eq!(pad.mode, ControlMode::Toggle);

        // Not binding the port leaves the binding open to any desk.
        let anywhere = ControlLearn {
            target: CUTOFF,
            bind_port: false,
        };
        let binding = anywhere.resolve(&cc(1, 5, 21, 64), &ports()).unwrap();
        assert_eq!(
            binding.source,
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::One(5),
                controller: 21,
            }
        );

        // A release does not complete a learn, and neither does a transport
        // message: neither names a control to bind.
        let release = MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: 0,
            kind: MidiKind::NoteOff { note: 36 },
        };
        assert_eq!(learn.resolve(&release, &ports()), None);
        let start = MidiMessage {
            offset: 0,
            port: MidiPortId(0),
            channel: SYSTEM_CHANNEL,
            kind: MidiKind::Start,
        };
        assert_eq!(learn.resolve(&start, &ports()), None);
    }

    /// The map's own pass: several bindings, one message, only the ones that
    /// heard it produce an outcome.
    #[test]
    fn the_map_applies_only_the_bindings_that_heard_the_message() {
        let mut map = ControlMap::default();
        let mut jump = param_binding(74);
        jump.mode = ControlMode::Absolute {
            takeover: Takeover::Jump,
        };
        map.bind(jump);
        let mut other = param_binding(75);
        other.target =
            ControlTarget::Param(ParamKey::strip(ChainKey::Channel(crate::ChannelId(1)), 0));
        other.mode = ControlMode::Absolute {
            takeover: Takeover::Jump,
        };
        map.bind(other);
        map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Any,
                channel: MidiChannelFilter::Omni,
                controller: 118,
            },
            ControlTarget::Transport(TransportControl::PlayPause),
        ));

        let mut state = ControlMapState::default();
        state.resolve(&map, &ports());
        assert_eq!(
            state.apply(&map, &cc(0, 3, 74, 127), |_| 0.0),
            vec![(0, ControlOutcome::Set(1.0))]
        );
        assert_eq!(
            state.apply(&map, &cc(0, 3, 118, 127), |_| 0.0),
            vec![(2, ControlOutcome::Fire)]
        );
        assert_eq!(state.listening(&map, &cc(0, 3, 74, 127)), vec![0]);
        assert_eq!(state.listening(&map, &cc(0, 3, 90, 127)), Vec::<usize>::new());
    }

    /// Conflict is overlap, not equality. Learning a knob on one channel takes
    /// it back from a binding that listened on every channel -- which is what
    /// "this knob now does that" means, and what equality could not express.
    #[test]
    fn a_binding_displaces_every_binding_the_same_control_could_fire() {
        let omni = |controller, channel| ControlSource::Cc {
            port: MidiPortFilter::Any,
            channel,
            controller,
        };
        let all = omni(74, MidiChannelFilter::Omni);
        let one = omni(74, MidiChannelFilter::One(3));
        let other = omni(74, MidiChannelFilter::One(4));
        assert!(all.conflicts_with(&one));
        assert!(one.conflicts_with(&all));
        assert!(!one.conflicts_with(&other), "two channels do not overlap");
        assert!(!all.conflicts_with(&omni(75, MidiChannelFilter::Omni)));

        // A named port and any port overlap; two named ports do not. A
        // binding on a device that is unplugged still owns its knob.
        let named = |name: &str| ControlSource::Cc {
            port: MidiPortFilter::Named(name.to_owned()),
            channel: MidiChannelFilter::Omni,
            controller: 74,
        };
        assert!(named("Faderfox").conflicts_with(&all));
        assert!(!named("Faderfox").conflicts_with(&named("Launchkey MK3")));

        // Different kinds of control never conflict, whatever their numbers.
        let note = ControlSource::Note {
            port: MidiPortFilter::Any,
            channel: MidiChannelFilter::Omni,
            note: 74,
        };
        assert!(!note.conflicts_with(&all));

        // Two bindings on the same controller but different channels can
        // coexist -- a desk whose banks send on channels 4 and 5 is ordinary.
        let mut map = ControlMap::default();
        map.bind(ControlBinding::new(one, CUTOFF));
        map.bind(ControlBinding::new(other, CUTOFF));
        assert_eq!(map.bindings.len(), 2, "two channels overlap neither");

        // And an Omni binding on that controller takes the knob from both,
        // because it is the one that would fire on either of their messages.
        let displaced = map.bind(ControlBinding::new(all, CUTOFF));
        assert_eq!(displaced.len(), 2);
        assert_eq!(map.bindings.len(), 1);
    }

    /// Releasing a target's pickup makes its control catch the value again.
    #[test]
    fn releasing_a_target_re_arms_its_pickup() {
        let mut map = ControlMap::default();
        map.bind(param_binding(74));
        let mut state = ControlMapState::default();
        state.resolve(&map, &ports());

        // Catch the value, then move it from elsewhere.
        assert!(state.apply(&map, &cc(0, 0, 74, 0), |_| 0.0).len() == 1);
        assert_eq!(
            state.apply(&map, &cc(0, 0, 74, 127), |_| 0.0),
            vec![(0, ControlOutcome::Set(1.0))]
        );
        state.release(&map, &CUTOFF);
        assert_eq!(state.apply(&map, &cc(0, 0, 74, 127), |_| 0.0), vec![]);
    }

    /// A binding naming a port that is not plugged in is reported, not
    /// dropped: an unplugged controller is not a mistake in the project.
    #[test]
    fn unresolved_bindings_are_reported_rather_than_dropped() {
        let mut map = ControlMap::default();
        map.bind(param_binding(74));
        map.bind(ControlBinding::new(
            ControlSource::Cc {
                port: MidiPortFilter::Named("A desk nobody owns".to_owned()),
                channel: MidiChannelFilter::Omni,
                controller: 21,
            },
            ControlTarget::Transport(TransportControl::Play),
        ));
        assert_eq!(map.unresolved(&ports()), vec![1]);
    }

    /// Every transport gesture is named, so a eighth cannot be added without
    /// someone writing down what it is called.
    #[test]
    fn every_transport_gesture_has_a_label() {
        for (gesture, label) in [
            (TransportControl::Play, "Play"),
            (TransportControl::Stop, "Stop"),
            (TransportControl::Pause, "Pause"),
            (TransportControl::PlayPause, "Play/Pause"),
            (TransportControl::ReturnToStart, "Return to Start"),
            (TransportControl::ToggleRecord, "Record Arm"),
            (TransportControl::ToggleLoop, "Loop"),
        ] {
            assert_eq!(gesture.label(), label);
            assert!(TransportControl::ALL.contains(&gesture));
        }
        assert_eq!(TransportControl::ALL.len(), 7);
    }

    /// The map is a persisted document, so its round trip is part of its
    /// contract, and a binding written with nothing but a source and a target
    /// reads back as one with the learn defaults.
    #[test]
    fn a_binding_round_trips_and_defaults_its_optional_halves() {
        let mut map = ControlMap::default();
        map.bind(param_binding(74));
        let text = toml::to_string(&map).expect("a map serializes");
        let back: ControlMap = toml::from_str(&text).expect("and reads back");
        assert_eq!(back, map);

        let sparse: ControlBinding = toml::from_str(
            r#"
            target = { param = { scope = { channel = 0 }, owner = { effect = { device = 0 } }, param = 3 } }
            [source.cc]
            controller = 74
            "#,
        )
        .expect("a binding needs only a source and a target");
        assert_eq!(sparse, param_binding(74));
    }
}
