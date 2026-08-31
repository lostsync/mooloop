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

pub(crate) struct ActionSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
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

macro_rules! action {
    ($id:literal, $label:literal, $category:literal, ctrl+shift+$key:literal) => {
        ActionSpec {
            id: $id,
            label: $label,
            category: $category,
            default: Some(RawChord {
                ctrl: true,
                shift: true,
                alt: false,
                key: $key,
            }),
        }
    };
    ($id:literal, $label:literal, $category:literal, ctrl+alt+$key:literal) => {
        ActionSpec {
            id: $id,
            label: $label,
            category: $category,
            default: Some(RawChord {
                ctrl: true,
                shift: false,
                alt: true,
                key: $key,
            }),
        }
    };
    ($id:literal, $label:literal, $category:literal, ctrl+$key:literal) => {
        ActionSpec {
            id: $id,
            label: $label,
            category: $category,
            default: Some(RawChord {
                ctrl: true,
                shift: false,
                alt: false,
                key: $key,
            }),
        }
    };
    ($id:literal, $label:literal, $category:literal, $key:literal) => {
        ActionSpec {
            id: $id,
            label: $label,
            category: $category,
            default: Some(RawChord {
                ctrl: false,
                shift: false,
                alt: false,
                key: $key,
            }),
        }
    };
}

/// The registry. Order is display order within a category in the
/// Preferences > Shortcuts page.
pub(crate) static ACTIONS: &[ActionSpec] = &[
    action!("transport.play-pause", "Play/Pause", "Transport", "space"),
    action!("file.open", "Open Song", "File", ctrl + "o"),
    action!("file.save", "Save Song", "File", ctrl + "s"),
    action!("file.save-as", "Save Song As", "File", ctrl + shift + "s"),
    action!("file.export", "Export Audio", "File", ctrl + "e"),
    action!("file.quit", "Quit", "File", ctrl + "q"),
    action!("edit.undo", "Undo", "Edit", ctrl + "z"),
    action!("edit.redo", "Redo", "Edit", ctrl + shift + "z"),
    action!("edit.cut-channel", "Cut Channel", "Edit", ctrl + "x"),
    action!("edit.copy-channel", "Copy Channel", "Edit", ctrl + "c"),
    action!("edit.paste-channel", "Paste Channel", "Edit", ctrl + "v"),
    action!("edit.select-all", "Select All Notes", "Edit", ctrl + "a"),
    action!(
        "edit.delete-note",
        "Delete Selected Notes",
        "Edit",
        "delete"
    ),
    action!("notes.nudge-earlier", "Nudge Notes Earlier", "Notes", "left"),
    action!("notes.nudge-later", "Nudge Notes Later", "Notes", "right"),
    action!("notes.nudge-up", "Transpose Notes Up", "Notes", "up"),
    action!("notes.nudge-down", "Transpose Notes Down", "Notes", "down"),
    action!("notes.tool-select", "Select Tool", "Notes", "1"),
    action!("notes.tool-draw", "Draw Tool", "Notes", "2"),
    action!("notes.tool-paint", "Paint Tool", "Notes", "3"),
    action!("notes.tool-slice", "Slice Tool", "Notes", "4"),
    action!("notes.tool-erase", "Erase Tool", "Notes", "5"),
    action!("notes.snap-toggle", "Toggle Snap", "Notes", "6"),
    action!("view.pane-next", "Next Pane", "View", ctrl + "right"),
    action!("view.pane-prev", "Previous Pane", "View", ctrl + "left"),
    action!("view.pane-steps", "Show Steps", "View", ctrl + "1"),
    action!("view.pane-mixer", "Show Mixer", "View", ctrl + "2"),
    action!("view.pane-source", "Show Source", "View", ctrl + "3"),
    action!("view.pane-notes", "Show Notes", "View", ctrl + "4"),
    action!("view.pane-playlist", "Show Playlist", "View", ctrl + "5"),
    action!("view.zoom-in", "Zoom In", "View", ctrl + "="),
    action!("view.zoom-out", "Zoom Out", "View", ctrl + "-"),
    action!("channel.add", "Add Channel", "Channel", ctrl + shift + "n"),
    action!(
        "channel.remove",
        "Remove Channel",
        "Channel",
        ctrl + "delete"
    ),
    action!("channel.clone", "Clone Channel", "Channel", ctrl + "d"),
    action!("pattern.add", "Add Pattern", "Pattern", ctrl + shift + "p"),
    action!(
        "pattern.remove",
        "Remove Pattern",
        "Pattern",
        ctrl + shift + "delete"
    ),
    action!(
        "pattern.clone",
        "Clone Pattern",
        "Pattern",
        ctrl + alt + "d"
    ),
    // No default chord: every nearby Pattern-menu action already claims a
    // Ctrl+<modifier>+key combination, and Ctrl+Alt+Delete is a poor choice
    // to fight the desktop environment over. Still registered so it shows
    // up in Preferences > Shortcuts for anyone who wants to bind it.
    ActionSpec {
        id: "pattern.clear",
        label: "Clear Pattern",
        category: "Pattern",
        default: None,
    },
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
