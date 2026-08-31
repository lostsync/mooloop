//! Document integrity: one traversal that both explains what is wrong with a
//! document and, on request, puts it right.
//!
//! Saving used to stop at the first failed range check and hand back a single
//! sentence with no location and no way to act on it. Two things were wrong
//! with that. A song the user cannot save is a song they lose, and a song the
//! user cannot load is worse -- the same checks guard both doors. So every
//! check here comes in a pair: what is wrong, and, wherever the correction
//! keeps every note, clip, and setting the user authored, how to put it right.
//! [`repair_project`] applies those corrections. What survives a repair pass
//! is only the class of problem whose sole fix would throw authored work away
//! (a pattern over the note ceiling, a channel bank past the address space),
//! and those are reported with the exact location and count needed to fix them
//! by hand.
//!
//! Detection and correction deliberately share one code path. A check and its
//! repair are one match arm, not two functions that drift apart the first time
//! a range moves.

use std::collections::HashSet;
use std::fmt;

use mooloop_core::{
    sanitize_route, BusSetup, ChannelSetup, ChannelSource, DrumSynthParams, EffectSlotState,
    MlM1Params, ModRack, MonoSynthParams, NoteId, PolySynthParams, Project, ProjectChannel,
    SamplerParams, DEFAULT_STEPS, MASTER_BUS, MAX_AUTOMATION_LANES_PER_CHANNEL,
    MAX_AUTOMATION_POINTS_PER_LANE, MAX_BUSES, MAX_CHANNELS, MAX_CHOKE_GROUP,
    MAX_NOTES_PER_CHANNEL_PATTERN, MAX_PATTERNS, MAX_PATTERN_STEPS, MAX_PLAYLIST_PLACEMENTS,
    MAX_PLAYLIST_TICKS, MAX_POLY_VOICES, MAX_SAMPLER_VOICES, TICKS_PER_STEP,
};

use crate::DocumentKind;

/// Longest channel name the format stores, in bytes.
const MAX_CHANNEL_NAME: usize = 128;

/// One thing wrong with a document, located precisely enough to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// Stable machine tag for the check, e.g. `channel.source.range`. Meant
    /// for a bug report and for tests to match on; never shown on its own.
    pub code: &'static str,
    /// Where it is, in the words the user sees on screen: `Channel 4 "Bass",
    /// pattern 2`. Channels and patterns are numbered from one here, as they
    /// are in the UI, not from zero as they are on disk.
    pub location: String,
    /// What is wrong, in plain language, including the offending value.
    pub problem: String,
    /// What can be done about it, and what that would cost.
    pub remedy: Remedy,
    /// Whether this traversal actually applied the remedy. Only a
    /// [`Remedy::Safe`] on a repairing pass is ever applied.
    pub repaired: bool,
}

/// What a check knows how to do about the problem it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remedy {
    /// A correction that keeps every note, clip, and setting the user made.
    /// Reads as an instruction: "set the tempo to 999".
    Safe(String),
    /// The only correction would discard authored work. Reads as its price --
    /// "delete 3 notes from it" -- said out loud so the user can decide,
    /// rather than done quietly on their behalf.
    Costly(String),
    /// Nothing to suggest.
    None,
}

impl Issue {
    /// True when this issue stops a save or load: nothing corrected it.
    pub fn is_blocking(&self) -> bool {
        !self.repaired
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.location, self.problem)?;
        match (&self.remedy, self.repaired) {
            (Remedy::Safe(fix), true) => write!(f, " (corrected: {fix})"),
            (Remedy::Safe(fix), false) => write!(f, " (can be corrected: {fix})"),
            (Remedy::Costly(cost), _) => write!(f, " (fixing it would {cost})"),
            (Remedy::None, _) => Ok(()),
        }
    }
}

/// Everything one traversal found, plus enough about the document's shape to
/// triage a report without the file itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnosis {
    pub document: DocumentKind,
    pub issues: Vec<Issue>,
    /// Document shape (`channels`, `patterns`, ...), for the copyable report.
    pub context: Vec<(&'static str, String)>,
}

impl Diagnosis {
    /// Problems this pass could not correct. A save or load must stop when
    /// this is non-empty, and only then.
    pub fn blocking(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|issue| issue.is_blocking())
    }

    /// Corrections this pass applied, in the order it made them.
    pub fn repairs(&self) -> impl Iterator<Item = &Issue> {
        self.issues.iter().filter(|issue| issue.repaired)
    }

    /// True when the document needed nothing at all.
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// True when nothing stops the document from being written or opened.
    pub fn is_usable(&self) -> bool {
        self.blocking().next().is_none()
    }

    /// The whole thing as text, for the clipboard. Everything a bug report
    /// needs and nothing that identifies the machine: versions, document
    /// shape, every problem with its code, and every correction applied.
    pub fn report(&self) -> String {
        let mut out = String::new();
        let blocking = self.blocking().count();
        let repaired = self.repairs().count();
        out.push_str("mooloop document diagnostics\n");
        out.push_str(&format!("mooloop-project: {}\n", env!("CARGO_PKG_VERSION")));
        out.push_str(&format!("format version:  {}\n", crate::FORMAT_VERSION));
        out.push_str(&format!("document:        {}\n", self.document.as_str()));
        for (key, value) in &self.context {
            out.push_str(&format!("{key:<16} {value}\n"));
        }
        out.push_str(&format!(
            "result:          {blocking} unfixable, {repaired} corrected\n"
        ));

        if blocking > 0 {
            out.push_str("\nCOULD NOT BE FIXED -- these need a change in the app:\n");
            for (n, issue) in self.blocking().enumerate() {
                out.push_str(&format!("{:>3}. [{}]\n", n + 1, issue.code));
                out.push_str(&format!("     {}\n", issue.location));
                out.push_str(&format!("     {}\n", issue.problem));
                match &issue.remedy {
                    Remedy::Safe(fix) => out.push_str(&format!("     not applied: {fix}\n")),
                    Remedy::Costly(cost) => out.push_str(&format!("     only fix would {cost}\n")),
                    Remedy::None => out.push_str("     no automatic fix\n"),
                }
            }
        }

        if repaired > 0 {
            out.push_str("\nCORRECTED AUTOMATICALLY:\n");
            for (n, issue) in self.repairs().enumerate() {
                out.push_str(&format!("{:>3}. [{}]\n", n + 1, issue.code));
                out.push_str(&format!("     {}\n", issue.location));
                out.push_str(&format!("     {}\n", issue.problem));
                if let Remedy::Safe(fix) = &issue.remedy {
                    out.push_str(&format!("     -> {fix}\n"));
                }
            }
        }
        out
    }
}

/// The blocking problems, in plain language, one per line. This is what a
/// failed save shows first; [`Diagnosis::report`] has the rest.
impl fmt::Display for Diagnosis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let blocking: Vec<&Issue> = self.blocking().collect();
        let kind = self.document.as_str();
        match blocking.len() {
            0 => write!(f, "the {kind} is fine")?,
            1 => write!(f, "one problem in this {kind} could not be corrected:")?,
            n => write!(f, "{n} problems in this {kind} could not be corrected:")?,
        }
        for issue in &blocking {
            write!(
                f,
                "\n  \u{2022} {} \u{2014} {}",
                issue.location, issue.problem
            )?;
            match &issue.remedy {
                Remedy::Safe(fix) => write!(f, ". It can be corrected: {fix}.")?,
                Remedy::Costly(cost) => {
                    write!(f, ". Correcting it would {cost}, so mooloop did not.")?
                }
                Remedy::None => write!(f, ".")?,
            }
        }
        let repaired = self.repairs().count();
        if repaired > 0 {
            let other = if blocking.is_empty() { "" } else { " other" };
            let plural = if repaired == 1 { "" } else { "s" };
            write!(
                f,
                "\n({repaired}{other} problem{plural} were corrected automatically.)"
            )?;
        }
        Ok(())
    }
}

