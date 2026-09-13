//! The colours a channel or a pattern can be given from the swatch row.
//!
//! **One table, in Rust, because the markup cannot have one.** Slint has no
//! way to turn a hex string into a colour -- `appearance-dialog.slint` records
//! the same constraint about its seed colours -- so a swatch cannot derive its
//! own tint from the value it writes. If the palette lived in the markup it
//! would have to be spelled twice: once as the colours to draw and once as the
//! strings to store. Here it is spelled once and both halves are derived.
//!
//! These are suggestions, not a fixed set: what a song stores is the colour
//! itself, so the hex field beside the swatches reaches every other colour and
//! nothing here bounds what a project can hold. Adding or removing an entry
//! needs no markup change -- the grid derives its row count from the model.

use mooloop_core::ProjectColor;

/// Eleven hues plus the "no colour" square the sidebar draws after them, which
/// is twelve cells: two tidy rows of six.
///
/// Chosen to stay apart at a rack row's height rather than to be a ramp -- a
/// channel colour is an identifier, and two greens that agree at 20px are one
/// colour with extra steps.
pub(crate) const CHANNEL_COLORS: [&str; 11] = [
    "#EF4444", // red
    "#F97316", // orange
    "#EAB308", // yellow
    "#84CC16", // lime
    "#22C55E", // green
    "#14B8A6", // teal
    "#0EA5E9", // sky
    "#6366F1", // indigo
    "#A855F7", // violet
    "#EC4899", // pink
    "#A16207", // brown
];

/// The palette as the swatch row wants it: the string the song stores beside
/// the colour to draw it in.
pub(crate) fn color_choices() -> Vec<crate::ColorChoice> {
    CHANNEL_COLORS
        .iter()
        .map(|hex| {
            let color = ProjectColor::from_hex(hex).expect("a palette entry that is not a colour");
            crate::ColorChoice {
                value: (*hex).into(),
                tint: slint::Color::from_rgb_u8(color.r, color.g, color.b),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// `color_choices` unwraps every entry, so a typo here would panic at
    /// startup rather than fail a test. This is the test that stops that.
    #[test]
    fn every_palette_entry_is_a_colour_the_format_can_store() {
        for hex in CHANNEL_COLORS {
            let parsed = ProjectColor::from_hex(hex)
                .unwrap_or_else(|| panic!("{hex} is not a #RRGGBB colour"));
            // Round-tripped through the canonical spelling, because the swatch
            // compares the stored hex against this string to decide whether it
            // is the selected one: `#84cc16` stored against `#84CC16` offered
            // would draw nothing as selected while a colour was plainly set.
            assert_eq!(parsed.to_hex(), hex, "{hex} is not in canonical form");
        }
    }

    #[test]
    fn no_two_swatches_offer_the_same_colour() {
        let unique: HashSet<&str> = CHANNEL_COLORS.into_iter().collect();
        assert_eq!(unique.len(), CHANNEL_COLORS.len(), "a colour is offered twice");
    }
}
