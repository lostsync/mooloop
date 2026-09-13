//! JACK adapter: a client with two audio outputs and one MIDI input, whose
//! process callback hands its buffers to the shared [`Executor`].
//!
//! Works against pipewire-jack transparently.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use jack::{
    AudioOut, Client, ClientOptions, Control, MidiIn, Port, PortId, ProcessHandler, ProcessScope,
};

use crate::driver::{AudioConfig, OutputTarget};
use crate::executor::Executor;
use crate::Error;

const CLIENT_NAME: &str = "mooloop";
const OUT_L_NAME: &str = "mooloop:out_l";
const OUT_R_NAME: &str = "mooloop:out_r";
const DEFAULT_OUTPUT_L: &str = "system:playback_1";
/// JACK's built-in audio port type, as `Client::ports` wants it. Named here
/// rather than spelled at the call site because a typo in it silently matches
/// nothing rather than failing.
const AUDIO_PORT_TYPE: &str = "32 bit float mono audio";
const DEFAULT_OUTPUT_R: &str = "system:playback_2";

struct Graph {
    executor: Executor,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    midi_in: Port<MidiIn>,
}

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        // JACK hands over whole messages already ordered by time, which is
        // the executor's contract for its MIDI input.
        let midi = self.midi_in.iter(scope).map(|raw| (raw.time, raw.bytes));
        self.executor.process(
            midi,
            self.out_l.as_mut_slice(scope),
            self.out_r.as_mut_slice(scope),
        );
        Control::Continue
    }
}

struct Notifications {
    xrun_count: Arc<AtomicU64>,
    /// Whether to retry connecting `target` when the port graph changes and
    /// it is currently unconnected. Shared with `JackDriver::set_auto_reconnect`.
    auto_reconnect: Arc<AtomicBool>,
    /// The configured output target, shared with `JackDriver`.
    target: Arc<ArcSwap<(String, String)>>,
}

impl jack::NotificationHandler for Notifications {
    fn xrun(&mut self, _: &Client) -> Control {
        self.xrun_count.fetch_add(1, Ordering::Relaxed);
        Control::Continue
    }

    // Runs on JACK's notification thread, not the realtime audio thread, so
    // ordinary allocation and the `ArcSwap` load below are fine here. A
    // hot-plugged device (e.g. headphones) surfaces to a JACK client as
    // ports registering, not as a "default device changed" event, so port
    // registration is what auto-reconnect actually watches.
    fn port_registration(&mut self, client: &Client, _port_id: PortId, is_registered: bool) {
        if !is_registered || !self.auto_reconnect.load(Ordering::Relaxed) {
            return;
        }
        let target = self.target.load_full();
        if client.port_by_name(&target.0).is_none() || client.port_by_name(&target.1).is_none() {
            return;
        }
        for (src, dst) in [
            (OUT_L_NAME, target.0.as_str()),
            (OUT_R_NAME, target.1.as_str()),
        ] {
            match client.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                // JACK's graph-change notification, not the process callback:
                // formatting and locking are both fine here.
                Err(e) => mooloop_core::log_warn!(
                    "audio",
                    "auto-reconnect could not connect {src} -> {dst} ({e})"
                ),
            }
        }
    }
}

type AsyncClient = jack::AsyncClient<Notifications, Graph>;

/// A JACK client that is open but not yet running: enough to learn the sample
/// rate the render state has to be built for.
pub(crate) struct Opening {
    client: Client,
}

impl Opening {
    pub(crate) fn connect() -> Result<Self, Error> {
        let (client, _status) = Client::new(CLIENT_NAME, ClientOptions::NO_START_SERVER)
            .map_err(|e| Error::ClientOpen(e.to_string()))?;
        Ok(Self { client })
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.client.sample_rate()
    }

