//! What a rename field shows after somebody has typed in it.
//!
//! Every rename in the application goes through `NameField`: patterns in the
//! transport toolbar, tracks on the bus face, channels on the device-chain
//! header. All three were wired the obvious way -- `text <=> root.text` down
//! to the `TextInput` -- and all three were quietly broken by it, because a
//! `TextInput`'s `text` is an ordinary property and typing into it does not
//! *update* a binding, it **replaces** it. So the field tracked whatever the
//! application pushed until the first keystroke and then never again: rename
//! pattern 1, switch to pattern 2, and the box still read "Chorus" while the
//! menu, the playlist and the document all read "Pattern 2".
//!
//! That is invisible to a snapshot test -- one frame is always self-consistent
//! -- and invisible to a session test, because the store was correct the whole
//! time. It only shows in the sequence below, which is why this file exists.
//!
//! The harness is built here rather than driving `MainWindow`, for the reason
//! `channel_reorder.rs` gives: the geometry is then known rather than searched,
//! and the subject is one widget.

use slint::platform::WindowEvent;
use slint::{ComponentHandle, LogicalPosition, LogicalSize, SharedString};
use std::cell::RefCell;
use std::rc::Rc;

mod common;

slint::slint! {
    import { NameField } from "../ui/toolbar.slint";

    export component NameFieldHarness inherits Window {
        width: 300px;
        height: 80px;
        background: #101010;
        // The application's idea of the name, exactly as `MainWindow` binds
        // it: one-way, into the field.
        in-out property <string> name;
        // What the box is showing, which is the whole question.
        out property <string> shown: field.current-text;
        // Whether the caret is still in the box.
        out property <bool> editing: field.editing;
        callback edited(string);

        field := NameField {
            x: 20px;
            y: 20px;
            width: 200px;
            text: root.name;
            placeholder: "Channel";
            edited(value) => { root.edited(value); }
        }
    }
}

/// Let the window catch up with a property the application just set.
///
/// `NameField` re-seeds its input from a `changed` handler, and Slint runs
/// change trackers as part of the event loop rather than inside the setter.
/// A running application has one; this test does not, so the frame it would
/// have drawn is asked for here. That is not a workaround for the mechanism
/// -- it is what the mechanism waits for, and a field that updates on the
/// next drawn frame is what a user sees.
fn settle(ui: &NameFieldHarness) {
    i_slint_backend_testing::mock_elapsed_time(std::time::Duration::from_millis(16));
    ui.window().take_snapshot().expect("render a frame");
}

/// Click the field, then type `text` one character at a time.
fn type_into(ui: &NameFieldHarness, text: &str) {
    let window = ui.window();
    let centre = LogicalPosition::new(120.0, 32.0);
    window.dispatch_event(WindowEvent::PointerMoved { position: centre });
    window.dispatch_event(WindowEvent::PointerPressed {
        position: centre,
        button: slint::platform::PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position: centre,
        button: slint::platform::PointerEventButton::Left,
    });
    for character in text.chars() {
        window.dispatch_event(WindowEvent::KeyPressed {
            text: character.into(),
        });
        window.dispatch_event(WindowEvent::KeyReleased {
            text: character.into(),
        });
    }
}

#[test]
fn the_application_still_owns_the_name_after_somebody_types() {
    common::install_testing_backend();

    let ui = NameFieldHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(300.0, 80.0));

    let edits: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let edits = edits.clone();
        ui.on_edited(move |value| edits.borrow_mut().push(value.to_string()));
    }

    // The application names the thing being edited.
    ui.set_name(SharedString::from("Kick"));
    settle(&ui);
    assert_eq!(
        ui.get_shown(),
        "Kick",
        "the field did not show the name it was given"
    );

    // Somebody types. The field reports every keystroke, and shows what they
    // typed rather than what the application last said.
    type_into(&ui, "!");
    settle(&ui);
    assert!(
        !edits.borrow().is_empty(),
        "typing into the field reported no edit at all"
    );
    let typed = ui.get_shown().to_string();
    assert_ne!(typed, "Kick", "typing did not reach the field");
    assert_eq!(
        edits.borrow().last().map(String::as_str),
        Some(typed.as_str()),
        "the edit reported and the text shown disagree"
    );

    // The regression. The application now says the name is something else --
    // a different channel was selected, a project was opened, an undo landed
    // -- and the field must follow it rather than keep the keystrokes.
    ui.set_name(SharedString::from("Snare"));
    settle(&ui);
    assert_eq!(
        ui.get_shown(),
        "Snare",
        "the field kept what was typed after the application renamed the subject; \
         `NameField.text` has been bound two-way to its TextInput again"
    );

    // And it keeps following, so this is not a one-shot recovery.
    ui.set_name(SharedString::from("Closed Hat"));
    settle(&ui);
    assert_eq!(ui.get_shown(), "Closed Hat");
}


