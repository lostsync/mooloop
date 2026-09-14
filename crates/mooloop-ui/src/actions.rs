//! The action registry: every operation a keyboard shortcut can target.
//!
//! This is deliberately the seam described in `docs/ACTIONS.md`. Every
//! action has a stable string id independent of any particular surface —
//! today only the keyboard and the menu bar dispatch through it, but a
//! future console or MCP server would target the same ids. Adding a new
//! bindable action means adding one `ActionSpec` here and one arm in
//! `lib.rs`'s `on_shortcut_key` dispatcher; it never requires touching the
//! key-decoding logic in `main.slint`.

use std::collections::HashMap;
use std::fmt;

/// A default binding, expressed without allocation so `ACTIONS` can be a
/// plain `static`. `key` is the canonical lowercase form `KeyChord` uses
/// internally (see `KeyChord::parse`/`display`).
struct RawChord {
    ctrl: bool,
    shift: bool,
    alt: bool,
    key: &'static str,
}

/// Where an action applies, and what Preferences > Shortcuts tells a user
/// about it.
///
/// A chord resolves to exactly one action id -- `ShortcutTable` is a
/// `HashMap<KeyChord, &str>` and cannot be anything else -- so "Ctrl+C
/// means three different things" is one action that asks what has focus,
/// not three actions sharing a chord. `Focused` is that action's scope, and
/// the label is what the prefpane column has to be able to say.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Scope {
    /// Fires wherever focus happens to be. The default, and most of them.
    Anywhere,
    /// Resolves against the focused surface (`Surface`), falling back to
    /// the channel list when nothing else has been focused -- which is what
    /// the clipboard chords have always meant.
    Focused,
    /// Only while the piano roll is the visible editor.
    Notes,
    /// Only with a device selected in the rack.
    Rack,
    /// Only while the rack is editing a track rather than a channel, which
    /// is where a track's own mute and solo can be aimed.
    Track,
    /// Only while the browser panel holds the keyboard.
    Browser,
}

impl Scope {
    /// The prefpane's Context column. Prose rather than an id: this is the
    /// one place a user is told why a chord did nothing.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Scope::Anywhere => "Anywhere",
            Scope::Focused => "Focused panel",
            Scope::Notes => "Piano roll",
            Scope::Rack => "Selected device",
            Scope::Track => "Edited track",
            Scope::Browser => "Browser",
        }
    }
}

/// Which panel a `Scope::Focused` action resolves against.
///
/// Deliberately a name rather than an index: the value crosses into
/// `main.slint` as a string (`focused-surface`), so neither side spells a
/// number, and `decoding::focused_surface_names_match_the_markup` below
/// holds the two spellings together.
///
/// `Channels` is the fallback, not a fifth state meaning "nothing": before
/// this existed the clipboard chords meant the channel unconditionally, and
/// a surface nobody has clicked should behave the way it did then.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Surface {
    #[default]
    Channels,
    Notes,
    Rack,
    Browser,
}

impl Surface {
    pub(crate) const ALL: &'static [Surface] = &[
        Surface::Channels,
        Surface::Notes,
        Surface::Rack,
        Surface::Browser,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Surface::Channels => "channels",
            Surface::Notes => "notes",
            Surface::Rack => "rack",
            Surface::Browser => "browser",
        }
    }

    /// Unknown text is the fallback rather than an error: the property is
    /// writable from the markup, and a typo there should leave the chords
    /// doing what they did before this existed.
    pub(crate) fn from_name(name: &str) -> Self {
        Surface::ALL
            .iter()
            .copied()
            .find(|surface| surface.name() == name)
            .unwrap_or_default()
    }
}

pub(crate) struct ActionSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    pub scope: Scope,
    default: Option<RawChord>,
}

impl ActionSpec {
    pub(crate) fn default_chord(&self) -> Option<KeyChord> {
        self.default.as_ref().map(|raw| KeyChord {
            ctrl: raw.ctrl,
            shift: raw.shift,
            alt: raw.alt,
            meta: false,
            key: raw.key.to_string(),
        })
    }
}

