//! What the rack draws of a chain that holds layers
//! (`docs/plans/archive/containers/09-the-rack-draws-branches.md`).
//!
//! A layer shows one branch at a time in the rack, the way Bitwig's FX Layer
//! does: its face lists every branch, and only the **selected** branch's rows
//! are drawn to the right of it, under a bracket. Everything here is derived
//! from the flat chain and the selection, for the reason the retired
//! `containers_closing_at` gave: a repeater item cannot walk its own model,
//! and "where does this box end" is not a fact any one row holds. That
//! function answered it from the next *row*; with branches hidden the answer
//! has to come from the next *drawn* row, so this replaced it.
//!
//! The selection is view state. It lives in `UiState`, keyed by the layer's
//! identity, is not saved and not in undo, and never reaches the engine --
//! choosing which branch to look at is looking, not an edit.

use mooloop_core::{DeviceId, EffectSlotState};

/// One branch of a layer, as its face lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BranchView {
    /// The rack index of the branch head.
    pub slot: usize,
    pub name: String,
    /// Whether the head has Level, Mute and Solo: whether it is a container.
    pub controls: bool,
}

/// What the rack draws of one row, beyond what the row itself says.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct RowView {
    /// The row lies in a branch its layer is not showing.
    pub hidden: bool,
    /// The depth of the next row that is drawn, or 0 past the last: what a
    /// box asks to know whether it ends here.
    pub next_depth: i32,
    /// The row the join after this one adds a device before: the next drawn
    /// row, or the chain's length past the last. -1 on a layer's own row,
    /// whose join leads into the branch it shows: a device added there would
    /// be a new branch, which is the layer face's `+`, not the rack's
    /// (MOO-218).
    pub join_before: i32,
    /// The containers whose last *drawn* row this is, innermost first, as
    /// rack indices.
    pub closing: Vec<i32>,
    /// On a layer's row: its branches, in rack order.
    pub branches: Vec<BranchView>,
    /// On a layer's row: the rack index of the branch it is showing, or -1
    /// when it has none.
    pub selected_branch: i32,
    /// The row lies in the branch its innermost layer is showing, and so
    /// wears that layer's bracket; `bracket_start` and `bracket_end` are its
    /// first and last drawn rows.
    pub bracket: bool,
    pub bracket_start: bool,
    pub bracket_end: bool,
}

/// The name a branch wears in its layer's list: the preset it was loaded
/// from, otherwise the first device inside it, otherwise its own kind --
/// "Empty" for a container holding nothing.
pub(crate) fn branch_name(
    effects: &[EffectSlotState],
    head: usize,
    preset_name: impl Fn(DeviceId) -> Option<String>,
) -> String {
    let Some(effect) = effects.get(head) else {
        return String::new();
    };
    if let Some(name) = preset_name(effect.id).filter(|name| !name.is_empty()) {
        return name;
    }
    if !effect.params.is_container() {
        return effect.kind().label().to_string();
    }
    // The first leaf in the run, skipping boxes: a branch that is a chain of
    // Drive then Bitcrush reads "Drive", as Bitwig names a layer after its
    // first device.
    let span = mooloop_core::span_of(effects, head);
    effects[span]
        .iter()
        .find(|effect| !effect.params.is_container())
        .map_or_else(|| "Empty".to_string(), |effect| effect.kind().label().to_string())
}

