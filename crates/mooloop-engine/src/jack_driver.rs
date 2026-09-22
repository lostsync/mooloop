//! JACK adapter: a client with two audio outputs and one MIDI input, whose
//! process callback hands its buffers to the shared [`Executor`].
//!
//! Works against pipewire-jack transparently.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use jack::{
    AudioIn, AudioOut, Client, ClientOptions, Control, LatencyType, MidiIn, Port, PortFlags,
    PortId, ProcessHandler,
    ProcessScope,
};

use mooloop_core::{MidiPortId, MidiPortInfo};

use crate::driver::{remember_output, AudioConfig, OutputTarget};
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
const MIDI_IN_NAME: &str = "mooloop:midi_in";
/// What the one JACK input is called in the input picker.
///
/// **JACK gives mooloop one merged MIDI port**, with every hardware source
/// auto-connected to it, and a message arriving on it carries no record of
/// which source sent it. So there is exactly one port to pick here, and it is
/// named for what it actually is rather than for a device. Telling two
/// keyboards apart under JACK needs a port per source, which is a driver
/// change (`docs/CONTROL_SURFACES.md`); until then a two-keyboard setup is
/// separated by MIDI channel, which is what the channel filter is for.
///
/// The name itself lives in `lib.rs`, because the MIDI preferences page
/// explains this to the user and has to recognise the port to do it.
use crate::MERGED_MIDI_IN_LABEL as MIDI_IN_LABEL;
/// JACK's built-in MIDI port type, spelled here for the same reason as
/// [`AUDIO_PORT_TYPE`].
const MIDI_PORT_TYPE: &str = "8 bit raw midi";

struct Graph {
    executor: Executor,
    in_l: Port<AudioIn>,
    in_r: Port<AudioIn>,
    out_l: Port<AudioOut>,
    out_r: Port<AudioOut>,
    midi_in: Port<MidiIn>,
}

const IN_L_NAME: &str = "mooloop:in_l";

impl ProcessHandler for Graph {
    fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
        // JACK hands over whole messages already ordered by time, which is
        // the executor's contract for its MIDI input. One port, so every
        // message carries the same id; see `MIDI_IN_LABEL`.
        let midi = self
            .midi_in
            .iter(scope)
            .map(|raw| (MidiPortId::FIRST, raw.time, raw.bytes));
        self.executor.process_with_input(
            midi,
            self.in_l.as_slice(scope),
            self.in_r.as_slice(scope),
            self.out_l.as_mut_slice(scope),
            self.out_r.as_mut_slice(scope),
        );
        Control::Continue
    }
}

/// How long the port graph has to stay still before the output is checked.
///
/// A device arrives and leaves one port at a time. Checked mid-change, a
/// pair can have its left port and not yet its right, and the output would
/// go to whatever else is complete -- and then stay there, because a
/// connected output is not moved.
const GRAPH_SETTLE: Duration = Duration::from_millis(500);

/// How often the output is checked with no graph change to prompt it. A
/// notification can be missed, and a connection can fail without one; this
/// is what turns "connect it by hand in the patchbay" into a second's wait.
const OUTPUT_RECHECK: Duration = Duration::from_secs(1);

struct Notifications {
    xrun_count: Arc<AtomicU64>,
    /// Bumped on every port registered or unregistered, for
    /// [`JackDriver::service`] to notice. The output is not reconnected from
    /// here: this is JACK's notification thread, and under pipewire-jack a
    /// graph request made from it cannot wait for its own answer.
    graph_generation: Arc<AtomicU64>,
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
    fn port_registration(&mut self, client: &Client, port_id: PortId, is_registered: bool) {
        self.graph_generation.fetch_add(1, Ordering::Relaxed);
        if !is_registered {
            return;
        }
        // A keyboard plugged in while mooloop runs is listened to the way one
        // present at startup is. Not behind auto-reconnect, which is about
        // where the audio goes.
        if let Some(port) = client.port_by_id(port_id) {
            if is_hardware_midi_source(port.flags(), port.port_type().ok().as_deref()) {
                if let Ok(name) = port.name() {
                    connect_midi_source(client, &name);
                }
            }
        }
    }
}