/// One default chord, as it is written in the table below. Split out of
/// `action!` so the two spellings of an entry -- with a scope and without --
/// share one parser instead of doubling its arms.
macro_rules! chord {
    (ctrl+shift+$key:literal) => {
        Some(RawChord { ctrl: true, shift: true, alt: false, key: $key })
    };
    (ctrl+alt+$key:literal) => {
        Some(RawChord { ctrl: true, shift: false, alt: true, key: $key })
    };
    (ctrl+$key:literal) => {
        Some(RawChord { ctrl: true, shift: false, alt: false, key: $key })
    };
    (shift+$key:literal) => {
        Some(RawChord { ctrl: false, shift: true, alt: false, key: $key })
    };
    ($key:literal) => {
        Some(RawChord { ctrl: false, shift: false, alt: false, key: $key })
    };
    () => {
        None
    };
}

/// An action that fires wherever focus happens to be. Most of them.
macro_rules! action {
    ($id:literal, $label:literal, $category:literal $(, $($chord:tt)*)?) => {
        ActionSpec {
            id: $id,
            label: $label,
            category: $category,
            scope: Scope::Anywhere,
            default: chord!($($($chord)*)?),
        }
    };
}

/// An action that does not. The scope is what Preferences > Shortcuts shows
/// in its Context column, and it is the only thing that tells a user why a
/// chord they can see did nothing where they pressed it.
macro_rules! scoped_action {
    ($id:literal, $label:literal, $category:literal, $scope:expr $(, $($chord:tt)*)?) => {
        ActionSpec {
            id: $id,
            label: $label,
            category: $category,
            scope: $scope,
            default: chord!($($($chord)*)?),
        }
    };
}

