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
//! `DeviceKind::label()`. Since 2026-09-26 each kind also has a full name,
//! `DeviceKind::title()` ("Munotone ML-M1"), spelled in `SourceKinds.titles`
//! at the same positions, and this holds that list the same way (MOO-140).
//! It parses rather than evaluates, so it needs no
//! backend. It also checks the *wiring*, not just the strings: a row built
//! from the list still has to send its own index to `add-channel-clicked`,
//! which is the number `device_kind_from_int` decodes, and the popup still
//! has to be tall enough for the rows it now has.

use mooloop_ui::{device_kind_to_int, RETIRED_SOURCE_KINDS, SOURCE_KINDS_IN_PICKER_ORDER};

const MAIN_SLINT: &str = include_str!("../ui/main.slint");
const CHANNEL_RACK_SLINT: &str = include_str!("../ui/channel-rack.slint");

/// The strings of the `SourceKinds` list declared as
/// `out property <[string]> {name}:`, in the order the markup lists them.
///
/// It reads `channel-rack.slint` itself, the file the application compiles,
/// and it wants exactly one such declaration there: a second would leave this
/// holding one copy while the interface might read the other.
fn markup_source_strings(name: &str) -> Vec<&'static str> {
    let declaration = format!("out property <[string]> {name}:");
    let mut hits = CHANNEL_RACK_SLINT.match_indices(declaration.as_str());
    let (start, _) = hits
        .next()
        .unwrap_or_else(|| panic!("channel-rack.slint declares SourceKinds.{name}"));
    assert!(
        hits.next().is_none(),
        "channel-rack.slint declares `{declaration}` more than once"
    );
    let rest = &CHANNEL_RACK_SLINT[start + declaration.len()..];
    let end = rest
        .find("];")
        .unwrap_or_else(|| panic!("SourceKinds.{name} is a closed array literal"));
    rest[..end].split('"').skip(1).step_by(2).collect()
}

/// `SourceKinds.labels`: each kind's model number, or its plain name.
fn markup_source_labels() -> Vec<&'static str> {
    markup_source_strings("labels")
}

/// `SourceKinds.titles`: each kind's name and model number together.
fn markup_source_titles() -> Vec<&'static str> {
    markup_source_strings("titles")
}

