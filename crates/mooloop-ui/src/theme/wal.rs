//! Reading the colours the wallpaper already decided.
//!
//! pywal and wallust both generate a sixteen-colour terminal palette from an
//! image and cache it, and on a riced Linux desktop half the programs on
//! screen are already wearing it. A DAW that cannot join in is the one window
//! that looks wrong, so mooloop reads the same caches.
//!
//! **It reads the cache; it does not run the generator.** Shelling out to
//! `wal` would mean owning somebody's wallpaper, a backend choice and a
//! several-second image analysis, and getting all three wrong is easy. Reading
//! a file that is already there is neither.
//!
//! Four shapes turn up in practice and all four are handled:
//!
//! | Path | Shape |
//! | --- | --- |
//! | `~/.cache/wal/colors.json` | pywal's JSON: `special.background`, `colors.colorN` |
//! | `~/.cache/wallust/*.json` | wallust's, pywal-compatible or a bare array |
//! | `~/.cache/wal/colors` | sixteen lines of `#RRGGBB` |
//! | `~/.cache/wallust/colors` | the same, from a wallust template |
//!
//! The JSON is scanned rather than deserialized. That is a deliberate
//! narrowing and not a shortcut: the documents are machine-written, the whole
//! payload is quoted hex strings under known keys, and the alternative is a
//! JSON dependency in a crate that has none for the sake of eighteen colours.
//! What the scanner gives up is the ability to tell a nested `background` from
//! a top-level one, and neither generator writes two.

use super::color::Rgb;
use super::ramp::Ramp;
use std::path::{Path, PathBuf};

/// One generator's cached palette, and where it was read from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WalPalette {
    pub colors: [Rgb; 16],
    pub background: Option<Rgb>,
    pub foreground: Option<Rgb>,
    pub source: PathBuf,
}

impl WalPalette {
    pub(crate) fn ramp(&self) -> Ramp {
        Ramp::from_ansi(&self.colors, self.background, self.foreground)
    }

    /// What the Appearance page says under the Wallpaper row: which file, and
    /// whether it is the light palette or the dark one.
    pub(crate) fn describe(&self) -> String {
        let cache = cache_dir().unwrap_or_default();
        let name = self.source.strip_prefix(&cache).unwrap_or(&self.source);
        name.display().to_string()
    }
}

/// The name the Appearance page lists the wallpaper palette under. It is not
/// in `builtins::all()` because it is not content: it is whatever is in the
/// cache this minute, and it is absent from the list entirely when nothing is.
pub(crate) const WALLPAPER_THEME: &str = "Wallpaper";

fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
}

/// Every cache file worth reading, newest first.
///
/// wallust keys its cache by image and backend, so its directory holds one
/// file per wallpaper anybody has ever set and only the newest is the current
/// one. pywal overwrites in place, so its path is fixed.
fn candidates() -> Vec<PathBuf> {
    let Some(cache) = cache_dir() else {
        return Vec::new();
    };
    let mut paths = vec![
        cache.join("wal/colors.json"),
        cache.join("wallust/colors.json"),
        cache.join("wal/colors"),
        cache.join("wallust/colors"),
    ];
    if let Ok(entries) = std::fs::read_dir(cache.join("wallust")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                paths.push(path);
            }
        }
    }
    paths.retain(|path| path.is_file());
    paths.sort();
    paths.dedup();
    paths.sort_by_key(|path| {
        std::cmp::Reverse(
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok(),
        )
    });
    paths
}

/// The newest readable palette, or nothing if neither generator has ever run.
pub(crate) fn load() -> Option<WalPalette> {
    candidates().into_iter().find_map(|path| read(&path))
}

pub(crate) fn read(path: &Path) -> Option<WalPalette> {
    let text = std::fs::read_to_string(path).ok()?;
    parse(&text).map(|(colors, background, foreground)| WalPalette {
        colors,
        background,
        foreground,
        source: path.to_path_buf(),
    })
}

type Parsed = ([Rgb; 16], Option<Rgb>, Option<Rgb>);