/// The registry. Order is display order within a category in the
/// Preferences > Shortcuts page.
pub(crate) static ACTIONS: &[ActionSpec] = &[
    action!("transport.play-pause", "Play/Pause", "Transport", "space"),
    // Stop is play/pause's partner and rewinds with it, so it takes the
    // same key with Shift. It could not have been bound before 2026-09-14:
    // `main.slint` forwarded a space press with all four modifier flags
    // hardcoded `false`, so Shift+Space and Space were the same chord.
    action!("transport.stop", "Stop", "Transport", shift + "space"),
    // Rewind without stopping. `EngineCommand::Seek` is what dragging the
    // playhead already sends; this is that gesture with no pointer.
    action!(
        "transport.return-to-start",
        "Return To Start",
        "Transport",
        "home"
    ),
    action!("transport.loop-toggle", "Toggle Loop", "Transport", "l"),
    action!("file.new", "New Song", "File", ctrl + "n"),
    action!("file.open", "Open Song", "File", ctrl + "o"),
    action!("file.save", "Save Song", "File", ctrl + "s"),
    action!("file.save-as", "Save Song As", "File", ctrl + shift + "s"),
    action!("file.export", "Export Audio", "File", ctrl + "e"),
    action!("file.quit", "Quit", "File", ctrl + "q"),
    action!("edit.undo", "Undo", "Edit", ctrl + "z"),
    action!("edit.redo", "Redo", "Edit", ctrl + shift + "z"),
    // The three contextual chords, and the reason `Scope::Focused` exists.
    // The ids keep `-channel` because a user's rebindings are stored
    // against them (`docs/ACTIONS.md`: an id outlives its label); what they
    // *mean* is now the focused panel's clipboard, with the channel as the
    // fallback it has been since before there was a second clipboard.
    scoped_action!("edit.cut-channel", "Cut", "Edit", Scope::Focused, ctrl + "x"),
    scoped_action!("edit.copy-channel", "Copy", "Edit", Scope::Focused, ctrl + "c"),
    scoped_action!(
        "edit.paste-channel",
        "Paste",
        "Edit",
        Scope::Focused,
        ctrl + "v"
    ),
    scoped_action!(
        "edit.select-all",
        "Select All Notes",
        "Edit",
        Scope::Notes,
        ctrl + "a"
    ),
    scoped_action!(
        "edit.delete-note",
        "Delete Selected Notes",
        "Edit",
        Scope::Notes,
        "delete"
    ),
    // The arrow keys are the other contextual set, and the ids are the
    // `notes.nudge-*` ones for the reason the clipboard ids are unchanged.
    // Up and Down moved here from a hand-written branch inside
    // `main.slint`'s root FocusScope, which could only ever know about two
    // of the three answers -- the browser could not have been the third
    // without leaving the markup.
    scoped_action!(
        "notes.nudge-earlier",
        "Move Left",
        "Navigation",
        Scope::Focused,
        "left"
    ),
    scoped_action!(
        "notes.nudge-later",
        "Move Right",
        "Navigation",
        Scope::Focused,
        "right"
    ),
    scoped_action!(
        "notes.nudge-up",
        "Move Up",
        "Navigation",
        Scope::Focused,
        "up"
    ),
    scoped_action!(
        "notes.nudge-down",
        "Move Down",
        "Navigation",
        Scope::Focused,
        "down"
    ),
    scoped_action!("notes.tool-select", "Select Tool", "Notes", Scope::Notes, "1"),
    scoped_action!("notes.tool-draw", "Draw Tool", "Notes", Scope::Notes, "2"),
    scoped_action!("notes.tool-paint", "Paint Tool", "Notes", Scope::Notes, "3"),
    scoped_action!("notes.tool-slice", "Slice Tool", "Notes", Scope::Notes, "4"),
    scoped_action!("notes.tool-erase", "Erase Tool", "Notes", Scope::Notes, "5"),
    scoped_action!("notes.snap-toggle", "Toggle Snap", "Notes", Scope::Notes, "6"),
    action!("view.pane-next", "Next Pane", "View", ctrl + "right"),
    action!("view.pane-prev", "Previous Pane", "View", ctrl + "left"),
    action!("view.pane-steps", "Show Steps", "View", ctrl + "1"),
    action!("view.pane-mixer", "Show Mixer", "View", ctrl + "2"),
    // Labelled for the view; the *id* keeps `source` because a user's
    // rebindings are stored against it and renaming an id silently drops
    // whatever they had bound.
    action!("view.pane-source", "Show Devices", "View", ctrl + "3"),
    action!("view.pane-notes", "Show Notes", "View", ctrl + "4"),
    action!("view.pane-playlist", "Show Playlist", "View", ctrl + "5"),
    action!("view.split-toggle", "Split Top Pane", "View", ctrl + "\\"),
    // Same key as the split, because they are the two questions about a
    // pane's size and answering them from one place is easier to remember
    // than two unrelated chords.
    action!("view.zoom-pane", "Zoom Pane", "View", ctrl + shift + "\\"),
    // Only the roll zooms -- the markup's own handlers check `showing-notes`
    // before doing anything -- so the prefpane says so rather than offering
    // them as global.
    scoped_action!("view.zoom-in", "Zoom In", "View", Scope::Notes, ctrl + "="),
    scoped_action!("view.zoom-out", "Zoom Out", "View", Scope::Notes, ctrl + "-"),
    action!("channel.add", "Add Channel", "Channel", ctrl + shift + "n"),
    action!(
        "channel.remove",
        "Remove Channel",
        "Channel",
        ctrl + "delete"
    ),
    action!("channel.clone", "Clone Channel", "Channel", ctrl + "d"),
    action!("channel.mute", "Mute Channel", "Channel", ctrl + "m"),
    // Solo landed on a *track* on 2026-09-11 -- `BusSetup::solo`, in place
    // rather than the monitor tap `archive/MIXER_PLAN.md` had specified -- so these
    // bind the track's. A channel still has no solo of its own, and an
    // action named for one would be an action bound to nothing.
    scoped_action!(
        "track.solo",
        "Solo Track",
        "Track",
        Scope::Track,
        ctrl + shift + "m"
    ),
    scoped_action!(
        "track.mute",
        "Mute Track",
        "Track",
        Scope::Track,
        ctrl + alt + "m"
    ),
    // Device clipboard. Kept on their own chords now that the bare ones
    // resolve against focus as well: an unambiguous way to reach the rack's
    // clipboard from the roll is worth four bindings, and rebinding these
    // to nothing is a preferences change for anyone who disagrees.
    scoped_action!(
        "device.copy",
        "Copy Device",
        "Device",
        Scope::Rack,
        ctrl + shift + "c"
    ),
    scoped_action!(
        "device.cut",
        "Cut Device",
        "Device",
        Scope::Rack,
        ctrl + shift + "x"
    ),
    scoped_action!(
        "device.paste",
        "Paste Device",
        "Device",
        Scope::Rack,
        ctrl + shift + "v"
    ),
    scoped_action!(
        "device.duplicate",
        "Duplicate Device",
        "Device",
        Scope::Rack,
        ctrl + shift + "d"
    ),
    scoped_action!(
        "device.bypass",
        "Bypass Device",
        "Device",
        Scope::Rack,
        ctrl + shift + "b"
    ),
    scoped_action!(
        "device.remove",
        "Remove Device",
        "Device",
        Scope::Rack,
        ctrl + shift + "backspace"
    ),
    scoped_action!(
        "device.wrap",
        "Wrap Device In Container",
        "Device",
        Scope::Rack,
        ctrl + shift + "g"
    ),
    scoped_action!(
        "device.save-preset",
        "Save Device Preset",
        "Device",
        Scope::Rack,
        ctrl + alt + "s"
    ),
    // Not `Scope::Rack`: with nothing selected these select the first
    // device, which is how the rack is reached from the keyboard at all.
    action!(
        "device.next",
        "Select Next Device",
        "Device",
        ctrl + shift + "right"
    ),
    action!(
        "device.prev",
        "Select Previous Device",
        "Device",
        ctrl + shift + "left"
    ),
    // The browser tree. `browser.focus` reveals the panel as well as taking
    // the keys, because a shortcut that silently aims at a collapsed panel
    // is indistinguishable from one that does nothing.
    action!("browser.focus", "Focus Browser", "Browser", ctrl + "b"),
    scoped_action!(
        "browser.activate",
        "Open Browser Row",
        "Browser",
        Scope::Browser,
        "return"
    ),
    scoped_action!(
        "browser.load",
        "Load Browser Row Into Channel",
        "Browser",
        Scope::Browser,
        ctrl + "return"
    ),
    action!("pattern.add", "Add Pattern", "Pattern", ctrl + shift + "p"),
    action!(
        "pattern.remove",
        "Remove Pattern",
        "Pattern",
        ctrl + shift + "delete"
    ),
    action!("pattern.clone", "Clone Pattern", "Pattern", ctrl + alt + "d"),
    // No default chord: every nearby Pattern-menu action already claims a
    // Ctrl+<modifier>+key combination, and Ctrl+Alt+Delete is a poor choice
    // to fight the desktop environment over. Still registered so it shows
    // up in Preferences > Shortcuts for anyone who wants to bind it.
    action!("pattern.clear", "Clear Pattern", "Pattern"),
    // A beat at a time, which is the unit the grid draws in and the unit
    // pattern lengths are actually chosen in. Adam: "it would be cool if
    // there was an easier way to go from 16 steps to 32 -- maybe shift click
    // moves by 4 or something, or a hotkey to grow it by 4." Both: Shift on
    // the STEPS field's arrows and wheel, and these.
    action!(
        "pattern.length-grow",
        "Lengthen Pattern By A Beat",
        "Pattern",
        ctrl + shift + "="
    ),
    action!(
        "pattern.length-shrink",
        "Shorten Pattern By A Beat",
        "Pattern",
        ctrl + shift + "-"
    ),
];

