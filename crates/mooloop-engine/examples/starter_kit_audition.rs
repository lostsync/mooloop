//! The starter kit's audition, MOO-267's listening case.
//!
//! A new song opens with four DS-01 channels built from the Machine Kick,
//! Machine Snare, Machine Hat and Machine Open Hat factory patches. This
//! plays them as a plain two-bar 80s beat at 120 BPM -- kick on 1 and 3,
//! snare on 2 and 4, closed hats on the eighths, and one open hat on the "and"
//! of 4 in the first bar, which the next bar's first closed hat chokes -- and
//! then each hit on its own.
//!
//! Authored from `Project::starter_kit()` itself, saved with
//! `mooloop_project::save_song`, reopened with `load_bundle` (which must
//! repair nothing), and rendered offline to float WAVs. Then measured,
//! because an agent cannot listen: each hit's peak, and whether the choke
//! actually cuts the open hat.
//!
//! ```sh
//! scripts/antibox --no-incremental --pull target/starter-kit-audition \
//!     cargo run -p mooloop-engine --example starter_kit_audition -- target/starter-kit-audition
//! ```
//!
//! Then play `target/starter-kit-audition/starter-kit-80s.wav`, and
//! `kick.wav`, `snare.wav`, `closed-hat.wav` and `open-hat.wav` for the hits
//! on their own (`docs/FOCUS.md`, "Listening is a step").

use std::path::{Path, PathBuf};

use mooloop_core::{NoteEvent, Project};
use mooloop_engine::{ExportFormat, ExportSpec, OfflineRenderer, RenderScope, WavEncoding};

const SAMPLE_RATE: u32 = 48_000;
/// A sixteenth, in ticks.
const STEP: u32 = 24;
const STEPS_PER_BAR: u32 = 16;
/// The note every DS-01 patch plays at its written pitch.
const NOTE: u8 = 60;

const KICK: usize = 0;
const SNARE: usize = 1;
const CLOSED_HAT: usize = 2;
const OPEN_HAT: usize = 3;

/// The starter kit with `hits` written into its one pattern, `bars` long.
fn song(bars: u32, hits: &[(usize, u32, u8)]) -> Project {
    let mut project = Project::starter_kit();
    project.bpm = 120;
    project.pattern_lengths = vec![(bars * STEPS_PER_BAR) as u16];
    for &(channel, step, velocity) in hits {
        let channel = &mut project.channels[channel];
        let id = channel.next_note_id;
        channel.next_note_id += 1;
        channel.notes[0].push(NoteEvent::new(id, step * STEP, STEP, NOTE, velocity));
    }
    project
}

/// The beat: two bars, kick on 1 and 3, snare on 2 and 4, closed hats on the
/// eighths, and one open hat in place of the first bar's last closed hat.
fn beat() -> Project {
    let mut hits = Vec::new();
    for bar in 0..2 {
        let at = |step: u32| bar * STEPS_PER_BAR + step;
        hits.push((KICK, at(0), 115));
        hits.push((KICK, at(8), 115));
        hits.push((SNARE, at(4), 110));
        hits.push((SNARE, at(12), 110));
        for eighth in (0..STEPS_PER_BAR).step_by(2) {
            if bar == 0 && eighth == 14 {
                hits.push((OPEN_HAT, at(eighth), 100));
            } else {
                // A little lift on the off-beats, the way a drum machine's
                // accent is usually programmed.
                let velocity = if eighth % 4 == 0 { 90 } else { 75 };
                hits.push((CLOSED_HAT, at(eighth), velocity));
            }
        }
    }
    song(2, &hits)
}

fn save_and_reopen(project: &Project, path: &Path) -> Project {
    mooloop_project::save_song(path, project, mooloop_project::AssetMode::Referenced)
        .expect("the song saves");
    let report = mooloop_project::load_bundle(path).expect("the song reopens");
    assert!(report.repairs.is_empty(), "the loader repaired it: {:?}", report.repairs);
    let mooloop_project::LoadedDocument::Song(reopened) = report.document else {
        panic!("a song came back as something else");
    };
    reopened
}