/// Report everything wrong with `project` without changing it.
pub fn inspect_project(project: &Project) -> Diagnosis {
    walk_project(&mut project.clone(), false)
}

/// Correct everything in `project` that can be corrected without discarding
/// authored work, and report both what was corrected and what was left.
pub fn repair_project(project: &mut Project) -> Diagnosis {
    walk_project(project, true)
}

/// Report everything wrong with a bare channel bank (a kit, a channel preset)
/// without changing it.
pub fn inspect_setups(document: DocumentKind, setups: &[ChannelSetup]) -> Diagnosis {
    walk_setups(document, &mut setups.to_vec(), false)
}

/// Correct a bare channel bank in place. Used by kit, channel, and generator
/// presets, which have no patterns or playlist to check.
pub fn repair_setups(document: DocumentKind, setups: &mut [ChannelSetup]) -> Diagnosis {
    walk_setups(document, setups, true)
}

/// Correct a lone generator's parameters in place.
pub fn repair_source(document: DocumentKind, source: &mut ChannelSource) -> Diagnosis {
    let mut doctor = Doctor::new(true);
    check_source(&mut doctor, "This preset", source);
    Diagnosis {
        document,
        issues: doctor.issues,
        context: Vec::new(),
    }
}

fn walk_project(project: &mut Project, apply: bool) -> Diagnosis {
    let mut doctor = Doctor::new(apply);
    check_project(&mut doctor, project);
    Diagnosis {
        document: DocumentKind::Song,
        context: vec![
            ("channels:", project.channels.len().to_string()),
            ("patterns:", project.pattern_lengths.len().to_string()),
            ("playlist:", project.playlist.len().to_string()),
            ("buses:", project.buses.len().to_string()),
            (
                "notes:",
                project
                    .channels
                    .iter()
                    .map(|channel| channel.notes.iter().map(Vec::len).sum::<usize>())
                    .sum::<usize>()
                    .to_string(),
            ),
        ],
        issues: doctor.issues,
    }
}

fn walk_setups(document: DocumentKind, setups: &mut [ChannelSetup], apply: bool) -> Diagnosis {
    let mut doctor = Doctor::new(apply);
    if setups.is_empty() || setups.len() > MAX_CHANNELS {
        doctor.block(
            "setups.count",
            "This document",
            format!(
                "it holds {} channels; a document must have between 1 and {MAX_CHANNELS}",
                setups.len()
            ),
        );
    }
    for (index, setup) in setups.iter_mut().enumerate() {
        check_setup(&mut doctor, index, setup);
    }
    Diagnosis {
        document,
        context: vec![("channels:", setups.len().to_string())],
        issues: doctor.issues,
    }
}

// --- The traversal ---------------------------------------------------------

/// Collects issues, and applies their corrections when `apply` is set. Every
/// check goes through one of its `fit*`/`correct`/`block` methods, so a new
/// check cannot forget to say where it is or what it would do about it.
struct Doctor {
    issues: Vec<Issue>,
    apply: bool,
}

impl Doctor {
    fn new(apply: bool) -> Self {
        Self {
            issues: Vec::new(),
            apply,
        }
    }

    /// Record a problem that has a correction keeping everything the user
    /// authored. Returns whether the caller should now apply it.
    fn correct(
        &mut self,
        code: &'static str,
        location: impl Into<String>,
        problem: String,
        fix: String,
    ) -> bool {
        self.issues.push(Issue {
            code,
            location: location.into(),
            problem,
            remedy: Remedy::Safe(fix),
            repaired: self.apply,
        });
        self.apply
    }

    /// Record a problem whose only correction would discard authored work.
    /// `cost` completes "fixing it would ...", so the user can decide.
    fn refuse(
        &mut self,
        code: &'static str,
        location: impl Into<String>,
        problem: String,
        cost: String,
    ) {
        self.issues.push(Issue {
            code,
            location: location.into(),
            problem,
            remedy: Remedy::Costly(cost),
            repaired: false,
        });
    }

    /// Record a problem with no correction at all.
    fn block(&mut self, code: &'static str, location: impl Into<String>, problem: String) {
        self.issues.push(Issue {
            code,
            location: location.into(),
            problem,
            remedy: Remedy::None,
            repaired: false,
        });
    }

    /// Pull a real-valued setting back inside its range. Also catches NaN and
    /// infinity, which no range test does on its own.
    fn fit(
        &mut self,
        code: &'static str,
        location: &str,
        field: &str,
        value: &mut f32,
        min: f32,
        max: f32,
    ) {
        if value.is_finite() && (min..=max).contains(value) {
            return;
        }
        let found = *value;
        // A NaN clamps to nothing, so it takes the in-range value nearest
        // zero: NaNs arrive through arithmetic on depths and offsets, where
        // "no contribution" is the honest reading.
        let corrected = if found.is_nan() {
            0.0f32.clamp(min, max)
        } else {
            found.clamp(min, max)
        };
        if self.correct(
            code,
            location,
            format!(
                "{field} is {}, outside the allowed range {} to {}",
                number(found),
                number(min),
                number(max)
            ),
            format!("set {field} to {}", number(corrected)),
        ) {
            *value = corrected;
        }
    }

    /// The whole-number counterpart of [`Doctor::fit`].
    fn fit_int<T>(
        &mut self,
        code: &'static str,
        location: &str,
        field: &str,
        value: &mut T,
        min: T,
        max: T,
    ) where
        T: Ord + Copy + fmt::Display,
    {
        if (min..=max).contains(value) {
            return;
        }
        let found = *value;
        let corrected = found.clamp(min, max);
        if self.correct(
            code,
            location,
            format!("{field} is {found}, outside the allowed range {min} to {max}"),
            format!("set {field} to {corrected}"),
        ) {
            *value = corrected;
        }
    }

    /// Replace a non-finite value that nothing else ranges. Effect and
    /// modulation settings are not range-checked on save -- their descriptors
    /// already bound what the UI can write -- but a NaN reaching the audio
    /// thread silences a channel, so it is worth catching on the way past.
    fn fit_finite(
        &mut self,
        code: &'static str,
        location: &str,
        field: &str,
        value: &mut f32,
        fallback: f32,
    ) {
        if value.is_finite() {
            return;
        }
        let found = *value;
        if self.correct(
            code,
            location,
            format!("{field} is {}, which is not a usable number", number(found)),
            format!("set {field} back to {}", number(fallback)),
        ) {
            *value = fallback;
        }
    }
}