/// One key combination. `key` is a canonical lowercase identifier: a single
/// ASCII letter/digit, a symbol (`"="`, `"-"`), or one of the short names
/// produced by `main.slint`'s key decoder (`"space"`, `"left"`, `"right"`,
/// `"up"`, `"down"`, `"delete"`, `"backspace"`, `"escape"`, `"tab"`,
/// `"return"`, `"home"`, `"end"`, `"pageup"`, `"pagedown"`, `"insert"`).
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct KeyChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: String,
}

const NAMED_KEYS: &[(&str, &str)] = &[
    ("space", "Space"),
    ("left", "Left"),
    ("right", "Right"),
    ("up", "Up"),
    ("down", "Down"),
    ("delete", "Delete"),
    ("backspace", "Backspace"),
    ("escape", "Esc"),
    ("tab", "Tab"),
    ("return", "Enter"),
    ("home", "Home"),
    ("end", "End"),
    ("pageup", "PgUp"),
    ("pagedown", "PgDn"),
    ("insert", "Ins"),
];

impl KeyChord {
    pub(crate) fn new(ctrl: bool, shift: bool, alt: bool, meta: bool, key: &str) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            meta,
            key: key.to_lowercase(),
        }
    }

    /// Parses the canonical `display()` form (also accepted case-insensitively).
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut ctrl = false;
        let mut shift = false;
        let mut alt = false;
        let mut meta = false;
        let mut key = None;
        for token in text.split('+') {
            let token = token.trim();
            if token.is_empty() {
                // A literal "+" key produces an empty token next to the
                // separator that introduced it, e.g. "Ctrl+=+" is never
                // generated by us, but "Ctrl++" (Ctrl+Shift+=) can be typed
                // by hand; treat the empty token as the "+" key itself.
                key = Some("+".to_string());
                continue;
            }
            match token.to_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" | "option" => alt = true,
                "meta" | "cmd" | "super" | "win" => meta = true,
                other => key = Some(other.to_string()),
            }
        }
        key.map(|key| Self {
            ctrl,
            shift,
            alt,
            meta,
            key,
        })
    }

    fn key_label(&self) -> String {
        NAMED_KEYS
            .iter()
            .find(|(id, _)| *id == self.key)
            .map(|(_, label)| label.to_string())
            .unwrap_or_else(|| self.key.to_uppercase())
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.meta {
            parts.push("Meta".to_string());
        }
        parts.push(self.key_label());
        write!(f, "{}", parts.join("+"))
    }
}