/// Lay out `effects` for the rack, showing for each layer the branch
/// `selected` names by the layer's identity, or its first branch when that
/// names none of them.
pub(crate) fn rack_view(
    effects: &[EffectSlotState],
    selected: impl Fn(DeviceId) -> Option<DeviceId>,
    preset_name: impl Fn(DeviceId) -> Option<String>,
) -> Vec<RowView> {
    let count = effects.len();
    let mut rows = vec![
        RowView {
            selected_branch: -1,
            ..RowView::default()
        };
        count
    ];
    // Which branch each layer shows, and so which rows are hidden. A layer
    // is visited before anything inside it, so a layer that is itself hidden
    // still hides its own unselected branches -- which changes nothing, since
    // its whole run is hidden already.
    let mut shown: Vec<Option<(usize, usize)>> = vec![None; count];
    for layer in 0..count {
        if effects[layer].params.container_flow() != Some(mooloop_core::ContainerFlow::Parallel) {
            continue;
        }
        let heads: Vec<usize> = mooloop_core::layer_branches(effects, layer).collect();
        let chosen = selected(effects[layer].id)
            .and_then(|id| heads.iter().copied().find(|head| effects[*head].id == id))
            .or_else(|| heads.first().copied());
        rows[layer].branches = heads
            .iter()
            .map(|head| BranchView {
                slot: *head,
                name: branch_name(effects, *head, &preset_name),
                controls: effects[*head].params.is_container(),
            })
            .collect();
        rows[layer].selected_branch = chosen.map_or(-1, |head| head as i32);
        for head in heads {
            let run = mooloop_core::run_of(effects, head);
            if Some(head) == chosen {
                for row in run {
                    // The innermost layer wins: a later (deeper) layer
                    // overwrites what an outer one said.
                    shown[row] = Some((layer, head));
                }
            } else {
                for row in run {
                    rows[row].hidden = true;
                }
            }
        }
    }
    let visible = |row: usize, rows: &[RowView]| !rows[row].hidden;
    // The bracket: each drawn row wears the bracket of its innermost layer,
    // when it is in the branch that layer shows. A row inside a deeper
    // layer's shown branch wears that layer's instead, so an outer bracket
    // stops where an inner one starts.
    let mut owner: Vec<Option<(usize, usize)>> = vec![None; count];
    for row in 0..count {
        let Some((layer, head)) = shown[row] else { continue };
        if !visible(row, &rows) {
            continue;
        }
        let innermost = (0..row).rev().find(|outer| {
            effects[*outer].params.container_flow() == Some(mooloop_core::ContainerFlow::Parallel)
                && mooloop_core::span_of(effects, *outer).contains(&row)
        });
        if innermost == Some(layer) {
            owner[row] = Some((layer, head));
        }
    }
    // Its ends are the first and last rows that wear it.
    for row in 0..count {
        let Some((layer, head)) = owner[row] else { continue };
        let run = mooloop_core::run_of(effects, head);
        let wears = |other: usize| owner[other].is_some_and(|(of, _)| of == layer);
        rows[row].bracket = true;
        rows[row].bracket_start = !(head..row).any(wears);
        rows[row].bracket_end = !(row + 1..run.end).any(wears);
    }
    // Where each drawn row's next drawn neighbour sits.
    for row in 0..count {
        rows[row].next_depth = (row + 1..count)
            .find(|later| visible(*later, &rows))
            .map_or(0, |later| mooloop_core::depth_at(effects, later) as i32);
    }
    // Where a device added from each drawn row's join lands: before the next
    // drawn row, which is at the depth the join is drawn at -- a join past a
    // box's last row is outside the box, and so is the row after it
    // (`mooloop_core::insert_effect`: landing on a run's end is landing after
    // it).
    for row in 0..count {
        rows[row].join_before = if effects[row].params.container_flow()
            == Some(mooloop_core::ContainerFlow::Parallel)
        {
            -1
        } else {
            (row + 1..count)
                .find(|later| visible(*later, &rows))
                .unwrap_or(count) as i32
        };
    }
    // Which boxes end at each drawn row: every drawn container closes at the
    // last drawn row of its span, or on its own row when none is drawn.
    for container in 0..count {
        if !effects[container].params.is_container() || !visible(container, &rows) {
            continue;
        }
        let span = mooloop_core::span_of(effects, container);
        let last = span.rev().find(|row| visible(*row, &rows)).unwrap_or(container);
        rows[last].closing.push(container as i32);
    }
    for row in &mut rows {
        // Innermost first is the greatest index first, because boxes nest.
        row.closing.sort_unstable_by(|a, b| b.cmp(a));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{EffectKind, EffectSlotState};

    /// `kinds` in order, with the containers' `children` given beside them,
    /// and each row's position as its identity.
    fn chain(rows: &[(EffectKind, u8)]) -> Vec<EffectSlotState> {
        rows.iter()
            .enumerate()
            .map(|(index, (kind, children))| {
                let mut effect = EffectSlotState::of_kind(*kind).with_id(DeviceId(index as u32));
                effect.params.set_container_children(*children);
                effect
            })
            .collect()
    }

    /// `[Layer, Chain, Drive, Bitcrush, Chain, Filter, Delay]`: a layer of
    /// two chain branches, and a device after it.
    fn two_branches() -> Vec<EffectSlotState> {
        chain(&[
            (EffectKind::Layer, 5),
            (EffectKind::Chain, 2),
            (EffectKind::Drive, 0),
            (EffectKind::Bitcrush, 0),
            (EffectKind::Chain, 1),
            (EffectKind::Filter, 0),
            (EffectKind::Delay, 0),
        ])
    }

    fn hidden(view: &[RowView]) -> Vec<bool> {
        view.iter().map(|row| row.hidden).collect()
    }

    #[test]
    fn a_layer_shows_its_first_branch_until_told_otherwise() {
        let effects = two_branches();
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, false, false, false, true, true, false]);
        assert_eq!(view[0].selected_branch, 1);
        assert_eq!(
            view[0].branches,
            [
                BranchView { slot: 1, name: "Drive".into(), controls: true },
                BranchView { slot: 4, name: "Filter".into(), controls: true },
            ]
        );
        // The first branch's chain closes at its last row, and the layer
        // closes there too, because the rest of its run is not drawn.
        assert_eq!(view[3].closing, [1, 0]);
        assert_eq!(view[3].next_depth, 0, "the Delay after the layer is next");
        assert!(view[1].bracket && view[1].bracket_start && !view[1].bracket_end);
        assert!(view[3].bracket && view[3].bracket_end);
        assert!(!view[6].bracket, "the device after the layer is under no bracket");
    }

    #[test]
    fn selecting_the_other_branch_swaps_what_is_drawn() {
        let effects = two_branches();
        let view = rack_view(
            &effects,
            |layer| (layer == DeviceId(0)).then_some(DeviceId(4)),
            |_| None,
        );
        assert_eq!(hidden(&view), [false, true, true, true, false, false, false]);
        assert_eq!(view[0].selected_branch, 4);
        // The layer's head is followed, as drawn, by the second branch.
        assert_eq!(view[0].next_depth, 1);
        assert_eq!(view[5].closing, [4, 0]);
        assert!(view[4].bracket_start && view[5].bracket_end);
    }

    /// A join adds before the next *drawn* row (MOO-218): past a branch a
    /// layer is hiding, and out of every box that ends where it is drawn. A
    /// layer's own join adds nothing -- a new branch is the layer face's.
    #[test]
    fn a_join_inserts_before_the_next_drawn_row() {
        let effects = two_branches();
        let view = rack_view(&effects, |_| None, |_| None);
        let joins: Vec<i32> = view.iter().map(|row| row.join_before).collect();
        assert_eq!(joins[0], -1, "the layer's own join");
        assert_eq!(joins[1], 2, "the shown branch's head leads into it");
        assert_eq!(joins[2], 3);
        assert_eq!(joins[3], 6, "past the hidden branch, and out of the layer");
        assert_eq!(joins[6], 7, "the last row's join appends");
    }

    #[test]
    fn a_selection_naming_a_branch_that_has_gone_shows_the_first() {
        let effects = two_branches();
        let view = rack_view(&effects, |_| Some(DeviceId(99)), |_| None);
        assert_eq!(view[0].selected_branch, 1);
    }

    #[test]
    fn an_empty_layer_closes_itself() {
        let effects = chain(&[(EffectKind::Layer, 0), (EffectKind::Filter, 0)]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(view[0].closing, [0]);
        assert_eq!(view[0].selected_branch, -1);
        assert!(view[0].branches.is_empty());
    }

    #[test]
    fn a_chain_with_no_layer_is_drawn_as_before() {
        // [Chain, Drive, Filter]: the pre-09 derivation, row for row.
        let effects = chain(&[
            (EffectKind::Chain, 1),
            (EffectKind::Drive, 0),
            (EffectKind::Filter, 0),
        ]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, false, false]);
        assert_eq!(view[1].closing, [0]);
        assert_eq!(
            view.iter().map(|row| row.next_depth).collect::<Vec<_>>(),
            [1, 0, 0]
        );
        assert!(view.iter().all(|row| !row.bracket));
    }

    /// A layer inside the shown branch of another: each row wears the
    /// bracket of its innermost layer, and the outer bracket ends where the
    /// inner one begins rather than running on under it.
    #[test]
    fn a_nested_layer_brackets_its_own_branch() {
        // [Layer, Chain, Layer, Chain, Drive, Chain, Filter, Chain, Delay]:
        // the outer layer's first branch holds an inner layer of two.
        let effects = chain(&[
            (EffectKind::Layer, 8),
            (EffectKind::Chain, 5),
            (EffectKind::Layer, 4),
            (EffectKind::Chain, 1),
            (EffectKind::Drive, 0),
            (EffectKind::Chain, 1),
            (EffectKind::Filter, 0),
            (EffectKind::Chain, 1),
            (EffectKind::Delay, 0),
        ]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(
            hidden(&view),
            [false, false, false, false, false, true, true, true, true]
        );
        // The outer bracket: the chain and the inner layer's head.
        assert!(view[1].bracket && view[1].bracket_start && !view[1].bracket_end);
        assert!(view[2].bracket && !view[2].bracket_start && view[2].bracket_end);
        // The inner one: its first branch.
        assert!(view[3].bracket && view[3].bracket_start && !view[3].bracket_end);
        assert!(view[4].bracket && view[4].bracket_end);
        // Every box that ends here ends at the Drive, innermost first.
        assert_eq!(view[4].closing, [3, 2, 1, 0]);
    }

    #[test]
    fn a_leaf_branch_is_named_after_itself_and_has_no_switches() {
        let effects = chain(&[
            (EffectKind::Layer, 2),
            (EffectKind::Drive, 0),
            (EffectKind::Chain, 0),
        ]);
        let view = rack_view(&effects, |_| None, |id| (id == DeviceId(2)).then(|| "Dry".to_string()));
        assert_eq!(
            view[0].branches,
            [
                BranchView { slot: 1, name: "Drive".into(), controls: false },
                BranchView { slot: 2, name: "Dry".into(), controls: true },
            ]
        );
    }
}