fn check_project(doctor: &mut Doctor, project: &mut Project) {
    const SONG: &str = "Song settings";

    if project.ppq != 96 {
        let found = project.ppq;
        if doctor.correct(
            "song.ppq",
            SONG,
            format!("the timebase is {found} ticks per quarter note; this format stores only 96"),
            "set the timebase to 96 ticks per quarter note".into(),
        ) {
            project.ppq = 96;
        }
    }
    if project.beats_per_bar != 4 {
        let found = project.beats_per_bar;
        if doctor.correct(
            "song.meter",
            SONG,
            format!("the meter is {found}/4; this format stores only 4/4"),
            "set the meter to 4/4".into(),
        ) {
            project.beats_per_bar = 4;
        }
    }
    doctor.fit_int("song.bpm", SONG, "the tempo", &mut project.bpm, 1, 999);
    doctor.fit_int(
        "song.swing",
        SONG,
        "swing",
        &mut project.swing_percent,
        mooloop_core::MIN_SWING_PERCENT,
        mooloop_core::MAX_SWING_PERCENT,
    );

    check_patterns(doctor, project);
    check_buses(doctor, project);

    if project.channels.is_empty() {
        let patterns = project.pattern_lengths.len().max(1);
        if doctor.correct(
            "song.channels.empty",
            SONG,
            "the song has no channels at all".into(),
            "add one empty sampler channel so the song can be opened".into(),
        ) {
            project.channels.push(ProjectChannel::sampler(0, patterns));
        }
    } else if project.channels.len() > MAX_CHANNELS {
        let found = project.channels.len();
        doctor.refuse(
            "song.channels.count",
            SONG,
            format!("the song has {found} channels; the audio engine addresses {MAX_CHANNELS}"),
            format!("delete {} channels", found - MAX_CHANNELS),
        );
    }

    // Clamped against what survived the checks above, so an out-of-range
    // selection is repaired against the real bank rather than the broken one.
    let last_channel = project.channels.len().saturating_sub(1);
    doctor.fit_int(
        "song.selected_channel",
        SONG,
        "the selected channel",
        &mut project.selected_channel,
        0,
        u8::try_from(last_channel).unwrap_or(u8::MAX),
    );
    let last_pattern = project.pattern_lengths.len().saturating_sub(1);
    doctor.fit_int(
        "song.current_pattern",
        SONG,
        "the open pattern",
        &mut project.current_pattern,
        0,
        u16::try_from(last_pattern).unwrap_or(u16::MAX),
    );

    check_playlist(doctor, project);

    let pattern_count = project.pattern_lengths.len();
    for (index, channel) in project.channels.iter_mut().enumerate() {
        check_setup(doctor, index, &mut channel.setup);
        check_channel_banks(doctor, index, channel, pattern_count);
    }
}

fn check_patterns(doctor: &mut Doctor, project: &mut Project) {
    const SONG: &str = "Song settings";

    if project.pattern_lengths.is_empty() {
        if doctor.correct(
            "song.patterns.empty",
            SONG,
            "the song has no patterns at all".into(),
            format!("add one empty {DEFAULT_STEPS}-step pattern"),
        ) {
            project.pattern_lengths.push(DEFAULT_STEPS);
        }
    } else if project.pattern_lengths.len() > MAX_PATTERNS {
        let found = project.pattern_lengths.len();
        doctor.refuse(
            "song.patterns.count",
            SONG,
            format!("the song has {found} patterns; the format stores {MAX_PATTERNS}"),
            format!("delete {} patterns", found - MAX_PATTERNS),
        );
    }
    for (index, length) in project.pattern_lengths.iter_mut().enumerate() {
        doctor.fit_int(
            "song.pattern.length",
            &format!("Pattern {}", index + 1),
            "the length",
            length,
            1,
            MAX_PATTERN_STEPS,
        );
    }
}

fn check_buses(doctor: &mut Doctor, project: &mut Project) {
    const MIXER: &str = "Mixer";

    if project.buses.len() != MAX_BUSES {
        let found = project.buses.len();
        // Every index exists whether or not anything feeds it, so a short
        // bank is a missing destination rather than a smaller mixer.
        if doctor.correct(
            "song.buses.count",
            MIXER,
            format!("the mixer has {found} buses; it always has {MAX_BUSES}"),
            format!("restore the missing buses (it will have {MAX_BUSES})"),
        ) {
            project.buses.truncate(MAX_BUSES);
            for index in project.buses.len()..MAX_BUSES {
                project.buses.push(BusSetup::new(index));
            }
        }
    }

    for (index, setup) in project.buses.iter_mut().enumerate() {
        let name = if index == MASTER_BUS as usize {
            "Master bus".to_string()
        } else {
            format!("Bus {index}")
        };
        doctor.fit(
            "song.bus.volume",
            &name,
            "the output level",
            &mut setup.bus.volume,
            0.0,
            mooloop_core::MAX_LINEAR_GAIN,
        );
        doctor.fit(
            "song.bus.pan",
            &name,
            "the pan",
            &mut setup.bus.pan,
            -1.0,
            1.0,
        );

        let bus = u8::try_from(index).unwrap_or(u8::MAX);
        let routed = sanitize_route(bus, setup.bus.output);
        if index != MASTER_BUS as usize && routed != setup.bus.output {
            let found = setup.bus.output;
            if doctor.correct(
                "song.bus.output",
                &name,
                format!("it feeds bus {found}, which it cannot reach"),
                "feed it into the master bus instead".into(),
            ) {
                setup.bus.output = routed;
            }
        }

        if setup.effects.len() > mooloop_core::MAX_EFFECTS_PER_CHANNEL {
            let found = setup.effects.len();
            doctor.refuse(
                "song.bus.effects",
                &name,
                format!(
                    "it has {found} effects; the audio engine addresses {}",
                    mooloop_core::MAX_EFFECTS_PER_CHANNEL
                ),
                format!(
                    "delete {} effects from it",
                    found - mooloop_core::MAX_EFFECTS_PER_CHANNEL
                ),
            );
        }
        for (slot, effect) in setup.effects.iter_mut().enumerate() {
            check_effect(doctor, &name, slot, effect);
        }
    }
}

fn check_playlist(doctor: &mut Doctor, project: &mut Project) {
    if project.playlist.len() > MAX_PLAYLIST_PLACEMENTS {
        let found = project.playlist.len();
        doctor.refuse(
            "song.playlist.count",
            "Playlist",
            format!("it has {found} clips; the format stores {MAX_PLAYLIST_PLACEMENTS}"),
            format!("delete {} clips", found - MAX_PLAYLIST_PLACEMENTS),
        );
    }
    let last_pattern =
        u8::try_from(project.pattern_lengths.len().saturating_sub(1)).unwrap_or(u8::MAX);
    for (index, placement) in project.playlist.iter_mut().enumerate() {
        let location = format!("Playlist clip {}", index + 1);
        doctor.fit_int(
            "song.playlist.pattern",
            &location,
            "the pattern it plays",
            &mut placement.pattern,
            0,
            last_pattern,
        );
        doctor.fit_int(
            "song.playlist.start",
            &location,
            "its start position",
            &mut placement.start_tick,
            0,
            MAX_PLAYLIST_TICKS - 1,
        );
    }
}

