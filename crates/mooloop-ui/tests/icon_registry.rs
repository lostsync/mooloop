//! The icon registry, `ui/icons.slint`, held to itself and to the Rust kind
//! tables (`docs/plans/icon-pass/01-the-registry.md`, MOO-279).
//!
//! Adam asked for the registry *"to help with consistency and avoid dupes"*
//! (2026-09-26), so the first thing this checks is that no two entries draw
//! the same path. The kind tables are indexed by the numbers the markup
//! already sends across the boundary -- `device_kind_to_int` and
//! `effect_kind_index` -- the way `SourceKinds.labels` is, so a kind table
//! one entry short draws the next kind's icon on every device after the gap,
//! and nothing on the screen would say so.
//!
//! It reads the production `icons.slint`, the file the application compiles,
//! not a copy, and it parses rather than evaluates, so it needs no backend.
//! `scripts/dupe-audit icon-literal` is the other half: it counts icons drawn
//! anywhere *but* this file.

use std::collections::BTreeMap;

use mooloop_core::EffectKind;
use mooloop_ui::{device_kind_to_int, effect_kind_index, SOURCE_KINDS_IN_PICKER_ORDER};

const ICONS_SLINT: &str = include_str!("../ui/icons.slint");

/// `icons.slint` with `//` comments removed, so a name or a quote in the
/// prose is not read as markup.
fn code() -> String {
    ICONS_SLINT
        .lines()
        .map(|line| {
            let mut inside = false;
            let bytes = line.as_bytes();
            for (at, byte) in bytes.iter().enumerate() {
                match byte {
                    b'"' if at == 0 || bytes[at - 1] != b'\\' => inside = !inside,
                    b'/' if !inside && bytes.get(at + 1) == Some(&b'/') => return &line[..at],
                    _ => {}
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The body of `export global Icons { ... }`.
fn icons_global(code: &str) -> String {
    let start = code
        .find("export global Icons {")
        .expect("icons.slint declares `export global Icons`");
    let mut depth = 0;
    for (at, ch) in code[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return code[start..start + at].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("`export global Icons` is not closed");
}

/// Every `property <string> name: expression;` in the global, by name, with
/// whether it is `out` (an entry) or `private` (a part entries are built
/// from).
fn string_properties(global: &str) -> BTreeMap<String, (bool, String)> {
    let mut found = BTreeMap::new();
    for (at, _) in global.match_indices("property <string> ") {
        let head = &global[..at];
        let public = head.trim_end().ends_with("out");
        let rest = &global[at + "property <string> ".len()..];
        let colon = rest.find(':').expect("a string property has a binding");
        let name = rest[..colon].trim().to_string();
        let end = rest.find(';').expect("a string property's binding ends");
        let expression = rest[colon + 1..end].trim().to_string();
        assert!(
            found.insert(name.clone(), (public, expression)).is_none(),
            "Icons.{name} is declared twice"
        );
    }
    found
}

/// A binding resolved to the string it produces: `+`-joined string literals
/// and `root.<name>` references to other properties of the global. Anything
/// else is a binding this test cannot read, and it says so rather than
/// guessing.
fn resolve(name: &str, all: &BTreeMap<String, (bool, String)>, depth: usize) -> String {
    assert!(depth < 8, "Icons.{name} refers to itself");
    let (_, expression) = &all[name];
    let mut out = String::new();
    for term in split_terms(expression) {
        let term = term.trim();
        if let Some(literal) = term.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
            out.push_str(literal);
        } else if let Some(other) = term.strip_prefix("root.") {
            assert!(all.contains_key(other), "Icons.{name} refers to unknown Icons.{other}");
            out.push_str(&resolve(other, all, depth + 1));
        } else {
            panic!("Icons.{name}'s binding has a term this test cannot read: `{term}`");
        }
    }
    out
}

/// `expression` split on the `+` signs that are outside string literals.
fn split_terms(expression: &str) -> Vec<&str> {
    let mut terms = Vec::new();
    let mut inside = false;
    let mut start = 0;
    for (at, ch) in expression.char_indices() {
        match ch {
            '"' => inside = !inside,
            '+' if !inside => {
                terms.push(&expression[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    terms.push(&expression[start..]);
    terms
}

/// One slot of a kind table: a path drawn there, or a reference to a named
/// entry (`root.plugin`), which is that entry and not a second copy of it.
#[derive(Debug, PartialEq)]
enum Slot {
    Path(String),
    Entry(String),
}

/// The slots of `out property <[string]> {name}: [ ... ];`.
fn string_array(global: &str, name: &str) -> Vec<Slot> {
    let declaration = format!("out property <[string]> {name}:");
    let start = global
        .find(&declaration)
        .unwrap_or_else(|| panic!("Icons declares `{declaration}`"));
    let rest = &global[start + declaration.len()..];
    let end = rest.find("];").unwrap_or_else(|| panic!("Icons.{name} is a closed array"));
    let body = rest[..end].trim().trim_start_matches('[');
    body.split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| {
            if let Some(entry) = item.strip_prefix("root.") {
                return Slot::Entry(entry.to_string());
            }
            Slot::Path(
                item.strip_prefix('"')
                    .and_then(|i| i.strip_suffix('"'))
                    .unwrap_or_else(|| {
                        panic!("Icons.{name} holds `{item}`, not a string literal or `root.<entry>`")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// Every entry drawn from the registry: each `out` string, and each kind
/// table slot that draws its own path, as (where, path). A slot naming an
/// entry is that entry, already listed, so it is checked to exist and not
/// listed twice.
fn entries() -> Vec<(String, String)> {
    let code = code();
    let global = icons_global(&code);
    let properties = string_properties(&global);
    let mut entries: Vec<(String, String)> = properties
        .iter()
        .filter(|(_, (public, _))| *public)
        .map(|(name, _)| (format!("Icons.{name}"), resolve(name, &properties, 0)))
        .collect();
    for table in ["source-kinds", "effect-kinds"] {
        for (index, slot) in string_array(&global, table).into_iter().enumerate() {
            match slot {
                Slot::Path(path) => entries.push((format!("Icons.{table}[{index}]"), path)),
                Slot::Entry(entry) => assert!(
                    properties.get(&entry).is_some_and(|(public, _)| *public),
                    "Icons.{table}[{index}] names Icons.{entry}, which is not an entry"
                ),
            }
        }
    }
    entries
}

/// Adam's reason for the registry: no two entries draw the same thing.
/// Compared with whitespace normalised, because `"M 1 2"` and `"M 1  2"`
/// are one drawing.
#[test]
fn no_two_icons_are_the_same_path() {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (name, path) in entries() {
        let normal = path.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!normal.is_empty(), "{name} is empty; an entry that draws nothing is not an icon");
        if let Some(first) = seen.insert(normal, name.clone()) {
            panic!("{name} draws the same path as {first}; name one meaning once");
        }
    }
}

/// The registry is read by the parse above, so a parse that silently found
/// nothing would pass the duplicate check vacuously. It must see the entries
/// the toolbar is known to draw.
#[test]
fn the_parse_sees_the_registry() {
    let names: Vec<String> = entries().into_iter().map(|(name, _)| name).collect();
    for expected in ["Icons.tool-select", "Icons.loop", "Icons.panel-left", "Icons.close"] {
        assert!(names.iter().any(|n| n == expected), "the parse did not find {expected}");
    }
    let code = code();
    let properties = string_properties(&icons_global(&code));
    assert_eq!(
        resolve("panel-left", &properties, 0),
        "M 2.5 3 L 13.5 3 L 13.5 13 L 2.5 13 Z M 6.5 3 L 6.5 13",
        "a composed entry resolves through the private part it is built from"
    );
}

/// `Icons.source-kinds[i]` is the icon of the source whose number is `i`, at
/// `SourceKinds.labels`'s positions: one slot per source the picker offers.
/// A plugin instrument (8) is not one of them; it takes the generic plugin
/// icon (step 02).
#[test]
fn source_kinds_has_one_slot_per_source_number() {
    let code = code();
    let slots = string_array(&icons_global(&code), "source-kinds");
    assert_eq!(
        slots.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len(),
        "Icons.source-kinds has {} slots, the picker offers {} sources",
        slots.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len()
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let index = device_kind_to_int(kind) as usize;
        assert!(index < slots.len(), "{kind:?} is number {index}, past Icons.source-kinds");
    }
}

/// `Icons.effect-kinds[i]` is the icon of the effect whose number is `i`.
/// The numbers are `effect_kind_index`'s, which leave 15 to a plugin insert
/// that `EffectKind::ALL` does not list, so the table runs to the highest
/// number rather than to `ALL.len()`: every kind `ALL` offers has its slot,
/// the plugin's is there too, and there is none past the last number.
#[test]
fn effect_kinds_has_one_slot_per_effect_number() {
    let code = code();
    let slots = string_array(&icons_global(&code), "effect-kinds");
    let numbers: Vec<i32> = EffectKind::ALL
        .iter()
        .copied()
        .chain([EffectKind::Plugin])
        .map(effect_kind_index)
        .collect();
    let highest = *numbers.iter().max().expect("there are effect kinds") as usize;
    assert_eq!(
        slots.len(),
        highest + 1,
        "Icons.effect-kinds has {} slots; effect numbers run 0..={highest}",
        slots.len()
    );
    for kind in EffectKind::ALL {
        let index = effect_kind_index(kind) as usize;
        assert!(index < slots.len(), "{kind:?} is number {index}, past Icons.effect-kinds");
    }
}

/// An entry nothing draws is a lead, not a failure (the step file): the
/// relay moves Mixer's, Instruments' and Effects' sets in before their faces
/// point at them, and step 02 draws kind icons before the header uses them.
/// Printed so `cargo test -- --nocapture` lists them.
#[test]
fn unused_icons_are_reported() {
    let code = code();
    let properties = string_properties(&icons_global(&code));
    // Every markup file but the registry, read from the tree rather than
    // from a list here: a hand-written list is the one that misses the file
    // added next month.
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let markup: Vec<String> = std::fs::read_dir(&ui)
        .expect("ui/ is readable")
        .map(|entry| entry.expect("a ui/ entry").path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "slint")
                && path.file_name().is_some_and(|name| name != "icons.slint")
        })
        .map(|path| std::fs::read_to_string(path).expect("a .slint file is readable"))
        .collect();
    let unused: Vec<&String> = properties
        .iter()
        .filter(|(_, (public, _))| *public)
        .map(|(name, _)| name)
        .filter(|name| {
            let reference = format!("Icons.{name}");
            !markup.iter().any(|text| {
                text.match_indices(&reference).any(|(at, _)| {
                    // `Icons.panel-left` must not count as a use of
                    // `Icons.panel-left-fill`'s prefix, nor the reverse, and
                    // `SamplerDeviceIcons.previous` is not `Icons.previous`.
                    let name_char = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
                    let before = text[..at].chars().next_back();
                    let after = text[at + reference.len()..].chars().next();
                    !before.is_some_and(name_char) && !after.is_some_and(name_char)
                })
            })
        })
        .collect();
    if !unused.is_empty() {
        eprintln!("icon registry: entries nothing draws yet (a lead, not a failure): {unused:?}");
    }
}

/// Every kind has its drawing (MOO-273): an empty slot would draw nothing in
/// that kind's header, and nothing on the screen would say why. The
/// duplicate check refuses an empty path too; this one names the kind.
#[test]
fn every_kind_has_an_icon() {
    let code = code();
    let global = icons_global(&code);
    for table in ["source-kinds", "effect-kinds"] {
        for (index, slot) in string_array(&global, table).into_iter().enumerate() {
            assert!(
                slot != Slot::Path(String::new()),
                "Icons.{table}[{index}] is empty; every kind has an icon"
            );
        }
    }
}

/// A plugin is one generic icon whether it is a source or an effect
/// (MOO-273). The source header reaches it past the end of the source
/// table; an effect row reaches it at the plugin's own number, which must
/// therefore name the entry rather than draw a second plug.
#[test]
fn a_plugin_is_the_one_plugin_icon() {
    let code = code();
    let slots = string_array(&icons_global(&code), "effect-kinds");
    let index = effect_kind_index(EffectKind::Plugin) as usize;
    assert_eq!(
        slots[index],
        Slot::Entry("plugin".to_string()),
        "Icons.effect-kinds[{index}], a plugin insert's, should be `root.plugin`"
    );
    assert_eq!(
        device_kind_to_int(mooloop_core::DeviceKind::Plugin) as usize,
        SOURCE_KINDS_IN_PICKER_ORDER.len(),
        "a plugin instrument's number is the first past Icons.source-kinds, \
         which is what the source header's `Icons.plugin` fallback reads"
    );
}