    /// Register the ports, activate the client around `executor`, and wire
    /// the outputs to the configured target.
    pub(crate) fn start(
        self,
        executor: Executor,
        xrun_count: Arc<AtomicU64>,
        config: AudioConfig,
    ) -> Result<JackDriver, Error> {
        let client = self.client;
        if let Some(frames) = config.buffer_size {
            if let Err(e) = client.set_buffer_size(frames) {
                mooloop_core::log_warn!(
                    "audio",
                    "could not set JACK buffer size to {frames} frames ({e}); \
                     leaving the server's current buffer size in place"
                );
            }
        }

        let out_l = client
            .register_port("out_l", AudioOut::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        let out_r = client
            .register_port("out_r", AudioOut::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        // One input for now. Per-channel MIDI routing is a later concern;
        // what the control layer needs first is any way in at all.
        let midi_in = client
            .register_port("midi_in", MidiIn::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        let graph = Graph {
            executor,
            out_l,
            out_r,
            midi_in,
        };

        let target = config
            .output_target
            .unwrap_or_else(|| (DEFAULT_OUTPUT_L.to_owned(), DEFAULT_OUTPUT_R.to_owned()));
        let output_target = Arc::new(ArcSwap::from_pointee(target.clone()));
        let auto_reconnect = Arc::new(AtomicBool::new(config.auto_reconnect));

        let async_client = client
            .activate_async(
                Notifications {
                    xrun_count,
                    auto_reconnect: auto_reconnect.clone(),
                    target: output_target.clone(),
                },
                graph,
            )
            .map_err(|e| Error::Activate(e.to_string()))?;

        // Best-effort: wire our outputs to the configured target so the app is
        // audible out of the box. Auto-reconnect (if enabled) picks this back
        // up whenever the JACK graph changes and this connection is missing.
        let c = async_client.as_client();
        let sources = [OUT_L_NAME, OUT_R_NAME];
        let destinations = [target.0.as_str(), target.1.as_str()];
        let mut connected = true;
        for (src, dst) in sources.iter().zip(destinations.iter()) {
            match c.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                Err(_) => connected = false,
            }
        }
        // A saved destination outlives the thing it names. A device is
        // unplugged, a profile changes, the audio server is restarted and
        // renames its nodes -- and the target recorded in settings then
        // matches nothing. Connecting to nothing is the one outcome with no
        // symptom: the engine runs, the meters move, the transport rolls, and
        // there is silence with nothing on screen to say why.
        //
        // So take any working stereo destination rather than none, and say so.
        // A wrong output is audible and one click from right in Preferences;
        // no output is a bug report.
        if !connected {
            for (src, dst) in sources.iter().zip(destinations.iter()) {
                let _ = c.disconnect_ports_by_name(src, dst);
            }
            // In order, not just the first: a candidate can be present in the
            // graph and still refuse the connection, and stopping at one would
            // leave the silence this exists to prevent.
            let available = stereo_destinations(c);
            let landed = fallback_destinations(&available, &target).find(|candidate| {
                let pair = [candidate.port_l.as_str(), candidate.port_r.as_str()];
                let mut ok = true;
                for (src, dst) in sources.iter().zip(pair.iter()) {
                    match c.connect_ports_by_name(src, dst) {
                        Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                        Err(_) => ok = false,
                    }
                }
                if !ok {
                    for (src, dst) in sources.iter().zip(pair.iter()) {
                        let _ = c.disconnect_ports_by_name(src, dst);
                    }
                }
                ok
            });
            match landed {
                Some(fallback) => {
                    output_target
                        .store(Arc::new((fallback.port_l.clone(), fallback.port_r.clone())));
                    mooloop_core::log_warn!(
                        "audio",
                        "the saved audio output {:?} is not available; connected to {:?} \
                         instead. Preferences -> Audio picks a different one",
                        target.0,
                        fallback.client
                    );
                }
                None => mooloop_core::log_warn!(
                    "audio",
                    "the saved audio output {:?} is not available and nothing else accepted \
                     a connection; connect mooloop manually in a patchbay \
                     (e.g. qpwgraph, qjackctl, Helvum)",
                    target.0
                ),
            }
        }

        Ok(JackDriver {
            client: async_client,
            output_target,
            auto_reconnect,
        })
    }
}

/// The running JACK client. Dropping it deactivates audio.
pub(crate) struct JackDriver {
    client: AsyncClient,
    output_target: Arc<ArcSwap<(String, String)>>,
    auto_reconnect: Arc<AtomicBool>,
}

impl JackDriver {
    pub(crate) fn available_output_targets(&self) -> Vec<OutputTarget> {
        stereo_destinations(self.client.as_client())
    }

    /// Disconnect the previously configured output target (if connected) and
    /// connect the new one, or the system default if `target` is `None`.
    pub(crate) fn set_output_target(&self, target: Option<(String, String)>) -> Result<(), String> {
        let jack_client = self.client.as_client();
        let previous = self.output_target.load_full();
        let next =
            target.unwrap_or_else(|| (DEFAULT_OUTPUT_L.to_owned(), DEFAULT_OUTPUT_R.to_owned()));
        if *previous != next {
            let _ = jack_client.disconnect_ports_by_name(OUT_L_NAME, &previous.0);
            let _ = jack_client.disconnect_ports_by_name(OUT_R_NAME, &previous.1);
        }
        for (src, dst) in [(OUT_L_NAME, &next.0), (OUT_R_NAME, &next.1)] {
            match jack_client.connect_ports_by_name(src, dst) {
                Ok(()) | Err(jack::Error::PortAlreadyConnected(_, _)) => {}
                Err(e) => return Err(format!("could not connect {src} to {dst}: {e}")),
            }
        }
        self.output_target.store(Arc::new(next));
        Ok(())
    }

    /// Server-wide: this changes the buffer for every JACK client connected,
    /// not only mooloop.
    pub(crate) fn set_buffer_size(&self, frames: u32) -> Result<(), String> {
        self.client
            .as_client()
            .set_buffer_size(frames)
            .map_err(|e| format!("could not change the JACK buffer size: {e}"))
    }

    /// Retry the configured output target when the JACK port graph changes
    /// and the target is currently unconnected.
    pub(crate) fn set_auto_reconnect(&self, enabled: bool) {
        self.auto_reconnect.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn buffer_size(&self) -> u32 {
        self.client.as_client().buffer_size()
    }

    pub(crate) fn current_target(&self) -> (String, String) {
        (*self.output_target.load_full()).clone()
    }

    /// Nothing to do: JACK's notification thread reconnects on the graph
    /// change that makes it possible, where Core Audio needs the control
    /// thread to look.
    pub(crate) fn service(&self) {}
}

/// JACK input ports grouped by owning client, as candidate output
/// destinations. A non-realtime JACK graph query.
///
/// Shared by the preferences page and by startup's fallback, so that what the
/// engine reaches for when a saved target has gone is exactly what the
/// interface would have offered.
fn stereo_destinations(jack_client: &Client) -> Vec<OutputTarget> {
    // Audio inputs only. Unfiltered, this returns MIDI destinations too --
    // a machine with `Midi-Bridge` on the graph offers it as an output pair,
    // and connecting an audio port to it simply fails. That was survivable
    // while the list only populated a menu a human read; it is not, now that
    // the fallback below picks from it without asking.
    let ports = jack_client.ports(
        None,
        Some(AUDIO_PORT_TYPE),
        jack::PortFlags::IS_INPUT,
    );
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for port in ports {
        let Some((client_name, _)) = port.split_once(':') else {
            continue;
        };
        match grouped.iter_mut().find(|(name, _)| name == client_name) {
            Some((_, ports)) => ports.push(port),
            None => grouped.push((client_name.to_owned(), vec![port])),
        }
    }
    grouped
        .into_iter()
        .filter_map(|(client, ports)| {
            let mut ports = ports.into_iter();
            let port_l = ports.next()?;
            let port_r = ports.next()?;
            Some(OutputTarget {
                client,
                port_l,
                port_r,
            })
        })
        .collect()
}

/// Which destinations to try when the saved one does not exist, in order.
///
/// Split from the JACK calls around it because this is the only part with a
/// decision in it, and a decision that cannot be tested without an audio
/// server is a decision nobody will revisit.
///
/// The rule is deliberately dull: the first destination that is not the one
/// already tried, and not mooloop itself. Ranking outputs by desirability --
/// preferring speakers over HDMI, say -- is a guess about a machine this code
/// cannot see, and being audible somewhere is the whole of what is wanted
/// here. Preferences owns the actual choice.
fn fallback_destinations<'a>(
    available: &'a [OutputTarget],
    tried: &'a (String, String),
) -> impl Iterator<Item = &'a OutputTarget> + 'a {
    available.iter().filter(move |candidate| {
        candidate.client != CLIENT_NAME
            && candidate.port_l != tried.0
            && candidate.port_r != tried.1
    })
}

#[cfg(test)]
mod output_fallback_tests {
    use super::{fallback_destinations, OutputTarget, CLIENT_NAME};

    fn target(client: &str) -> OutputTarget {
        OutputTarget {
            client: client.to_owned(),
            port_l: format!("{client}:playback_FL"),
            port_r: format!("{client}:playback_FR"),
        }
    }

    fn clients(available: &[OutputTarget], tried: &(String, String)) -> Vec<String> {
        fallback_destinations(available, tried)
            .map(|t| t.client.clone())
            .collect()
    }

    fn gone() -> (String, String) {
        ("gone:playback_FL".into(), "gone:playback_FR".into())
    }

    /// The ordinary case: a saved output that no longer exists, and a machine
    /// that has something else to offer.
    #[test]
    fn a_missing_output_falls_back_to_whatever_is_there() {
        let available = vec![target("hdmi"), target("speaker")];
        assert_eq!(clients(&available, &gone()), ["hdmi", "speaker"]);
    }

    /// Every candidate is offered, not just the first. A destination can be
    /// in the graph and still refuse the connection, and the caller walks this
    /// until one accepts.
    #[test]
    fn every_candidate_is_offered_in_order() {
        let available = vec![target("a"), target("b"), target("c")];
        assert_eq!(clients(&available, &gone()), ["a", "b", "c"]);
    }

    /// The destination that was already tried is not a fallback for itself.
    /// Reaching here means connecting to it failed, and the identical pair
    /// would fail the same way.
    #[test]
    fn the_destination_that_just_failed_is_not_offered_again() {
        let available = vec![target("headphones"), target("speaker")];
        let tried = (
            "headphones:playback_FL".into(),
            "headphones:playback_FR".into(),
        );
        assert_eq!(clients(&available, &tried), ["speaker"]);
    }

    /// Mooloop's own input ports are in the graph like anyone else's. Routing
    /// the master output back into the program would be a feedback loop
    /// arrived at by accident.
    #[test]
    fn mooloop_is_never_its_own_output() {
        let available = vec![target(CLIENT_NAME), target("speaker")];
        assert_eq!(clients(&available, &gone()), ["speaker"]);
    }

    /// A machine with nothing to play through gets the warning, not a panic
    /// and not a wrong guess.
    #[test]
    fn nothing_available_means_no_fallback() {
        assert!(clients(&[], &gone()).is_empty());
    }
}
