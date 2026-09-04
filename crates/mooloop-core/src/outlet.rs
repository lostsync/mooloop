//! What a device publishes, as opposed to what it accepts.
//!
//! A device's parameters are its inlets: `ParamDescriptor` says how a value
//! maps, and `ModDestinationDescriptor` says whether modulation may reach it.
//! This module is the other direction — the named signals a device offers to
//! the rest of the program, per `COMPOSABLE_DEVICE_UNITS.md` ("Inlets and
//! outlets are designed, not inferred") and `MODULATOR_SYSTEM_SPEC.md`'s
//! source table, where `Generator outlet` and `Device outlet` have been listed
//! as planned since the spec was written.
//!
//! Three rules the rest of the program depends on, and which are the reason
//! this is a declaration rather than something inferred from a device's
//! fields:
//!
//! - **An outlet is designed, not discovered.** Nothing here is derived from
//!   implementation structure. A device publishes the signals that are useful
//!   to somebody else, and keeps the rest private, so refactoring a voice does
//!   not silently change what a project can route.
//! - **An outlet is not telemetry.** Meters, plots and waveform displays are
//!   best-effort observation and may be sampled, dropped or smoothed. An
//!   outlet has a declared range, rate and latency, and drives parameters.
//!   The two must never be the same value read twice.
//! - **A control outlet and an audio outlet are different things.** A control
//!   destination does not accept `Osc 1` merely because both are numeric
//!   samples; crossing that boundary takes an explicit adapter, such as an
//!   envelope follower. [`OutletDomain`] is what makes the refusal
//!   structural rather than a rule somebody has to remember.
//!
//! Outlet ids are device-interface identifiers, like parameter ids: a project
//! that saves a route to outlet 3 must find the same signal there next time.
//! They are never renumbered, and a retired outlet leaves its id spent.

use crate::mod_metadata::{ControlLatency, ControlRate, SignalShape};

/// Whether an outlet carries a control signal or audio.
///
/// The distinction is load-bearing rather than descriptive. Control outlets
/// go through the per-channel control table at its declared rate and latency;
/// audio outlets need the typed auxiliary audio edges
/// `AUDIO_ARCHITECTURE.md` describes, and downsampling one into the control
/// table would destroy the thing that makes it worth publishing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutletDomain {
    Control,
    Audio,
}

/// Where inside the device an audio outlet is tapped.
///
/// Published as user-facing status text, because the surprising cases are the
/// useful ones: a pre-level tap keeps signalling while its source is muted in
/// the device's own mix, which is exactly what makes a silent internal
/// modulator available to somebody else. A tap that surprises a user once is
/// a tap that gets mistrusted forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutletTap {
    /// Not an audio tap: the outlet is a control signal.
    Control,
    /// Before the source's own level control, so a source muted in the
    /// device's mix still publishes.
    PreLevel,
    /// After the source mix and the drive, before the filter.
    PreFilter,
    /// After the filter, before the amplifier.
    PreVca,
    /// The device's finished output.
    Output,
}

impl OutletTap {
    /// The status text a port surface shows. Empty for control outlets,
    /// which have a rate and a range to show instead of a tap point.
    pub fn status(self) -> &'static str {
        match self {
            Self::Control => "",
            Self::PreLevel => "pre-level",
            Self::PreFilter => "pre-filter",
            Self::PreVca => "pre-vca",
            Self::Output => "output",
        }
    }
}

/// One published signal: everything a consumer needs to decide whether it can
/// use this outlet, and what it will get if it does.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutletDescriptor {
    /// Stable within the publishing device's interface. Never renumbered:
    /// a saved route names this.
    pub id: u16,
    pub name: &'static str,
    pub domain: OutletDomain,
    pub signal: SignalShape,
    pub update: ControlRate,
    pub latency: ControlLatency,
    pub tap: OutletTap,
}

impl OutletDescriptor {
    /// A control outlet: one value per block, read one block later.
    ///
    /// The latency is not a property of any one device. It is the rule from
    /// `MODULATOR_SYSTEM_SPEC.md` that makes realtime and offline renders
    /// identical and stops graph order deciding what a route hears, so every
    /// control outlet declares it and a consumer never has to ask which
    /// devices are the punctual ones.
    ///
    /// The *rate* is a choice, and it is [`ControlRate::PerBlock`] rather
    /// than the 32-frame tick a local modulator runs at. A device reduces
    /// per-voice state into one published number at the end of its block;
    /// publishing per tick would mean rendering the device in 32-frame pieces
    /// so the reduction had somewhere to happen, and storing a tick's worth
    /// of every outlet against the chance somebody reads it. Neither is worth
    /// paying before a musical case asks for it, and declaring the coarser
    /// rate honestly is what leaves that upgrade open: a consumer that has
    /// been told `PerBlock` cannot come to depend on more.
    pub const fn control(id: u16, name: &'static str, signal: SignalShape) -> Self {
        Self {
            id,
            name,
            domain: OutletDomain::Control,
            signal,
            update: ControlRate::PerBlock,
            latency: ControlLatency::OUTLET,
            tap: OutletTap::Control,
        }
    }