/// Resolves key chords to action ids and back, merging the registry's
/// defaults with the user's persisted overrides.
pub(crate) struct ShortcutTable {
    bindings: HashMap<&'static str, KeyChord>,
    lookup: HashMap<KeyChord, &'static str>,
}

impl ShortcutTable {
    pub(crate) fn build(overrides: &HashMap<String, String>) -> Self {
        let mut bindings = HashMap::new();
        for spec in ACTIONS {
            // An empty override means "explicitly unbound" (the prefpane's
            // Reset clears an action to this rather than dropping the key
            // from `overrides`, so a chord that collided with the default
            // stays cleared instead of springing back). Unparseable
            // non-empty text -- hand-edited settings.toml -- falls back to
            // the registry default rather than silently going unbound.
            let chord = match overrides.get(spec.id) {
                Some(text) if text.is_empty() => None,
                Some(text) => KeyChord::parse(text).or_else(|| spec.default_chord()),
                None => spec.default_chord(),
            };
            if let Some(chord) = chord {
                bindings.insert(spec.id, chord);
            }
        }
        let mut lookup = HashMap::new();
        for spec in ACTIONS {
            if let Some(chord) = bindings.get(spec.id) {
                lookup.insert(chord.clone(), spec.id);
            }
        }
        Self { bindings, lookup }
    }

