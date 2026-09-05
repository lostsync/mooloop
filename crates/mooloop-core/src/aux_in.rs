//! Aux In: a channel whose sound is another channel's published audio outlet.
//!
//! The consumer half of `docs/plans/typed-audio-edges/`. It takes a channel's
//! generator slot like any other source, and what it renders is the samples a
//! producer wrote into a tap this same block -- not the block before, which is
//! why the compiled order in [`crate::compile_audio_graph`] exists.
//!
//! **It is an input, not a send.** The producing channel does not know it is
//! being read, its own output is unchanged, and nothing about its routing
//! moves. Two Aux Ins may read the same outlet and neither affects the other.
//!
//! **It is not a router.** One subscription, one channel, one outlet. A device
//! that could sum four taps would be a mixer, and what makes this worth
//! building is the edge underneath it rather than the breadth of the device on
//! top.

use crate::effect::{ParamCurve, ParamDescriptor};
use crate::outlet::{AudioSubscription, OutletDescriptor, OutletTap};
use crate::MAX_CHANNELS;

/// `Event::ParamValue` ids. Frozen from here: an automation lane persists
/// them.
pub const PARAM_SOURCE_CHANNEL: u32 = 0;
pub const PARAM_SOURCE_OUTLET: u32 = 1;
pub const PARAM_LEVEL: u32 = 2;

/// The Source Channel value that means "nothing is subscribed".
///
/// A position on the same selector rather than a separate switch, because
/// "which channel" and "whether any channel" are one question to the person
/// turning it, and two controls that can disagree is how an Aux In ends up
/// pointing at a channel it is not reading.
pub const NO_SOURCE: f32 = -1.0;

/// The highest outlet id an Aux In can name.
///
/// A limit on the *selector*, not on what a device may publish: the id is
/// carried as a stepped parameter so it can be automated and addressed like
/// every other value, and a stepped parameter declares its positions up
/// front. Sixty-four is eight times the widest table anybody has declared, and
/// `every_declared_audio_outlet_is_reachable` fails the day that stops being
/// true rather than leaving an outlet quietly unnameable.
pub const MAX_SOURCE_OUTLET: u16 = 63;

/// Where a subscription is pointed when the channel it named is deleted.
///
/// Retained rather than cleared, which is what `04-the-aux-in-device.md` asks
/// for -- "leaves the subscription inspectable and the consumer silent,
/// rather than pointing at whichever channel inherited the index". The last
/// addressable index is the marker because it is the one a project almost
/// never holds, so the compiler refuses the edge as `NoSuchChannel` and the
/// face has something honest to draw. (A song that genuinely holds all
/// [`MAX_CHANNELS`] would resolve it; nothing else can, and a 256-channel
/// project cannot grow into that state, only shrink out of it.)
pub const DEPARTED_SOURCE: i16 = u8::MAX as i16;

/// Aux In's own published outlet: what it renders, before the channel's
/// effect chain.
///
/// It publishes so that Aux Ins can feed each other. That is worth having on
/// its own -- one tap reaching two chains through a passthrough is an obvious
/// thing to want -- and it is also what makes a ring of edges constructible at
/// all, which is what `compile_audio_graph`'s cycle refusal is for. A consumer
/// that could never be a producer would have left that refusal unreachable
/// from the program and testable only in the abstract.
pub const OUTLET_OUT: u16 = 0;

pub static OUTLETS: [OutletDescriptor; 1] =
    [OutletDescriptor::audio(OUTLET_OUT, "Out", OutletTap::PreChain)];

/// One Aux In's authored state.
///
/// The subscription lives here rather than in a routing table of its own, for
/// the reason a modulation route's destination does: the thing that owns the
/// subscription is the thing that would be meaningless without it, so a
/// channel that changes its generator kind cannot leave a stranded edge
/// behind.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AuxInParams {
    /// The producing channel, or negative for none.
    ///
    /// Signed and wider than the `u8` a channel index is, because the
    /// selector's "none" position has to be a value of the same parameter.
    #[serde(default = "no_source_channel")]
    pub source_channel: i16,
    /// Which of that channel's audio outlets, by its durable device-interface
    /// id.
    ///
    /// Kept when the source channel is set to none, so turning a source off
    /// and back on returns to the outlet it was reading rather than to the
    /// first one in the table.
    #[serde(default)]
    pub source_outlet: u16,
    /// Linear output level.
    #[serde(default = "default_level")]
    pub level: f32,
}

fn no_source_channel() -> i16 {
    -1
}

/// An Aux In plays whatever somebody else published, so it is by definition
/// uncalibrated -- the same position the sampler is in with an arbitrary
/// file. It spends headroom the same way rather than measuring: a pre-level
/// oscillator tap arrives at full scale, and this puts it where a calibrated
/// generator's output sits. See `docs/GAIN_STRUCTURE.md`.
fn default_level() -> f32 {
    crate::gain::reference_level_gain()
}

impl Default for AuxInParams {
    fn default() -> Self {
        Self {
            source_channel: no_source_channel(),
            source_outlet: 0,
            level: default_level(),
        }
    }
}