/// A MIDI output port that belongs to hardware: a keyboard or controller, as
/// the JACK server (or PipeWire's MIDI bridge) presents it. Other programs'
/// MIDI outputs are left to the patchbay, since something sending to mooloop
/// on purpose is already connected by whoever set that up.
fn is_hardware_midi_source(flags: PortFlags, port_type: Option<&str>) -> bool {
    flags.contains(PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL)
        && port_type == Some(MIDI_PORT_TYPE)
}

/// Listen to one MIDI source. Called from JACK's notification thread or the
/// control thread, never the process callback.
fn connect_midi_source(client: &Client, source: &str) {
    match client.connect_ports_by_name(source, MIDI_IN_NAME) {
        Ok(()) => mooloop_core::log_info!("midi", "listening to the MIDI input {source}"),
        Err(jack::Error::PortAlreadyConnected(_, _)) => {}
        Err(e) => {
            mooloop_core::log_warn!("midi", "could not connect {source} -> {MIDI_IN_NAME} ({e})")
        }
    }
}

/// Wire the first two physical capture ports into `in_l` and `in_r`, so a
/// microphone is recordable without a patchbay. Best effort: a machine with
/// no capture ports simply records silence from the input.
fn connect_audio_input(client: &Client) {
    let sources = client.ports(
        None,
        Some(AUDIO_PORT_TYPE),
        PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL,
    );
    let right = sources.get(1).or(sources.first());
    for (source, destination) in [(sources.first(), IN_L_NAME), (right, "mooloop:in_r")] {
        let Some(source) = source else {
            continue;
        };
        if let Err(error) = client.connect_ports_by_name(source, destination) {
            mooloop_core::log_warn!(
                "audio",
                "could not connect {source} to {destination} ({error}); connect the \
                 input in a patchbay to record from it"
            );
        }
    }
}