/// The pattern-indexed banks and the notes and automation inside them.
fn check_channel_banks(
    doctor: &mut Doctor,
    index: usize,
    channel: &mut ProjectChannel,
    pattern_count: usize,
) {
    let who = channel_name(index, &channel.setup);

    if channel.notes.len() != pattern_count {
        let found = channel.notes.len();
        // Extra banks address patterns that no longer exist, so nothing
        // reachable is lost by dropping them; a short bank is simply empty.
        if doctor.correct(
            "channel.notes.banks",
            &who,
            format!("it stores notes for {found} patterns, but the song has {pattern_count}"),
            format!("give it one note bank per pattern ({pattern_count})"),
        ) {
            channel.notes.resize_with(pattern_count, Vec::new);
            // Automation is padded rather than checked here: a bank shorter
            // than the notes is the legitimate shape of a song written before
            // clip automation, so squaring it up is part of this fix instead
            // of a second problem to report.
            channel.automation.resize_with(pattern_count, Vec::new);
        }
    }
    // A song written before clip automation carries none, and one written
    // before a pattern was added carries fewer banks than it has patterns.
    // Both are padded on load, so only a surplus is a real problem.
    if channel.automation.len() > channel.notes.len() {
        let found = channel.automation.len();
        let want = channel.notes.len();
        if doctor.correct(
            "channel.automation.banks",
            &who,
            format!("it stores automation for {found} patterns, but has {want} note banks"),
            format!("drop the {} unreachable automation banks", found - want),
        ) {
            channel.automation.truncate(want);
        }
    }

    for (pattern, notes) in channel.notes.iter_mut().enumerate() {
        let where_ = format!("{who}, pattern {}", pattern + 1);
        if notes.len() > MAX_NOTES_PER_CHANNEL_PATTERN {
            let found = notes.len();
            doctor.refuse(
                "channel.notes.count",
                &where_,
                format!(
                    "it holds {found} notes; one pattern stores {MAX_NOTES_PER_CHANNEL_PATTERN}"
                ),
                format!(
                    "delete {} notes from it",
                    found - MAX_NOTES_PER_CHANNEL_PATTERN
                ),
            );
        }
        check_notes(doctor, &where_, notes);
    }

    // Ids are handed out from this counter, so a counter that has fallen
    // behind the notes hands out a duplicate on the very next edit. This is
    // where most duplicate ids come from; fixing it here closes the source
    // rather than the symptom.
    let highest = channel
        .notes
        .iter()
        .flatten()
        .map(|note| note.id)
        .max()
        .unwrap_or(0);
    if channel.next_note_id <= highest {
        let found = channel.next_note_id;
        if doctor.correct(
            "channel.next_note_id",
            &who,
            format!(
                "its next note id is {found}, but a note already uses {highest}; \
                 the next note drawn would collide with one already there"
            ),
            format!("move the counter past every note in use ({})", highest + 1),
        ) {
            channel.recompute_next_note_id();
        }
    }

    for (pattern, lanes) in channel.automation.iter_mut().enumerate() {
        let where_ = format!("{who}, pattern {}", pattern + 1);
        check_lanes(doctor, &where_, lanes);
    }
}

fn check_notes(doctor: &mut Doctor, where_: &str, notes: &mut [mooloop_core::NoteEvent]) {
    let capacity_ticks = u32::from(MAX_PATTERN_STEPS) * TICKS_PER_STEP;

    // Ids only have to be unique inside one pattern, so the pool to avoid is
    // this pattern's. Collect it up front: a replacement must dodge ids that
    // come later in the list as well as ones already passed.
    let mut used: HashSet<NoteId> = notes.iter().map(|note| note.id).collect();
    let mut next = used
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
        .wrapping_add(1)
        .max(1);
    let mut seen: HashSet<NoteId> = HashSet::with_capacity(notes.len());

    for note in notes.iter_mut() {
        if note.id == 0 || !seen.insert(note.id) {
            let found = note.id;
            let fresh = loop {
                let candidate = next;
                next = next.wrapping_add(1).max(1);
                if candidate != 0 && !used.contains(&candidate) {
                    break candidate;
                }
            };
            let problem = if found == 0 {
                "a note has no id, so editing it would move a different note".to_string()
            } else {
                format!("two notes share the id {found}, so editing one would move both")
            };
            if doctor.correct(
                "channel.note.id",
                where_,
                problem,
                format!("give the second note its own id ({fresh})"),
            ) {
                note.id = fresh;
                seen.insert(fresh);
                used.insert(fresh);
            }
        }
        doctor.fit_int(
            "channel.note.start",
            where_,
            "a note's start position",
            &mut note.start_tick,
            0,
            capacity_ticks - 1,
        );
        // Spelled out rather than run through `fit_int`, which would offer
        // the user a range ending at 4294967295.
        if note.duration_ticks == 0
            && doctor.correct(
                "channel.note.duration",
                where_,
                "a note has no length, so it would never sound".to_string(),
                "give it the shortest length that plays".into(),
            )
        {
            note.duration_ticks = 1;
        }
        doctor.fit_int(
            "channel.note.pitch",
            where_,
            "a note's pitch",
            &mut note.note,
            0,
            127,
        );
        doctor.fit_int(
            "channel.note.velocity",
            where_,
            "a note's velocity",
            &mut note.velocity,
            1,
            127,
        );
    }
}

fn check_lanes(doctor: &mut Doctor, where_: &str, lanes: &mut Vec<mooloop_core::AutomationLane>) {
    if lanes.len() > MAX_AUTOMATION_LANES_PER_CHANNEL {
        let found = lanes.len();
        doctor.refuse(
            "channel.automation.count",
            where_,
            format!(
                "it has {found} automation lanes; one pattern stores \
                 {MAX_AUTOMATION_LANES_PER_CHANNEL}"
            ),
            format!(
                "delete {} lanes from it",
                found - MAX_AUTOMATION_LANES_PER_CHANNEL
            ),
        );
    }

    // Only one lane per destination can be honoured, and the editor addresses
    // lanes by destination, so a second lane on the same knob is invisible as
    // well as ambiguous. Keep whichever holds more, drop the other, and say
    // exactly what went.
    let mut keep: Vec<usize> = Vec::with_capacity(lanes.len());
    let mut drop: Vec<usize> = Vec::new();
    for index in 0..lanes.len() {
        match keep
            .iter()
            .position(|kept| lanes[*kept].target == lanes[index].target)
        {
            None => keep.push(index),
            Some(slot) => {
                let kept = keep[slot];
                let (loser, winner) = if lanes[index].points().len() > lanes[kept].points().len() {
                    keep[slot] = index;
                    (kept, index)
                } else {
                    (index, kept)
                };
                let lost = lanes[loser].points().len();
                let held = lanes[winner].points().len();
                if doctor.correct(
                    "channel.automation.duplicate",
                    where_,
                    format!(
                        "two automation lanes drive the same control, one with {held} points \
                         and one with {lost}; only one of them can ever play"
                    ),
                    format!("keep the lane with {held} points and drop the one with {lost}"),
                ) {
                    drop.push(loser);
                }
            }
        }
    }
    if !drop.is_empty() {
        let dropped: HashSet<usize> = drop.into_iter().collect();
        let mut index = 0;
        lanes.retain(|_| {
            let keep = !dropped.contains(&index);
            index += 1;
            keep
        });
    }

    let capacity_ticks = u32::from(MAX_PATTERN_STEPS) * TICKS_PER_STEP;
    for lane in lanes.iter_mut() {
        if lane.points().len() > MAX_AUTOMATION_POINTS_PER_LANE {
            let found = lane.points().len();
            doctor.refuse(
                "channel.automation.points",
                where_,
                format!(
                    "one automation lane holds {found} points; a lane stores \
                     {MAX_AUTOMATION_POINTS_PER_LANE}"
                ),
                format!(
                    "delete {} points from it",
                    found - MAX_AUTOMATION_POINTS_PER_LANE
                ),
            );
            continue;
        }

        let mut points = lane.points().to_vec();
        let mut used: HashSet<mooloop_core::PointId> =
            points.iter().map(|point| point.id).collect();
        let mut next = used
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .wrapping_add(1)
            .max(1);
        let mut seen = HashSet::with_capacity(points.len());
        let mut changed = false;
        for point in points.iter_mut() {
            if point.id == 0 || !seen.insert(point.id) {
                let fresh = loop {
                    let candidate = next;
                    next = next.wrapping_add(1).max(1);
                    if candidate != 0 && !used.contains(&candidate) {
                        break candidate;
                    }
                };
                if doctor.correct(
                    "channel.automation.point.id",
                    where_,
                    "two automation points share one id, so dragging one would move both"
                        .to_string(),
                    format!("give the second point its own id ({fresh})"),
                ) {
                    point.id = fresh;
                    seen.insert(fresh);
                    used.insert(fresh);
                    changed = true;
                }
            }
            let before = (point.tick, point.value);
            doctor.fit_int(
                "channel.automation.point.tick",
                where_,
                "an automation point's position",
                &mut point.tick,
                0,
                capacity_ticks - 1,
            );
            doctor.fit(
                "channel.automation.point.value",
                where_,
                "an automation point's value",
                &mut point.value,
                0.0,
                1.0,
            );
            changed |= before != (point.tick, point.value);
        }
        if changed {
            lane.reset_points(points);
        }
    }
}