impl AuxInParams {
    /// The edge this Aux In is asking for, if it is asking for one.
    ///
    /// Whether it *resolves* is `compile_audio_graph`'s answer, not this
    /// one: a subscription naming a departed channel is still authored, and
    /// keeping the two questions apart is what lets a refusal stay
    /// inspectable instead of being erased on the way in.
    pub fn subscription(&self) -> Option<AudioSubscription> {
        let channel = u8::try_from(self.source_channel).ok()?;
        Some(AudioSubscription::new(channel, self.source_outlet))
    }

    /// Follow a channel edit.
    ///
    /// A subscription is a channel-scoped address like a route's destination
    /// or an automation lane's target, and it moves the same way: indices
    /// after the edit renumber, and the one that named the deleted channel is
    /// sent to [`DEPARTED_SOURCE`] rather than left to inherit whichever
    /// channel closed the gap. Returns whether anything moved.
    pub fn rescope(&mut self, edit: crate::structure::ChannelEdit) -> bool {
        let Ok(channel) = u8::try_from(self.source_channel) else {
            return false;
        };
        let moved = match edit.channel(channel) {
            Some(channel) => i16::from(channel),
            None => DEPARTED_SOURCE,
        };
        let changed = moved != self.source_channel;
        self.source_channel = moved;
        changed
    }

    /// Point this Aux In at an outlet, or at nothing.
    pub fn set_subscription(&mut self, subscription: Option<AudioSubscription>) {
        match subscription {
            Some(subscription) => {
                self.source_channel = i16::from(subscription.channel);
                self.source_outlet = subscription.outlet;
            }
            None => self.source_channel = -1,
        }
    }
}

pub static DESCRIPTORS: [ParamDescriptor; 3] = [
    ParamDescriptor {
        id: PARAM_SOURCE_CHANNEL,
        name: "Source",
        unit: "",
        min: NO_SOURCE,
        max: (MAX_CHANNELS - 1) as f32,
        // Stepped, so it is modulation-ineligible without anybody declaring
        // it so: this parameter recompiles the render schedule, and a route
        // onto it would mean doing that from the audio thread.
        curve: ParamCurve::Stepped(MAX_CHANNELS as u16 + 1),
        default: NO_SOURCE,
    },
    ParamDescriptor {
        id: PARAM_SOURCE_OUTLET,
        name: "Outlet",
        unit: "",
        min: 0.0,
        max: MAX_SOURCE_OUTLET as f32,
        curve: ParamCurve::Stepped(MAX_SOURCE_OUTLET + 1),
        default: 0.0,
    },
    ParamDescriptor {
        id: PARAM_LEVEL,
        name: "Level",
        // Linear rather than the knob's dB taper, for the reason the
        // sampler's Output gives: modulation depth is a fraction of the
        // normalized range, and the taper belongs to the control surface
        // rather than to the destination's numeric truth.
        unit: "x",
        min: 0.0,
        max: crate::gain::MAX_LINEAR_GAIN,
        curve: ParamCurve::Linear,
        default: 0.355_234_4,
    },
];

pub fn descriptor(id: u32) -> Option<&'static ParamDescriptor> {
    DESCRIPTORS.iter().find(|descriptor| descriptor.id == id)
}

pub fn get(params: &AuxInParams, id: u32) -> Option<f32> {
    Some(match id {
        PARAM_SOURCE_CHANNEL => f32::from(params.source_channel),
        PARAM_SOURCE_OUTLET => f32::from(params.source_outlet),
        PARAM_LEVEL => params.level,
        _ => return None,
    })
}

