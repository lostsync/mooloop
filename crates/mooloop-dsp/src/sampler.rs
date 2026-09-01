//! A sample-playback instrument. The first real `AudioNode` in mooloop.
//!
//! Behaviour:
//! - On a `NoteOn` event, captures the currently-published sample (from the
//!   shared `ArcSwapOption` slot) and starts a voice from `params.start`.
//! - The voice runs through an ADSR amplitude envelope. In loop mode `Off`,
//!   reaching `loop_end` enters release; in `Forward`/`Pingpong`, the voice
//!   loops until retrigged or released.
//! - Sample rate conversion goes through the band-limited reader in
//!   `crate::interpolate`, which is told the region the head is in so an
//!   overhanging kernel folds across a loop instead of reading silence.
//!
//! Processing is **segment-based**: the block is split at each event's
//! sample offset, the voice renders the segment, then the event is applied.
//! This keeps note timing sample-accurate at any block size without a
//! per-sample event scan.

use std::sync::Arc;

use crate::bus::StereoBus;
use crate::event::{Event, EventList};
use crate::filter::apply_drive;
use crate::interpolate::{Region, RegionEdge, SincTable};
use crate::stretch::{StretchPool, StretchReader};
use crate::node::{AudioNode, ProcessContext};
use crate::scale::hz_from_normalized;
use crate::smooth::Smoothed;
use mooloop_core::{
    clamp01, EnvTimes, LoopMode, RetriggerMode, SamplerParams, VoiceMode, MAX_CHOKE_GROUP,
    MAX_LINEAR_GAIN, MAX_SAMPLER_VOICES,
};

use arc_swap::ArcSwapOption;

/// Minimum envelope stage time, to avoid divide-by-zero and infinite rates.
const MIN_STAGE_S: f32 = 1.0e-4;

/// Time constant for the output trim's lag. Short enough that a knob feels
/// attached to the sound, long enough that a jump across the whole range --
/// by hand, by automation, or by modulation -- ramps instead of clicking.
const OUTPUT_GAIN_SMOOTHING_S: f32 = 0.01;

/// Keep the trim inside the shared +12 dB ceiling, and treat a non-finite
/// value as silence rather than letting it reach the bus.
fn clamp_output_gain(gain: f32) -> f32 {
    if gain.is_finite() {
        gain.clamp(0.0, MAX_LINEAR_GAIN)
    } else {
        0.0
    }
}

/// Decoded sample data: stereo frames of f32 in `[-1, 1]`, plus the source
/// sample rate and a root MIDI note (default middle-C).
pub struct SampleData {
    pub frames: Vec<[f32; 2]>,
    pub sample_rate: u32,
    pub root_note: u8,
}

