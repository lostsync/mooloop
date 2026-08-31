//! The pointer-gesture registry: which modifier each piano-roll drag role
//! answers to.
//!
//! This is the `actions.rs` idea applied to the mouse. A gesture role has a
//! stable string id and a default modifier, the user's changes are stored as
//! sparse overrides so the registry can grow without a settings migration,
//! and the resolved table is published to `.slint` rather than the modifiers
//! being hardcoded in the grid's pointer handler.
//!
//! Alt is a legal default here. It was avoided at first because several
//! window managers claim Alt+drag for moving windows, but stretch has no
//! other conventional binding, and a desktop that eats it can remap the role
//! rather than everyone else losing the convention.

use std::collections::HashMap;
use std::fmt;

/// A modifier combination, as a role requires it.
///
/// Slint's expression language has no bitwise operators, so this cannot be
/// published as a mask and tested with `&` on the other side. It crosses as
/// a struct of booleans and the grid tests it by implication -- see
/// `has-gesture` in `piano-grid.slint`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct GestureMod {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl GestureMod {
    const NONE: Self = Self {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
    };
    const SHIFT: Self = Self {
        shift: true,
        ..Self::NONE
    };
    const CTRL: Self = Self {
        ctrl: true,
        ..Self::NONE
    };
    const CTRL_SHIFT: Self = Self {
        ctrl: true,
        shift: true,
        ..Self::NONE
    };
    const ALT: Self = Self {
        alt: true,
        ..Self::NONE
    };
    const CTRL_ALT: Self = Self {
        ctrl: true,
        alt: true,
        ..Self::NONE
    };
    const SHIFT_ALT: Self = Self {
        shift: true,
        alt: true,
        ..Self::NONE
    };
    const META: Self = Self {
        meta: true,
        ..Self::NONE
    };

    /// Parses the canonical `Display` form, case-insensitively.
    pub(crate) fn parse(text: &str) -> Self {
        let mut out = Self::NONE;
        for token in text.split('+') {
            match token.trim().to_lowercase().as_str() {
                "ctrl" | "control" => out.ctrl = true,
                "shift" => out.shift = true,
                "alt" | "option" => out.alt = true,
                "meta" | "cmd" | "super" | "win" => out.meta = true,
                _ => {}
            }
        }
        out
    }
}

impl fmt::Display for GestureMod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.meta {
            parts.push("Meta");
        }
        if parts.is_empty() {
            return write!(f, "Not set");
        }
        write!(f, "{}", parts.join("+"))
    }
}

/// What the picker offers, in display order.
pub(crate) const CHOICES: &[GestureMod] = &[
    GestureMod::NONE,
    GestureMod::SHIFT,
    GestureMod::CTRL,
    GestureMod::CTRL_SHIFT,
    GestureMod::ALT,
    GestureMod::CTRL_ALT,
    GestureMod::SHIFT_ALT,
    GestureMod::META,
];

pub(crate) fn choice_labels() -> Vec<String> {
    CHOICES.iter().map(|mask| mask.to_string()).collect()
}

pub(crate) fn choice_index(mask: GestureMod) -> i32 {
    CHOICES
        .iter()
        .position(|choice| *choice == mask)
        .unwrap_or(0) as i32
}

pub(crate) struct GestureSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default: GestureMod,
}

/// The registry. Order is display order in Preferences > Shortcuts.
pub(crate) static GESTURES: &[GestureSpec] = &[
    GestureSpec {
        id: "gesture.snap-override",
        label: "Snap override",
        description: "Inverts the snap toggle for one drag",
        default: GestureMod::SHIFT,
    },
    GestureSpec {
        id: "gesture.add-to-selection",
        label: "Add to selection",
        description: "Marquee or click adds instead of replacing",
        default: GestureMod::CTRL,
    },
    GestureSpec {
        id: "gesture.subtract-from-selection",
        label: "Remove from selection",
        description: "Marquee or click removes from the selection",
        default: GestureMod::CTRL_SHIFT,
    },
    GestureSpec {
        id: "gesture.copy-drag",
        label: "Copy on drag",
        description: "Drags a duplicate away, leaving the original",
        default: GestureMod::CTRL,
    },
    GestureSpec {
        id: "gesture.stretch-drag",
        label: "Stretch selection",
        description: "Drag a note edge to scale the whole selection in time",
        default: GestureMod::ALT,
    },
];

/// Resolves gesture ids to modifiers, merging the registry's defaults with
/// the user's persisted overrides.
pub(crate) struct GestureTable {
    bindings: HashMap<&'static str, GestureMod>,
}

impl GestureTable {
    pub(crate) fn build(overrides: &HashMap<String, String>) -> Self {
        let bindings = GESTURES
            .iter()
            .map(|spec| {
                let mask = overrides
                    .get(spec.id)
                    .map(|text| GestureMod::parse(text))
                    .unwrap_or(spec.default);
                (spec.id, mask)
            })
            .collect();
        Self { bindings }
    }

    pub(crate) fn modifier(&self, id: &str) -> GestureMod {
        self.bindings.get(id).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{GestureMod, GestureTable, CHOICES, GESTURES};
    use std::collections::HashMap;

    #[test]
    fn every_default_is_offered_by_the_picker_and_is_bound() {
        for spec in GESTURES {
            assert!(
                CHOICES.contains(&spec.default),
                "{} defaults to something the picker cannot show",
                spec.id
            );
            // "Not set" turns a role off entirely; nothing should ship that
            // way, or the feature is dark with no visible cause.
            assert_ne!(
                spec.default,
                GestureMod::default(),
                "{} defaults to nothing at all",
                spec.id
            );
        }
    }

    #[test]
    fn display_and_parse_round_trip_every_choice() {
        for choice in CHOICES {
            assert_eq!(GestureMod::parse(&choice.to_string()), *choice);
        }
    }

    #[test]
    fn an_override_replaces_only_its_own_role() {
        let overrides = HashMap::from([(
            "gesture.copy-drag".to_string(),
            "Shift+Alt".to_string(),
        )]);
        let table = GestureTable::build(&overrides);
        assert_eq!(
            table.modifier("gesture.copy-drag"),
            GestureMod::parse("Shift+Alt")
        );
        assert_eq!(
            table.modifier("gesture.snap-override"),
            GestureMod::parse("Shift"),
            "an unrelated role keeps its default"
        );
    }

    #[test]
    fn not_set_parses_to_unbound_so_the_role_stops_firing() {
        assert_eq!(GestureMod::parse("Not set"), GestureMod::default());
        let overrides =
            HashMap::from([("gesture.snap-override".to_string(), "Not set".to_string())]);
        assert_eq!(
            GestureTable::build(&overrides).modifier("gesture.snap-override"),
            GestureMod::default()
        );
    }
}
