//! A colour a song owns.
//!
//! Channel and pattern colours are *content*: they travel with the project the
//! way a name does, and they mean the same thing under every theme. That is
//! why this is an RGB triple rather than an index into the palette
//! `Appearance` derives -- an index would make a song look different under a
//! different scheme, and would quietly commit the palette to having a fixed
//! number of slots. `docs/plans/interface-iteration/03-channel-identity.md`
//! states the rule; `ENHANCEMENTS.md` holds the palette question it must not
//! settle.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An opaque 24-bit colour, stored as `#RRGGBB`.
///
/// Hex because the project file is read by people: `#84CC16` is the same
/// spelling the settings file and the CSS-descended half of the world use, and
/// a table of three integers is not better for anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ProjectColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// `#RRGGBB`, always six digits and always upper case, so a round trip
    /// through the file is byte-identical and a diff of two saves shows only
    /// what changed.
    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }

    /// Parses `#RRGGBB` or `RRGGBB`, in either case. `None` for anything else:
    /// a colour is cosmetic, so the caller's job is to fall back to "no colour
    /// chosen" rather than to refuse the document.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits = text.strip_prefix('#').unwrap_or(text);
        if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
        Some(Self::new(byte(0)?, byte(2)?, byte(4)?))
    }

    /// Relative luminance, 0 (black) to 1 (white), by Rec. 709 weights.
    ///
    /// The gamma-encoded form rather than the linearised one the WCAG
    /// contrast formula uses. That is deliberate and it is the cheaper of two
    /// defensible answers: this decides one thing -- black ink or white ink
    /// over a filled swatch -- and on the eleven swatches offered the two
    /// methods disagree about none of them. A contrast *ratio* would need the
    /// linear form; a threshold does not.
    pub fn luminance(self) -> f32 {
        let channel = |value: u8| f32::from(value) / 255.0;
        0.2126 * channel(self.r) + 0.7152 * channel(self.g) + 0.0722 * channel(self.b)
    }

    /// Black or white, whichever can be read on top of this colour.
    ///
    /// A filled swatch with a label on it -- a playlist clip -- cannot use one
    /// fixed ink: the accent this replaced is a light lime and took dark text,
    /// and a user who picks indigo would get dark text on a dark field.
    ///
    /// **The threshold was chosen by looking, not by deriving.** 0.55 is the
    /// tidier-sounding number and it is wrong here: it puts orange (0.536)
    /// and sky (0.541) on light ink, and rendering the eleven swatches with
    /// both inks side by side shows dark ink plainly crisper on each. The
    /// midpoint agrees with the eye on all eleven.
    pub fn ink(self) -> Self {
        if self.luminance() > 0.50 {
            Self::new(0x18, 0x18, 0x1B)
        } else {
            Self::new(0xE4, 0xE4, 0xE7)
        }
    }

    /// The 0xRRGGBB packing Slint's `Colors.from-argb-encoded` wants. The UI
    /// adds the alpha, because an opaque colour is the only kind a song
    /// stores and a stored alpha would be a second way to say "no colour".
    pub fn to_rgb_u32(self) -> u32 {
        (u32::from(self.r) << 16) | (u32::from(self.g) << 8) | u32::from(self.b)
    }

    pub fn from_rgb_u32(packed: u32) -> Self {
        Self::new(
            ((packed >> 16) & 0xFF) as u8,
            ((packed >> 8) & 0xFF) as u8,
            (packed & 0xFF) as u8,
        )
    }
}

impl Serialize for ProjectColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ProjectColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::from_hex(&text)
            .ok_or_else(|| serde::de::Error::custom(format!("{text:?} is not a #RRGGBB colour")))
    }
}

/// Field deserializer for an optional colour that refuses to fail the load.
///
/// A malformed colour reads as "no colour chosen". The alternative -- an
/// error -- would refuse an entire song over a cosmetic field somebody
/// hand-edited, and the format's defaulted-field rule exists to stop exactly
/// that. `PROJECT_FORMAT.md` records it, because a silent correction has to be
/// written down somewhere to be honest.
pub fn deserialize_lenient<'de, D>(deserializer: D) -> Result<Option<ProjectColor>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)
        .unwrap_or_default()
        .and_then(|text| ProjectColor::from_hex(&text)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_through_both_spellings() {
        let color = ProjectColor::new(0x84, 0xCC, 0x16);
        assert_eq!(color.to_hex(), "#84CC16");
        assert_eq!(ProjectColor::from_hex("#84CC16"), Some(color));
        assert_eq!(ProjectColor::from_hex("84cc16"), Some(color));
        assert_eq!(ProjectColor::from_hex(&color.to_hex()), Some(color));
    }

    #[test]
    fn a_malformed_colour_is_no_colour_rather_than_an_error() {
        for text in ["", "#", "#84CC1", "#84CC167", "#84CC1G", "rebeccapurple"] {
            assert_eq!(ProjectColor::from_hex(text), None, "{text:?} parsed");
        }
    }

    /// Every colour a user can pick from the swatch row gets ink it can be
    /// read through. Listed as the answers rather than recomputed, because a
    /// test that re-derives the threshold agrees with it by construction and
    /// checks nothing: these are what the eye was asked about.
    #[test]
    fn each_offered_colour_gets_ink_that_can_be_read_on_it() {
        // The palette in `mooloop-ui`'s `channel_colors`, and the answer for
        // each. Duplicated here on purpose: this is the *decision*, and the
        // crate that owns the arithmetic cannot see the crate that owns the
        // list. A colour added there and not here is not a failure; a colour
        // whose answer changes silently is what this is for.
        let dark = ProjectColor::new(0x18, 0x18, 0x1B);
        let light = ProjectColor::new(0xE4, 0xE4, 0xE7);
        for (hex, expected) in [
            ("#EF4444", light), // red
            ("#F97316", dark),  // orange
            ("#EAB308", dark),  // yellow
            ("#84CC16", dark),  // lime
            ("#22C55E", dark),  // green
            ("#14B8A6", dark),  // teal
            ("#0EA5E9", dark),  // sky
            ("#6366F1", light), // indigo
            ("#A855F7", light), // violet
            ("#EC4899", light), // pink
            ("#A16207", light), // brown
        ] {
            let color = ProjectColor::from_hex(hex).expect("a palette entry that is not a colour");
            assert_eq!(color.ink(), expected, "{hex} got ink it cannot be read through");
        }
    }

    #[test]
    fn the_extremes_get_the_obvious_answer() {
        assert_eq!(ProjectColor::new(0xFF, 0xFF, 0xFF).ink().r, 0x18);
        assert_eq!(ProjectColor::new(0x00, 0x00, 0x00).ink().r, 0xE4);
    }

    /// The packing the face reads. Written down as a test because the shift
    /// order is the kind of thing that is correct once and then copied wrong.
    #[test]
    fn the_packed_form_round_trips() {
        let color = ProjectColor::new(0x12, 0x34, 0x56);
        assert_eq!(color.to_rgb_u32(), 0x123456);
        assert_eq!(ProjectColor::from_rgb_u32(0x123456), color);
    }
}