/// Listen to every hardware MIDI source in the graph. Without this a keyboard
/// plays nothing until it is wired in a patchbay, which is not where anybody
/// looks when a key makes no sound.
fn connect_midi_sources(client: &Client) {
    let sources = client.ports(
        None,
        Some(MIDI_PORT_TYPE),
        PortFlags::IS_OUTPUT | PortFlags::IS_PHYSICAL,
    );
    for source in sources {
        connect_midi_source(client, &source);
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
        // One input, which every hardware source is connected to below. The
        // notes play whichever channel the editor has selected.
        let midi_in = client
            .register_port("midi_in", MidiIn::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        // The hardware input (`audio-recording/01`): one stereo pair, wired to
        // the system capture ports below, and chosen in the JACK graph from
        // then on, as the MIDI input is.
        let in_l = client
            .register_port("in_l", AudioIn::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        let in_r = client
            .register_port("in_r", AudioIn::default())
            .map_err(|e| Error::PortRegister(e.to_string()))?;
        let graph = Graph {
            executor,
            in_l,
            in_r,
            out_l,
            out_r,
            midi_in,
        };

        let target = config
            .output_target
            .unwrap_or_else(|| (DEFAULT_OUTPUT_L.to_owned(), DEFAULT_OUTPUT_R.to_owned()));
        let output_target = Arc::new(ArcSwap::from_pointee(target.clone()));
        let auto_reconnect = Arc::new(AtomicBool::new(config.auto_reconnect));
        // Oldest first into `remember_output`, so the saved target ends up at
        // the front and a malformed list is put back in order.
        let mut picks = Vec::new();
        for pick in config.earlier_outputs.iter().rev() {
            remember_output(&mut picks, pick.clone());
        }
        remember_output(&mut picks, target.clone());
        let graph_generation = Arc::new(AtomicU64::new(0));

        let async_client = client
            .activate_async(
                Notifications {
                    xrun_count,
                    graph_generation: graph_generation.clone(),
                },
                graph,
            )
            .map_err(|e| Error::Activate(e.to_string()))?;

        // Best-effort: wire the outputs to the most recent pick that is there,
        // so the app is audible out of the box.
        //
        // A saved destination outlives the thing it names. A device is
        // unplugged, a profile changes, the audio server is restarted and
        // renames its nodes -- and the target recorded in settings then
        // matches nothing. Connecting to nothing is the one outcome with no
        // symptom: the engine runs, the meters move, the transport rolls, and
        // there is silence with nothing on screen to say why. So an earlier
        // pick is taken over none, and any working stereo destination over
        // that, and the log says so. A wrong output is audible and one click
        // from right in Preferences; no output is a bug report.
        let c = async_client.as_client();
        connect_midi_sources(c);
        let candidates = output_candidates(&picks, &audio_destination_ports(c));
        match connect_first(c, &target, &candidates) {
            Some(landed) => {
                if landed != target {
                    mooloop_core::log_warn!(
                        "audio",
                        "the saved audio output {:?} is not available; connected to {:?} \
                         instead. Preferences -> Audio picks a different one",
                        target.0,
                        landed.0
                    );
                }
                output_target.store(Arc::new(landed));
            }
            None => mooloop_core::log_warn!(
                "audio",
                "the saved audio output {:?} is not available and nothing else accepted \
                 a connection; connect mooloop manually in a patchbay \
                 (e.g. qpwgraph, qjackctl, Helvum)",
                target.0
            ),
        }

        connect_audio_input(async_client.as_client());

        let now = Instant::now();
        Ok(JackDriver {
            client: async_client,
            output_target,
            auto_reconnect,
            picks: Mutex::new(picks),
            graph_generation,
            watch: Mutex::new(GraphWatch {
                seen: 0,
                changed_at: now,
                pending: false,
                last_check: now,
                reported_stranded: false,
            }),
        })
    }
}

/// The running JACK client. Dropping it deactivates audio.
pub(crate) struct JackDriver {
    client: AsyncClient,
    /// The pair the outputs are connected to, or were last.
    output_target: Arc<ArcSwap<(String, String)>>,
    auto_reconnect: Arc<AtomicBool>,
    /// The outputs picked in Preferences, most recent first. Kept apart from
    /// `output_target`, which a fallback overwrites, so that the output
    /// asked for is not forgotten because it was missing once.
    picks: Mutex<Vec<(String, String)>>,
    graph_generation: Arc<AtomicU64>,
    watch: Mutex<GraphWatch>,
}

/// What [`JackDriver::service`] has seen of the port graph.
struct GraphWatch {
    /// The `graph_generation` last seen, and when it was seen to change.
    seen: u64,
    changed_at: Instant,
    /// A change that has not been checked, waiting for [`GRAPH_SETTLE`].
    pending: bool,
    last_check: Instant,
    /// Whether "no output" has been logged since the output last connected,
    /// so a machine with nothing to play through logs it once, not every
    /// second.
    reported_stranded: bool,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl JackDriver {
    /// The MIDI inputs a channel or a binding can name. One, under JACK; see
    /// [`MIDI_IN_LABEL`].
    pub(crate) fn midi_ports(&self) -> Vec<MidiPortInfo> {
        vec![MidiPortInfo {
            id: MidiPortId::FIRST,
            name: MIDI_IN_LABEL.to_owned(),
        }]
    }

    /// What the AUDIO menu calls the hardware input. One, under JACK, for the
    /// reason there is one MIDI input: what feeds it is chosen in the graph.
    pub(crate) fn audio_input_label(&self) -> Option<String> {
        Some(crate::AUDIO_IN_LABEL.to_owned())
    }

    /// Frames between a sound leaving `out_l` and the same moment arriving
    /// back at `in_l`: the output's playback latency plus the input's capture
    /// latency, as JACK reports them. A take from the input starts this much
    /// after its bar line, so it lines up with what the performer heard.
    pub(crate) fn input_latency_frames(&self) -> u32 {
        let client = self.client.as_client();
        let capture = client
            .port_by_name(IN_L_NAME)
            .map_or(0, |port| port.get_latency_range(LatencyType::Capture).1);
        let playback = client
            .port_by_name(OUT_L_NAME)
            .map_or(0, |port| port.get_latency_range(LatencyType::Playback).1);
        capture.saturating_add(playback)
    }

    pub(crate) fn available_output_targets(&self) -> Vec<OutputTarget> {
        stereo_destinations(&audio_destination_ports(self.client.as_client()))
    }

    /// Connect the outputs to `target`, or to the system default if it is
    /// `None`, and only then let go of the previous target. See [`retarget`].
    /// A pair that connects becomes the most recent pick.
    pub(crate) fn set_output_target(&self, target: Option<(String, String)>) -> Result<(), String> {
        let previous = self.output_target.load_full();
        let next =
            target.unwrap_or_else(|| (DEFAULT_OUTPUT_L.to_owned(), DEFAULT_OUTPUT_R.to_owned()));
        retarget(self.client.as_client(), &previous, &next)?;
        remember_output(&mut lock(&self.picks), next.clone());
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

    /// Find another output when the one playing goes away. See
    /// [`Self::service`].
    pub(crate) fn set_auto_reconnect(&self, enabled: bool) {
        self.auto_reconnect.store(enabled, Ordering::Relaxed);
    }

    pub(crate) fn buffer_size(&self) -> u32 {
        self.client.as_client().buffer_size()
    }

    pub(crate) fn current_target(&self) -> (String, String) {
        (*self.output_target.load_full()).clone()
    }

    /// Control-thread upkeep, called from the handle's event poll: when the
    /// outputs are connected to nothing, connect them to the most recent pick
    /// that is there, or to anything that is ([`restore_output`]).
    ///
    /// Checked once the port graph has been still for [`GRAPH_SETTLE`], and
    /// every [`OUTPUT_RECHECK`] without a change. **A connected output is
    /// never moved**, even when a more recent pick appears: a USB interface
    /// plugged in while the speakers play leaves them playing. When plugging
    /// something in takes the playing device away -- headphones replacing
    /// the speakers on the same card -- the output has gone, and it moves.
    pub(crate) fn service(&self) {
        let generation = self.graph_generation.load(Ordering::Relaxed);
        let now = Instant::now();
        let mut watch = lock(&self.watch);
        if generation != watch.seen {
            watch.seen = generation;
            watch.changed_at = now;
            watch.pending = true;
            return;
        }
        let due = if watch.pending {
            now.duration_since(watch.changed_at) >= GRAPH_SETTLE
        } else {
            now.duration_since(watch.last_check) >= OUTPUT_RECHECK
        };
        if !due {
            return;
        }
        watch.pending = false;
        watch.last_check = now;
        if !self.auto_reconnect.load(Ordering::Relaxed) {
            return;
        }

        let client = self.client.as_client();
        let current = self.output_target.load_full();
        let picks = lock(&self.picks).clone();
        match restore_output(client, &current, &picks, &audio_destination_ports(client)) {
            Restored::Connected => watch.reported_stranded = false,
            Restored::Moved(landed) => {
                mooloop_core::log_info!(
                    "audio",
                    "the audio output {:?} is not connected; playing through {:?}",
                    current.0,
                    landed.0
                );
                self.output_target.store(Arc::new(landed));
                watch.reported_stranded = false;
            }
            Restored::Nowhere => {
                if !watch.reported_stranded {
                    mooloop_core::log_warn!(
                        "audio",
                        "the audio output {:?} is not connected and nothing else accepted \
                         a connection; waiting for an output to appear",
                        current.0
                    );
                    watch.reported_stranded = true;
                }
            }
        }
    }
}

/// Every audio input port in the graph: everything the outputs could be
/// connected to. A non-realtime JACK graph query.
///
/// Audio inputs only. Unfiltered, this returns MIDI destinations too -- a
/// machine with `Midi-Bridge` on the graph offers it as an output pair, and
/// connecting an audio port to it simply fails. That was survivable while the
/// list only populated a menu a human read; it is not, now that the fallback
/// picks from it without asking.
fn audio_destination_ports(jack_client: &Client) -> Vec<String> {
    jack_client.ports(None, Some(AUDIO_PORT_TYPE), jack::PortFlags::IS_INPUT)
}

/// Destination ports grouped by owning client, by the first two of each.
///
/// Shared by the preferences page and by the fallback, so that what the
/// engine reaches for when no pick is available is exactly what the
/// interface would have offered.
fn stereo_destinations(ports: &[String]) -> Vec<OutputTarget> {
    let mut grouped: Vec<(String, Vec<String>)> = Vec::new();
    for port in ports {
        let Some((client_name, _)) = port.split_once(':') else {
            continue;
        };
        match grouped.iter_mut().find(|(name, _)| name == client_name) {
            Some((_, ports)) => ports.push(port.clone()),
            None => grouped.push((client_name.to_owned(), vec![port.clone()])),
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

/// What one connection attempt came to, reduced to what [`retarget`] needs.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Connection {
    Made,
    AlreadyThere,
    Refused(String),
}

/// The two port-graph operations moving the output needs, so the order they
/// run in can be tested without a JACK server.
trait Patchbay {
    fn connect(&self, source: &str, destination: &str) -> Connection;
    fn disconnect(&self, source: &str, destination: &str);
    /// Whether `source` is connected to anything at all.
    fn is_connected(&self, source: &str) -> bool;
}

impl Patchbay for jack::Client {
    fn connect(&self, source: &str, destination: &str) -> Connection {
        match self.connect_ports_by_name(source, destination) {
            Ok(()) => Connection::Made,
            Err(jack::Error::PortAlreadyConnected(_, _)) => Connection::AlreadyThere,
            Err(error) => Connection::Refused(error.to_string()),
        }
    }

    fn disconnect(&self, source: &str, destination: &str) {
        let _ = self.disconnect_ports_by_name(source, destination);
    }

    fn is_connected(&self, source: &str) -> bool {
        self.port_by_name(source)
            .is_some_and(|port| port.connected_count().is_ok_and(|count| count > 0))
    }
}

/// Move the outputs from `previous` to `next`: connect first, disconnect
/// after.
///
/// The other order turned every failed choice into silence. It disconnected
/// the pair that was playing, then found the new one would not connect and
/// returned an error with nothing connected at all -- which is what startup
/// did to its own fallback when the saved output had gone: the fallback
/// found a working pair, and then applying the saved one took it away
/// (P3 in `reports/teams-2026-09-22.md`). Now a target that will not connect
/// leaves the output exactly where it was, and a half that did connect is
/// undone so the output is never left split across two destinations.
fn retarget(
    bay: &impl Patchbay,
    previous: &(String, String),
    next: &(String, String),
) -> Result<(), String> {
    let halves = [(OUT_L_NAME, next.0.as_str()), (OUT_R_NAME, next.1.as_str())];
    let mut made = [false; 2];
    for (index, (source, destination)) in halves.iter().enumerate() {
        match bay.connect(source, destination) {
            Connection::Made => made[index] = true,
            Connection::AlreadyThere => {}
            Connection::Refused(error) => {
                for ((half_source, half_destination), made) in halves.iter().zip(made) {
                    if made {
                        bay.disconnect(half_source, half_destination);
                    }
                }
                return Err(format!("could not connect {source} to {destination}: {error}"));
            }
        }
    }
    // A half the new target shares with the old one is the connection just
    // confirmed, so it stays.
    if previous.0 != next.0 {
        bay.disconnect(OUT_L_NAME, &previous.0);
    }
    if previous.1 != next.1 {
        bay.disconnect(OUT_R_NAME, &previous.1);
    }
    Ok(())
}

/// Where the outputs may go, in the order to try: the picks whose ports are
/// all in the graph, most recent first, then every other stereo destination.
///
/// Split from the JACK calls around it because this is the only part with a
/// decision in it, and a decision that cannot be tested without an audio
/// server is a decision nobody will revisit.
///
/// Past the picks the rule is deliberately dull: whatever is there, in the
/// graph's order. Ranking outputs by desirability -- preferring speakers over
/// HDMI, say -- is a guess about a machine this code cannot see, and being
/// audible somewhere is the whole of what is wanted then. Preferences owns
/// the actual choice. Mooloop's own inputs are never offered: routing the
/// master output back into the program is a feedback loop.
fn output_candidates(picks: &[(String, String)], ports: &[String]) -> Vec<(String, String)> {
    let present = |port: &String| ports.contains(port);
    let mut candidates: Vec<(String, String)> = picks
        .iter()
        .filter(|(l, r)| present(l) && present(r))
        .cloned()
        .collect();
    for destination in stereo_destinations(ports) {
        let pair = (destination.port_l, destination.port_r);
        if destination.client != CLIENT_NAME && !candidates.contains(&pair) {
            candidates.push(pair);
        }
    }
    candidates
}

/// Move the outputs from `current` to the first of `candidates` that
/// connects, and name it. A candidate can be in the graph and still refuse
/// the connection, so this walks them rather than trying one.
fn connect_first(
    bay: &impl Patchbay,
    current: &(String, String),
    candidates: &[(String, String)],
) -> Option<(String, String)> {
    candidates
        .iter()
        .find(|candidate| retarget(bay, current, candidate).is_ok())
        .cloned()
}

/// What a check of the outputs came to.
#[derive(Debug, PartialEq, Eq)]
enum Restored {
    /// Either output was connected somewhere, so nothing was touched.
    Connected,
    Moved((String, String)),
    /// Nothing was connected and nothing would take a connection.
    Nowhere,
}

/// Adam's rule for the output, 2026-09-22: stay on the most recently picked
/// output that is available, **but never move one that is connected** --
/// only one that is connected to nothing.
///
/// "Connected" is either output connected to anything, whoever made the
/// connection: a pair patched by hand in qpwgraph is a choice too.
fn restore_output(
    bay: &impl Patchbay,
    current: &(String, String),
    picks: &[(String, String)],
    ports: &[String],
) -> Restored {
    if bay.is_connected(OUT_L_NAME) || bay.is_connected(OUT_R_NAME) {
        return Restored::Connected;
    }
    connect_first(bay, current, &output_candidates(picks, ports))
        .map_or(Restored::Nowhere, Restored::Moved)
}

#[cfg(test)]
mod output_candidate_tests {
    use super::{output_candidates, CLIENT_NAME};

    fn ports(clients: &[&str]) -> Vec<String> {
        clients
            .iter()
            .flat_map(|client| [format!("{client}:playback_FL"), format!("{client}:playback_FR")])
            .collect()
    }

    fn pair(client: &str) -> (String, String) {
        (format!("{client}:playback_FL"), format!("{client}:playback_FR"))
    }

    fn clients(picks: &[(String, String)], graph: &[String]) -> Vec<String> {
        output_candidates(picks, graph)
            .into_iter()
            .map(|(l, _)| l.split_once(':').unwrap().0.to_owned())
            .collect()
    }

    /// The picks come first, most recent first, whatever order the graph
    /// lists them in -- then everything else, once.
    #[test]
    fn picks_come_before_the_rest_in_the_order_they_were_picked() {
        let picks = [pair("usb"), pair("speakers")];
        let graph = ports(&["hdmi", "speakers", "usb"]);
        assert_eq!(clients(&picks, &graph), ["usb", "speakers", "hdmi"]);
    }

    /// A pick that is not in the graph is not offered: the most recent pick
    /// that *is* there is the one to go to.
    #[test]
    fn a_missing_pick_gives_way_to_the_next_one() {
        let picks = [pair("headphones"), pair("speakers")];
        let graph = ports(&["hdmi", "speakers"]);
        assert_eq!(clients(&picks, &graph), ["speakers", "hdmi"]);
    }

    /// A device part-way through arriving has its left port and not its
    /// right. It is not there yet.
    #[test]
    fn a_pick_with_one_port_is_not_there() {
        let picks = [pair("usb")];
        let graph = vec![
            "usb:playback_FL".to_owned(),
            "speakers:playback_FL".to_owned(),
            "speakers:playback_FR".to_owned(),
        ];
        assert_eq!(clients(&picks, &graph), ["speakers"]);
    }

    /// With no pick in the graph, anything there is better than silence.
    #[test]
    fn with_no_pick_there_anything_will_do() {
        let graph = ports(&["hdmi", "speaker"]);
        assert_eq!(clients(&[pair("gone")], &graph), ["hdmi", "speaker"]);
    }

    /// Mooloop's own input ports are in the graph like anyone else's. Routing
    /// the master output back into the program would be a feedback loop
    /// arrived at by accident.
    #[test]
    fn mooloop_is_never_its_own_output() {
        let graph = ports(&[CLIENT_NAME, "speaker"]);
        assert_eq!(clients(&[], &graph), ["speaker"]);
    }

    /// A machine with nothing to play through gets the warning, not a panic
    /// and not a wrong guess.
    #[test]
    fn nothing_available_means_nothing_offered() {
        assert!(clients(&[pair("gone")], &[]).is_empty());
    }
}

#[cfg(test)]
mod retarget_tests {
    use super::{
        restore_output, retarget, Connection, Patchbay, Restored, OUT_L_NAME, OUT_R_NAME,
    };
    use std::cell::RefCell;
    use std::collections::BTreeSet;

    /// A port graph that holds connections and refuses the destinations it
    /// is told are gone.
    struct FakeGraph {
        connected: RefCell<BTreeSet<(String, String)>>,
        gone: Vec<&'static str>,
    }

    impl FakeGraph {
        fn playing(pair: &(String, String), gone: Vec<&'static str>) -> Self {
            let connected = [
                (OUT_L_NAME.to_owned(), pair.0.clone()),
                (OUT_R_NAME.to_owned(), pair.1.clone()),
            ];
            Self {
                connected: RefCell::new(connected.into_iter().collect()),
                gone,
            }
        }

        /// Nothing connected: the device that was playing has gone, and
        /// its connections with it.
        fn silent(gone: Vec<&'static str>) -> Self {
            Self {
                connected: RefCell::new(BTreeSet::new()),
                gone,
            }
        }

        fn outputs(&self) -> Vec<String> {
            self.connected.borrow().iter().map(|(_, to)| to.clone()).collect()
        }
    }

    impl Patchbay for FakeGraph {
        fn connect(&self, source: &str, destination: &str) -> Connection {
            if self.gone.contains(&destination) {
                return Connection::Refused("no such port".into());
            }
            let edge = (source.to_owned(), destination.to_owned());
            if self.connected.borrow_mut().insert(edge) {
                Connection::Made
            } else {
                Connection::AlreadyThere
            }
        }

        fn disconnect(&self, source: &str, destination: &str) {
            self.connected
                .borrow_mut()
                .remove(&(source.to_owned(), destination.to_owned()));
        }

        fn is_connected(&self, source: &str) -> bool {
            self.connected.borrow().iter().any(|(from, _)| from == source)
        }
    }

    fn pair(client: &str) -> (String, String) {
        (format!("{client}:playback_FL"), format!("{client}:playback_FR"))
    }

    #[test]
    fn a_new_target_replaces_the_old_one() {
        let (speakers, headphones) = (pair("speakers"), pair("headphones"));
        let graph = FakeGraph::playing(&speakers, vec![]);
        assert!(retarget(&graph, &speakers, &headphones).is_ok());
        assert_eq!(graph.outputs(), [headphones.0, headphones.1]);
    }

    /// Shaped against the old order, which disconnected first: a target that
    /// would not connect left nothing connected at all. That is startup's
    /// fallback being taken away when the saved output had gone.
    #[test]
    fn a_target_that_will_not_connect_leaves_the_output_where_it_was() {
        let (fallback, saved) = (pair("speakers"), pair("headphones"));
        let graph = FakeGraph::playing(&fallback, vec!["headphones:playback_FL"]);
        assert!(retarget(&graph, &fallback, &saved).is_err());
        assert_eq!(graph.outputs(), [fallback.0, fallback.1]);
    }

    /// Half a target is worse than the old one: the half that connected is
    /// undone, so the output is never split across two destinations.
    #[test]
    fn a_half_connected_target_is_undone() {
        let (speakers, headphones) = (pair("speakers"), pair("headphones"));
        let graph = FakeGraph::playing(&speakers, vec!["headphones:playback_FR"]);
        assert!(retarget(&graph, &speakers, &headphones).is_err());
        assert_eq!(graph.outputs(), [speakers.0, speakers.1]);
    }

    #[test]
    fn retargeting_to_the_same_pair_keeps_it_connected() {
        let speakers = pair("speakers");
        let graph = FakeGraph::playing(&speakers, vec![]);
        assert!(retarget(&graph, &speakers, &speakers).is_ok());
        assert_eq!(graph.outputs(), [speakers.0, speakers.1]);
    }

    fn graph(pairs: &[&(String, String)]) -> Vec<String> {
        pairs.iter().flat_map(|(l, r)| [l.clone(), r.clone()]).collect()
    }

    /// Adam's case, 2026-09-22: mooloop is playing through the speakers and
    /// a USB interface he picked more recently is plugged in. The speakers
    /// are still there and still connected, so nothing moves.
    #[test]
    fn a_more_recent_pick_appearing_does_not_move_a_connected_output() {
        let (speakers, usb) = (pair("speakers"), pair("usb"));
        let graph_now = FakeGraph::playing(&speakers, vec![]);
        let picks = [usb.clone(), speakers.clone()];
        assert_eq!(
            restore_output(&graph_now, &speakers, &picks, &graph(&[&speakers, &usb])),
            Restored::Connected
        );
        assert_eq!(graph_now.outputs(), [speakers.0, speakers.1]);
    }

    /// The other half of the same rule: when plugging the headphones in takes
    /// the speakers away, the output is connected to nothing, and it goes to
    /// the most recent pick that is there -- not to whatever the graph lists
    /// first.
    #[test]
    fn an_output_left_with_nothing_goes_to_the_most_recent_pick_there() {
        let (speakers, headphones, hdmi) = (pair("speakers"), pair("headphones"), pair("hdmi"));
        let graph_now = FakeGraph::silent(vec![]);
        let picks = [headphones.clone(), speakers.clone()];
        assert_eq!(
            restore_output(&graph_now, &speakers, &picks, &graph(&[&hdmi, &headphones])),
            Restored::Moved(headphones.clone())
        );
        assert_eq!(graph_now.outputs(), [headphones.0, headphones.1]);
    }

    /// No pick is there, so anything that is: silence is the one wrong
    /// answer.
    #[test]
    fn with_no_pick_there_the_output_goes_anywhere_that_takes_it() {
        let (gone, hdmi) = (pair("gone"), pair("hdmi"));
        let graph_now = FakeGraph::silent(vec![]);
        assert_eq!(
            restore_output(&graph_now, &gone, std::slice::from_ref(&gone), &graph(&[&hdmi])),
            Restored::Moved(hdmi)
        );
    }

    /// A destination that refuses is passed over for the next one.
    #[test]
    fn a_pick_that_refuses_gives_way_to_the_next() {
        let (usb, speakers) = (pair("usb"), pair("speakers"));
        let graph_now = FakeGraph::silent(vec!["usb:playback_FL"]);
        let picks = [usb.clone(), speakers.clone()];
        assert_eq!(
            restore_output(&graph_now, &usb, &picks, &graph(&[&usb, &speakers])),
            Restored::Moved(speakers.clone())
        );
        assert_eq!(graph_now.outputs(), [speakers.0, speakers.1]);
    }

    /// A connection made by hand in a patchbay is a choice too, even half of
    /// one.
    #[test]
    fn one_output_patched_by_hand_counts_as_connected() {
        let (speakers, elsewhere) = (pair("speakers"), pair("elsewhere"));
        let graph_now = FakeGraph::silent(vec![]);
        assert_eq!(graph_now.connect(OUT_L_NAME, &elsewhere.0), Connection::Made);
        assert_eq!(
            restore_output(&graph_now, &speakers, std::slice::from_ref(&speakers), &graph(&[&speakers])),
            Restored::Connected
        );
        assert_eq!(graph_now.outputs(), [elsewhere.0]);
    }

    #[test]
    fn nothing_there_is_nowhere() {
        let gone = pair("gone");
        assert_eq!(
            restore_output(&FakeGraph::silent(vec![]), &gone, std::slice::from_ref(&gone), &[]),
            Restored::Nowhere
        );
    }
}