fn check_setup(doctor: &mut Doctor, index: usize, setup: &mut ChannelSetup) {
    let who = channel_name(index, setup);

    let trimmed = setup.channel.name.trim();
    if trimmed.is_empty() {
        let fallback = format!("Channel {}", index + 1);
        if doctor.correct(
            "channel.name.empty",
            &who,
            "it has no name, so nothing in the mixer identifies it".into(),
            format!("name it \"{fallback}\""),
        ) {
            setup.channel.name = fallback;
        }
    } else if setup.channel.name.len() > MAX_CHANNEL_NAME {
        let shortened = truncate_on_boundary(&setup.channel.name, MAX_CHANNEL_NAME);
        if doctor.correct(
            "channel.name.length",
            &who,
            format!(
                "its name is {} bytes long; the format stores {MAX_CHANNEL_NAME}",
                setup.channel.name.len()
            ),
            format!("shorten the name to \"{shortened}\""),
        ) {
            setup.channel.name = shortened;
        }
    }

    doctor.fit(
        "channel.volume",
        &who,
        "the output level",
        &mut setup.channel.volume,
        0.0,
        mooloop_core::MAX_LINEAR_GAIN,
    );
    doctor.fit(
        "channel.pan",
        &who,
        "the pan",
        &mut setup.channel.pan,
        -1.0,
        1.0,
    );
    doctor.fit_int(
        "channel.bus",
        &who,
        "the mixer bus it feeds",
        &mut setup.channel.bus,
        0,
        u8::try_from(MAX_BUSES - 1).unwrap_or(u8::MAX),
    );

    // The source holds the parameters, so it is the one telling the truth
    // about what kind of device this is.
    if setup.channel.kind != setup.source.kind() {
        let found = setup.channel.kind;
        let actual = setup.source.kind();
        if doctor.correct(
            "channel.kind",
            &who,
            format!("the mixer calls it a {found:?} but it holds {actual:?} settings"),
            format!("call it a {actual:?}, matching the settings it actually has"),
        ) {
            setup.channel.kind = actual;
        }
    }

    if setup.effects.len() > mooloop_core::MAX_EFFECTS_PER_CHANNEL {
        let found = setup.effects.len();
        doctor.refuse(
            "channel.effects.count",
            &who,
            format!(
                "it has {found} effects; the audio engine addresses {}",
                mooloop_core::MAX_EFFECTS_PER_CHANNEL
            ),
            format!(
                "delete {} effects from it",
                found - mooloop_core::MAX_EFFECTS_PER_CHANNEL
            ),
        );
    }
    for (slot, effect) in setup.effects.iter_mut().enumerate() {
        check_effect(doctor, &who, slot, effect);
    }
    check_modulation(doctor, &who, &mut setup.modulation);
    check_source(doctor, &who, &mut setup.source);
}

/// Effect settings are bounded by their descriptors on the way in, so this
/// only looks for values no range check would catch.
fn check_effect(doctor: &mut Doctor, who: &str, slot: usize, effect: &mut EffectSlotState) {
    let kind = effect.params.kind();
    let where_ = format!("{who}, {kind:?} in slot {}", slot + 1);
    doctor.fit_finite(
        "effect.wet_dry",
        &where_,
        "its wet/dry mix",
        &mut effect.wet_dry,
        1.0,
    );
    doctor.fit_finite(
        "effect.input_trim",
        &where_,
        "its input trim",
        &mut effect.input_trim,
        1.0,
    );
    doctor.fit_finite(
        "effect.output_trim",
        &where_,
        "its output trim",
        &mut effect.output_trim,
        1.0,
    );
    for descriptor in kind.descriptors() {
        let Some(value) = effect.params.get(descriptor.id) else {
            continue;
        };
        if value.is_finite() {
            continue;
        }
        if doctor.correct(
            "effect.param",
            &where_,
            format!(
                "{} is {}, which is not a usable number",
                descriptor.name,
                number(value)
            ),
            format!(
                "set {} back to {}",
                descriptor.name,
                number(descriptor.default)
            ),
        ) {
            effect.params.set(descriptor.id, descriptor.default);
        }
    }
}

fn check_modulation(doctor: &mut Doctor, who: &str, rack: &mut ModRack) {
    for (index, route) in rack.routes.iter_mut().enumerate() {
        let Some(route) = route else { continue };
        doctor.fit_finite(
            "modulation.depth",
            &format!("{who}, modulation route {}", index + 1),
            "its depth",
            &mut route.depth,
            0.0,
        );
    }
}

fn check_source(doctor: &mut Doctor, who: &str, source: &mut ChannelSource) {
    match source {
        ChannelSource::Sampler(state) => check_sampler(doctor, who, &mut state.params),
        ChannelSource::DrumSynth(state) => check_drum_synth(doctor, who, &mut state.params),
        ChannelSource::MonoSynth(state) => check_mono_synth(doctor, who, &mut state.params),
        ChannelSource::PolySynth(state) => check_poly_synth(doctor, who, &mut state.params),
        ChannelSource::MlM1(state) => check_mlm1(doctor, who, &mut state.params),
    }
}