fn parse(text: &str) -> Option<Parsed> {
    let pairs = quoted_pairs(text);
    let lookup = |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, value)| Rgb::parse(value))
    };

    let mut colors = [Rgb::parse_or_black("#000000"); 16];
    let keyed = (0..16).all(|index| match lookup(&format!("color{index}")) {
        Some(color) => {
            colors[index] = color;
            true
        }
        None => false,
    });

    if !keyed {
        // Either a bare `"colors": ["#...", ...]` array or a plain text file:
        // both are just hex strings in order, and sixteen of them is a
        // palette. Fewer is something else and is left alone.
        let found: Vec<Rgb> = hex_runs(text).collect();
        if found.len() < 16 {
            return None;
        }
        // The background and foreground, when a JSON file states them, are
        // written before the colour list; taking the *last* sixteen would
        // swallow them. Taking the first sixteen of a plain file is exactly
        // right, and of a JSON one is right because `special` precedes
        // `colors` in both generators' output -- so skip a leading three when
        // that is what they are.
        let offset = if found.len() >= 19 {
            found.len() - 16
        } else {
            0
        };
        colors.copy_from_slice(&found[offset..offset + 16]);
    }

    Some((colors, lookup("background"), lookup("foreground")))
}

/// Every `"key": "value"` pair in a JSON document whose value is a *string*,
/// flattened. Order is document order and nesting is ignored, which is what
/// makes this a scanner rather than a parser.
///
/// **A key whose value is an object is not a pair**, and getting that wrong is
/// subtle rather than loud: `"special": { "background": ... }` paired `special`
/// with `background` and then every pair after it was shifted by one, so
/// `color0` went missing and the palette silently fell through to the
/// positional fallback. It read the right sixteen colours by luck and lost the
/// background entirely. What separates the two cases is what sits between the
/// key and the next string: exactly a colon means a string value, and a colon
/// followed by a brace or a bracket means something else begins there.
fn quoted_pairs(text: &str) -> Vec<(String, String)> {
    let strings = quoted_strings(text);
    let mut pairs = Vec::new();
    let mut index = 0;
    while index + 1 < strings.len() {
        let (key, _, key_end) = &strings[index];
        let (value, value_start, _) = &strings[index + 1];
        if text[*key_end..*value_start].trim() == ":" {
            pairs.push((key.clone(), value.clone()));
            index += 2;
        } else {
            index += 1;
        }
    }
    pairs
}

/// Every double-quoted string in the text, with the byte its opening quote
/// sits on and the byte just past its closing quote.
///
/// Both offsets, because [`quoted_pairs`] has to look at what lies *between*
/// two strings rather than only at what follows one. No escape handling: a
/// colour cache holds hex and file paths, and a backslash in a path would at
/// worst end a string early and produce a value that is not a colour, which
/// every caller here already tolerates.
fn quoted_strings(text: &str) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'"' {
            if let Some(length) = bytes[index + 1..].iter().position(|byte| *byte == b'"') {
                let start = index + 1;
                let end = start + length;
                if let Ok(piece) = std::str::from_utf8(&bytes[start..end]) {
                    out.push((piece.to_owned(), index, end + 1));
                }
                index = end + 1;
                continue;
            }
            break;
        }
        index += 1;
    }
    out
}