    /// Looks up which action (if any) a just-pressed chord should trigger.
    pub(crate) fn resolve(&self, chord: &KeyChord) -> Option<&'static str> {
        self.lookup.get(chord).copied()
    }

    pub(crate) fn chord_for(&self, action_id: &str) -> Option<KeyChord> {
        self.bindings.get(action_id).cloned()
    }

    /// Whether `action_id` is currently bound to its registry default (or,
    /// for the rare action with no default, currently unbound). Drives the
    /// prefpane's per-row Reset button.
    pub(crate) fn is_default(&self, action_id: &str) -> bool {
        let default = ACTIONS
            .iter()
            .find(|spec| spec.id == action_id)
            .and_then(ActionSpec::default_chord);
        self.bindings.get(action_id).cloned() == default
    }

    /// Other actions currently bound to `chord`, for conflict warnings when
    /// rebinding `excluding` to it.
    pub(crate) fn owners_of(&self, chord: &KeyChord, excluding: &str) -> Vec<&'static str> {
        self.bindings
            .iter()
            .filter(|entry| *entry.0 != excluding && entry.1 == chord)
            .map(|entry| *entry.0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_are_unique() {
        let mut ids: Vec<_> = ACTIONS.iter().map(|spec| spec.id).collect();
        ids.sort_unstable();
        let mut deduped = ids.clone();
        deduped.dedup();
        assert_eq!(ids, deduped, "duplicate action id in ACTIONS");
    }

    #[test]
    fn default_chords_do_not_collide() {
        let table = ShortcutTable::build(&HashMap::new());
        for spec in ACTIONS {
            let Some(chord) = spec.default_chord() else {
                continue;
            };
            let owners = table.owners_of(&chord, spec.id);
            assert!(
                owners.is_empty(),
                "{} collides with {owners:?} on {chord}",
                spec.id
            );
        }
    }

    /// The prefpane's Context column is the only thing that tells a user
    /// why a chord they can see did nothing where they pressed it, so an
    /// action that is *not* global has to say so. `Anywhere` is the macro
    /// default, which makes forgetting the scope the silent failure; this
    /// pins the set that must not be global.
    #[test]
    fn every_contextual_action_declares_its_scope() {
        for id in [
            "edit.cut-channel",
            "edit.copy-channel",
            "edit.paste-channel",
            "notes.nudge-earlier",
            "notes.nudge-later",
            "notes.nudge-up",
            "notes.nudge-down",
            "notes.tool-draw",
            "notes.snap-toggle",
            "edit.select-all",
            "edit.delete-note",
            "view.zoom-in",
            "device.copy",
            "device.bypass",
            "track.solo",
            "browser.activate",
        ] {
            let spec = ACTIONS
                .iter()
                .find(|spec| spec.id == id)
                .unwrap_or_else(|| panic!("{id} left the registry"));
            assert_ne!(
                spec.scope,
                Scope::Anywhere,
                "{id} is contextual but says it fires anywhere"
            );
        }
    }

    #[test]
    fn a_surface_name_round_trips() {
        for surface in Surface::ALL {
            assert_eq!(Surface::from_name(surface.name()), *surface);
        }
        // Unknown text is the fallback, not a panic: the property is
        // writable from the markup.
        assert_eq!(Surface::from_name("nonsense"), Surface::Channels);
    }

    #[test]
    fn chord_display_round_trips_through_parse() {
        let chord = KeyChord::new(true, true, false, false, "z");
        let text = chord.to_string();
        assert_eq!(text, "Ctrl+Shift+Z");
        assert_eq!(KeyChord::parse(&text).unwrap(), chord);
    }

    #[test]
    fn rebinding_reports_the_previous_owner() {
        let mut overrides = HashMap::new();
        overrides.insert("edit.redo".to_string(), "Ctrl+O".to_string());
        let table = ShortcutTable::build(&overrides);
        let chord = KeyChord::new(true, false, false, false, "o");
        // "file.open" still owns Ctrl+O by default; "edit.redo" was just
        // rebound onto the same chord, so the table's reverse lookup
        // resolves to whichever was inserted last (edit.redo, since ACTIONS
        // lists it after file.open), while owners_of surfaces the collision
        // for the prefpane to warn about before it commits the rebind.
        assert_eq!(table.resolve(&chord), Some("edit.redo"));
        let owners = table.owners_of(&chord, "edit.redo");
        assert_eq!(owners, vec!["file.open"]);
    }
}

/// The two key-decoding ladders, held together and held to the registry.
///
/// `main.slint`'s root `FocusScope` turns a raw key event into the canonical
/// `(key, ctrl, shift, alt, meta)` tuple this module resolves, and
/// `appearance-dialog.slint`'s per-row capture scope does the same job for
/// the Shortcuts page's recorder. Both are written out by hand, because
/// Slint has no way to share one across files, and neither was checked
/// against anything until 2026-09-14.
///
/// What that cost: `transport.loop-toggle` shipped on a bare L on
/// 2026-09-07. The registry held it, the prefpane drew it, `ShortcutTable`
/// resolved it -- and no L ever arrived, because the root ladder forwarded
/// six digits and nothing else unmodified. The action was dead for a week
/// and every test was green, because every test asked the registry what it
/// held rather than asking the markup what it could deliver.
///
/// So this asks the markup. It is the question `AGENTS.md` says to ask of
/// any mirrored value -- *does anything read the copy the test checks?* --
/// aimed at the one copy that is not Rust.
#[cfg(test)]
mod decoding {
    use super::*;
    use std::collections::BTreeMap;

    const MAIN_SLINT: &str = include_str!("../ui/main.slint");
    const PREFS_SLINT: &str = include_str!("../ui/appearance-dialog.slint");