fn check_sampler(doctor: &mut Doctor, who: &str, params: &mut SamplerParams) {
    for (field, value, min, max) in [
        ("the sample start", &mut params.start, 0.0, 1.0),
        ("the sample end", &mut params.end, 0.0, 1.0),
        ("the loop start", &mut params.loop_start, 0.0, 1.0),
        ("the loop end", &mut params.loop_end, 0.0, 1.0),
        ("the sustain level", &mut params.sustain, 0.0, 1.0),
        ("the filter cutoff", &mut params.filter_cutoff, 0.0, 1.0),
        (
            "the filter resonance",
            &mut params.filter_resonance,
            0.0,
            1.0,
        ),
        ("the drive", &mut params.drive, 0.0, 1.0),
        ("the bit reduction", &mut params.bit_reduction, 0.0, 1.0),
        ("the rate reduction", &mut params.rate_reduction, 0.0, 1.0),
        (
            "the output gain",
            &mut params.output_gain,
            0.0,
            mooloop_core::MAX_LINEAR_GAIN,
        ),
        (
            "the tuning in semitones",
            &mut params.tune_semitones,
            -48.0,
            48.0,
        ),
        ("the tuning in cents", &mut params.tune_cents, -100.0, 100.0),
        ("the attack", &mut params.attack, 0.0, f32::MAX),
        ("the decay", &mut params.decay, 0.0, f32::MAX),
        ("the release", &mut params.release, 0.0, f32::MAX),
        (
            "the filter envelope amount",
            &mut params.filter_env_amount,
            -1.0,
            1.0,
        ),
    ] {
        doctor.fit("channel.sampler.range", who, field, value, min, max);
    }
    // Only when the filter envelope has its own stages: a patch still
    // following the amplitude envelope has nothing separate to repair, and
    // materializing one here would turn a repair pass into an edit.
    if let Some(filter_env) = params.filter_env.as_mut() {
        for (field, value, min, max) in [
            ("the filter attack", &mut filter_env.attack, 0.0, f32::MAX),
            ("the filter decay", &mut filter_env.decay, 0.0, f32::MAX),
            ("the filter sustain", &mut filter_env.sustain, 0.0, 1.0),
            ("the filter release", &mut filter_env.release, 0.0, f32::MAX),
        ] {
            doctor.fit("channel.sampler.range", who, field, value, min, max);
        }
    }
    doctor.fit_int(
        "channel.sampler.root",
        who,
        "the root note",
        &mut params.root_note,
        0,
        127,
    );
    doctor.fit_int(
        "channel.sampler.polyphony",
        who,
        "the voice count",
        &mut params.polyphony,
        1,
        MAX_SAMPLER_VOICES,
    );
    doctor.fit_int(
        "channel.sampler.choke",
        who,
        "the choke group",
        &mut params.choke_group,
        0,
        MAX_CHOKE_GROUP,
    );

    // Checked after the two are individually in range, so this only fires on
    // a window that is genuinely backwards or empty. Spelled through
    // `partial_cmp` because "not less than" has to include the case where one
    // of them is still a NaN, which no comparison operator reports.
    if params.start.partial_cmp(&params.end) != Some(std::cmp::Ordering::Less) {
        let (start, end) = (params.start, params.end);
        if doctor.correct(
            "channel.sampler.window",
            who,
            format!(
                "the sample plays from {} to {}, which is nothing at all",
                number(start),
                number(end)
            ),
            "play the whole sample".into(),
        ) {
            params.start = 0.0;
            params.end = 1.0;
        }
    }
    if params.loop_start > params.loop_end {
        let (start, end) = (params.loop_start, params.loop_end);
        if doctor.correct(
            "channel.sampler.loop",
            who,
            format!(
                "the loop runs from {} back to {}, which is backwards",
                number(start),
                number(end)
            ),
            "swap the loop start and end".into(),
        ) {
            std::mem::swap(&mut params.loop_start, &mut params.loop_end);
        }
    }
}

fn check_drum_synth(doctor: &mut Doctor, who: &str, params: &mut DrumSynthParams) {
    doctor.fit_int(
        "channel.drum.choke",
        who,
        "the choke group",
        &mut params.choke_group,
        0,
        MAX_CHOKE_GROUP,
    );
    for (field, value, min, max) in [
        ("the decay", &mut params.decay, 0.0, 10.0),
        ("the tuning", &mut params.tune_semitones, -48.0, 48.0),
        ("the drive", &mut params.drive, 0.0, 1.0),
        ("the punch", &mut params.punch, 0.0, 1.0),
        (
            "the kick start frequency",
            &mut params.kick_start_hz,
            20.0,
            20_000.0,
        ),
        (
            "the kick end frequency",
            &mut params.kick_end_hz,
            20.0,
            20_000.0,
        ),
        ("the kick sweep", &mut params.kick_sweep, 0.0, 10.0),
        ("the kick click", &mut params.kick_click, 0.0, 1.0),
        (
            "the snare tone frequency",
            &mut params.snare_tone_hz,
            20.0,
            20_000.0,
        ),
        (
            "the snare second tone frequency",
            &mut params.snare_tone2_hz,
            20.0,
            20_000.0,
        ),
        (
            "the snare second tone mix",
            &mut params.snare_tone2_mix,
            0.0,
            1.0,
        ),
        ("the snare noise mix", &mut params.snare_noise_mix, 0.0, 1.0),
        (
            "the snare noise decay",
            &mut params.snare_noise_decay,
            0.0,
            10.0,
        ),
        (
            "the snare noise colour",
            &mut params.snare_noise_color,
            0.0,
            1.0,
        ),
        (
            "the hat high-pass frequency",
            &mut params.hat_hp_hz,
            20.0,
            20_000.0,
        ),
        (
            "the hat metallic amount",
            &mut params.hat_metallic,
            0.0,
            1.0,
        ),
    ] {
        doctor.fit("channel.drum.range", who, field, value, min, max);
    }
}

fn check_mono_synth(doctor: &mut Doctor, who: &str, params: &mut MonoSynthParams) {
    check_oscillators(doctor, who, "mono synth", &mut params.osc);
    check_synth_body(
        doctor,
        who,
        SynthBody {
            glide: &mut params.glide,
            attack: &mut params.attack,
            decay: &mut params.decay,
            sustain: &mut params.sustain,
            release: &mut params.release,
            cutoff: &mut params.filter_cutoff,
            resonance: &mut params.filter_resonance,
            env: &mut params.filter_env_amount,
            drive: &mut params.drive,
        },
    );
    check_lfo(doctor, who, &mut params.lfo);
}

fn check_poly_synth(doctor: &mut Doctor, who: &str, params: &mut PolySynthParams) {
    check_oscillators(doctor, who, "poly synth", &mut params.osc);
    check_synth_body(
        doctor,
        who,
        SynthBody {
            glide: &mut params.glide,
            attack: &mut params.attack,
            decay: &mut params.decay,
            sustain: &mut params.sustain,
            release: &mut params.release,
            cutoff: &mut params.filter_cutoff,
            resonance: &mut params.filter_resonance,
            env: &mut params.filter_env_amount,
            drive: &mut params.drive,
        },
    );
    check_lfo(doctor, who, &mut params.lfo);
    doctor.fit_int(
        "channel.poly.polyphony",
        who,
        "the voice count",
        &mut params.polyphony,
        1,
        MAX_POLY_VOICES,
    );
    doctor.fit(
        "channel.poly.spread",
        who,
        "the stereo spread",
        &mut params.spread,
        0.0,
        1.0,
    );
}

/// The ML-M1 has no device-local LFO and two envelopes, so it gets its own
/// field list rather than being squeezed through the shared one.
fn check_mlm1(doctor: &mut Doctor, who: &str, params: &mut MlM1Params) {
    check_oscillators(doctor, who, "ML-M1", &mut params.osc);
    for (field, value, min, max) in [
        ("the glide", &mut params.glide, 0.0, 10.0),
        ("the attack", &mut params.attack, 0.0, 10.0),
        ("the decay", &mut params.decay, 0.0, 10.0),
        ("the sustain", &mut params.sustain, 0.0, 1.0),
        ("the release", &mut params.release, 0.0, 10.0),
        ("the filter cutoff", &mut params.filter_cutoff, 0.0, 1.0),
        (
            "the filter resonance",
            &mut params.filter_resonance,
            0.0,
            1.0,
        ),
        (
            "the filter envelope amount",
            &mut params.filter_env_amount,
            -1.0,
            1.0,
        ),
        ("the drive", &mut params.drive, 0.0, 1.0),
        ("the filter attack", &mut params.filter_attack, 0.0, 10.0),
        ("the filter decay", &mut params.filter_decay, 0.0, 10.0),
        ("the filter sustain", &mut params.filter_sustain, 0.0, 1.0),
        ("the filter release", &mut params.filter_release, 0.0, 10.0),
        ("the filter keytrack", &mut params.filter_keytrack, 0.0, 1.0),
        ("the accent", &mut params.accent, 0.0, 1.0),
    ] {
        doctor.fit(
            "channel.ml1.range",
            who,
            &format!("{field} (ML-M1)"),
            value,
            min,
            max,
        );
    }
}

