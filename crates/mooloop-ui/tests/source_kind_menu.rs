//! Both places that offer a source device must offer every source device.
//!
//! The channel toolbar's picker and the channel rack's add-channel menu are
//! two lists of the same kinds, and for four of them they were not: the menu
//! still offered the four devices that existed when it was written, so a
//! channel could be *switched* to the ML-M1, the ML-P8, the DS-01 or Aux In
//! but never *started* as one. Everything downstream already handled all
//! eight -- `device_kind_from_int` decodes 0..=7 and `Session::add_channel`
//! takes any kind -- so nothing was broken enough to fail a test, and nothing
//! could have reported it: a picker row is markup, and no Rust table reaches
//! it.
//!
//! The markup now spells the list once, in `SourceKinds.labels`, and this
//! reads that list out of the production `.slint` and holds it against
//! `DeviceKind::label()`. It parses rather than evaluates, so it needs no
//! backend. It also checks the *wiring*, not just the strings: a row built
//! from the list still has to send its own index to `add-channel-clicked`,
//! which is the number `device_kind_from_int` decodes, and the popup still
//! has to be tall enough for the rows it now has.

use mooloop_ui::{device_kind_to_int, SOURCE_KINDS_IN_PICKER_ORDER};

const MAIN_SLINT: &str = include_str!("../ui/main.slint");
const CHANNEL_RACK_SLINT: &str = include_str!("../ui/channel-rack.slint");

/// The strings in `SourceKinds.labels`, in the order the markup lists them.
fn markup_source_labels() -> Vec<&'static str> {
    let declaration = "out property <[string]> labels:";
    let start = CHANNEL_RACK_SLINT
        .find(declaration)
        .expect("channel-rack.slint declares SourceKinds.labels");
    let rest = &CHANNEL_RACK_SLINT[start + declaration.len()..];
    let end = rest
        .find("];")
        .expect("SourceKinds.labels is a closed array literal");
    rest[..end].split('"').skip(1).step_by(2).collect()
}

/// The text of the block opened by `marker`, up to its matching brace.
///
/// The marker has to be unique; a second copy of one of these is the thing
/// this file exists to prevent, so finding two is a failure rather than a
/// reason to pick one.
fn block(markup: &str, marker: &str) -> String {
    let mut hits = markup.match_indices(marker);
    let (start, _) = hits
        .next()
        .unwrap_or_else(|| panic!("main.slint contains `{marker}`"));
    assert!(
        hits.next().is_none(),
        "`{marker}` appears more than once in main.slint"
    );
    let mut depth = 0usize;
    let mut end = start;
    for (offset, ch) in markup[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + offset + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(end > start, "`{marker}` block is not closed");
    markup[start..end].to_string()
}

/// The list in the markup is the Rust table, in the order the rest of the
/// program decodes: a label's position in it *is* its kind's number.
#[test]
fn the_markup_source_list_is_the_rust_labels_in_picker_order() {
    let labels = markup_source_labels();
    assert_eq!(
        labels.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len(),
        "SourceKinds.labels lists {} sources, DeviceKind has {}",
        labels.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len()
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let index = device_kind_to_int(kind) as usize;
        assert_eq!(
            labels.get(index).copied(),
            Some(kind.label()),
            "SourceKinds.labels[{index}] should be {kind:?}'s label"
        );
    }
}

/// The toolbar picker offers the list rather than a copy of it.
#[test]
fn the_channel_source_picker_reads_the_one_list() {
    let picker = block(MAIN_SLINT, "if !root.editing-bus : PickerChip {");
    assert!(
        picker.contains("options: SourceKinds.labels;"),
        "the source picker should take its options from SourceKinds.labels"
    );
}

/// The add-channel menu offers the same list, and each row sends its own
/// position -- which is what `device_kind_from_int` reads on the other side.
#[test]
fn the_add_channel_menu_offers_every_source_and_sends_its_own_row() {
    let menu = block(MAIN_SLINT, "add-source-menu := PopupWindow {");
    assert!(
        menu.contains("for label[i] in SourceKinds.labels"),
        "the add-channel menu should repeat over SourceKinds.labels"
    );
    assert!(
        menu.contains("root.add-channel-clicked(i)"),
        "each add-channel row should send its own index"
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let hand_written = format!("\"Add {}\"", kind.label());
        assert!(
            !menu.contains(&hand_written),
            "the add-channel menu still spells {hand_written} by hand"
        );
    }
}

/// The popup was 108px for four rows, and a fifth device would have been
/// drawn outside it. Its height has to follow the list it is now built from,
/// so this fails on any fixed height rather than on a particular expression.
#[test]
fn the_add_channel_menu_is_as_tall_as_its_rows() {
    let menu = block(MAIN_SLINT, "add-source-menu := PopupWindow {");
    let height = menu
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("height:"))
        .expect("the add-channel menu states its height");
    assert!(
        !height.trim_end_matches(';').ends_with("px"),
        "the add-channel menu's height must follow its rows, not a row count \
         written down once: found `{height}`"
    );
}

/// The device rack's header names the source from the same list.
///
/// It spelled its own eight strings as a ternary ladder on `source-kind`,
/// and nothing read it -- `the_markup_source_list_is_the_rust_labels_in_picker_order`
/// holds `SourceKinds.labels` to `DeviceKind::label()`, the picker and the
/// menu are held to the list, and the header was outside all three. A ninth
/// kind would have extended two guarded lists and a compile-time match while
/// the header went on saying "Aux In" for everything past the seventh.
#[test]
fn the_source_device_header_reads_the_one_list() {
    let header = block(MAIN_SLINT, "if !root.editing-bus : DeviceHeader {");
    assert!(
        header.contains("SourceKinds.labels[root.source-kind]"),
        "the source device header should take its name from SourceKinds.labels"
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let hand_written = format!("\"{}\"", kind.label());
        assert!(
            !header.contains(&hand_written),
            "the source device header still spells {hand_written} by hand"
        );
    }
}

/// `SourceKinds.labels` is the only place a source kind is named.
///
/// The ladder's last arm was the bare fallback `: "Aux In"`, which is how a
/// ninth kind would have been mislabelled silently rather than caught. The
/// label belongs to `channel-rack.slint`; a copy of it in `main.slint` is the
/// fault returning.
#[test]
fn main_slint_names_no_source_kind_of_its_own() {
    assert!(
        !MAIN_SLINT.contains("\"Aux In\""),
        "main.slint spells a source kind's label; it should read \
         SourceKinds.labels instead"
    );
}