pub fn set(params: &mut AuxInParams, id: u32, value: f32) -> bool {
    match id {
        PARAM_SOURCE_CHANNEL => {
            let index = value.round().clamp(NO_SOURCE, (MAX_CHANNELS - 1) as f32);
            params.source_channel = index as i16;
        }
        PARAM_SOURCE_OUTLET => {
            let outlet = value.round().clamp(0.0, MAX_SOURCE_OUTLET as f32);
            params.source_outlet = outlet as u16;
        }
        PARAM_LEVEL => params.level = value.clamp(0.0, crate::gain::MAX_LINEAR_GAIN),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outlet::OutletDomain;
    use crate::{DeviceKind, PublishesOutlets};

    #[test]
    fn the_outlet_table_is_one_audio_port_upstream_of_the_chain() {
        crate::outlet::tests::check_table(&OUTLETS);
        assert_eq!(crate::outlet::control_count(&OUTLETS), 0);
        let out = crate::outlet::find(&OUTLETS, OUTLET_OUT).expect("Aux In publishes Out");
        assert_eq!(out.domain, OutletDomain::Audio);
        assert!(
            out.tap.is_upstream_of_chain(),
            "an Aux In's own output would arrive late"
        );
    }

    /// The selector has to be able to name anything anybody publishes, and
    /// every channel a project can hold. Both bounds are declared numbers
    /// rather than derived ones, so this is what notices when one moves.
    #[test]
    fn every_declared_audio_outlet_is_reachable() {
        let channel = descriptor(PARAM_SOURCE_CHANNEL).unwrap();
        assert_eq!(channel.from_normalized(0.0), NO_SOURCE);
        assert_eq!(channel.from_normalized(1.0), (MAX_CHANNELS - 1) as f32);
        // Every position lands exactly on an integer channel index: the
        // parameter's value *is* the index, so there is no index-to-value
        // mapping for a face to keep a second copy of.
        for index in -1..MAX_CHANNELS as i32 {
            let natural = index as f32;
            let round_trip = channel.from_normalized(channel.to_normalized(natural));
            assert_eq!(round_trip, natural, "channel {index} is not addressable");
        }

        let outlet = descriptor(PARAM_SOURCE_OUTLET).unwrap();
        for kind in [DeviceKind::MlP8, DeviceKind::Ds01, DeviceKind::AuxIn] {
            for published in kind.outlets().iter().filter(|o| !o.is_control()) {
                assert!(
                    published.id <= MAX_SOURCE_OUTLET,
                    "{kind:?}'s {} cannot be named by an Aux In",
                    published.name
                );
                let natural = f32::from(published.id);
                assert_eq!(
                    outlet.from_normalized(outlet.to_normalized(natural)),
                    natural
                );
            }
        }
    }

    /// The two structural parameters must not be modulation destinations:
    /// resolving a route onto one would mean recompiling the render schedule
    /// from the audio thread. They get that from being stepped rather than
    /// from a rule somebody remembered to write, which is the point.
    #[test]
    fn only_level_takes_modulation() {
        use crate::mod_metadata::ModDestinationDescriptor;
        for descriptor in DESCRIPTORS.iter() {
            let policy = ModDestinationDescriptor::for_param(descriptor);
            assert_eq!(
                policy.allowed,
                descriptor.id == PARAM_LEVEL,
                "{} has the wrong modulation policy",
                descriptor.name
            );
        }
    }

    #[test]
    fn a_fresh_aux_in_subscribes_to_nothing_and_is_silent_rather_than_invalid() {
        let params = AuxInParams::default();
        assert!(params.subscription().is_none());
        assert_eq!(params.level, crate::gain::reference_level_gain());
    }

    /// Turning the source off keeps the outlet, so turning it back on returns
    /// to what was being read rather than to the top of the table.
    #[test]
    fn clearing_the_source_keeps_the_outlet_it_was_reading() {
        let mut params = AuxInParams::default();
        params.set_subscription(Some(AudioSubscription::new(3, 9)));
        assert_eq!(params.subscription(), Some(AudioSubscription::new(3, 9)));
        params.set_subscription(None);
        assert!(params.subscription().is_none());
        assert_eq!(params.source_outlet, 9);
        set(&mut params, PARAM_SOURCE_CHANNEL, 3.0);
        assert_eq!(params.subscription(), Some(AudioSubscription::new(3, 9)));
    }

    /// The one the plan names: deleting the producing channel must not hand
    /// the subscription to whichever channel inherited the index.
    #[test]
    fn a_departed_source_is_retained_rather_than_inherited() {
        use crate::structure::ChannelEdit;
        let mut params = AuxInParams::default();
        params.set_subscription(Some(AudioSubscription::new(3, 9)));

        // A channel deleted below it: the edge follows its producer down.
        let mut moved = params;
        assert!(moved.rescope(ChannelEdit::Removed(1)));
        assert_eq!(moved.subscription(), Some(AudioSubscription::new(2, 9)));

        // A channel inserted below it: back up again.
        assert!(moved.rescope(ChannelEdit::Inserted(0)));
        assert_eq!(moved.subscription(), Some(AudioSubscription::new(3, 9)));

        // The producer itself deleted: not channel 3, which is now somebody
        // else, and not silently forgotten either.
        let mut orphaned = params;
        assert!(orphaned.rescope(ChannelEdit::Removed(3)));
        assert_eq!(orphaned.source_channel, DEPARTED_SOURCE);
        assert_eq!(orphaned.source_outlet, 9);
        assert_ne!(orphaned.subscription(), Some(AudioSubscription::new(3, 9)));

        // And an Aux In subscribed to nothing is unmoved by any of it.
        let mut idle = AuxInParams::default();
        assert!(!idle.rescope(ChannelEdit::Removed(0)));
        assert!(idle.subscription().is_none());
    }

    #[test]
    fn every_parameter_reads_back_what_it_was_written() {
        let mut params = AuxInParams::default();
        for (id, value) in [
            (PARAM_SOURCE_CHANNEL, 17.0),
            (PARAM_SOURCE_OUTLET, 12.0),
            (PARAM_LEVEL, 0.5),
        ] {
            assert!(set(&mut params, id, value));
            assert_eq!(get(&params, id), Some(value));
        }
        assert!(!set(&mut params, 99, 0.0));
        assert!(get(&params, 99).is_none());
    }
}