/// The parameters every three-oscillator synth shares, borrowed field by field
/// so one table drives both the check and its correction.
struct SynthBody<'a> {
    glide: &'a mut f32,
    attack: &'a mut f32,
    decay: &'a mut f32,
    sustain: &'a mut f32,
    release: &'a mut f32,
    cutoff: &'a mut f32,
    resonance: &'a mut f32,
    env: &'a mut f32,
    drive: &'a mut f32,
}

fn check_synth_body(doctor: &mut Doctor, who: &str, body: SynthBody<'_>) {
    for (field, value, min, max) in [
        ("the glide", body.glide, 0.0, 10.0),
        ("the attack", body.attack, 0.0, 10.0),
        ("the decay", body.decay, 0.0, 10.0),
        ("the sustain", body.sustain, 0.0, 1.0),
        ("the release", body.release, 0.0, 10.0),
        ("the filter cutoff", body.cutoff, 0.0, 1.0),
        ("the filter resonance", body.resonance, 0.0, 1.0),
        ("the filter envelope amount", body.env, -1.0, 1.0),
        ("the drive", body.drive, 0.0, 1.0),
    ] {
        doctor.fit("channel.synth.range", who, field, value, min, max);
    }
}

fn check_lfo(doctor: &mut Doctor, who: &str, lfo: &mut mooloop_core::LfoParams) {
    for (field, value, min, max) in [
        ("the LFO rate", &mut lfo.rate_hz, 0.0, 20.0),
        ("the LFO pitch depth", &mut lfo.to_pitch, -24.0, 24.0),
        ("the LFO filter depth", &mut lfo.to_filter, -4.0, 4.0),
        (
            "the LFO pulse-width depth",
            &mut lfo.to_pulse_width,
            -0.45,
            0.45,
        ),
        ("the LFO amplitude depth", &mut lfo.to_amp, 0.0, 1.0),
    ] {
        doctor.fit("channel.synth.lfo", who, field, value, min, max);
    }
}

fn check_oscillators(
    doctor: &mut Doctor,
    who: &str,
    kind: &str,
    oscillators: &mut [mooloop_core::OscParams],
) {
    for (index, osc) in oscillators.iter_mut().enumerate() {
        let number = index + 1;
        for (field, value, min, max) in [
            ("tuning in semitones", &mut osc.semitones, -48.0, 48.0),
            ("tuning in cents", &mut osc.cents, -100.0, 100.0),
            ("level", &mut osc.level, 0.0, 1.0),
            ("pulse width", &mut osc.pulse_width, 0.05, 0.95),
        ] {
            doctor.fit(
                "channel.oscillator.range",
                who,
                &format!("{kind} oscillator {number}'s {field}"),
                value,
                min,
                max,
            );
        }
    }
}

// --- Formatting ------------------------------------------------------------

/// `Channel 4 "Bass"`. Numbered from one, matching the mixer rather than the
/// file, because the number is there for the user to count along with.
fn channel_name(index: usize, setup: &ChannelSetup) -> String {
    let name = setup.channel.name.trim();
    if name.is_empty() {
        format!("Channel {}", index + 1)
    } else {
        format!("Channel {} \"{name}\"", index + 1)
    }
}

/// `f32`'s own `Display` already prints the shortest text that round-trips, so
/// a value just over a limit still reads as just over it. Only the three
/// values with no useful decimal form are spelled out.
fn number(value: f32) -> String {
    if value.is_nan() {
        "not a number".to_string()
    } else if value == f32::INFINITY {
        "infinity".to_string()
    } else if value == f32::NEG_INFINITY {
        "negative infinity".to_string()
    } else if value == f32::MAX {
        "unlimited".to_string()
    } else {
        value.to_string()
    }
}