impl SampleData {
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// A punchy synthesised kick so the app makes sound out of the box. The
    /// user can load a real WAV to replace it. This is also the *known test
    /// asset* for gain structure: arbitrary user samples cannot be
    /// calibrated, but this one is generated to peak within a dB of
    /// `mooloop_core::gain::REFERENCE_PEAK_DBFS` at the master.
    pub fn default_kick(sample_rate: u32) -> Arc<Self> {
        /// The builtin kick's output reference, matched to
        /// `mooloop_core::gain::REFERENCE_PEAK_DBFS`.
        const OUTPUT_REFERENCE: f32 = 0.278;
        let dur_s = 0.25;
        let n = (dur_s * sample_rate as f64) as usize;
        let mut frames = Vec::with_capacity(n);
        let mut phase = 0.0_f64;
        for i in 0..n {
            let t = i as f64 / sample_rate as f64;
            // Exponential pitch drop 150 Hz -> 50 Hz across the body.
            let pitch = 150.0 * (50.0_f64 / 150.0).powf(t / dur_s);
            phase += pitch / sample_rate as f64;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            let body = (phase * core::f64::consts::TAU).sin();
            // Click at the very start for beater attack.
            let click = if t < 0.003 {
                (1.0 - t / 0.003) * 0.6
            } else {
                0.0
            };
            let amp = (-t * 12.0).exp();
            let s = (((body + click) * amp) as f32) * OUTPUT_REFERENCE;
            frames.push([s, s]);
        }
        Arc::new(Self {
            frames,
            sample_rate,
            root_note: 60,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

/// A scalar ADSR envelope. `advance` moves it one sample; the caller reads
/// `level` to shape amplitude.
#[derive(Clone, Copy, Debug)]
struct AdsrEnv {
    stage: Stage,
    level: f32,
    attack_inc: f32,
    decay_dec: f32,
    sustain: f32,
    release_dec: f32,
    release_s: f32,
    sample_rate: u32,
}

impl AdsrEnv {
    fn new(sample_rate: u32) -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
            attack_inc: 0.0,
            decay_dec: 0.0,
            sustain: 0.0,
            release_dec: 0.0,
            release_s: MIN_STAGE_S,
            sample_rate,
        }
    }

    /// Recompute rates from a set of stages.
    ///
    /// Takes the stages rather than the whole patch because a voice runs two
    /// of these now, from two different places in `SamplerParams`, and an
    /// envelope that reaches into the patch to find its own times could only
    /// ever be the amplitude one.
    fn configure(&mut self, times: EnvTimes) {
        let sr = self.sample_rate as f32;
        self.attack_inc = 1.0 / (times.attack.max(MIN_STAGE_S) * sr);
        self.decay_dec = (1.0 - times.sustain) / (times.decay.max(MIN_STAGE_S) * sr);
        self.sustain = clamp01(times.sustain);
        self.release_s = times.release.max(MIN_STAGE_S);
    }

    fn note_on(&mut self) {
        self.stage = Stage::Attack;
        self.level = 0.0;
    }

    /// Enter release from the current level.
    fn release(&mut self) {
        self.release_dec = self.level / (self.release_s * self.sample_rate as f32);
        self.stage = Stage::Release;
    }

    fn release_with(&mut self, seconds: f32) {
        self.release_dec = self.level / (seconds.max(MIN_STAGE_S) * self.sample_rate as f32);
        self.stage = Stage::Release;
    }

    fn advance(&mut self) {
        match self.stage {
            Stage::Idle => self.level = 0.0,
            Stage::Attack => {
                self.level += self.attack_inc;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                self.level -= self.decay_dec;
                if self.level <= self.sustain {
                    self.level = self.sustain;
                    self.stage = Stage::Sustain;
                }
            }
            Stage::Sustain => {}
            Stage::Release => {
                self.level -= self.release_dec;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
    }

    fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }
}

const CHOKE_RELEASE_S: f32 = 0.005;

/// One independently enveloped sample playback voice.
struct Voice {
    event_id: u64,
    midi_note: u8,
    age: u64,
    sample: Option<Arc<SampleData>>,
    play_pos: f64,
    playback_rate: f64,
    direction: f64,
    env: AdsrEnv,
    /// The filter's own envelope, advanced beside the amplitude one.
    ///
    /// Separate state, not a second reader of `env`: a sustained amp with a
    /// short filter decay is the whole point, and that shape cannot exist
    /// while one envelope drives both. The amplitude envelope stays the
    /// authority for how long the voice lives -- an unfinished filter release
    /// never holds a silent voice open.
    filter_env: AdsrEnv,
    velocity_amp: f32,
    filter_low: [f32; 2],
    filter_band: [f32; 2],
    held_frame: [f32; 2],
    hold_remaining: u32,
    loop_enabled: bool,
    active: bool,
}

impl Voice {
    fn new(sample_rate: u32) -> Self {
        Self {
            event_id: 0,
            midi_note: 60,
            age: 0,
            sample: None,
            play_pos: 0.0,
            playback_rate: 1.0,
            direction: 1.0,
            env: AdsrEnv::new(sample_rate),
            filter_env: AdsrEnv::new(sample_rate),
            velocity_amp: 0.0,
            filter_low: [0.0, 0.0],
            filter_band: [0.0, 0.0],
            held_frame: [0.0, 0.0],
            hold_remaining: 0,
            loop_enabled: false,
            active: false,
        }
    }

    /// Clear audible voice state while retaining the sample handle. Keeping
    /// that `Arc` avoids moving a potentially large sample deallocation onto
    /// the realtime thread when a channel changes source.
    fn reset(&mut self, params: SamplerParams, sample_rate: u32) {
        self.event_id = 0;
        self.midi_note = 60;
        self.age = 0;
        self.play_pos = 0.0;
        self.playback_rate = 1.0;
        self.direction = 1.0;
        self.env = AdsrEnv::new(sample_rate);
        self.env.configure(params.amp_env());
        self.filter_env = AdsrEnv::new(sample_rate);
        self.filter_env.configure(params.resolved_filter_env());
        self.velocity_amp = 0.0;
        self.filter_low = [0.0, 0.0];
        self.filter_band = [0.0, 0.0];
        self.held_frame = [0.0, 0.0];
        self.hold_remaining = 0;
        self.loop_enabled = false;
        self.active = false;
    }
}

/// Everything a voice render needs that is fixed for the whole segment.
///
/// Grouped rather than passed loose because they always travel together: the
/// block is already split at every event, so parameters and transport cannot
/// change inside one segment by construction.
#[derive(Clone, Copy)]
struct VoiceContext {
    params: SamplerParams,
    sample_rate: u32,
    bpm: f64,
}

/// The sampler node.
pub struct Sampler {
    sample_slot: Arc<ArcSwapOption<SampleData>>,
    params: SamplerParams,
    sample_rate: u32,
    voices: [Voice; MAX_SAMPLER_VOICES as usize],
    next_age: u64,
    /// Per-voice time-stretch state, or `None` when this sampler does not
    /// stretch.
    ///
    /// Device-level and optional because a `StretchReader` is ~100 KB and
    /// every one of the 256 addressable channels builds a `Sampler` with 16
    /// voices at startup. Held here it costs nothing until used; held in
    /// `Voice` it would cost hundreds of megabytes for an empty project.
    /// Installed and reclaimed structurally, never allocated on the audio
    /// thread -- `set_params` runs on the realtime command drain.
    stretch: Option<Box<StretchPool>>,
    /// Last tempo the transport reported. Only read when `stretch_sync` is
    /// on, where the bar length is what the ratio is derived from. Held here
    /// because `render_range` splits a block at event offsets and would
    /// otherwise have to carry it through every split.
    bpm: f64,
    /// The device's patch-level output trim, lagged so it cannot click.
    ///
    /// One gain for the whole sampler, not one per voice: every voice copies
    /// this smoother at the start of a segment and walks its own copy, and
    /// `render_range` catches the original up once for the segment. Copies
    /// are cheap, share a start value, and step identically, so the voices
    /// stay in agreement without the frame loop having to own them all.
    output_gain: Smoothed,
}

impl Sampler {
    /// Construct with a shared sample slot. The engine publishes samples into
    /// the same slot from the non-RT thread.
    pub fn new(
        sample_slot: Arc<ArcSwapOption<SampleData>>,
        mut params: SamplerParams,
        sample_rate: u32,
    ) -> Self {
        params.polyphony = params.polyphony.clamp(1, MAX_SAMPLER_VOICES);
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        // Build the shared interpolation table here, on whatever thread
        // constructs the device, so no `process()` call is ever the first to
        // touch it.
        SincTable::shared();
        let mut voices = std::array::from_fn(|_| Voice::new(sample_rate));
        for voice in &mut voices {
            voice.env.configure(params.amp_env());
            voice.filter_env.configure(params.resolved_filter_env());
        }
        Self {
            sample_slot,
            params,
            sample_rate,
            voices,
            next_age: 1,
            stretch: None,
            bpm: 120.0,
            output_gain: Smoothed::new(
                clamp_output_gain(params.output_gain),
                OUTPUT_GAIN_SMOOTHING_S,
                sample_rate,
            ),
        }
    }

    /// Replace the parameter set. Called from the RT command drain.
    pub fn set_params(&mut self, mut params: SamplerParams) {
        params.polyphony = params.polyphony.clamp(1, MAX_SAMPLER_VOICES);
        params.choke_group = params.choke_group.min(MAX_CHOKE_GROUP);
        self.params = params;
        let trim = clamp_output_gain(params.output_gain);
        if self.voices.iter().any(|voice| voice.active) {
            self.output_gain.set_target(trim);
        } else {
            // Nothing is sounding, so there is nothing for a ramp to protect
            // -- and a ramp here would be paid for by the *next* note's
            // attack. That is precisely the project-load path: install the
            // patch onto a silent device, then play it. Lagging into the
            // first hit swallowed 5 dB of a kick's transient.
            self.output_gain.reset_to(trim);
        }
        for (index, voice) in self.voices.iter_mut().enumerate() {
            voice.env.configure(params.amp_env());
            voice.filter_env.configure(params.resolved_filter_env());
            if index >= params.polyphony as usize {
                voice.active = false;
            }
        }
    }

    /// Apply one descriptor-addressed parameter, leaving the rest alone.
    ///
    /// Routed through `set_params` rather than writing the field directly so a
    /// control-rate change gets exactly the same clamping and voice
    /// reconfiguration a whole-struct update does. Both are non-allocating.
    fn apply_param(&mut self, id: u32, value: f32) {
        let mut params = mooloop_core::GeneratorParams::Sampler(self.params);
        if params.set(id, value).is_none() {
            return;
        }
        if let mooloop_core::GeneratorParams::Sampler(params) = params {
            self.set_params(params);
        }
    }

    /// The device's currently resolved parameters. This is what it was last
    /// *sent*, which after a control tick is base plus modulation, not the
    /// knob; the engine keeps the knob separately.
    pub fn params(&self) -> SamplerParams {
        self.params
    }

    /// Whether the patch asks to stretch. This is intent; `has_stretch`
    /// reports whether the state to do it has actually been installed. The
    /// engine reconciles the two off the realtime thread.
    pub fn wants_stretch(&self) -> bool {
        self.params.stretch_enabled
    }

    pub fn has_stretch(&self) -> bool {
        self.stretch.is_some()
    }

    /// Install prepared stretch state, returning whatever it displaced so the
    /// caller can hand it back for off-thread disposal. Realtime-safe: this
    /// moves boxes, it does not allocate or drop.
    pub fn install_stretch(&mut self, pool: Box<StretchPool>) -> Option<Box<StretchPool>> {
        self.stretch.replace(pool)
    }

    /// Surrender the stretch state, for the same round trip in reverse. The
    /// realtime thread must never be the one to drop it.
    pub fn take_stretch(&mut self) -> Option<Box<StretchPool>> {
        self.stretch.take()
    }

    /// Whether stretching should actually run, as opposed to merely being
    /// switched on.
    ///
    /// Unity ratio in a searching mode is deliberately a bypass: today's
    /// reader is sample-exact at unity and WSOLA is only *nearly* so, and
    /// enabling stretch without moving the ratio should not quietly change
    /// how an existing patch sounds. `Grain` has no such exemption -- its
    /// character is the point at any ratio.
    ///
    /// Reverse and ping-pong are refused rather than approximated. The
    /// stretcher's analysis pointer only moves forwards, and the spike
    /// measured neither; the UI disables the combination, and this makes the
    /// DSP independent of the UI being right about that.
    fn stretch_is_active(params: SamplerParams) -> bool {
        params.stretch_enabled
            && !params.reverse
            && params.loop_mode != LoopMode::Pingpong
            && (params.stretch_sync
                || params.stretch_mode == mooloop_core::StretchMode::Grain
                || (params.stretch_ratio - 1.0).abs() > 1.0e-4)
    }

    /// The ratio the stretcher should actually run.
    ///
    /// With `stretch_sync` off this is just the knob. With it on the ratio is
    /// *derived* so the region lasts `stretch_bars` bars, and the derivation
    /// is the reason pitch and duration stop fighting:
    ///
    /// ```text
    /// output_frames * rate = stretched_frames
    /// stretched_frames / ratio = source_frames
    /// ```
    ///
    /// so holding `output_frames` at a musical length while `source_frames`
    /// is fixed at the region means `ratio = target * rate / region`. The
    /// playback rate is in the numerator, which is the whole trick: transpose
    /// a voice up an octave and the ratio doubles to match, so the loop still
    /// lands on the bar. Pitch becomes a tuning control rather than a speed
    /// control, and modulating either one leaves the other alone.
    ///
    /// The loop is what gets fitted when there is one, since the loop is the
    /// thing that repeats against the grid; otherwise the playback region is.
    fn effective_ratio(
        params: SamplerParams,
        len: usize,
        sample_rate: u32,
        bpm: f64,
        playback_rate: f64,
    ) -> f64 {
        if !params.stretch_sync {
            return f64::from(params.stretch_ratio);
        }
        let (region_start, region_end) = if params.loop_mode == LoopMode::Off {
            Self::resolve_playback_bounds(params, len)
        } else {
            Self::resolve_loop_bounds(params, len)
        };
        let region = region_end - region_start;
        if region <= 0.0 {
            return 1.0;
        }
        let bars = f64::from(
            params
                .stretch_bars
                .clamp(mooloop_core::MIN_STRETCH_BARS, mooloop_core::MAX_STRETCH_BARS),
        );
        let target = bars * mooloop_core::frames_per_bar(sample_rate, bpm);
        (target * playback_rate / region).clamp(
            f64::from(mooloop_core::MIN_STRETCH_RATIO),
            f64::from(mooloop_core::MAX_STRETCH_RATIO),
        )
    }

    pub fn choke_group(&self) -> u8 {
        self.params.choke_group
    }

    /// Normalized (0..1) playback position of every active voice, for a UI
    /// playhead. Inactive slots (and voices with no loaded sample) report
    /// `f32::NAN`, which the caller filters out rather than drawing a
    /// playhead at frame zero.
    pub fn voice_positions(&self) -> [f32; MAX_SAMPLER_VOICES as usize] {
        let mut positions = [f32::NAN; MAX_SAMPLER_VOICES as usize];
        for (voice, position) in self.voices.iter().zip(positions.iter_mut()) {
            if let (true, Some(sample)) = (voice.active, voice.sample.as_ref()) {
                let len = sample.len().max(1) as f64;
                *position = (voice.play_pos / len).clamp(0.0, 1.0) as f32;
            }
        }
        positions
    }

    /// Immediately invalidate all active voices. Sample handles remain owned
    /// by their slots/voices so resetting is allocation- and drop-free.
    pub fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.reset(self.params, self.sample_rate);
        }
        self.next_age = 1;
        // Every voice is silent now, so there is nothing for the trim to
        // click against: start the next patch at its own level rather than
        // ramping there from the last one's.
        self.output_gain
            .reset_to(clamp_output_gain(self.params.output_gain));
    }

    fn voice_limit(&self) -> usize {
        self.params.polyphony.clamp(1, MAX_SAMPLER_VOICES) as usize
    }

    fn select_voice(&self, midi_note: u8) -> usize {
        let voices = &self.voices[..self.voice_limit()];
        if self.params.retrigger_mode == RetriggerMode::Restart {
            if let Some((index, _)) = voices
                .iter()
                .enumerate()
                .filter(|(_, voice)| voice.active && voice.midi_note == midi_note)
                .min_by_key(|(_, voice)| voice.age)
            {
                return index;
            }
        }
        if let Some(index) = voices.iter().position(|voice| !voice.active) {
            return index;
        }
        voices
            .iter()
            .enumerate()
            .min_by_key(|(_, voice)| voice.age)
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn trigger(&mut self, event_id: u64, note: u8, velocity: u8) {
        let Some(sample) = self.sample_slot.load_full() else {
            return;
        };
        let index = self.select_voice(note);
        let len = sample.len().max(1);
        let (start, end) = Self::resolve_playback_bounds(self.params, len);
        let root_note = self.params.root_note.min(127);
        let key_semitones = i16::from(note.min(127)) - i16::from(root_note);
        let tuning = f64::from(self.params.tune_semitones.clamp(-48.0, 48.0))
            + f64::from(self.params.tune_cents.clamp(-100.0, 100.0)) / 100.0;
        let pitch_ratio = 2.0_f64.powf((f64::from(key_semitones) + tuning) / 12.0);
        let age = self.next_age;
        self.next_age = self.next_age.wrapping_add(1).max(1);

        let voice = &mut self.voices[index];
        voice.event_id = event_id;
        voice.midi_note = note;
        voice.age = age;
        voice.sample = Some(sample.clone());
        voice.active = !sample.is_empty();
        if !voice.active {
            return;
        }
        voice.play_pos = if self.params.reverse {
            end - 1.0
        } else {
            start
        };
        voice.playback_rate = sample.sample_rate as f64 / self.sample_rate as f64 * pitch_ratio;
        voice.direction = if self.params.reverse { -1.0 } else { 1.0 };
        voice.velocity_amp = f32::from(velocity) / 127.0;
        voice.filter_low = [0.0, 0.0];
        voice.filter_band = [0.0, 0.0];
        voice.held_frame = [0.0, 0.0];
        voice.hold_remaining = 0;
        voice.loop_enabled = self.params.loop_mode != LoopMode::Off;
        voice.env.configure(self.params.amp_env());
        voice.env.note_on();
        voice.filter_env.configure(self.params.resolved_filter_env());
        voice.filter_env.note_on();

        // Point this voice's stretcher at the same frame the read head starts
        // on, so output frame 0 is input frame `start` and the note has no
        // more latency than an unstretched one.
        let params = self.params;
        let start_pos = self.voices[index].play_pos;
        if let Some(reader) = self.stretch.as_mut().and_then(|pool| pool.reader_mut(index)) {
            let stretcher = reader.stretcher_mut();
            stretcher.set_mode(params.stretch_mode);
            stretcher.set_grain_frames(u32::from(params.stretch_grain));
            stretcher.set_ratio(f64::from(params.stretch_ratio));
            reader.reset(start_pos);
        }
    }

    fn release_note(&mut self, event_id: u64) {
        let mode = self.params.voice_mode;
        for voice in self
            .voices
            .iter_mut()
            .filter(|voice| voice.active && voice.event_id == event_id)
        {
            match mode {
                VoiceMode::Gate => {
                    voice.env.release();
                    voice.filter_env.release();
                }
                VoiceMode::OneShot if voice.loop_enabled => voice.loop_enabled = false,
                VoiceMode::OneShot => {}
            }
        }
    }

    fn release_all(&mut self) {
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            if !voice.env.is_releasing() {
                voice.env.release();
                voice.filter_env.release();
            }
        }
    }

    pub fn choke(&mut self) {
        for voice in self.voices.iter_mut().filter(|voice| voice.active) {
            voice.loop_enabled = false;
            voice.env.release_with(CHOKE_RELEASE_S);
            voice.filter_env.release_with(CHOKE_RELEASE_S);
        }
    }

    fn shape_frame(
        params: SamplerParams,
        sample_rate: u32,
        voice: &mut Voice,
        frame: [f32; 2],
    ) -> [f32; 2] {
        let rate_reduction = clamp01(params.rate_reduction);
        let hold_frames = 1 + (rate_reduction * 31.0).round() as u32;
        if voice.hold_remaining == 0 {
            let bit_reduction = clamp01(params.bit_reduction);
            voice.held_frame = if bit_reduction <= f32::EPSILON {
                frame
            } else {
                let bits = (16.0 - bit_reduction * 12.0).round().clamp(4.0, 16.0);
                let scale = 2.0_f32.powf(bits - 1.0);
                [
                    (frame[0] * scale).round() / scale,
                    (frame[1] * scale).round() / scale,
                ]
            };
            voice.hold_remaining = hold_frames;
        }
        voice.hold_remaining -= 1;
        let mut frame = voice.held_frame;

        let drive = clamp01(params.drive);
        if drive > f32::EPSILON {
            let shaped = apply_drive(frame[0], drive);
            let shaped_r = apply_drive(frame[1], drive);
            frame = [shaped, shaped_r];
        }

        let cutoff = clamp01(params.filter_cutoff);
        let env_amount = params.filter_env_amount.clamp(-1.0, 1.0);
        let resonance = clamp01(params.filter_resonance);
        if cutoff >= 0.999 && env_amount.abs() <= f32::EPSILON && resonance <= f32::EPSILON {
            return frame;
        }
        let max_hz = sample_rate as f32 * 0.45;
        let base_hz = hz_from_normalized(cutoff, max_hz);
        let cutoff_hz =
            (base_hz * 2.0_f32.powf(voice.filter_env.level * env_amount * 6.0)).clamp(20.0, max_hz);
        // Topology-preserving state-variable low-pass. Unlike a biquad this
        // remains well behaved while cutoff and envelope move every sample.
        let g = (core::f32::consts::PI * cutoff_hz / sample_rate as f32).tan();
        let damping = (2.0 - resonance * 1.9).clamp(0.1, 2.0);
        let a1 = 1.0 / (1.0 + g * (g + damping));
        let a2 = g * a1;
        let a3 = g * a2;
        for (channel, output) in frame.iter_mut().enumerate() {
            let input = *output;
            let v3 = input - voice.filter_low[channel];
            let v1 = a1 * voice.filter_band[channel] + a2 * v3;
            let v2 = voice.filter_low[channel] + a2 * voice.filter_band[channel] + a3 * v3;
            voice.filter_band[channel] = 2.0 * v1 - voice.filter_band[channel];
            voice.filter_low[channel] = 2.0 * v2 - voice.filter_low[channel];
            *output = v2;
        }
        frame
    }

    /// Normalized playback region resolved against the current sample length.
    fn resolve_playback_bounds(params: SamplerParams, len: usize) -> (f64, f64) {
        let len = len.max(1) as f64;
        let start = f64::from(clamp01(params.start)) * len;
        let end = (f64::from(clamp01(params.end)) * len)
            .max(start + 1.0)
            .min(len);
        (start.min(end - 1.0), end)
    }

    #[cfg(test)]
    fn playback_bounds(&self, len: usize) -> (f64, f64) {
        Self::resolve_playback_bounds(self.params, len)
    }

    /// Normalized loop bounds resolved against the current sample length.
    fn resolve_loop_bounds(params: SamplerParams, len: usize) -> (f64, f64) {
        let len_f = len.max(1) as f64;
        let (play_start, play_end) = Self::resolve_playback_bounds(params, len);
        let loop_start =
            (f64::from(clamp01(params.loop_start)) * len_f).clamp(play_start, play_end - 1.0);
        let loop_end = (f64::from(clamp01(params.loop_end)) * len_f)
            .max(loop_start + 1.0)
            .min(play_end);
        (loop_start.min(loop_end - 1.0), loop_end)
    }

    #[cfg(test)]
    fn loop_bounds(&self, len: usize) -> (f64, f64) {
        Self::resolve_loop_bounds(self.params, len)
    }

    /// Render the voice into `bus[start..end]`, adding into the buffers.
    /// Handles looping, envelope advancement, and voice termination.
    fn render_voice_range(
        cx: VoiceContext,
        voice: &mut Voice,
        stretch: Option<&mut StretchReader>,
        output_gain: &mut Smoothed,
        bus: &mut StereoBus,
        // One range rather than two loose indices: they are always the
        // segment the block was split into, never independent.
        range: core::ops::Range<usize>,
    ) {
        let VoiceContext {
            params,
            sample_rate,
            bpm,
        } = cx;
        let (start, end) = (range.start, range.end);
        if !voice.active {
            return;
        }
        // The sample, the resolved bounds, and the loop mode are all fixed
        // for the whole segment -- events are what change them, and the block
        // is already split at every event. Resolving them once here is both
        // cheaper than the per-frame recomputation this replaces and what
        // lets the read below know its region before it reads.
        let Some(len) = voice.sample.as_ref().map(|sample| sample.len()).filter(|len| *len > 0)
        else {
            voice.active = false;
            return;
        };
        let (play_start, play_end) = Self::resolve_playback_bounds(params, len);
        let (ls, le) = Self::resolve_loop_bounds(params, len);
        let loop_mode = if voice.loop_enabled {
            params.loop_mode
        } else {
            LoopMode::Off
        };
        let table = SincTable::shared();

        // Resolved once for the segment, like the bounds above: parameters do
        // not move inside a segment, because the block is already split at
        // every event.
        let mut stretch = stretch.filter(|_| Self::stretch_is_active(params));
        if let Some(reader) = stretch.as_mut() {
            let ratio =
                Self::effective_ratio(params, len, sample_rate, bpm, voice.playback_rate);
            let stretcher = reader.stretcher_mut();
            stretcher.set_mode(params.stretch_mode);
            stretcher.set_grain_frames(u32::from(params.stretch_grain));
            stretcher.set_ratio(ratio);
        }

        for i in start..end {
            let Some(sample) = voice.sample.as_ref() else {
                voice.active = false;
                return;
            };

            // Advance both envelopes; only the amplitude one can end the
            // voice. A filter release still running under a finished amp
            // release is inaudible, and holding the voice open for it would
            // spend a slot on silence.
            voice.env.advance();
            voice.filter_env.advance();
            if voice.env.is_idle() {
                voice.active = false;
                return;
            }
            // The trim is the last stage before the bus, after the shaping
            // in `shape_frame`: it is the patch's output level, not another
            // colour control.
            let amp = voice.env.level * voice.velocity_amp * output_gain.advance();

            // Fetch the frame through the band-limited reader. The region
            // it is told about is the one the head is actually in: a looping
            // voice still plays its run-in from the region start up to the
            // loop, and only inside the loop does an overhanging kernel wrap
            // or turn around rather than reading real neighbouring material.
            let pos = voice.play_pos;
            let region = if loop_mode != LoopMode::Off && pos >= ls {
                Region {
                    start: ls,
                    end: le,
                    edge: match loop_mode {
                        LoopMode::Pingpong => RegionEdge::Mirror,
                        _ => RegionEdge::Wrap,
                    },
                }
            } else {
                Region {
                    start: play_start,
                    end: play_end,
                    edge: RegionEdge::Silent,
                }
            };
            let raw = match stretch.as_mut() {
                Some(reader) => reader.read(&sample.frames, region, voice.playback_rate),
                None => table.read(&sample.frames, pos, voice.playback_rate, region),
            };
            let frame = Self::shape_frame(params, sample_rate, voice, raw);
            bus.l[i] += amp * frame[0];
            bus.r[i] += amp * frame[1];

            // Advance the read position and handle looping / end-of-region.
            //
            // A stretching voice does not step its own head: the reader
            // reports where the frame just handed out came from, and
            // mirroring that into `play_pos` keeps end-of-region detection,
            // the loop checks below, and the UI playhead all reading one
            // value. Deliberately the *sounding* position and not the
            // stretcher's analysis frontier, which runs ahead by up to a hop
            // and would end a one-shot before its tail had been played. The
            // reader has already wrapped inside a forward loop, so the wrap
            // below finds nothing to do.
            match stretch.as_ref() {
                Some(reader) => voice.play_pos = reader.source_pos(),
                None => voice.play_pos += voice.direction * voice.playback_rate,
            }

            match loop_mode {
                LoopMode::Off => {
                    let reached_end = voice.direction > 0.0 && voice.play_pos >= play_end;
                    let reached_start = voice.direction < 0.0 && voice.play_pos < play_start;
                    if reached_end || reached_start {
                        // There is no audio beyond a non-looping region, so an
                        // envelope tail would only hold its final sample value.
                        voice.active = false;
                        return;
                    }
                }
                LoopMode::Forward => {
                    if voice.direction > 0.0 && voice.play_pos >= le {
                        voice.play_pos = ls + (voice.play_pos - le);
                    } else if voice.direction < 0.0 && voice.play_pos < ls {
                        voice.play_pos = le - (ls - voice.play_pos);
                    }
                }
                LoopMode::Pingpong => {
                    if voice.direction > 0.0 && voice.play_pos >= le {
                        voice.play_pos = le - (voice.play_pos - le);
                        voice.direction = -1.0;
                    } else if voice.direction < 0.0 && voice.play_pos <= ls {
                        voice.play_pos = ls + (ls - voice.play_pos);
                        voice.direction = 1.0;
                    }
                }
            }
        }
    }

    fn render_range(&mut self, bus: &mut StereoBus, start: usize, end: usize) {
        let params = self.params;
        let cx = VoiceContext {
            params,
            sample_rate: self.sample_rate,
            bpm: self.bpm,
        };
        self.output_gain
            .set_target(clamp_output_gain(params.output_gain));
        let entry = self.output_gain;
        let limit = params.polyphony.clamp(1, MAX_SAMPLER_VOICES) as usize;
        let mut readers = self.stretch.as_mut().map(|pool| pool.readers_mut());
        for voice in &mut self.voices[..limit] {
            let mut gain = entry;
            // Advanced in lockstep with the voices, so voice `n` always gets
            // reader `n` whether or not it is sounding.
            let reader = readers.as_mut().and_then(Iterator::next);
            Self::render_voice_range(cx, voice, reader, &mut gain, bus, start..end);
        }
        // A voice that ends mid-segment stops walking its copy, and a silent
        // sampler renders no voices at all, so the trim advances here rather
        // than inheriting whichever voice happened to run last.
        self.output_gain.advance_by(end.saturating_sub(start));
    }

    #[cfg(test)]
    fn active_voice_count(&self) -> usize {
        self.voices.iter().filter(|voice| voice.active).count()
    }

    #[cfg(test)]
    fn shape_first_voice(&mut self, frame: [f32; 2]) -> [f32; 2] {
        Self::shape_frame(self.params, self.sample_rate, &mut self.voices[0], frame)
    }
}

impl AudioNode for Sampler {
    fn process(
        &mut self,
        ctx: &ProcessContext,
        bus: &mut StereoBus,
        events_in: &EventList,
        _events_out: Option<&mut EventList>,
    ) {
        let frames = ctx.frames.min(bus.capacity());
        self.bpm = ctx.bpm;

        if !ctx.playing {
            self.release_all();
        }

        // Split the block at event offsets: render, apply event, repeat.
        let mut pos = 0usize;
        for ev in events_in.iter() {
            let off = (ev.offset as usize).min(frames).max(pos);
            self.render_range(bus, pos, off);
            match ev.event {
                Event::NoteOn { id, note, velocity } => self.trigger(id, note, velocity),
                Event::NoteOff { id, .. } => self.release_note(id),
                Event::Choke => self.choke(),
                Event::ParamValue { id, value } => self.apply_param(id, value),
                Event::Buffer(_) | Event::BufferRelease | Event::BufferScrub { .. } => {}
            }
            pos = off;
        }
        self.render_range(bus, pos, frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TimedEvent;

    fn make_sampler(sr: u32) -> Sampler {
        let kick = SampleData::default_kick(sr);
        let slot: Arc<ArcSwapOption<SampleData>> = Arc::new(ArcSwapOption::from(Some(kick)));
        Sampler::new(slot, SamplerParams::default(), sr)
    }

    fn sampler_with_frames(sr: u32, len: usize, params: SamplerParams) -> Sampler {
        let frames = (0..len)
            .map(|index| {
                let value = index as f32 / len.max(1) as f32;
                [value, value]
            })
            .collect();
        let sample = Arc::new(SampleData {
            frames,
            sample_rate: sr,
            root_note: 60,
        });
        let slot = Arc::new(ArcSwapOption::from(Some(sample)));
        Sampler::new(slot, params, sr)
    }

    /// Fit-to-tempo derives the ratio so the region lasts the requested
    /// number of bars. One bar at 120 BPM is 96,000 frames, so a 48,000-frame
    /// sample has to be stretched 2x to fill it.
    #[test]
    fn fit_to_tempo_derives_the_ratio_from_the_bar_length() {
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
        let ratio = Sampler::effective_ratio(params, 48_000, 48_000, 120.0, 1.0);
        assert!((ratio - 2.0).abs() < 1.0e-6, "expected 2x, got {ratio}");

        // Half the tempo, twice the bar, twice the stretch.
        let slower = Sampler::effective_ratio(params, 48_000, 48_000, 60.0, 1.0);
        assert!((slower - 4.0).abs() < 1.0e-6, "expected 4x, got {slower}");

        // Two bars of the same sample needs twice as much again.
        let two_bars = Sampler::effective_ratio(
            SamplerParams {
                stretch_bars: 2.0,
                ..params
            },
            48_000,
            48_000,
            120.0,
            1.0,
        );
        assert!((two_bars - 4.0).abs() < 1.0e-6, "expected 4x, got {two_bars}");
    }

    /// The reason fit-to-tempo exists: transposing a voice must not change how
    /// long it lasts. The playback rate enters the derivation, so pitching up
    /// an octave doubles the ratio to compensate and the loop still lands on
    /// the bar.
    #[test]
    fn fit_to_tempo_makes_pitch_and_duration_independent() {
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            ..SamplerParams::default()
        };
        let unity = Sampler::effective_ratio(params, 48_000, 48_000, 120.0, 1.0);
        for rate in [0.5, 1.0, 2.0, 3.0] {
            let ratio = Sampler::effective_ratio(params, 48_000, 48_000, 120.0, rate);
            // Output length is `region * ratio / rate`; holding that constant
            // is exactly what keeps the loop on the grid.
            let output = 48_000.0 * ratio / rate;
            let reference = 48_000.0 * unity / 1.0;
            assert!(
                (output - reference).abs() < 1.0,
                "rate {rate} changed the duration: {output} vs {reference}"
            );
        }
    }

    /// With sync off the knob is the ratio, untouched.
    #[test]
    fn without_sync_the_ratio_is_the_knob() {
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: false,
            stretch_ratio: 3.25,
            ..SamplerParams::default()
        };
        let ratio = Sampler::effective_ratio(params, 48_000, 48_000, 120.0, 2.0);
        assert!((ratio - 3.25).abs() < 1.0e-6, "the rate leaked in: {ratio}");
    }

