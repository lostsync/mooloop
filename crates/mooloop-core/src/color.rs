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

    /// The packing the face reads. Written down as a test because the shift
    /// order is the kind of thing that is correct once and then copied wrong.
    #[test]
    fn the_packed_form_round_trips() {
        let color = ProjectColor::new(0x12, 0x34, 0x56);
        assert_eq!(color.to_rgb_u32(), 0x123456);
        assert_eq!(ProjectColor::from_rgb_u32(0x123456), color);
    }
}
