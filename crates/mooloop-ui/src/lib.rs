//! Slint UI wrapper. Owns the `EngineHandle`, wires Slint callbacks to engine
//! commands, and runs a high-frequency timer that forwards commands and drains
//! audio events onto window properties.

slint::include_modules!();

use mooloop_core::{EngineCommand, EngineEvent};
use mooloop_engine::EngineHandle;
use slint::{ComponentHandle, Timer, TimerMode};
use std::sync::mpsc;

const PUMP_INTERVAL_MS: u64 = 8;
const INITIAL_METRONOME_VOLUME: f32 = 0.6;
const INITIAL_BPM: i32 = 120;

pub struct AppUi {
    window: MainWindow,
    _pump: Timer,
}

impl AppUi {
    /// Build the window and wire it to the engine handle. The handle is
    /// consumed and polled by an internal timer for the lifetime of the UI.
    pub fn new(mut handle: EngineHandle) -> Result<Self, slint::PlatformError> {
        let window = MainWindow::new()?;

        // Initialise UI + engine to a known state.
        window.set_bpm(INITIAL_BPM);
        window.set_metronome_volume(INITIAL_METRONOME_VOLUME);
        window.set_playing(false);
        window.set_beat_in_bar(0);
        window.set_peak_l(0.0);
        window.set_peak_r(0.0);
        handle.send(EngineCommand::SetTempo(INITIAL_BPM as f64));
        handle.send(EngineCommand::SetMetronomeVolume(INITIAL_METRONOME_VOLUME));

        // Commands from UI closures arrive through this channel; the pump
        // forwards them to the (non-Clone) rtrb producer it owns.
        let (cmd_tx, cmd_rx) = mpsc::channel::<EngineCommand>();

        {
            let tx = cmd_tx.clone();
            window.on_play_clicked(move || {
                let _ = tx.send(EngineCommand::Play);
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_stop_clicked(move || {
                let _ = tx.send(EngineCommand::Stop);
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_bpm_changed(move |bpm| {
                let _ = tx.send(EngineCommand::SetTempo(bpm as f64));
            });
        }
        {
            let tx = cmd_tx.clone();
            window.on_metronome_volume_changed(move |v| {
                let _ = tx.send(EngineCommand::SetMetronomeVolume(v));
            });
        }

        // The pump owns the handle: forwards queued commands, drains audio
        // events, mirrors them onto the window.
        let weak = window.as_weak();
        let pump = Timer::default();
        pump.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(PUMP_INTERVAL_MS),
            move || {
                while let Ok(cmd) = cmd_rx.try_recv() {
                    handle.send(cmd);
                }
                let Some(w) = weak.upgrade() else { return };
                for ev in handle.drain() {
                    match ev {
                        EngineEvent::Position { beat_in_bar, playing, .. } => {
                            w.set_beat_in_bar(beat_in_bar as i32);
                            w.set_playing(playing);
                        }
                        EngineEvent::Metering { peak_l, peak_r } => {
                            w.set_peak_l(peak_l.clamp(0.0, 1.0));
                            w.set_peak_r(peak_r.clamp(0.0, 1.0));
                        }
                        EngineEvent::Xrun => {}
                    }
                }
            },
        );

        Ok(AppUi { window, _pump: pump })
    }

    pub fn show(&self) -> Result<(), slint::PlatformError> {
        self.window.show()
    }

    pub fn run(&self) -> Result<(), slint::PlatformError> {
        self.window.run()
    }
}
