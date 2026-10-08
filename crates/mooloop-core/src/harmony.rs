//! Pitch, chords and modes for the patch's note boxes
//! (`docs/plans/song-patch/08-note-boxes.md`).
//!
//! The one place these tables live: the boxes' text, their faces and the
//! engine's note pass all read them. There is no song key in mooloop; a
//! box's root and mode are its own.

/// A chord's quality: the intervals above its root, in semitones.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChordQuality {
    #[default]
    Major,
    Minor,
    Diminished,
    Augmented,
    Sus2,
    Sus4,
    Major7,
    Minor7,
    Dominant7,
    Diminished7,
}

impl ChordQuality {
    pub const ALL: [Self; 10] = [
        Self::Major,
        Self::Minor,
        Self::Diminished,
        Self::Augmented,
        Self::Sus2,
        Self::Sus4,
        Self::Major7,
        Self::Minor7,
        Self::Dominant7,
        Self::Diminished7,
    ];

    /// How a box spells it: `maj`, `min7`, `7`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Major => "maj",
            Self::Minor => "min",
            Self::Diminished => "dim",
            Self::Augmented => "aug",
            Self::Sus2 => "sus2",
            Self::Sus4 => "sus4",
            Self::Major7 => "maj7",
            Self::Minor7 => "min7",
            Self::Dominant7 => "7",
            Self::Diminished7 => "dim7",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|quality| quality.name() == name)
    }

    /// Semitones above the root, the root first.
    pub const fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Sus2 => &[0, 2, 7],
            Self::Sus4 => &[0, 5, 7],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Diminished7 => &[0, 3, 6, 9],
        }
    }

    pub fn to_index(self) -> usize {
        Self::ALL.iter().position(|quality| *quality == self).unwrap_or(0)
    }

    pub fn from_index(index: i32) -> Self {
        Self::ALL[index.clamp(0, Self::ALL.len() as i32 - 1) as usize]
    }
}

/// A mode: the seven church modes, and harmonic and melodic minor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Major,
    Dorian,
    Phrygian,
    Lydian,
    Mixolydian,
    Minor,
    Locrian,
    HarmonicMinor,
    MelodicMinor,
}

impl Mode {
    pub const ALL: [Self; 9] = [
        Self::Major,
        Self::Dorian,
        Self::Phrygian,
        Self::Lydian,
        Self::Mixolydian,
        Self::Minor,
        Self::Locrian,
        Self::HarmonicMinor,
        Self::MelodicMinor,
    ];

    /// How a box spells it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Major => "major",
            Self::Dorian => "dorian",
            Self::Phrygian => "phrygian",
            Self::Lydian => "lydian",
            Self::Mixolydian => "mixolydian",
            Self::Minor => "minor",
            Self::Locrian => "locrian",
            Self::HarmonicMinor => "harmonic",
            Self::MelodicMinor => "melodic",
        }
    }

    /// A mode as typed: its name, or `ionian` and `aeolian` for major and
    /// minor.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "ionian" => Some(Self::Major),
            "aeolian" => Some(Self::Minor),
            _ => Self::ALL.into_iter().find(|mode| mode.name() == name),
        }
    }

    /// Its seven degrees, in semitones above the root.
    pub const fn degrees(self) -> [u8; 7] {
        match self {
            Self::Major => [0, 2, 4, 5, 7, 9, 11],
            Self::Dorian => [0, 2, 3, 5, 7, 9, 10],
            Self::Phrygian => [0, 1, 3, 5, 7, 8, 10],
            Self::Lydian => [0, 2, 4, 6, 7, 9, 11],
            Self::Mixolydian => [0, 2, 4, 5, 7, 9, 10],
            Self::Minor => [0, 2, 3, 5, 7, 8, 10],
            Self::Locrian => [0, 1, 3, 5, 6, 8, 10],
            Self::HarmonicMinor => [0, 2, 3, 5, 7, 8, 11],
            Self::MelodicMinor => [0, 2, 3, 5, 7, 9, 11],
        }
    }

    pub fn to_index(self) -> usize {
        Self::ALL.iter().position(|mode| *mode == self).unwrap_or(0)
    }

    pub fn from_index(index: i32) -> Self {
        Self::ALL[index.clamp(0, Self::ALL.len() as i32 - 1) as usize]
    }
}

/// The twelve pitch classes as a box spells them, from C.
pub const PITCH_NAMES: [&str; 12] = ["c", "c#", "d", "d#", "e", "f", "f#", "g", "g#", "a", "a#", "b"];

/// A pitch class as typed: `c`, `f#`, `bb`.
pub fn pitch_class(name: &str) -> Option<u8> {
    let name = name.to_ascii_lowercase();
    if let Some(at) = PITCH_NAMES.iter().position(|pitch| *pitch == name) {
        return Some(at as u8);
    }
    let natural = name.strip_suffix('b')?;
    let at = PITCH_NAMES.iter().position(|pitch| *pitch == natural)?;
    Some(((at + 11) % 12) as u8)
}

/// The most notes one note box can turn one note into.
pub const MAX_CHORD_NOTES: usize = 4;