fn truncate_on_boundary(name: &str, max_bytes: usize) -> String {
    let mut end = max_bytes.min(name.len());
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{
        AutomationLane, AutomationPoint, DeviceKind, EffectTarget, NoteEvent, ParamAddr,
        PatternPlacement,
    };

    fn codes(diagnosis: &Diagnosis) -> Vec<&'static str> {
        diagnosis.issues.iter().map(|issue| issue.code).collect()
    }

    #[test]
    fn a_default_song_needs_nothing() {
        assert!(inspect_project(&Project::default()).is_clean());
    }

    #[test]
    fn a_starter_kit_needs_nothing() {
        let project = Project::starter_kit(7);
        let diagnosis = inspect_project(&project);
        assert!(diagnosis.is_clean(), "{diagnosis}");
    }

    #[test]
    fn repairing_a_clean_song_changes_nothing() {
        let mut project = Project::starter_kit(3);
        let before = project.clone();
        assert!(repair_project(&mut project).is_clean());
        assert_eq!(project, before);
    }

    #[test]
    fn an_out_of_range_parameter_is_clamped_rather_than_refused() {
        let mut project = Project::default();
        project.channels[0].setup = ChannelSetup::poly_synth("Lead");
        project.channels[0]
            .setup
            .source
            .poly_synth_state_mut()
            .unwrap()
            .params
            .filter_cutoff = 1.4;

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(codes(&diagnosis), ["channel.synth.range"]);
        assert_eq!(
            project.channels[0]
                .setup
                .source
                .poly_synth_state()
                .unwrap()
                .params
                .filter_cutoff,
            1.0
        );
        let issue = &diagnosis.issues[0];
        assert!(issue.repaired);
        assert!(
            issue.problem.contains("filter cutoff") && issue.problem.contains("1.4"),
            "{}",
            issue.problem
        );
        assert!(issue.location.contains("Lead"), "{}", issue.location);
    }

    #[test]
    fn a_not_a_number_parameter_lands_inside_the_range() {
        let mut project = Project::default();
        project.channels[0].setup = ChannelSetup::mono_synth("Bass");
        project.channels[0]
            .setup
            .source
            .mono_synth_state_mut()
            .unwrap()
            .params
            .lfo
            .to_filter = f32::NAN;

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        let value = project.channels[0]
            .setup
            .source
            .mono_synth_state()
            .unwrap()
            .params
            .lfo
            .to_filter;
        assert_eq!(value, 0.0);
        assert!(
            diagnosis.issues[0].problem.contains("not a number"),
            "{}",
            diagnosis.issues[0].problem
        );
    }

    #[test]
    fn a_device_swap_that_left_the_mixer_behind_is_resolved_by_the_settings() {
        let mut project = Project::default();
        project.channels[0].setup = ChannelSetup::sampler("Kick");
        project.channels[0].setup.channel.kind = DeviceKind::PolySynth;

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(codes(&diagnosis), ["channel.kind"]);
        assert_eq!(project.channels[0].setup.channel.kind, DeviceKind::Sampler);
    }

    #[test]
    fn duplicate_note_ids_are_reissued_within_the_pattern() {
        let mut project = Project::default();
        project.channels[0].notes[0] = vec![
            NoteEvent::new(4, 0, 24, 60, 100),
            NoteEvent::new(4, 96, 24, 62, 100),
            NoteEvent::new(0, 192, 24, 64, 100),
        ];
        project.channels[0].next_note_id = 5;

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        let ids: HashSet<NoteId> = project.channels[0].notes[0]
            .iter()
            .map(|note| note.id)
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(!ids.contains(&0));
        assert!(project.channels[0].next_note_id > ids.iter().copied().max().unwrap());
    }

    #[test]
    fn a_stale_note_counter_is_moved_past_the_notes_it_would_collide_with() {
        let mut project = Project::default();
        project.channels[0].notes[0] = vec![NoteEvent::new(9, 0, 24, 60, 100)];
        project.channels[0].next_note_id = 3;

        let diagnosis = repair_project(&mut project);
        assert_eq!(codes(&diagnosis), ["channel.next_note_id"]);
        assert_eq!(project.channels[0].next_note_id, 10);
    }

    #[test]
    fn note_banks_are_squared_up_with_the_pattern_count() {
        let mut project = Project {
            pattern_lengths: vec![16, 16, 16],
            ..Project::default()
        };
        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(project.channels[0].notes.len(), 3);
        assert_eq!(project.channels[0].automation.len(), 3);
    }

    #[test]
    fn a_shorter_automation_bank_is_left_for_the_loader_to_pad() {
        let mut project = Project::default();
        project.channels[0].automation.clear();
        assert!(inspect_project(&project).is_clean());
    }

    #[test]
    fn two_lanes_on_one_control_keep_the_one_with_more_points() {
        let mut project = Project::default();
        let target = ParamAddr::effect(EffectTarget::Channel(0), 2, 7);
        let mut thin = AutomationLane::new(target);
        assert!(thin.upsert(AutomationPoint::new(1, 0, 0.25)));
        let mut thick = AutomationLane::new(target);
        assert!(thick.upsert(AutomationPoint::new(1, 0, 0.5)));
        assert!(thick.upsert(AutomationPoint::new(2, 96, 0.75)));
        project.channels[0].automation[0] = vec![thin, thick];

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(codes(&diagnosis), ["channel.automation.duplicate"]);
        assert_eq!(project.channels[0].automation[0].len(), 1);
        assert_eq!(project.channels[0].automation[0][0].points().len(), 2);
    }

    #[test]
    fn an_automation_point_outside_zero_to_one_is_clamped_in_place() {
        let mut project = Project::default();
        let mut lane = AutomationLane::new(ParamAddr::strip(EffectTarget::Channel(0), 0));
        assert!(lane.upsert(AutomationPoint::new(1, 0, 4.0)));
        assert!(lane.upsert(AutomationPoint::new(2, 96, 0.5)));
        project.channels[0].automation[0] = vec![lane];

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        let points = project.channels[0].automation[0][0].points();
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].value, 1.0);
        assert_eq!(points[1].value, 0.5);
    }

    #[test]
    fn an_empty_sample_window_is_opened_back_up() {
        let mut project = Project::default();
        let params = &mut project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .params;
        params.start = 0.8;
        params.end = 0.2;

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(codes(&diagnosis), ["channel.sampler.window"]);
        let params = project.channels[0].setup.sampler_state().unwrap().params;
        assert_eq!((params.start, params.end), (0.0, 1.0));
    }

    #[test]
    fn a_backwards_loop_is_swapped_rather_than_reset() {
        let mut project = Project::default();
        let params = &mut project.channels[0]
            .setup
            .sampler_state_mut()
            .unwrap()
            .params;
        params.loop_start = 0.75;
        params.loop_end = 0.25;

        let diagnosis = repair_project(&mut project);
        assert_eq!(codes(&diagnosis), ["channel.sampler.loop"]);
        let params = project.channels[0].setup.sampler_state().unwrap().params;
        assert_eq!((params.loop_start, params.loop_end), (0.25, 0.75));
    }

    #[test]
    fn out_of_range_song_selections_are_pulled_back_onto_what_exists() {
        let mut project = Project {
            current_pattern: 40,
            selected_channel: 12,
            bpm: 4000,
            playlist: vec![PatternPlacement::new(9, MAX_PLAYLIST_TICKS + 500)],
            ..Project::default()
        };

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(project.current_pattern, 0);
        assert_eq!(project.selected_channel, 0);
        assert_eq!(project.bpm, 999);
        assert_eq!(project.playlist[0].pattern, 0);
        assert_eq!(project.playlist[0].start_tick, MAX_PLAYLIST_TICKS - 1);
    }

    #[test]
    fn a_song_with_no_channels_gets_one_rather_than_becoming_unopenable() {
        let mut project = Project::default();
        project.channels.clear();
        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(project.channels.len(), 1);
        assert_eq!(
            project.channels[0].notes.len(),
            project.pattern_lengths.len()
        );
    }

    #[test]
    fn too_many_notes_is_refused_with_the_count_to_delete() {
        let mut project = Project::default();
        project.channels[0].notes[0] = (0..MAX_NOTES_PER_CHANNEL_PATTERN + 3)
            .map(|index| NoteEvent::new(index as NoteId + 1, 0, 24, 60, 100))
            .collect();

        let diagnosis = repair_project(&mut project);
        assert!(!diagnosis.is_usable());
        let issue = diagnosis.blocking().next().unwrap();
        assert_eq!(issue.code, "channel.notes.count");
        assert!(issue.location.contains("pattern 1"), "{}", issue.location);
        assert_eq!(
            issue.remedy,
            Remedy::Costly("delete 3 notes from it".into())
        );
    }

    #[test]
    fn a_blocked_save_names_the_place_and_the_count() {
        let mut project = Project::default();
        project.channels[0].notes[0] = (0..MAX_NOTES_PER_CHANNEL_PATTERN + 1)
            .map(|index| NoteEvent::new(index as NoteId + 1, 0, 24, 60, 100))
            .collect();
        project.bpm = 0;

        let diagnosis = repair_project(&mut project);
        let text = diagnosis.to_string();
        assert!(text.contains("could not be corrected"), "{text}");
        assert!(text.contains("pattern 1"), "{text}");
        assert!(text.contains("corrected automatically"), "{text}");

        let report = diagnosis.report();
        assert!(report.contains("channel.notes.count"), "{report}");
        assert!(report.contains("song.bpm"), "{report}");
        assert!(report.contains("CORRECTED AUTOMATICALLY"), "{report}");
        assert!(report.contains("notes:"), "{report}");
    }

    #[test]
    fn a_name_too_long_is_cut_on_a_character_boundary() {
        let mut project = Project::default();
        project.channels[0].setup.channel.name = "\u{e9}".repeat(100);
        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(project.channels[0].setup.channel.name.chars().count(), 64);
    }

    #[test]
    fn a_missing_bus_bank_is_rebuilt_without_disturbing_the_ones_present() {
        let mut project = Project::default();
        project.buses.truncate(2);
        project.buses[1].bus.name = "Drums".into();

        let diagnosis = repair_project(&mut project);
        assert!(diagnosis.is_usable(), "{diagnosis}");
        assert_eq!(project.buses.len(), MAX_BUSES);
        assert_eq!(project.buses[1].bus.name, "Drums");
    }

    #[test]
    fn a_channel_pointed_at_a_bus_that_does_not_exist_lands_on_the_master() {
        let mut project = Project::default();
        project.channels[0].setup.channel.bus = 200;
        let diagnosis = repair_project(&mut project);
        assert_eq!(codes(&diagnosis), ["channel.bus"]);
        assert!((project.channels[0].setup.channel.bus as usize) < MAX_BUSES);
    }
}
