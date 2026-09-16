//! The colours a channel, a track or a pattern can be given from the swatch
//! row.
//!
//! **One table, in Rust, because the markup cannot have one.** Slint has no
//! way to turn a hex string into a colour -- `appearance-dialog.slint` records
//! the same constraint about its seed colours -- so a swatch cannot derive its
//! own tint from the value it writes. If the palette lived in the markup it
//! would have to be spelled twice: once as the colours to draw and once as the
//! strings to store. Here it is spelled once and both halves are derived.
//!
//! **The table is now the theme's, which is the whole change.** It used to be
//! eleven hand-picked hues that were the same under every scheme; it is
//! `Ramp::swatches` now, so choosing Nord puts Nord's aurora in the picker.
//! The rule those eleven were chosen under did not change and is what
//! `swatches` implements: a channel colour is an identifier, so what matters
//! is that they stay apart at a rack row's height.
//!
//! These are still suggestions rather than a fixed set. What a song stores is
//! the colour itself, so **changing themes does not repaint anybody's
//! channels**, the hex field beside the swatches reaches every other colour,
//! and nothing here bounds what a project can hold.

use crate::theme::color::Rgb;
use mooloop_core::ProjectColor;

/// The palette as the swatch row wants it: the string the song stores beside
/// the colour to draw it in.
pub(crate) fn color_choices(swatches: &[Rgb]) -> Vec<crate::ColorChoice> {
    swatches
        .iter()
        .map(|color| crate::ColorChoice {
            value: color.to_hex().into(),
            tint: color.color(),
        })
        .collect()
}

/// A stored colour as the markup wants it.
///
/// One function rather than the same three lines at every push site: the
/// sidebar, the pattern chip and every channel row all need it, and a colour
/// nobody chose draws as the default rather than as a fifth spelling of
/// "absent" -- which is why every caller sends a `has-color` beside it.
pub(crate) fn to_slint(color: Option<ProjectColor>) -> slint::Color {
    color
        .map(|color| slint::Color::from_rgb_u8(color.r, color.g, color.b))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::AppearanceSettings;
    use crate::theme::builtins;
    use std::collections::HashSet;

    /// Every offered colour has to be one the project format can store and
    /// round-trip, because the swatch compares the stored hex against the
    /// string it offered to decide whether it is the selected one: `#84cc16`
    /// stored against `#84CC16` offered would draw nothing as selected while a
    /// colour was plainly set.
    ///
    /// This used to guard a hand-written table against a typo. It guards a
    /// derivation now, which is a stronger reason to have it: nobody reviews
    /// the output of `Ramp::swatches` for every theme.
    #[test]
    fn every_offered_colour_is_one_the_format_can_store() {
        for theme in builtins::all() {
            for dark in [true, false] {
                for choice in color_choices(&theme.variant(dark).ramp().swatches()) {
                    let hex = choice.value.to_string();
                    let parsed = ProjectColor::from_hex(&hex)
                        .unwrap_or_else(|| panic!("{} offered {hex}", theme.name));
                    assert_eq!(parsed.to_hex(), hex, "{} offered {hex} uncanonically", theme.name);
                }
            }
        }
    }

    /// Two rows of six is the shape `color-picker.slint` draws, and it derives
    /// its row count from this model: eleven colours plus the "no colour"
    /// square. A theme that produced ten or twelve would quietly re-shape
    /// every picker in the program.
    #[test]
    fn every_theme_offers_exactly_eleven() {
        for theme in builtins::all() {
            for dark in [true, false] {
                assert_eq!(
                    color_choices(&theme.variant(dark).ramp().swatches()).len(),
                    11,
                    "{} {}",
                    theme.name,
                    if dark { "dark" } else { "light" }
                );
            }
        }
    }

    #[test]
    fn the_offered_colours_are_distinct() {
        for theme in builtins::all() {
            let choices = color_choices(&theme.variant(true).ramp().swatches());
            let unique: HashSet<String> =
                choices.iter().map(|choice| choice.value.to_string()).collect();
            assert_eq!(unique.len(), choices.len(), "{} repeats a colour", theme.name);
        }
    }

    /// The default appearance still offers the palette that used to be the
    /// hard-coded table, so an existing project's channel colours are still
    /// shown as selected rather than as "some other colour".
    #[test]
    fn the_default_appearance_still_offers_the_old_table() {
        let offered: HashSet<String> = color_choices(&AppearanceSettings::default().swatches())
            .iter()
            .map(|choice| choice.value.to_string())
            .collect();
        for hex in [
            "#EF4444", "#F97316", "#EAB308", "#22C55E", "#14B8A6", "#0EA5E9", "#EC4899", "#A16207",
        ] {
            assert!(offered.contains(hex), "{hex} is no longer offered: {offered:?}");
        }
    }
}