    /// Ctrl+H/I/J arrive as the control codes for Backspace/Tab/Return
    /// (0x08/0x09/0x0A) and are taken by those branches before anything
    /// looks at the Ctrl flag. Both ladders say so in a comment; this is
    /// that sentence in a form that can fail.
    const CTRL_UNREACHABLE: &[&str] = &["h", "i", "j"];

    /// What one ladder can produce: the keys it names outright, mapped to
    /// how it fills the `ctrl` argument, plus whether it has a catch-all
    /// for anything it did not name.
    struct Ladder {
        named: BTreeMap<String, String>,
        catch_all_ctrl: bool,
        catch_all_bare: bool,
    }

    /// Scrapes `<call>("<key>", <ctrl-argument>, ...` out of the markup.
    /// Deliberately a text scrape of the real file rather than a table
    /// mirrored here: a mirrored table would be the third copy of the thing
    /// that already drifted twice.
    fn ladder(source: &str, call: &str) -> Ladder {
        let named_prefix = format!("{call}(\"");
        let mut named = BTreeMap::new();
        for line in source.lines() {
            let Some(rest) = line.split_once(&named_prefix).map(|split| split.1) else {
                continue;
            };
            let Some((key, rest)) = rest.split_once("\", ") else {
                continue;
            };
            let ctrl = rest.split(',').next().unwrap_or_default().trim();
            named.insert(key.to_string(), ctrl.to_string());
        }
        let catch_all = format!("{call}(event.text.to-lowercase(), ");
        let any = source.contains(&format!("{catch_all}event.modifiers.control,"));
        Ladder {
            named,
            catch_all_ctrl: any || source.contains(&format!("{catch_all}true,")),
            catch_all_bare: any || source.contains(&format!("{catch_all}false,")),
        }
    }

    impl Ladder {
        fn can_produce(&self, chord: &KeyChord) -> bool {
            if chord.ctrl && CTRL_UNREACHABLE.contains(&chord.key.as_str()) {
                return false;
            }
            if let Some(argument) = self.named.get(&chord.key) {
                let matches = match argument.as_str() {
                    "true" => chord.ctrl,
                    "false" => !chord.ctrl,
                    // Forwards the real flag, so it serves either.
                    _ => true,
                };
                if matches {
                    return true;
                }
            }
            // A catch-all forwards `event.text`, which is one character for
            // anything that is not a named key.
            if chord.key.chars().count() != 1 {
                return false;
            }
            if chord.ctrl {
                self.catch_all_ctrl
            } else {
                self.catch_all_bare
            }
        }
    }

    #[test]
    fn every_default_chord_reaches_the_dispatcher() {
        let ladder = ladder(MAIN_SLINT, "root.shortcut-key");
        for spec in ACTIONS {
            let Some(chord) = spec.default_chord() else {
                continue;
            };
            assert!(
                ladder.can_produce(&chord),
                "{} defaults to {chord}, which main.slint's key ladder never produces",
                spec.id
            );
        }
    }

    /// A chord a user can record has to be a chord that can fire, and the
    /// reverse: Reset followed by Record must be able to put a registry
    /// default back.
    #[test]
    fn the_recorder_decodes_what_the_dispatcher_does() {
        let mut dispatcher = ladder(MAIN_SLINT, "root.shortcut-key");
        let recorder = ladder(PREFS_SLINT, "root.key-captured");
        // The one difference that is a decision rather than drift: Escape
        // cancels a capture, so the recorder can never hand it back as a
        // chord. Nothing in the registry binds it.
        assert!(dispatcher.named.remove("escape").is_some());
        assert!(!ACTIONS
            .iter()
            .filter_map(ActionSpec::default_chord)
            .any(|chord| chord.key == "escape"));
        assert_eq!(
            dispatcher.named, recorder.named,
            "the two key ladders name different keys"
        );
        // And they have to agree about what falls through, not only about
        // what they name. Reverting `main.slint`'s catch-all to its
        // digits-only ladder is the exact 2026-09-14 defect, and it leaves
        // the named keys identical -- so without this line only
        // `every_default_chord_reaches_the_dispatcher` would notice, and
        // only for as long as some default happens to be a bare key.
        assert!(recorder.catch_all_bare && recorder.catch_all_ctrl);
        assert_eq!(dispatcher.catch_all_bare, recorder.catch_all_bare);
        assert_eq!(dispatcher.catch_all_ctrl, recorder.catch_all_ctrl);
        for spec in ACTIONS {
            let Some(chord) = spec.default_chord() else {
                continue;
            };
            assert!(
                recorder.can_produce(&chord),
                "{} defaults to {chord}, which the Shortcuts recorder cannot capture",
                spec.id
            );
        }
    }