/// Escape takes the caret out of a rename field.
///
/// Until 2026-09-14 the only way out was Enter. Click into one of these, then
/// click somewhere that is not a control, and the caret stayed -- and while it
/// is there Space types a space instead of starting the transport, which is
/// right for a field somebody is editing and wrong for one nobody is. The
/// 2026-09-07 focus pass made every *control* transparent to shortcuts and
/// left text fields as the remaining case; what they needed was an exit, not
/// a change to what they consume.
///
/// The typed text deliberately survives. A `NameField` reports every
/// keystroke through `edited` and the application has already taken them, so
/// there is nothing here to cancel -- Escape is an exit, not an undo.
#[test]
fn escape_takes_the_caret_out_of_a_rename_field() {
    common::install_testing_backend();

    let ui = NameFieldHarness::new().unwrap();
    ui.window().set_size(LogicalSize::new(300.0, 80.0));
    ui.set_name("Kick".into());
    settle(&ui);

    type_into(&ui, "!");
    settle(&ui);
    assert!(ui.get_editing(), "the click did not put the caret in the field");
    assert_eq!(ui.get_shown(), "Kick!");

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: slint::platform::Key::Escape.into(),
    });
    settle(&ui);

    assert!(
        !ui.get_editing(),
        "Escape left the caret in the field, so Space would still type a space"
    );
    assert_eq!(
        ui.get_shown(),
        "Kick!",
        "Escape is a way out, not an undo: the application already has these keystrokes"
    );
}

/// Every editable text field in the interface handles Escape.
///
/// The test above proves the exit works on `NameField`; this one is why the
/// other three got it too. A field with no way out is not a broken widget --
/// it looks and behaves correctly right up until somebody clicks away and
/// then presses Space -- so the failure is invisible in every rendered frame
/// and in every test that does not think to try leaving.
///
/// A `read-only` input is excluded and is not a field: it is a selectable
/// label, which is what `save-error-dialog.slint` and the Developer page's
/// log path use so a path or a reason can be lifted out by hand. The mockup
/// tool's two are excluded for the reason its catalogue is excluded from the
/// meter check -- they are specimens, behind the `mockup` feature, and not
/// part of the shipped interface.
#[test]
fn every_editable_text_field_has_a_way_out() {
    let mut checked = 0usize;
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/ui"))
        .expect("mooloop-ui/ui is unreadable")
    {
        let path = entry.expect("unreadable directory entry").path();
        if path.extension().is_none_or(|kind| kind != "slint") {
            continue;
        }
        let file = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if file.starts_with("mockup") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("unreadable .slint file");

        let mut from = 0usize;
        while let Some(found) = source[from..].find("TextInput {") {
            let start = from + found;
            let open = source[start..].find('{').expect("the brace just matched") + start;
            let mut depth = 0i32;
            let mut end = open;
            for (offset, byte) in source.as_bytes()[open..].iter().enumerate() {
                match byte {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + offset;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            from = end;
            let block = &source[open..=end];
            let line = source[..start].matches('\n').count() + 1;
            if block.contains("read-only: true") {
                continue;
            }
            checked += 1;
            assert!(
                block.contains("Key.Escape"),
                "{file}:{line}: an editable text field with no Escape handler traps the \
                 caret, and Space then types a space instead of starting the transport:\
                 \n{block}"
            );
        }
    }

    // Four today: the rename field, the tempo entry, and the knob's two
    // numeric entries. A walk that stops matching finds none and would
    // otherwise pass.
    assert!(
        checked >= 4,
        "only {checked} editable text fields were found; the walk has stopped matching \
         the markup it is meant to read"
    );
}