/// The left channel of a float render (DS-01 is mono and the kit is centred).
fn render(project: &Project, path: &Path, tail_seconds: f32) -> Vec<f32> {
    OfflineRenderer::render(
        project,
        &[],
        SAMPLE_RATE,
        &ExportSpec {
            path: path.to_path_buf(),
            scope: RenderScope::Pattern { index: 0 },
            tail_seconds,
            format: ExportFormat::Wav(WavEncoding::Float32),
        },
    )
    .expect("the render");
    let mut reader = hound::WavReader::open(path).expect("the render reads back");
    let samples: Vec<f32> = reader.samples::<f32>().map(|s| s.expect("a sample")).collect();
    samples.chunks(2).map(|pair| pair[0]).collect()
}

fn db(value: f32) -> f32 {
    20.0 * value.max(1.0e-12).log10()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |high, s| high.max(s.abs()))
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// How long the hit takes to fall 40 dB under its own peak, in ms.
fn decay_ms(samples: &[f32]) -> f32 {
    let top = peak(samples);
    let floor = top * 0.01;
    let last = samples.iter().rposition(|s| s.abs() > floor).unwrap_or(0);
    let first = samples.iter().position(|s| s.abs() > floor).unwrap_or(0);
    (last - first) as f32 * 1_000.0 / SAMPLE_RATE as f32
}

fn main() {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "target/starter-kit-audition".into()),
    );
    std::fs::create_dir_all(&dir).expect("the output directory");

    let beat = save_and_reopen(&beat(), &dir.join("starter-kit-80s.mooloop"));
    println!("saved and reopened the beat with nothing repaired");
    let out = render(&beat, &dir.join("starter-kit-80s.wav"), 1.0);
    println!(
        "beat: {:.2} s, peak {:.1} dBFS, RMS {:.1} dBFS",
        out.len() as f32 / SAMPLE_RATE as f32,
        db(peak(&out)),
        db(rms(&out))
    );

    for (channel, file) in [
        (KICK, "kick"),
        (SNARE, "snare"),
        (CLOSED_HAT, "closed-hat"),
        (OPEN_HAT, "open-hat"),
    ] {
        let one = save_and_reopen(
            &song(1, &[(channel, 0, 110)]),
            &dir.join(format!("{file}.mooloop")),
        );
        let hit = render(&one, &dir.join(format!("{file}.wav")), 0.0);
        println!(
            "{file}: peak {:.1} dBFS, RMS over its first 100 ms {:.1} dBFS, 40 dB down after {:.0} ms",
            db(peak(&hit)),
            db(rms(&hit[..SAMPLE_RATE as usize / 10])),
            decay_ms(&hit)
        );
    }

    // The choke: an open hat, then a closed hat an eighth later. Past the
    // closed hat's own tail (it is at -80 dB by 90 ms) the open hat should
    // be gone; without the closed hat it is still ringing there.
    // An eighth at 120 BPM is a quarter of a second.
    let eighth = SAMPLE_RATE as usize / 4;
    let window = |signal: &[f32]| {
        let from = eighth + SAMPLE_RATE as usize / 10;
        rms(&signal[from..from + SAMPLE_RATE as usize * 15 / 100])
    };
    let open_alone = render(
        &song(1, &[(OPEN_HAT, 0, 110)]),
        &dir.join("choke-open-alone.wav"),
        0.0,
    );
    let choked = render(
        &song(1, &[(OPEN_HAT, 0, 110), (CLOSED_HAT, 2, 110)]),
        &dir.join("choke-open-then-closed.wav"),
        0.0,
    );
    println!(
        "choke: 100-250 ms after the closed hat, the open hat alone is {:.1} dBFS and choked is {:.1} dBFS",
        db(window(&open_alone)),
        db(window(&choked))
    );
}