    /// `docs/ACTIONS.md` says how many actions there are, and has been
    /// wrong about it twice.
    ///
    /// It said 46 where the table held 45 on 2026-09-08. It was corrected to
    /// 47 on 2026-09-12 and the table held 49 by then. Both corrections were
    /// made by counting, and counting is the thing that went wrong both
    /// times -- which is why this reads the sentence rather than trusting
    /// the next person to recount. The document tells a reader to prefer the
    /// table; that instruction is a good one and is not a substitute for the
    /// sentence being true.
    #[test]
    fn the_registry_count_in_actions_md_is_the_registry_count() {
        const DOC: &str = include_str!("../../../docs/ACTIONS.md");
        let claim = DOC
            .split("**It holds ")
            .nth(1)
            .expect("ACTIONS.md no longer states how many actions there are");
        let (actions, rest) = claim
            .split_once(" actions in ")
            .expect("the count sentence changed shape");
        let (categories, _) = rest
            .split_once(" categories**")
            .expect("the count sentence changed shape");
        assert_eq!(
            actions.parse::<usize>().unwrap(),
            ACTIONS.len(),
            "ACTIONS.md states the wrong number of actions"
        );
        let mut seen: Vec<&str> = ACTIONS.iter().map(|spec| spec.category).collect();
        seen.dedup();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            categories.parse::<usize>().unwrap(),
            seen.len(),
            "ACTIONS.md states the wrong number of categories"
        );
    }

    /// A category's rows are contiguous, because `shortcut_rows` marks a
    /// section boundary by comparing each entry with the one before it. A
    /// category split across two runs of the table would draw two headings
    /// with the same name and no error anywhere.
    #[test]
    fn a_category_is_one_run_of_the_table() {
        let mut runs: Vec<&str> = ACTIONS.iter().map(|spec| spec.category).collect();
        runs.dedup();
        let mut unique = runs.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(runs.len(), unique.len(), "a category is split in two");
    }

    /// Every registered action has an arm in the dispatcher.
    ///
    /// `lib.rs`'s match ends in `_ => return false`, so a registered action
    /// with no arm is an action that is drawn in Preferences, resolved by
    /// `ShortcutTable`, and does nothing -- which is the bare-L failure one
    /// layer up, and just as invisible to a test that only reads the
    /// registry.
    #[test]
    fn every_action_is_dispatched() {
        const LIB: &str = include_str!("lib.rs");
        for spec in ACTIONS {
            assert!(
                LIB.contains(&format!("\"{}\"", spec.id)),
                "{} is registered but never named in lib.rs's dispatcher",
                spec.id
            );
        }
    }

    /// `focused-surface` is a string crossing into the markup, so the names
    /// are written twice. This is the guard that makes that safe.
    #[test]
    fn focused_surface_names_match_the_markup() {
        let mut assigned = Vec::new();
        for line in MAIN_SLINT.lines() {
            let Some(rest) = line.split_once("focused-surface = \"") else {
                continue;
            };
            let Some((name, _)) = rest.1.split_once('"') else {
                continue;
            };
            assigned.push(name.to_string());
        }
        assert!(
            !assigned.is_empty(),
            "nothing in main.slint sets focused-surface; the contextual chords \
             would all resolve to the fallback forever"
        );
        for name in &assigned {
            assert!(
                Surface::ALL.iter().any(|surface| surface.name() == name),
                "main.slint sets focused-surface to {name:?}, which no Surface answers to"
            );
        }
        assert!(
            MAIN_SLINT.contains(&format!(
                "in-out property <string> focused-surface: \"{}\";",
                Surface::default().name()
            )),
            "the markup's default focused-surface is not Surface::default()"
        );
    }
}
