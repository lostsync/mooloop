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
    /// The row is not drawn: it lies in a branch its layer is not showing, or
    /// it is the Chain heading the branch the layer *is* showing. A branch's
    /// Chain is the layer's own business, not a device of the rack: its Level
    /// is in the layer's list beside S, M and the meter, and its devices are
    /// drawn straight under the layer's bracket (MOO-456).
    pub hidden: bool,
    /// How many *drawn* boxes enclose the row. `mooloop_core::depth_at`
    /// counts every enclosing container, and a shown branch's hidden Chain is
    /// one of them: the box bands, rails and caps the rack draws are this
    /// number's, so a device in a layer's branch sits one box deep, not two.
    pub draw_depth: i32,
    /// `draw_depth` of the next row that is drawn, or 0 past the last: what a
    /// box asks to know whether it ends here.
    pub next_depth: i32,
    /// The row the join after this one adds a device before: the next drawn
    /// row, or the chain's length past the last. On a layer's own row that is
    /// the first device of the branch it shows, which lands inside the
    /// branch's Chain. -1 only where the next drawn row is a bare device
    /// directly in the layer, which a device added there would make a new
    /// branch -- the layer face's `+`, not the rack's (MOO-218).
    pub join_before: i32,
    /// The containers whose last *drawn* row this is, innermost first, as
    /// rack indices.
    pub closing: Vec<i32>,
    /// Beside `closing`, entry for entry: how many **append joins** are drawn
    /// up to and including the one before that box's output rail (MOO-299).
    ///
    /// A Chain whose last drawn row this is draws a join inside its box,
    /// after that row and before its rail, which adds a device at the end of
    /// the box (`Session::append_effect_into_container`). The join past the
    /// rails still adds after the box. A layer draws one when it is showing a
    /// Chain branch, and the rack wires it to that branch's Chain: the Chain
    /// is not drawn, so the layer's box holds the join its box would have
    /// (MOO-456). A layer showing a bare device draws none -- a device at a
    /// layer's end would be a new branch, the layer face's `+` -- and nor
    /// does a box closing on its own row, empty or showing an empty branch,
    /// whose join inside it is already there (`inner-join`).
    ///
    /// A running count rather than a flag per box, because the markup cannot
    /// sum a list and every rail after an append join moves along by one
    /// join's width: rail `t` sits `t` rails and `closing_joins[t]` joins
    /// past the row, box `t` draws an append join exactly when its count is
    /// one more than box `t - 1`'s (or than 0), and the last entry is how
    /// many joins the row's tail holds -- which is part of the row's pitch,
    /// and so of the drag's gap (`DeviceRackMetrics.join-width`). The rack
    /// reads it as `EffectSlotRow.closing-joins` (MOO-340).
    pub closing_joins: Vec<i32>,
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
    // The Chains heading a branch a layer is showing. Not drawn, and not
    // counted by `draw_depth`, nor closed by `closing` (MOO-456).
    let mut branch_head = vec![false; count];
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
                if effects[head].params.is_container() {
                    branch_head[head] = true;
                    rows[head].hidden = true;
                }
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
    // A folded container shows only its own strip (MOO-219): everything it
    // holds is hidden, whatever its layers were showing, and it draws no box
    // -- so it closes nothing, below.
    // A shown branch's Chain is never folded: nothing draws the strip that
    // would unfold it.
    for container in 0..count {
        if effects[container].collapsed
            && effects[container].params.is_container()
            && !branch_head[container]
        {
            for row in mooloop_core::span_of(effects, container) {
                rows[row].hidden = true;
            }
        }
    }
    // How many drawn boxes enclose each row: every enclosing container but
    // a shown branch's hidden Chain.
    let draw_depth = |row: usize| -> i32 {
        (0..row)
            .filter(|outer| {
                !branch_head[*outer] && mooloop_core::span_of(effects, *outer).contains(&row)
            })
            .count() as i32
    };
    for (row, view) in rows.iter_mut().enumerate() {
        view.draw_depth = draw_depth(row);
    }
    // Where each drawn row's next drawn neighbour sits.
    for row in 0..count {
        rows[row].next_depth = (row + 1..count)
            .find(|later| visible(*later, &rows))
            .map_or(0, |later| rows[later].draw_depth);
    }
    // Where a device added from each drawn row's join lands: before the next
    // drawn row, which is at the depth the join is drawn at -- a join past a
    // box's last row is outside the box, and so is the row after it
    // (`mooloop_core::insert_effect`: landing on a run's end is landing after
    // it).
    for row in 0..count {
        let next = (row + 1..count).find(|later| visible(*later, &rows));
        // A layer's join leads into the branch it shows: into its Chain
        // when that is a Chain, and that is the next drawn row's own place.
        // A bare device straight in the layer is not -- a device put there
        // would be a new branch.
        let into_bare_branch = effects[row].params.container_flow()
            == Some(mooloop_core::ContainerFlow::Parallel)
            && next.is_some_and(|next| mooloop_core::parent_of(effects, next) == Some(row));
        rows[row].join_before = if into_bare_branch {
            -1
        } else {
            next.unwrap_or(count) as i32
        };
    }
    // Which boxes end at each drawn row: every drawn container closes at the
    // last drawn row of its span, or on its own row when none is drawn.
    for container in 0..count {
        if !effects[container].params.is_container()
            || effects[container].collapsed
            || !visible(container, &rows)
        {
            continue;
        }
        let span = mooloop_core::span_of(effects, container);
        let last = span.rev().find(|row| visible(*row, &rows)).unwrap_or(container);
        rows[last].closing.push(container as i32);
    }
    // Whether a layer is showing a Chain branch, which its box then appends
    // into; read before the loop below borrows `rows` mutably.
    let shows_chain: Vec<bool> = rows
        .iter()
        .map(|row| {
            usize::try_from(row.selected_branch)
                .is_ok_and(|head| effects[head].params.is_container())
        })
        .collect();
    for (index, row) in rows.iter_mut().enumerate() {
        // Innermost first is the greatest index first, because boxes nest.
        row.closing.sort_unstable_by(|a, b| b.cmp(a));
        let mut joins = 0;
        row.closing_joins = row
            .closing
            .iter()
            .map(|&container| {
                let container = container as usize;
                let appends = match effects[container].params.container_flow() {
                    Some(mooloop_core::ContainerFlow::Series) => true,
                    Some(mooloop_core::ContainerFlow::Parallel) => shows_chain[container],
                    None => false,
                };
                if container != index && appends {
                    joins += 1;
                }
                joins
            })
            .collect();
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
        // The shown branch's Chain (row 1) is not drawn either: its devices
        // sit straight under the layer (MOO-456).
        assert_eq!(hidden(&view), [false, true, false, false, true, true, false]);
        assert_eq!(view[0].selected_branch, 1);
        assert_eq!(
            view[0].branches,
            [
                BranchView { slot: 1, name: "Drive".into(), controls: true },
                BranchView { slot: 4, name: "Filter".into(), controls: true },
            ]
        );
        // The layer closes at the shown branch's last device, because the
        // rest of its run is not drawn; the hidden Chain closes nothing.
        assert_eq!(view[3].closing, [0]);
        assert_eq!(view[3].next_depth, 0, "the Delay after the layer is next");
        assert!(!view[1].bracket, "the hidden Chain wears no bracket");
        assert!(view[2].bracket && view[2].bracket_start && !view[2].bracket_end);
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
        assert_eq!(hidden(&view), [false, true, true, true, true, false, false]);
        assert_eq!(view[0].selected_branch, 4);
        // The layer's head is followed, as drawn, by the second branch's
        // first device, one box deep.
        assert_eq!(view[0].next_depth, 1);
        assert_eq!(view[5].closing, [0]);
        assert!(view[5].bracket_start && view[5].bracket_end);
    }

    /// A join adds before the next *drawn* row (MOO-218): past a branch a
    /// layer is hiding, and out of every box that ends where it is drawn. A
    /// layer's own join adds nothing -- a new branch is the layer face's.
    #[test]
    fn a_join_inserts_before_the_next_drawn_row() {
        let effects = two_branches();
        let view = rack_view(&effects, |_| None, |_| None);
        let joins: Vec<i32> = view.iter().map(|row| row.join_before).collect();
        assert_eq!(joins[0], 2, "the layer's own join leads into its shown branch");
        assert_eq!(joins[2], 3, "between two devices of the branch");
        assert_eq!(joins[3], 6, "past the hidden branch, and out of the layer");
        assert_eq!(joins[6], 7, "the last row's join appends");
    }

    /// A folded container hides everything it holds and draws no box, and
    /// the join past it leads to the row after its whole run (MOO-219).
    #[test]
    fn a_folded_container_hides_its_run() {
        let mut effects = chain(&[
            (EffectKind::Chain, 2),
            (EffectKind::Drive, 0),
            (EffectKind::Filter, 0),
            (EffectKind::Delay, 0),
        ]);
        effects[0].collapsed = true;
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, true, true, false]);
        assert!(view.iter().all(|row| row.closing.is_empty()), "no box is drawn");
        assert_eq!(view[0].join_before, 3);
        assert_eq!(view[0].next_depth, 0);

        // A folded leaf is just a narrower row: nothing else changes.
        let mut effects = chain(&[(EffectKind::Drive, 0), (EffectKind::Filter, 0)]);
        effects[0].collapsed = true;
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, false]);
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
            [false, true, false, true, false, true, true, true, true]
        );
        // The outer bracket: the inner layer's head, the branch's only
        // drawn row (its Chain is not drawn).
        assert!(view[2].bracket && view[2].bracket_start && view[2].bracket_end);
        // The inner one: the Drive.
        assert!(view[4].bracket && view[4].bracket_start && view[4].bracket_end);
        // Every drawn box that ends here ends at the Drive, innermost first.
        assert_eq!(view[4].closing, [2, 0]);
        assert_eq!(view[4].draw_depth, 2, "two layers, and neither hidden Chain");
        assert_eq!(view[4].closing_joins, [1, 2], "each layer appends to its branch's Chain");
    }

    /// **MOO-299.** A Chain holding devices draws an append join after its
    /// last one, before its rail; the join past the rail still leads out.
    /// Nested Chains draw one each, innermost first, and each rail sits past
    /// the joins before it.
    #[test]
    fn a_chain_draws_an_append_join_before_its_rail() {
        // [Chain, Drive, Filter], Delay
        let effects = chain(&[
            (EffectKind::Chain, 2),
            (EffectKind::Drive, 0),
            (EffectKind::Filter, 0),
            (EffectKind::Delay, 0),
        ]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(view[2].closing, [0]);
        assert_eq!(view[2].closing_joins, [1], "the Filter's tail holds the Chain's append join");
        assert_eq!(view[2].join_before, 3, "and the join past the rail still leads out");
        assert!(
            [0, 1, 3].iter().all(|row| view[*row].closing_joins.is_empty()),
            "nothing else closes a box"
        );

        // [Outer, [Inner, Drive]]: both end at the Drive, and both append.
        let effects = chain(&[
            (EffectKind::Chain, 2),
            (EffectKind::Chain, 1),
            (EffectKind::Drive, 0),
        ]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(view[2].closing, [1, 0]);
        assert_eq!(view[2].closing_joins, [1, 2]);
    }

    /// An empty Chain's join inside it is its own (`inner-join`), so it draws
    /// no append join as well. A layer showing a Chain branch draws one, which
    /// appends to that Chain, since the Chain is not drawn to have its own
    /// (MOO-456); a layer showing a bare device draws none, because its end
    /// would be a new branch.
    #[test]
    fn empty_chains_and_layers_draw_no_append_join() {
        let effects = chain(&[(EffectKind::Chain, 0), (EffectKind::Drive, 0)]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(view[0].closing, [0]);
        assert_eq!(view[0].closing_joins, [0]);

        // [Layer, [Chain, Drive, Bitcrush], [Chain, Filter]], Delay: the
        // first branch is shown, and the layer ends at the Bitcrush.
        let view = rack_view(&two_branches(), |_| None, |_| None);
        assert_eq!(view[3].closing, [0]);
        assert_eq!(view[3].closing_joins, [1], "the layer appends to the branch's Chain");

        // A leaf branch closes only its layer.
        let effects = chain(&[(EffectKind::Layer, 1), (EffectKind::Drive, 0)]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(view[1].closing, [0]);
        assert_eq!(view[1].closing_joins, [0]);
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

    /// **MOO-456.** A branch's Chain is not drawn, and the boxes that are
    /// drawn are counted without it.
    #[test]
    fn a_shown_branch_chain_is_not_drawn_or_counted() {
        let view = rack_view(&two_branches(), |_| None, |_| None);
        assert!(view[1].hidden, "the shown branch's Chain is not a row of the rack");
        // The drawn rows only: the Delay, and the Drive and Bitcrush.
        assert_eq!(
            [view[2].draw_depth, view[3].draw_depth, view[6].draw_depth],
            [1, 1, 0],
            "a device in a branch is one box deep: the layer's"
        );
        assert_eq!(view[3].join_before, 6, "the join past the layer leads out of it");
        assert_eq!(view[3].closing_joins, [1], "the join before the layer's rail appends to the branch");
        // A Chain that is not a layer's branch is drawn as before.
        let effects = chain(&[(EffectKind::Chain, 1), (EffectKind::Drive, 0)]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert!(!view[0].hidden);
        assert_eq!(view[1].draw_depth, 1);
    }

    /// A branch's Chain that was folded when it was wrapped is shown open:
    /// nothing would draw the strip that unfolds it.
    #[test]
    fn a_folded_branch_chain_still_shows_its_devices() {
        let mut effects = two_branches();
        effects[1].collapsed = true;
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, true, false, false, true, true, false]);
        assert_eq!(view[3].closing, [0]);
    }

    /// A branch just made with `+` holds nothing, and its Chain is not drawn,
    /// so the layer closes on its own row: the join inside it is where the
    /// first device goes (`inner-join`, wired to the branch), and it draws no
    /// append join besides.
    #[test]
    fn an_empty_branch_closes_the_layer_on_its_own_row() {
        let effects = chain(&[
            (EffectKind::Layer, 1),
            (EffectKind::Chain, 0),
            (EffectKind::Filter, 0),
        ]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, true, false]);
        assert_eq!(view[0].selected_branch, 1);
        assert_eq!(view[0].closing, [0]);
        assert_eq!(view[0].closing_joins, [0]);
        assert_eq!(view[0].next_depth, 0);
        assert_eq!(view[0].join_before, 2, "past the layer's rail, out of it");
        assert!(view.iter().all(|row| !row.bracket), "nothing to bracket");
    }

    /// A bare device straight in a layer (a song the open could not wrap) is
    /// drawn as before: its layer's join leads to a new branch, and adds none.
    #[test]
    fn a_bare_branch_is_drawn_as_before() {
        let effects = chain(&[(EffectKind::Layer, 1), (EffectKind::Drive, 0)]);
        let view = rack_view(&effects, |_| None, |_| None);
        assert_eq!(hidden(&view), [false, false]);
        assert_eq!(view[0].join_before, -1);
        assert_eq!(view[1].draw_depth, 1);
        assert_eq!(view[1].closing, [0]);
        assert_eq!(view[1].closing_joins, [0]);
        assert!(view[1].bracket && view[1].bracket_start && view[1].bracket_end);
    }
}