    /// An audio outlet, at the sample rate, tapped at a declared point.
    ///
    /// Latency is zero because an audio edge carries the samples themselves
    /// rather than a reduction of them; what it costs instead is a typed edge
    /// that does not exist yet, which is why these are declared before they
    /// are connectable.
    pub const fn audio(id: u16, name: &'static str, tap: OutletTap) -> Self {
        Self {
            id,
            name,
            domain: OutletDomain::Audio,
            signal: SignalShape::Bipolar,
            update: ControlRate::Manual,
            latency: ControlLatency::IMMEDIATE,
            tap,
        }
    }

    /// Whether this outlet can drive a control destination.
    ///
    /// The one question a route surface has to ask, and the reason it is a
    /// method rather than a field comparison at every call site.
    pub fn is_control(&self) -> bool {
        matches!(self.domain, OutletDomain::Control)
    }
}

/// A device's published interface: its outlets, in the order a picker lists
/// them.
///
/// Control outlets come first because they are the ones that can be connected
/// today; a surface that only knows about control signals can take the
/// leading run and stop.
pub trait PublishesOutlets {
    fn outlets(&self) -> &'static [OutletDescriptor];
}

/// Look one outlet up by its durable id.
pub fn find(outlets: &'static [OutletDescriptor], id: u16) -> Option<&'static OutletDescriptor> {
    outlets.iter().find(|outlet| outlet.id == id)
}

/// How many of `outlets` are control outlets, given that they are declared
/// control-first.
pub fn control_count(outlets: &[OutletDescriptor]) -> usize {
    outlets.iter().take_while(|outlet| outlet.is_control()).count()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every published table has to satisfy these, whichever device wrote it.
    /// Called by each device's own outlet test rather than by a registry, so
    /// a new device gets the check by using the vocabulary.
    pub fn check_table(outlets: &'static [OutletDescriptor]) {
        for (index, outlet) in outlets.iter().enumerate() {
            for other in &outlets[index + 1..] {
                assert_ne!(outlet.id, other.id, "duplicate outlet id {}", outlet.id);
                assert_ne!(
                    outlet.name, other.name,
                    "two outlets named {}",
                    outlet.name
                );
            }
            assert!(!outlet.name.is_empty(), "outlet {} has no name", outlet.id);
            // The domain decides the rest of the declaration, so a table that
            // disagrees with itself is caught here rather than by a consumer
            // that trusted one field and not the other.
            match outlet.domain {
                OutletDomain::Control => {
                    assert_eq!(outlet.tap, OutletTap::Control, "{} taps audio", outlet.name);
                    assert_eq!(
                        outlet.latency,
                        ControlLatency::OUTLET,
                        "{} does not declare the one-block publish rule",
                        outlet.name
                    );
                }
                OutletDomain::Audio => {
                    assert_ne!(
                        outlet.tap,
                        OutletTap::Control,
                        "{} is audio but declares no tap point",
                        outlet.name
                    );
                    assert!(
                        !outlet.tap.status().is_empty(),
                        "{} has no status text to show",
                        outlet.name
                    );
                }
            }
        }
        // Control-first, so a control-only surface can take a prefix.
        let control = control_count(outlets);
        assert!(
            outlets[control..].iter().all(|outlet| !outlet.is_control()),
            "control and audio outlets are interleaved"
        );
    }

    #[test]
    fn a_control_outlet_declares_the_one_block_rule_without_being_asked() {
        let outlet = OutletDescriptor::control(0, "Gate", SignalShape::Gate);
        assert_eq!(outlet.latency, ControlLatency::OUTLET);
        assert_eq!(outlet.update, ControlRate::PerBlock);
        assert!(outlet.is_control());
        assert_eq!(outlet.tap.status(), "");
    }

    /// An audio outlet is not a control signal that happens to be fast. The
    /// domain is what a route surface asks, and it has to refuse.
    #[test]
    fn an_audio_outlet_never_reads_as_a_control_signal() {
        let outlet = OutletDescriptor::audio(1, "Osc 1", OutletTap::PreLevel);
        assert!(!outlet.is_control());
        assert_eq!(outlet.tap.status(), "pre-level");
    }

    #[test]
    fn find_resolves_by_durable_id_and_misses_cleanly() {
        static TABLE: [OutletDescriptor; 2] = [
            OutletDescriptor::control(0, "LFO", SignalShape::Bipolar),
            OutletDescriptor::audio(9, "Filter", OutletTap::PreVca),
        ];
        assert_eq!(find(&TABLE, 9).unwrap().name, "Filter");
        assert!(find(&TABLE, 1).is_none());
        assert_eq!(control_count(&TABLE), 1);
    }
}