/// `markup` with its `//` comments taken out, so a check reads what the
/// markup does rather than what its comments say about it. The `+` menu's
/// width note quotes "Add Munotone ML-M1" to say why the popup is as wide as
/// it is; a mention is not a row, and a wiring check that a comment could
/// satisfy is no check.
fn code_only(markup: &str) -> String {
    markup
        .lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n")
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
        .unwrap_or_else(|| panic!("the markup contains `{marker}`"));
    assert!(
        hits.next().is_none(),
        "`{marker}` appears more than once in the markup it was looked for in"
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

/// The titles in the markup are the Rust titles, at the labels' positions:
/// `SourceKinds.titles[i]` is `title()` of the kind whose number is `i`.
///
/// Adam's ruling, 2026-09-26: **both where there's room, the model number
/// where it's tight.** The `+` menu reads `titles` and the picker chip reads
/// `labels`, so the two are one list spelled twice. A kind in one and not
/// the other, or a title left behind by a rename of its label (a
/// "Drum Synth" beside "DS-SX"), is the fault this catches.
#[test]
fn the_markup_source_titles_are_the_rust_titles_in_picker_order() {
    let titles = markup_source_titles();
    assert_eq!(
        titles.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len(),
        "SourceKinds.titles lists {} sources, DeviceKind has {}",
        titles.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len()
    );
    assert_eq!(
        titles.len(),
        markup_source_labels().len(),
        "SourceKinds.titles and SourceKinds.labels must hold the same positions"
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let index = device_kind_to_int(kind) as usize;
        assert_eq!(
            titles.get(index).copied(),
            Some(kind.title()),
            "SourceKinds.titles[{index}] should be {kind:?}'s title"
        );
    }
}

/// `SourceKinds.retired`, one flag per label, in the markup's order.
fn markup_retired_flags() -> Vec<bool> {
    let declaration = "out property <[bool]> retired:";
    let start = CHANNEL_RACK_SLINT
        .find(declaration)
        .expect("channel-rack.slint declares SourceKinds.retired");
    let rest = &CHANNEL_RACK_SLINT[start + declaration.len()..];
    let end = rest
        .find("];")
        .expect("SourceKinds.retired is a closed array literal");
    rest[..end]
        .trim()
        .trim_start_matches('[')
        .split(',')
        .map(|flag| match flag.trim() {
            "true" => true,
            "false" => false,
            other => panic!("SourceKinds.retired holds `{other}`"),
        })
        .collect()
}

/// The markup retires exactly the kinds the Rust table does, one flag per
/// source, at each kind's own position.
#[test]
fn the_markup_retires_the_rust_list() {
    let flags = markup_retired_flags();
    assert_eq!(
        flags.len(),
        SOURCE_KINDS_IN_PICKER_ORDER.len(),
        "SourceKinds.retired needs one flag per source"
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        let index = device_kind_to_int(kind) as usize;
        assert_eq!(
            flags[index],
            RETIRED_SOURCE_KINDS.contains(&kind),
            "SourceKinds.retired[{index}] disagrees about {kind:?}"
        );
    }
}

/// The toolbar picker offers the list rather than a copy of it, and hides
/// the retired sources from it.
#[test]
fn the_channel_source_picker_reads_the_one_list() {
    let picker = block(MAIN_SLINT, "if !root.editing-bus : PickerChip {");
    assert!(
        picker.contains("options: SourceKinds.labels;"),
        "the source picker should take its options from SourceKinds.labels"
    );
    assert!(
        picker.contains("hidden: SourceKinds.retired;"),
        "the source picker should hide the retired sources"
    );
}

/// The add-channel menu offers the same list, and each row sends its own
/// position -- which is what `device_kind_from_int` reads on the other side.
///
/// The menu is `AddSourceButton` in `channel-rack.slint` rather than markup
/// inline in `MainWindow`, so that `tests/add_source_menu.rs` can click it.
/// That test is the one that can see whether a row *reports*; this one can
/// only see what it is built from, and being right about that while the menu
/// did nothing is exactly what happened on MOO-53.
#[test]
fn the_add_channel_menu_offers_the_list_and_sends_its_own_row() {
    let menu = code_only(&block(
        CHANNEL_RACK_SLINT,
        "export component AddSourceButton inherits ToolButton {",
    ));
    assert!(
        menu.contains("for label[i] in SourceKinds.labels"),
        "the add-channel menu should repeat over SourceKinds.labels"
    );
    assert!(
        menu.contains("if !SourceKinds.retired[i] : MenuRow"),
        "the add-channel menu should leave the retired sources out"
    );
    assert!(
        menu.contains("root.picked(i)"),
        "each add-channel row should send its own index"
    );
    assert!(
        menu.contains("\"Add \" + SourceKinds.titles[i]"),
        "each add-channel row should name its source by SourceKinds.titles, \
         since a menu has the room for the full name"
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        for name in [kind.label(), kind.title()] {
            let hand_written = format!("\"Add {name}\"");
            assert!(
                !menu.contains(&hand_written),
                "the add-channel menu still spells {hand_written} by hand"
            );
        }
    }
}

/// The rack's `+` is that component, and hands its choice straight on.
///
/// The component is only the menu the application opens if the application
/// opens it. A second copy of the popup inline in `MainWindow` would leave
/// `tests/add_source_menu.rs` clicking markup nothing reaches -- the
/// "does anything read the copy the test checks?" failure, arriving from the
/// other direction.
#[test]
fn the_rack_plus_button_is_the_add_source_component() {
    let plus = block(MAIN_SLINT, "AddSourceButton {");
    assert!(
        plus.contains("picked(i) => { root.add-channel-clicked(i); }"),
        "the rack's + should forward its choice to add-channel-clicked"
    );
    assert!(
        !MAIN_SLINT.contains("SourceKinds.labels : "),
        "main.slint repeats over SourceKinds.labels to build rows of its own; \
         the rack's + is AddSourceButton in channel-rack.slint"
    );
}

/// **A chosen row must not close the menu before it reports.**
///
/// MOO-53, and the regression this file can actually see. Closing a popup
/// tears down the repeater item whose handler is still running, so the call
/// after it never lands: the menu opens, draws every source, and adds no
/// channel. `close-policy: close-on-click` dismisses it anyway, which is why
/// the row needs no `close()` at all.
///
/// `tests/add_source_menu.rs` is the stronger statement of this -- it clicks
/// a row and waits for the callback, so it fails on any cause rather than on
/// this one spelling. This is here because it names the mistake, and because
/// `scripts/dupe-audit popup-close-order` looks for the same shape in every
/// other popup in the interface.
#[test]
fn an_add_channel_row_reports_before_the_menu_closes() {
    let menu = block(
        CHANNEL_RACK_SLINT,
        "export component AddSourceButton inherits ToolButton {",
    );
    let row = menu
        .lines()
        .find(|line| line.contains("root.picked(i)"))
        .expect("a row that reports its index");
    assert!(
        !row.contains("menu.close()"),
        "the add-channel row closes the menu before reporting, which is the \
         defect MOO-53 was: found `{}`",
        row.trim()
    );
}

/// The popup was 108px for four rows, and a fifth device would have been
/// drawn outside it. Its height has to follow the list it is now built from,
/// so this fails on any fixed height rather than on a particular expression.
#[test]
fn the_add_channel_menu_is_as_tall_as_its_rows() {
    let menu = block(
        CHANNEL_RACK_SLINT,
        "export component AddSourceButton inherits ToolButton {",
    );
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
        header.contains("SourceKinds.titles[root.source-kind]"),
        "the source device header should take its name from SourceKinds.titles, \
         since a header has the room for the full name (MOO-276)"
    );
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        for name in [kind.label(), kind.title()] {
            let hand_written = format!("\"{name}\"");
            assert!(
                !header.contains(&hand_written),
                "the source device header still spells {hand_written} by hand"
            );
        }
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
    // A kind's full name has one spelling too. Only the kinds with a
    // nickname: the rest have a title that is their label, and a short
    // label ("Sampler") is a word the interface uses for other things.
    for kind in SOURCE_KINDS_IN_PICKER_ORDER {
        if kind.nickname().is_some() {
            let hand_written = format!("\"{}\"", kind.title());
            assert!(
                !MAIN_SLINT.contains(&hand_written),
                "main.slint spells {hand_written}; it should read \
                 SourceKinds.titles instead"
            );
        }
    }
}
