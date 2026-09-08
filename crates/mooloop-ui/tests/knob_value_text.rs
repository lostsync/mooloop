//! Every wrapper that owns a value's text has to hand that text on.
//!
//! `ParameterKnob` renders its *value* as its tooltip, deliberately — the
//! label is already on screen, so the bubble exists to confirm the number.
//! The same string is its `accessible-value`. A wrapper that takes the
//! formatted value for itself (`display-text`), draws it in a field of its
//! own, and turns the knob's own readout off therefore owes the knob that
//! string back; otherwise the tooltip resolves to `@markdown("")` and the
//! knob hovers as an empty box with nothing to read out.
//!
//! That is exactly what `KnobStack` did from the day it was written: 85 knobs
//! across ML-P8 and DS-01, every one of them silent, while its sibling
//! `KnobField` forwarded the line correctly. Nothing caught it — the tooltip
//! sweep in `docs/plans/ui-consistency-pass/` audited what tooltip *strings*
//! said and never asked whether a rendered tooltip resolved to anything at
//! all, and no test in the workspace mentions a tooltip.
//!
//! This is a markup scan, like `slint_face_agreement.rs`: a rendered tooltip
//! cannot be read back without `ElementHandle`, which needs a build with
//! `SLINT_EMIT_DEBUG_INFO=1` that this workspace only makes under the `mcp`
//! feature. Scanning is what is left, and it is enough — the defect and its
//! fix are both a single line of markup.

const UI_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/ui");

/// The declaration a wrapper makes when it takes the formatted value over.
const DECLARES: &str = "in property <string> display-text";

/// The two ways that string may leave again: down to a widget that renders it
/// as its own tooltip, or straight onto the component's accessibility node.
const FORWARDS: [&str; 2] = ["value-text: root.display-text", "accessible-value: root.display-text"];

#[test]
fn a_wrapper_that_takes_display_text_hands_it_on() {
    let mut checked = 0;
    for entry in std::fs::read_dir(UI_DIR).expect("the ui directory is missing") {
        let path = entry.expect("unreadable ui directory entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("slint") {
            continue;
        }
        let markup = std::fs::read_to_string(&path).expect("unreadable .slint file");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();

        for component in components_declaring(&markup, DECLARES) {
            checked += 1;
            assert!(
                FORWARDS.iter().any(|forward| component.body.contains(forward)),
                "{name}'s `{}` takes `display-text` and never passes it on, so the \
                 knob inside it renders an empty tooltip and reports no \
                 accessible value. Forward it with `value-text: root.display-text;` \
                 the way `KnobField` does.",
                component.name
            );
        }
    }
    assert!(
        checked >= 2,
        "the scan found {checked} wrappers taking `display-text`; it used to find \
         at least KnobField, KnobStack and the toolbar's two fields, so either \
         the property was renamed or this test stopped looking at anything"
    );
}

struct Component {
    name: String,
    body: String,
}

/// Every top-level `component` block in `markup` whose body contains
/// `needle`. Components in this project's markup are always declared at
/// column zero and never nested, so the next such line ends the previous
/// block -- which is sturdier here than brace matching, since a brace inside
/// a comment or a string interpolation would throw that off.
fn components_declaring(markup: &str, needle: &str) -> Vec<Component> {
    let mut found: Vec<Component> = Vec::new();
    for line in markup.lines() {
        let declaration = line
            .strip_prefix("export component ")
            .or_else(|| line.strip_prefix("component "));
        if let Some(declaration) = declaration {
            found.push(Component {
                name: declaration
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                body: String::new(),
            });
        } else if let Some(current) = found.last_mut() {
            current.body.push_str(line);
            current.body.push('\n');
        }
    }
    found.retain(|component| component.body.contains(needle));
    found
}