/// Every `#RRGGBB` in the text, in order.
fn hex_runs(text: &str) -> impl Iterator<Item = Rgb> + '_ {
    text.match_indices('#').filter_map(|(at, _)| {
        let rest = &text[at..];
        let end = rest
            .char_indices()
            .take(8)
            .take_while(|(offset, c)| *offset == 0 || c.is_ascii_hexdigit())
            .count();
        if end == 7 {
            Rgb::parse(&rest[..7])
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PYWAL: &str = r##"{
    "wallpaper": "/home/lost/pictures/Escher #3.png",
    "alpha": "100",
    "special": {
        "background": "#1D1F21",
        "foreground": "#C5C8C6",
        "cursor": "#C5C8C6"
    },
    "colors": {
        "color0": "#1D1F21", "color1": "#CC6666", "color2": "#B5BD68",
        "color3": "#F0C674", "color4": "#81A2BE", "color5": "#B294BB",
        "color6": "#8ABEB7", "color7": "#C5C8C6", "color8": "#969896",
        "color9": "#CC6666", "color10": "#B5BD68", "color11": "#F0C674",
        "color12": "#81A2BE", "color13": "#B294BB", "color14": "#8ABEB7",
        "color15": "#FFFFFF"
    }
}"##;

    #[test]
    fn a_pywal_cache_parses_into_sixteen_colours_and_a_background() {
        let (colors, background, foreground) = parse(PYWAL).expect("pywal json parses");
        assert_eq!(colors[0].to_hex(), "#1D1F21");
        assert_eq!(colors[4].to_hex(), "#81A2BE");
        assert_eq!(colors[15].to_hex(), "#FFFFFF");
        assert_eq!(background.map(|c| c.to_hex()).as_deref(), Some("#1D1F21"));
        assert_eq!(foreground.map(|c| c.to_hex()).as_deref(), Some("#C5C8C6"));
    }

    /// A wallpaper path with a `#` in it is the reason `quoted_pairs` looks
    /// for keys rather than for hex: the fallback scanner would count that
    /// `#3` as a candidate colour and shift the whole palette by one.
    #[test]
    fn a_hash_in_the_wallpaper_path_does_not_shift_the_palette() {
        let (colors, ..) = parse(PYWAL).unwrap();
        assert_eq!(colors[1].to_hex(), "#CC6666");
    }

    #[test]
    fn sixteen_lines_of_hex_is_a_palette() {
        let plain = (0..16)
            .map(|index| format!("#{index:02X}{index:02X}{index:02X}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (colors, background, _) = parse(&plain).expect("plain colours parse");
        assert_eq!(colors[0].to_hex(), "#000000");
        assert_eq!(colors[15].to_hex(), "#0F0F0F");
        assert_eq!(background, None);
    }

    /// wallust's own cache writes the list as an array rather than as keys.
    #[test]
    fn a_bare_colour_array_is_a_palette() {
        let list: Vec<String> = (0..16)
            .map(|index| format!("\"#{index:02X}00{index:02X}\"", index = index * 3))
            .collect();
        let json = format!(
            "{{\"wallpaper\":\"/tmp/x.png\",\"colors\":[{}]}}",
            list.join(",")
        );
        let (colors, ..) = parse(&json).expect("array parses");
        assert_eq!(colors[0].to_hex(), "#000000");
        assert_eq!(colors[5].to_hex(), "#0F000F");
    }

    /// The nesting bug, stated as itself: `special` is a key whose value is an
    /// object, and pairing it with the first key *inside* that object shifted
    /// every pair after it -- which lost `color0` and the background, and
    /// still produced sixteen plausible colours through the positional
    /// fallback. A scanner that is wrong this quietly needs a test that names
    /// the shape rather than one that checks the output looks reasonable.
    #[test]
    fn a_key_whose_value_is_an_object_is_not_a_pair() {
        let pairs = quoted_pairs(PYWAL);
        let lookup = |key: &str| {
            pairs
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(lookup("background"), Some("#1D1F21"));
        assert_eq!(lookup("color0"), Some("#1D1F21"));
        assert_eq!(lookup("color15"), Some("#FFFFFF"));
        assert_eq!(lookup("special"), None, "an object is not a string value");
        assert_eq!(lookup("colors"), None);
    }

    #[test]
    fn something_that_is_not_a_palette_is_not_read_as_one() {
        assert!(parse("").is_none());
        assert!(parse("{}").is_none());
        assert!(parse("#FF0000\n#00FF00\n").is_none());
        assert!(parse("the quick brown fox").is_none());
    }

    /// The whole point: a cache turns into a ramp the interface can wear, and
    /// the ramp has a full swatch palette rather than the six hues ANSI names.
    #[test]
    fn a_cache_becomes_a_ramp_with_a_full_swatch_palette() {
        let (colors, background, foreground) = parse(PYWAL).unwrap();
        let ramp = Ramp::from_ansi(&colors, background, foreground);
        assert!(ramp.is_dark());
        assert_eq!(ramp.swatches().len(), super::super::ramp::SWATCH_COUNT);
        assert!(
            ramp.text_contrast(1.0) >= 4.5,
            "wallpaper ramp text ratio {:.2}",
            ramp.text_contrast(1.0)
        );
    }

    /// `candidates` touches the real filesystem and must be safe to call on a
    /// machine that has never run either generator.
    #[test]
    fn looking_for_caches_that_do_not_exist_is_not_an_error() {
        let _ = candidates();
        let _ = load();
    }
}