/// Up to [`MAX_CHORD_NOTES`] pitches, as signed semitones: a note pushed out
/// of `0..=127` is the caller's to drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pitches {
    notes: [i16; MAX_CHORD_NOTES],
    len: u8,
}

impl Pitches {
    pub fn one(note: i16) -> Self {
        let mut pitches = Self::default();
        pitches.push(note);
        pitches
    }

    fn push(&mut self, note: i16) {
        if usize::from(self.len) < MAX_CHORD_NOTES {
            self.notes[usize::from(self.len)] = note;
            self.len += 1;
        }
    }

    pub fn as_slice(&self) -> &[i16] {
        &self.notes[..usize::from(self.len)]
    }
}

/// The chord of `quality` on `root`, in `inversion`: each inversion moves the
/// lowest note up an octave, so a triad has three positions and a seventh
/// four, and an inversion past the last wraps.
pub fn chord(root: u8, quality: ChordQuality, inversion: u8) -> Pitches {
    let intervals = quality.intervals();
    let mut notes = [0i16; MAX_CHORD_NOTES];
    for (slot, interval) in notes.iter_mut().zip(intervals) {
        *slot = i16::from(root) + i16::from(*interval);
    }
    let len = intervals.len();
    for _ in 0..usize::from(inversion) % len {
        notes[..len].sort_unstable();
        notes[0] += 12;
    }
    notes[..len].sort_unstable();
    let mut pitches = Pitches::default();
    for &note in &notes[..len] {
        pitches.push(note);
    }
    pitches
}

/// `note` moved to the nearest note of `mode` on `root`, down on a tie.
pub fn snap(note: u8, root: u8, mode: Mode) -> i16 {
    let note = i16::from(note);
    (0..=6)
        .flat_map(|distance: i16| [note - distance, note + distance])
        .find(|&candidate| in_mode(candidate, root, mode))
        .unwrap_or(note)
}

fn in_mode(note: i16, root: u8, mode: Mode) -> bool {
    let class = (note - i16::from(root)).rem_euclid(12) as u8;
    mode.degrees().contains(&class)
}

/// `note` snapped to `mode` on `root`, then that degree's diatonic chord
/// stacked up from it in thirds: a triad, or a seventh with `seventh`.
pub fn modal_chord(note: u8, root: u8, mode: Mode, seventh: bool) -> Pitches {
    let base = snap(note, root, mode);
    let degrees = mode.degrees();
    let class = (base - i16::from(root)).rem_euclid(12) as u8;
    let degree = degrees.iter().position(|&step| step == class).unwrap_or(0);
    let mut pitches = Pitches::default();
    for third in 0..if seventh { 4 } else { 3 } {
        let step = degree + 2 * third;
        let above = i16::from(degrees[step % 7]) - i16::from(degrees[degree]) + 12 * (step / 7) as i16;
        pitches.push(base + above);
    }
    pitches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_stack_their_intervals_and_invert_by_octaves() {
        assert_eq!(chord(60, ChordQuality::Minor7, 0).as_slice(), [60, 63, 67, 70]);
        assert_eq!(chord(60, ChordQuality::Minor7, 1).as_slice(), [63, 67, 70, 72]);
        assert_eq!(chord(60, ChordQuality::Minor7, 2).as_slice(), [67, 70, 72, 75]);
        assert_eq!(chord(60, ChordQuality::Major, 3).as_slice(), [60, 64, 67], "a triad wraps");
        assert_eq!(chord(60, ChordQuality::Dominant7, 0).as_slice(), [60, 64, 67, 70]);
        for quality in ChordQuality::ALL {
            assert_eq!(ChordQuality::from_name(quality.name()), Some(quality));
        }
    }

    #[test]
    fn snapping_goes_to_the_nearest_degree_and_down_on_a_tie() {
        // C major: C# sits between C and D.
        assert_eq!(snap(61, 0, Mode::Major), 60);
        assert_eq!(snap(66, 0, Mode::Major), 65, "F# to F, not G");
        assert_eq!(snap(64, 0, Mode::Major), 64);
        // D dorian has no C#: it goes down to C.
        assert_eq!(snap(61, 2, Mode::Dorian), 60);
    }

    #[test]
    fn a_modal_chord_is_its_degree_stacked_in_thirds() {
        // D dorian, from D: D F A C.
        assert_eq!(modal_chord(62, 2, Mode::Dorian, true).as_slice(), [62, 65, 69, 72]);
        // From E: E G B, the minor second degree.
        assert_eq!(modal_chord(64, 2, Mode::Dorian, false).as_slice(), [64, 67, 71]);
        // C major from B: B D F, diminished.
        assert_eq!(modal_chord(71, 0, Mode::Major, false).as_slice(), [71, 74, 77]);
    }

    #[test]
    fn pitch_names_read_sharps_and_flats() {
        assert_eq!(pitch_class("d"), Some(2));
        assert_eq!(pitch_class("F#"), Some(6));
        assert_eq!(pitch_class("bb"), Some(10));
        assert_eq!(pitch_class("cb"), Some(11));
        assert_eq!(pitch_class("h"), None);
        for mode in Mode::ALL {
            assert_eq!(Mode::from_name(mode.name()), Some(mode));
        }
    }
}