    /// The loop is what gets fitted when there is one, since the loop is the
    /// part that repeats against the grid.
    #[test]
    fn fit_to_tempo_measures_the_loop_when_there_is_one() {
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: 1.0,
            loop_mode: LoopMode::Forward,
            loop_start: 0.0,
            loop_end: 0.5,
            ..SamplerParams::default()
        };
        // Half of a 48,000-frame sample is 24,000 frames, so filling a
        // 96,000-frame bar takes 4x rather than 2x.
        let ratio = Sampler::effective_ratio(params, 48_000, 48_000, 120.0, 1.0);
        assert!((ratio - 4.0).abs() < 1.0e-6, "expected 4x, got {ratio}");
    }

    /// A derived ratio still has to land inside what the stretcher accepts:
    /// an absurd tempo or a one-frame loop must not produce a ratio the DSP
    /// would have to clamp silently later.
    #[test]
    fn a_derived_ratio_stays_inside_the_supported_range() {
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_bars: mooloop_core::MAX_STRETCH_BARS,
            ..SamplerParams::default()
        };
        let ratio = Sampler::effective_ratio(params, 64, 48_000, 20.0, 4.0);
        assert!(ratio <= f64::from(mooloop_core::MAX_STRETCH_RATIO));

        let tiny = Sampler::effective_ratio(
            SamplerParams {
                stretch_bars: mooloop_core::MIN_STRETCH_BARS,
                ..params
            },
            48_000_000,
            48_000,
            300.0,
            0.25,
        );
        assert!(tiny >= f64::from(mooloop_core::MIN_STRETCH_RATIO));
    }

    /// Sync on is enough to make stretching active even at a knob ratio of
    /// exactly 1.0, because the knob is not what is being used.
    #[test]
    fn sync_activates_stretching_regardless_of_the_knob() {
        let params = SamplerParams {
            stretch_enabled: true,
            stretch_sync: true,
            stretch_ratio: 1.0,
            ..SamplerParams::default()
        };
        assert!(Sampler::stretch_is_active(params));
    }

    /// Play one note and report how far through the sample the head reached.
    fn playhead_after(mut sampler: Sampler, sr: u32, frames: usize) -> f32 {
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 127,
            },
        });
        sampler.process(&ctx(frames, sr), &mut bus, &events, None);
        sampler.voice_positions()[0]
    }

    fn stretching_params(ratio: f32) -> SamplerParams {
        SamplerParams {
            attack: 0.0,
            decay: 8.0,
            sustain: 1.0,
            output_gain: 1.0,
            stretch_enabled: true,
            stretch_ratio: ratio,
            ..SamplerParams::default()
        }
    }

    /// Intent without state must be harmless. A patch can be loaded with
    /// stretch on before the engine has provisioned the pool, and in that
    /// window the sampler simply plays the way it did the frame before rather
    /// than falling silent or allocating.
    #[test]
    fn a_sampler_that_wants_stretch_but_has_no_pool_plays_unstretched() {
        let sr = 48_000;
        let sampler = sampler_with_frames(sr, 8_192, stretching_params(2.0));
        assert!(sampler.wants_stretch());
        assert!(!sampler.has_stretch());

        let stretched = playhead_after(sampler, sr, 1_024);
        let plain = playhead_after(
            sampler_with_frames(sr, 8_192, SamplerParams {
                attack: 0.0,
                decay: 8.0,
                sustain: 1.0,
                output_gain: 1.0,
                ..SamplerParams::default()
            }),
            sr,
            1_024,
        );
        assert!(
            (stretched - plain).abs() < 1.0e-4,
            "unprovisioned stretch changed playback: {stretched} vs {plain}"
        );
    }

    /// With the pool installed, the stretcher owns the read head: at ratio 2
    /// the same number of output frames consumes half as much sample.
    #[test]
    fn an_installed_pool_slows_the_read_head() {
        let sr = 48_000;
        let len = 8_192;
        let frames = 1_024;

        let plain = playhead_after(
            sampler_with_frames(sr, len, SamplerParams {
                attack: 0.0,
                decay: 8.0,
                sustain: 1.0,
                output_gain: 1.0,
                ..SamplerParams::default()
            }),
            sr,
            frames,
        );

        let mut stretched = sampler_with_frames(sr, len, stretching_params(2.0));
        assert!(stretched
            .install_stretch(Box::new(StretchPool::new(
                mooloop_core::StretchMode::Music,
                sr,
                MAX_SAMPLER_VOICES as usize,
            )))
            .is_none());
        assert!(stretched.has_stretch());
        let slowed = playhead_after(stretched, sr, frames);

        let ratio = slowed / plain;
        assert!(
            (0.4..0.6).contains(&ratio),
            "ratio 2 should halve the head's travel, got {slowed} vs {plain}"
        );
    }

    /// Unity ratio in a searching mode is a bypass on purpose: switching
    /// stretch on without moving the ratio must not change how a patch
    /// sounds, and today's reader is sample-exact at unity where WSOLA is
    /// only nearly so.
    #[test]
    fn enabling_stretch_at_unity_changes_nothing() {
        let params = stretching_params(1.0);
        assert!(!Sampler::stretch_is_active(params));
        // ...but the artifact mode has no such exemption.
        assert!(Sampler::stretch_is_active(SamplerParams {
            stretch_mode: mooloop_core::StretchMode::Grain,
            ..params
        }));
    }

    /// Reverse and ping-pong were never measured and the analysis pointer
    /// only moves forwards. The UI disables the combination; the DSP refuses
    /// it too, so it does not depend on the UI being right.
    #[test]
    fn stretch_refuses_reverse_and_pingpong() {
        let params = stretching_params(2.0);
        assert!(Sampler::stretch_is_active(params));
        assert!(!Sampler::stretch_is_active(SamplerParams {
            reverse: true,
            ..params
        }));
        assert!(!Sampler::stretch_is_active(SamplerParams {
            loop_mode: LoopMode::Pingpong,
            ..params
        }));
    }

    /// The ownership round trip the realtime thread depends on: state goes in
    /// and comes back out as a box the caller disposes of elsewhere.
    #[test]
    fn stretch_state_can_be_installed_and_surrendered() {
        let sr = 48_000;
        let mut sampler = sampler_with_frames(sr, 1_024, stretching_params(2.0));
        let first = Box::new(StretchPool::new(
            mooloop_core::StretchMode::Music,
            sr,
            MAX_SAMPLER_VOICES as usize,
        ));
        assert!(sampler.install_stretch(first).is_none());

        let second = Box::new(StretchPool::new(
            mooloop_core::StretchMode::Grain,
            sr,
            MAX_SAMPLER_VOICES as usize,
        ));
        assert!(
            sampler.install_stretch(second).is_some(),
            "the displaced pool must come back rather than be dropped here"
        );
        assert!(sampler.take_stretch().is_some());
        assert!(!sampler.has_stretch());
    }

    fn ctx(frames: usize, sr: u32) -> ProcessContext {
        ProcessContext {
            sample_rate: sr,
            frames,
            playing: true,
            bpm: 120.0,
            position_ticks: 0.0,
            position_frames: 0,
        }
    }

    /// A note at offset K must produce exact silence before K and signal
    /// after — the point of segment-based processing.
    #[test]
    fn note_on_at_offset_is_sample_accurate() {
        let sr = 48_000;
        let frames = 512;
        let k = 200usize;
        let mut sampler = make_sampler(sr);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: k as u32,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(bus.l[..k].iter().all(|s| *s == 0.0));
        assert!(bus.l[k..].iter().any(|s| s.abs() > 0.01));
    }

    #[test]
    fn stale_note_off_does_not_release_a_retriggered_voice() {
        let sr = 48_000;
        let mut sampler = make_sampler(sr);
        sampler.set_params(SamplerParams {
            voice_mode: VoiceMode::Gate,
            ..SamplerParams::default()
        });
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 100,
            },
        });
        events.push(TimedEvent {
            offset: 8,
            event: Event::NoteOn {
                id: 2,
                note: 64,
                velocity: 100,
            },
        });
        events.push(TimedEvent {
            offset: 16,
            event: Event::NoteOff { id: 1, note: 60 },
        });

        sampler.process(&ctx(64, sr), &mut bus, &events, None);

        assert_eq!(sampler.voices[0].event_id, 2);
        assert!(!sampler.voices[0].env.is_releasing());
    }

    #[test]
    fn one_shot_ignores_note_off_without_a_loop() {
        let mut sampler = sampler_with_frames(48_000, 4096, SamplerParams::default());
        sampler.trigger(1, 60, 100);
        sampler.release_note(1);
        assert!(!sampler.voices[0].env.is_releasing());
        assert!(sampler.voices[0].active);
    }

    #[test]
    fn gated_note_enters_release_on_matching_note_off() {
        let params = SamplerParams {
            voice_mode: VoiceMode::Gate,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(7, 60, 100);
        sampler.release_note(7);
        assert!(sampler.voices[0].env.is_releasing());
    }

    #[test]
    fn one_shot_note_off_exits_loop_and_plays_tail() {
        let params = SamplerParams {
            loop_start: 0.2,
            loop_end: 0.4,
            loop_mode: LoopMode::Forward,
            voice_mode: VoiceMode::OneShot,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 100, params);
        sampler.trigger(1, 60, 127);
        let mut bus = StereoBus::with_capacity(200);
        sampler.render_range(&mut bus, 0, 80);
        assert!(sampler.voices[0].active);
        assert!(sampler.voices[0].loop_enabled);

        sampler.release_note(1);
        sampler.render_range(&mut bus, 80, 200);
        assert!(!sampler.voices[0].active);
    }

    #[test]
    fn layered_retriggers_fill_the_bounded_voice_pool() {
        let params = SamplerParams {
            polyphony: 3,
            retrigger_mode: RetriggerMode::Layer,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(1, 60, 100);
        sampler.trigger(2, 60, 100);
        sampler.trigger(3, 60, 100);
        assert_eq!(sampler.active_voice_count(), 3);

        sampler.trigger(4, 60, 100);
        assert_eq!(sampler.active_voice_count(), 3);
        assert!(!sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 1));
        assert!(sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 4));
    }

    #[test]
    fn restart_reuses_the_oldest_matching_pitch() {
        let params = SamplerParams {
            polyphony: 4,
            retrigger_mode: RetriggerMode::Restart,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(1, 60, 100);
        sampler.trigger(2, 64, 100);
        sampler.trigger(3, 60, 100);
        assert_eq!(sampler.active_voice_count(), 2);
        assert!(!sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 1));
        assert!(sampler
            .voices
            .iter()
            .any(|voice| voice.active && voice.event_id == 3));
    }

    #[test]
    fn choke_releases_every_active_voice_quickly() {
        let params = SamplerParams {
            polyphony: 4,
            retrigger_mode: RetriggerMode::Layer,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(48_000, 4096, params);
        sampler.trigger(1, 60, 100);
        sampler.trigger(2, 64, 100);
        let mut bus = StereoBus::with_capacity(512);
        sampler.render_range(&mut bus, 0, 32);
        sampler.choke();
        assert!(sampler
            .voices
            .iter()
            .filter(|voice| voice.active)
            .all(|voice| voice.env.is_releasing()));
        sampler.render_range(&mut bus, 32, 512);
        assert_eq!(sampler.active_voice_count(), 0);
    }

    /// No events, no prior note: silence (and no panic on the empty path).
    #[test]
    fn idle_is_silent() {
        let sr = 48_000;
        let mut sampler = make_sampler(sr);
        let mut bus = StereoBus::with_capacity(256);
        sampler.process(&ctx(256, sr), &mut bus, &EventList::empty(), None);
        assert_eq!(bus.peak(256), (0.0, 0.0));
    }

    #[test]
    fn midi_note_controls_playback_rate() {
        let sr = 48_000;
        let frames = 128;
        let render_note = |note| {
            let mut sampler = sampler_with_frames(sr, 4096, SamplerParams::default());
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 0,
                    note,
                    velocity: 127,
                },
            });
            sampler.process(&ctx(frames, sr), &mut bus, &events, None);
            sampler.voices[0].play_pos
        };

        let root_position = render_note(60);
        let octave_position = render_note(72);
        assert!((root_position - 128.0).abs() < 0.001);
        assert!((octave_position - 256.0).abs() < 0.001);
    }

    #[test]
    fn coarse_and_fine_tuning_change_playback_rate() {
        let sr = 48_000;
        let render_with = |tune_semitones, tune_cents| {
            let params = SamplerParams {
                tune_semitones,
                tune_cents,
                ..SamplerParams::default()
            };
            let mut sampler = sampler_with_frames(sr, 4096, params);
            let mut bus = StereoBus::with_capacity(100);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 0,
                    note: 60,
                    velocity: 127,
                },
            });
            sampler.process(&ctx(100, sr), &mut bus, &events, None);
            sampler.voices[0].play_pos
        };

        assert!((render_with(12.0, 0.0) - 200.0).abs() < 0.001);
        assert!((render_with(0.0, 100.0) - 105.946).abs() < 0.01);
    }

    #[test]
    fn reverse_playback_starts_at_region_end_and_runs_backwards() {
        let sr = 48_000;
        let params = SamplerParams {
            start: 0.25,
            end: 0.75,
            reverse: true,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 1000, params);
        let mut bus = StereoBus::with_capacity(100);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(100, sr), &mut bus, &events, None);

        assert!((sampler.voices[0].play_pos - 649.0).abs() < 0.001);
        assert!(sampler.voices[0].active);
    }

    #[test]
    fn non_looping_voice_stops_at_trimmed_end() {
        let sr = 48_000;
        let params = SamplerParams {
            end: 0.25,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 100, params);
        let mut bus = StereoBus::with_capacity(64);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(64, sr), &mut bus, &events, None);

        assert!(!sampler.voices[0].active);
        assert!(bus.l[..25].iter().any(|sample| *sample > 0.0));
        assert!(bus.l[25..].iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn loop_bounds_use_normalized_start_and_end() {
        let params = SamplerParams {
            loop_start: 0.25,
            loop_end: 0.75,
            ..SamplerParams::default()
        };
        let sampler = sampler_with_frames(48_000, 100, params);
        assert_eq!(sampler.loop_bounds(100), (25.0, 75.0));
    }

    #[test]
    fn loop_region_is_clamped_to_playback_region() {
        let params = SamplerParams {
            start: 0.2,
            end: 0.8,
            loop_start: 0.0,
            loop_end: 1.0,
            ..SamplerParams::default()
        };
        let sampler = sampler_with_frames(48_000, 100, params);
        let (play_start, play_end) = sampler.playback_bounds(100);
        let (loop_start, loop_end) = sampler.loop_bounds(100);
        assert!((play_start - 20.0).abs() < 0.001);
        assert!((play_end - 80.0).abs() < 0.001);
        assert!((loop_start - 20.0).abs() < 0.001);
        assert!((loop_end - 80.0).abs() < 0.001);
    }

    #[test]
    fn forward_loop_wraps_inside_selected_region() {
        let sr = 48_000;
        let frames = 160;
        let params = SamplerParams {
            loop_start: 0.25,
            loop_end: 0.5,
            loop_mode: LoopMode::Forward,
            sustain: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });

        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        assert!(sampler.voices[0].active);
        assert!((16.0..32.0).contains(&sampler.voices[0].play_pos));
        assert!(bus.l.iter().any(|sample| *sample > 0.01));
    }

    #[test]
    fn rate_reduction_holds_sample_values() {
        let sr = 48_000;
        let params = SamplerParams {
            rate_reduction: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);

        let first = sampler.shape_first_voice([0.125, -0.125]);
        let second = sampler.shape_first_voice([0.875, -0.875]);

        assert_eq!(first, second);
        assert_eq!(sampler.voices[0].hold_remaining, 30);
    }

    #[test]
    fn maximum_bit_reduction_quantizes_to_four_bits() {
        let sr = 48_000;
        let params = SamplerParams {
            bit_reduction: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);

        assert_eq!(sampler.shape_first_voice([0.19, -0.19]), [0.25, -0.25]);
    }

    #[test]
    fn low_cutoff_filters_an_impulse() {
        let sr = 48_000;
        let params = SamplerParams {
            filter_cutoff: 0.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);

        let filtered = sampler.shape_first_voice([1.0, 1.0]);
        assert!(filtered[0] > 0.0);
        assert!(filtered[0] < 0.01);
    }

    #[test]
    fn resonant_filter_remains_finite() {
        let sr = 48_000;
        let params = SamplerParams {
            filter_cutoff: 0.7,
            filter_resonance: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        for index in 0..20_000 {
            let input = if index == 0 { [1.0, 1.0] } else { [0.0, 0.0] };
            let output = sampler.shape_first_voice(input);
            assert!(output[0].is_finite() && output[1].is_finite());
        }
    }

    #[test]
    fn drive_saturates_without_boosting_level() {
        // Compensated saturation: at full drive a 0.2 input comes out
        // richer (compressed toward the reference ceiling), not four times
        // louder.
        let sr = 48_000;
        let params = SamplerParams {
            drive: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        let driven = sampler.shape_first_voice([0.2, -0.2]);
        assert!(driven[0] > 0.2);
        assert!(driven[0] < 0.26);
        assert!(driven[1] < -0.2);
        assert!(driven[1] > -0.26);
    }

    /// A held note, rendered with only the trim different. The trim is a
    /// plain gain on the finished voice, so the two renders differ by exactly
    /// its ratio once the lag has settled.
    #[test]
    fn the_output_trim_scales_the_finished_voice() {
        let sr = 48_000;
        let frames = 2048;
        let render_at = |gain: f32| {
            let params = SamplerParams {
                loop_mode: LoopMode::Forward,
                output_gain: gain,
                ..SamplerParams::default()
            };
            let mut sampler = sampler_with_frames(sr, 512, params);
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity: 127,
                },
            });
            sampler.process(&ctx(frames, sr), &mut bus, &events, None);
            bus.l[frames / 2..frames]
                .iter()
                .fold(0.0f32, |peak, sample| peak.max(sample.abs()))
        };

        let unity = render_at(1.0);
        let trimmed = render_at(mooloop_core::sampler::default_output_gain());
        assert!(unity > 0.1, "the unity render should be audible: {unity}");
        let ratio = trimmed / unity;
        assert!(
            (ratio - mooloop_core::sampler::default_output_gain()).abs() < 0.01,
            "a -12 dB trim scaled the voice by {ratio}"
        );
    }

    /// Loading a patch onto a silent sampler must not cost the first note its
    /// attack. The regression this pins measured 5 dB off a kick's transient,
    /// because the trim was still lagging up from the previous patch's value
    /// when the note arrived.
    #[test]
    fn a_patch_installed_while_silent_is_at_level_by_the_first_note() {
        let sr = 48_000;
        let frames = 256;
        let mut sampler = sampler_with_frames(
            sr,
            512,
            SamplerParams {
                output_gain: 0.05,
                ..SamplerParams::default()
            },
        );
        // The engine's load path: reset the device, then install the patch.
        sampler.reset();
        sampler.set_params(SamplerParams {
            output_gain: 1.0,
            ..SamplerParams::default()
        });

        // A sampler that was at unity all along. Comparing against it keeps
        // the assertion about the trim rather than about the envelope's
        // millisecond attack, which shapes the same early frames.
        let mut settled = sampler_with_frames(
            sr,
            512,
            SamplerParams {
                output_gain: 1.0,
                ..SamplerParams::default()
            },
        );

        let mut note = EventList::empty();
        note.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 127,
            },
        });
        let mut installed_bus = StereoBus::with_capacity(frames);
        let mut settled_bus = StereoBus::with_capacity(frames);
        sampler.process(&ctx(frames, sr), &mut installed_bus, &note, None);
        settled.process(&ctx(frames, sr), &mut settled_bus, &note, None);

        for frame in 0..frames {
            assert!(
                (installed_bus.l[frame] - settled_bus.l[frame]).abs() < 1.0e-6,
                "frame {frame}: installed {} vs settled {} -- the trim was \
                 still ramping when the note arrived",
                installed_bus.l[frame],
                settled_bus.l[frame]
            );
        }
    }

    /// Automation and modulation reach the trim through `ParamValue`, which
    /// can jump the whole range between one sample and the next. The lag is
    /// what stands between that and a click, so assert on the step size in
    /// the output rather than on the parameter.
    #[test]
    fn jumping_the_output_trim_ramps_instead_of_clicking() {
        let sr = 48_000;
        let frames = 1024;
        let params = SamplerParams {
            loop_mode: LoopMode::Forward,
            output_gain: 0.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 512, params);
        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 127,
            },
        });
        events.push(TimedEvent {
            offset: (frames / 2) as u32,
            event: Event::ParamValue {
                id: mooloop_core::generator::SAMPLER_PARAM_OUTPUT_GAIN,
                value: MAX_LINEAR_GAIN,
            },
        });
        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        // The sample under the voice is a rising ramp, so consecutive frames
        // differ a little on their own; a stepped gain would differ by the
        // whole jump at once.
        let largest_step = bus.l[..frames]
            .windows(2)
            .fold(0.0f32, |worst, pair| worst.max((pair[1] - pair[0]).abs()));
        assert!(
            largest_step < 0.05,
            "the trim stepped by {largest_step} in one sample"
        );
        assert!(
            bus.l[frames - 1].abs() > bus.l[frames / 2].abs(),
            "the trim never arrived"
        );
    }

    /// Every voice shares one trim. The smoother is copied per voice, so this
    /// is the assertion that the copies stay in step with each other and with
    /// the original.
    #[test]
    fn a_chord_hears_one_trim_rather_than_one_per_voice() {
        let sr = 48_000;
        let frames = 1024;
        let render_notes = |notes: &[u8], gain: f32| {
            let params = SamplerParams {
                polyphony: 4,
                retrigger_mode: RetriggerMode::Layer,
                loop_mode: LoopMode::Forward,
                output_gain: gain,
                ..SamplerParams::default()
            };
            let mut sampler = sampler_with_frames(sr, 512, params);
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            for (index, note) in notes.iter().enumerate() {
                events.push(TimedEvent {
                    offset: 0,
                    event: Event::NoteOn {
                        id: index as u64 + 1,
                        note: *note,
                        velocity: 127,
                    },
                });
            }
            sampler.process(&ctx(frames, sr), &mut bus, &events, None);
            bus.l[frames - 1]
        };

        let notes = [60u8, 64, 67];
        let unity = render_notes(&notes, 1.0);
        let halved = render_notes(&notes, 0.5);
        assert!(unity.abs() > 0.1, "the chord should be audible: {unity}");
        assert!(
            (halved - unity * 0.5).abs() < 1.0e-4,
            "three voices at half trim summed to {halved}, want {}",
            unity * 0.5
        );
    }

    /// Playing a sample at its root note asks for no rate conversion at all,
    /// and the reader has to notice: every frame comes back as it was
    /// written, not as a filtered approximation. This is the property that
    /// makes upgrading the interpolator safe for existing projects, since a
    /// one-shot at its own pitch is unchanged.
    #[test]
    fn unity_rate_playback_is_sample_exact() {
        let sr = 48_000;
        let frames = 512;
        let params = SamplerParams {
            attack: 0.0,
            decay: 8.0,
            sustain: 1.0,
            output_gain: 1.0,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 1024, params);
        let source: Vec<f32> = (0..1024).map(|index| index as f32 / 1024.0).collect();

        let mut bus = StereoBus::with_capacity(frames);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 127,
            },
        });
        sampler.process(&ctx(frames, sr), &mut bus, &events, None);

        // The envelope still shapes the first frames, so compare where it has
        // reached full: the claim is about the reader, not the envelope.
        for (frame, (played, expected)) in bus.l[64..frames]
            .iter()
            .zip(source[64..frames].iter())
            .enumerate()
        {
            assert!(
                (played - expected).abs() < 1.0e-4,
                "frame {}: played {played} vs source {expected}",
                frame + 64
            );
        }
    }

    /// Reverse, forward-loop and ping-pong playback all run the kernel over
    /// region edges it has to fold across. Whatever it reads there, it must
    /// stay finite and inside the sample.
    #[test]
    fn every_playback_direction_stays_finite_across_its_boundaries() {
        let sr = 48_000;
        let frames = 2048;
        for loop_mode in LoopMode::all() {
            for reverse in [false, true] {
                for note in [36u8, 60, 96] {
                    let params = SamplerParams {
                        loop_mode,
                        reverse,
                        start: 0.1,
                        end: 0.6,
                        loop_start: 0.2,
                        loop_end: 0.3,
                        output_gain: 1.0,
                        ..SamplerParams::default()
                    };
                    let mut sampler = sampler_with_frames(sr, 256, params);
                    let mut bus = StereoBus::with_capacity(frames);
                    let mut events = EventList::empty();
                    events.push(TimedEvent {
                        offset: 0,
                        event: Event::NoteOn {
                            id: 1,
                            note,
                            velocity: 127,
                        },
                    });
                    sampler.process(&ctx(frames, sr), &mut bus, &events, None);
                    for frame in 0..frames {
                        assert!(
                            bus.l[frame].is_finite() && bus.r[frame].is_finite(),
                            "{loop_mode:?} reverse={reverse} note={note} frame {frame}: {}",
                            bus.l[frame]
                        );
                        assert!(
                            bus.l[frame].abs() <= 2.0,
                            "{loop_mode:?} reverse={reverse} note={note} frame {frame}: {}",
                            bus.l[frame]
                        );
                    }
                }
            }
        }
    }

    /// Rendering the same note twice has to give the same samples: the
    /// reader carries no state of its own, so realtime and offline runs of
    /// one project cannot diverge.
    #[test]
    fn repeated_renders_of_one_note_are_identical() {
        let sr = 48_000;
        let frames = 1024;
        let render = || {
            let params = SamplerParams {
                loop_mode: LoopMode::Pingpong,
                output_gain: 1.0,
                ..SamplerParams::default()
            };
            let mut sampler = sampler_with_frames(sr, 300, params);
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 67,
                    velocity: 110,
                },
            });
            sampler.process(&ctx(frames, sr), &mut bus, &events, None);
            bus.l[..frames].to_vec()
        };
        assert_eq!(render(), render());
    }

    /// A bright fixture: alternating full-scale frames put all the energy at
    /// Nyquist, so what survives the low-pass is a direct readout of where
    /// its cutoff is sitting.
    fn bright_sampler(sr: u32, params: SamplerParams) -> Sampler {
        let frames = (0..4096)
            .map(|index| {
                let value = if index % 2 == 0 { 1.0 } else { -1.0 };
                [value, value]
            })
            .collect();
        let sample = Arc::new(SampleData {
            frames,
            sample_rate: sr,
            root_note: 60,
        });
        Sampler::new(Arc::new(ArcSwapOption::from(Some(sample))), params, sr)
    }

    fn window_rms(bus: &StereoBus, from: usize, to: usize) -> f32 {
        let sum: f32 = bus.l[from..to].iter().map(|s| s * s).sum();
        (sum / (to - from) as f32).sqrt()
    }

    /// The shape the split exists for: amplitude held flat while the filter
    /// plucks shut underneath it. With one shared envelope a sustain of 1.0
    /// pinned the filter open for the whole note, so this could not be
    /// expressed at all -- the control asserts exactly that.
    #[test]
    fn a_sustained_amp_can_carry_a_short_filter_pluck() {
        let sr = 48_000;
        let frames = 4096;
        let base = SamplerParams {
            attack: 0.0,
            decay: 8.0,
            sustain: 1.0,
            release: 0.5,
            filter_cutoff: 0.1,
            filter_env_amount: 1.0,
            loop_mode: LoopMode::Forward,
            output_gain: 1.0,
            ..SamplerParams::default()
        };
        let render = |params: SamplerParams| {
            let mut sampler = bright_sampler(sr, params);
            let mut bus = StereoBus::with_capacity(frames);
            let mut events = EventList::empty();
            events.push(TimedEvent {
                offset: 0,
                event: Event::NoteOn {
                    id: 1,
                    note: 60,
                    velocity: 127,
                },
            });
            sampler.process(&ctx(frames, sr), &mut bus, &events, None);
            (window_rms(&bus, 64, 512), window_rms(&bus, 3584, 4096))
        };

        // Following the amplitude envelope: sustain 1.0 holds the filter wide
        // open, so the end is as bright as the beginning.
        let (shared_early, shared_late) = render(base);
        assert!(
            shared_late > shared_early * 0.8,
            "a shared envelope should not pluck: {shared_early} -> {shared_late}"
        );

        // Its own short decay to nothing: the filter shuts while the
        // amplitude envelope holds.
        let plucked = SamplerParams {
            filter_env: Some(mooloop_core::EnvTimes {
                attack: 0.0,
                decay: 0.01,
                sustain: 0.0,
                release: 0.5,
            }),
            ..base
        };
        let (pluck_early, pluck_late) = render(plucked);
        assert!(
            pluck_early > pluck_late * 4.0,
            "the filter did not pluck: {pluck_early} -> {pluck_late}"
        );
    }

    /// The amplitude envelope owns the voice's lifetime. A filter release
    /// still running under a finished amp release must not keep a silent
    /// voice allocated.
    #[test]
    fn a_long_filter_release_does_not_hold_a_finished_voice_open() {
        let sr = 48_000;
        let params = SamplerParams {
            voice_mode: VoiceMode::Gate,
            release: 0.005,
            filter_env: Some(mooloop_core::EnvTimes {
                attack: 0.0,
                decay: 0.25,
                sustain: 1.0,
                release: 8.0,
            }),
            loop_mode: LoopMode::Forward,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 512, params);
        let mut bus = StereoBus::with_capacity(512);
        let mut on = EventList::empty();
        on.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 1,
                note: 60,
                velocity: 127,
            },
        });
        sampler.process(&ctx(512, sr), &mut bus, &on, None);
        let mut off = EventList::empty();
        off.push(TimedEvent {
            offset: 0,
            event: Event::NoteOff { id: 1, note: 60 },
        });
        sampler.process(&ctx(512, sr), &mut bus, &off, None);
        // Well past the 5 ms amp release, far short of the 8 s filter one.
        for _ in 0..8 {
            sampler.process(&ctx(512, sr), &mut bus, &EventList::empty(), None);
        }
        assert_eq!(
            sampler.active_voice_count(),
            0,
            "the filter release kept a silent voice allocated"
        );
    }

    /// A patch that never gave the filter its own stages runs the amplitude
    /// ones, whatever they are -- which is what makes an old project's filter
    /// motion survive exactly rather than approximately.
    #[test]
    fn an_unset_filter_envelope_follows_whatever_the_amp_envelope_is() {
        let params = SamplerParams {
            attack: 0.123,
            decay: 0.456,
            sustain: 0.25,
            release: 0.789,
            ..SamplerParams::default()
        };
        assert_eq!(params.resolved_filter_env(), params.amp_env());

        let mut owned = params;
        owned.filter_env_mut().decay = 0.01;
        // Materializing seeds from the amp stages, so the three untouched
        // stages do not jump when the first one is edited.
        assert_eq!(owned.resolved_filter_env().attack, 0.123);
        assert_eq!(owned.resolved_filter_env().sustain, 0.25);
        assert_eq!(owned.resolved_filter_env().decay, 0.01);
        assert_eq!(owned.amp_env(), params.amp_env());
    }

    #[test]
    fn stopping_transport_releases_a_looped_voice() {
        let sr = 48_000;
        let params = SamplerParams {
            loop_mode: LoopMode::Forward,
            ..SamplerParams::default()
        };
        let mut sampler = sampler_with_frames(sr, 64, params);
        let mut bus = StereoBus::with_capacity(32);
        let mut events = EventList::empty();
        events.push(TimedEvent {
            offset: 0,
            event: Event::NoteOn {
                id: 0,
                note: 60,
                velocity: 127,
            },
        });
        sampler.process(&ctx(32, sr), &mut bus, &events, None);

        let mut stopped = ctx(32, sr);
        stopped.playing = false;
        sampler.process(&stopped, &mut bus, &EventList::empty(), None);

        assert!(sampler.voices[0].env.is_releasing());
    }
}
