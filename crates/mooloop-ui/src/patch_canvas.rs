//! The song patch canvas (`docs/plans/song-patch/03-the-canvas.md`).
//!
//! The Modulation pane draws the song's patch: boxes where the song put
//! them, tags at the patch's edges, an assignment tag for every route from a
//! box, and a wire for each. Slint draws what this module lays out and hit
//! tests nothing: one `TouchArea` covers the canvas and reports press, move
//! and release in canvas units, and [`CanvasState`] decides what was hit and
//! what the gesture means, as the piano roll does. A per-item touch area
//! would move out from under the pointer as the drag moves its item.
//!
//! Everything here but [`wire`] and [`UiState::publish_patch_canvas`] is
//! free of Slint, so the geometry and the gestures are tested as plain data.

use crate::{
    project_snapshot, record_project_history, with_gesture_history, CommandState, MainWindow,
    UiState,
};
use mooloop_core::{
    Bend, BendAxis, CanvasPoint, InletSource, Jack, JackSort, ModSourceId, ModSourceRef,
    ModulatorParams, ParamAddr, Port, SongModulation, TagKind,
};
use mooloop_session::modulation::{PatchFeed, PatchRefusal};
use mooloop_session::session::Session;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// A box's height, and a tag's.
pub(crate) const BOX_HEIGHT: f32 = 28.0;
pub(crate) const TAG_HEIGHT: f32 = 26.0;
/// A box's jack is a bar this wide on its edge.
pub(crate) const JACK_WIDTH: f32 = 12.0;
/// An open box's face (song patch step 05): one cell per knob, the room
/// round the cells, and what a face with no knobs takes to say so.
pub(crate) const CELL_WIDTH: f32 = 46.0;
pub(crate) const CELL_HEIGHT: f32 = 52.0;
const FACE_PAD: f32 = 8.0;
const EMPTY_FACE: f32 = 24.0;
/// The fold arrow at the right of a box's spelling, which opens its face.
pub(crate) const FOLD_WIDTH: f32 = 18.0;
/// How far from a jack a press still takes it.
const JACK_REACH: f32 = 8.0;
/// How far from a wire a press still selects it.
const WIRE_REACH: f32 = 5.0;
/// How many pump ticks a note wire stays thick for a note: about 120 ms.
const FLASH_TICKS: i32 = 15;
/// How far the pointer travels before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 4.0;
/// The canvas is at least this big, and this much bigger than what is on it.
const MIN_EXTENT: (f32, f32) = (1200.0, 600.0);
const MARGIN: f32 = 240.0;
/// The rounded corner of a wire's bend, and how far a wire runs straight out
/// of a jack before it turns.
const CORNER: f32 = 7.0;
const LEAD: f32 = 16.0;
/// Text widths, estimated: Slint cannot measure for Rust, and a jack's place
/// must be known here. The mono face at `Theme.text-2xl` and the sans face
/// at `text-xl` and `text-2xl`, with room to spare.
const MONO_CHAR: f32 = 9.0;
const SMALL_CHAR: f32 = 7.2;
const LABEL_CHAR: f32 = 8.4;
const BADGE_CHAR: f32 = 7.0;

/// Something on the canvas that can be selected and moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NodeKey {
    /// A box or a tag.
    Node(ModSourceId),
    /// A route's assignment tag, by its source and destination: a song has
    /// one route for each pair.
    Route {
        source: ModSourceRef,
        destination: ParamAddr,
    },
}

/// A wire, by what it ends in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WireKey {
    /// A patch wire, by its inlet: an inlet takes one.
    Patch(Jack),
    /// The wire from a box to one of its assignment tags.
    Route {
        source: ModSourceRef,
        destination: ParamAddr,
    },
}

/// Which way a jack faces, which is the way its wire leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Facing {
    Up,
    Down,
    Left,
    Right,
}

/// Where a wire meets a jack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Anchor {
    pub x: f32,
    pub y: f32,
    pub facing: Facing,
    /// The node's left and right edges, which a wire routing round it keeps
    /// clear of.
    pub left: f32,
    pub right: f32,
}

/// What a node looks like.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Face {
    /// A box: its spelling in the mono face, and its face's knobs when it
    /// is open.
    Box {
        spelling: String,
        unknown: bool,
        open: bool,
        knobs: Vec<FaceKnob>,
    },
    /// A tag: an arrow pointing right (a source, its outlet at the point) or
    /// left (a sink, its inlet at the point).
    Tag {
        points_right: bool,
        kind: String,
        name: String,
        depth: String,
        note: bool,
        unbound: bool,
    },
}

/// One knob on an open box's face: the setting it turns, and the top-left
/// of its cell on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FaceKnob {
    pub param: u32,
    pub x: f32,
    pub y: f32,
}

/// One node, laid out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Node {
    pub key: NodeKey,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub face: Face,
    pub inlets: Vec<Port>,
    pub outlets: Vec<Port>,
}

impl Node {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.y && y <= self.y + self.height
    }

    fn is_box(&self) -> bool {
        matches!(self.face, Face::Box { .. })
    }

    /// Whether `(x, y)` is on a box's fold arrow.
    fn on_fold(&self, x: f32, y: f32) -> bool {
        self.is_box()
            && x >= self.x + self.width - FOLD_WIDTH - 2.0
            && x <= self.x + self.width
            && y >= self.y
            && y <= self.y + BOX_HEIGHT
    }

    /// Where inlet `port` (or outlet, with `outlet`) meets its wire.
    pub(crate) fn anchor(&self, outlet: bool, port: usize) -> Option<Anchor> {
        let count = if outlet {
            self.outlets.len()
        } else {
            self.inlets.len()
        };
        if port >= count {
            return None;
        }
        let (left, right) = (self.x, self.x + self.width);
        let mid = self.y + self.height / 2.0;
        let (x, y, facing) = match (&self.face, outlet) {
            (
                Face::Tag {
                    points_right: true, ..
                },
                _,
            ) => (right, mid, Facing::Right),
            (Face::Tag { .. }, _) => (left, mid, Facing::Left),
            (Face::Box { .. }, _) => {
                let spread = if count > 1 {
                    (self.width - JACK_WIDTH) * port as f32 / (count - 1) as f32
                } else {
                    0.0
                };
                let x = self.x + JACK_WIDTH / 2.0 + spread;
                if outlet {
                    (x, self.y + self.height, Facing::Down)
                } else {
                    (x, self.y, Facing::Up)
                }
            }
        };
        Some(Anchor {
            x,
            y,
            facing,
            left,
            right,
        })
    }
}

/// One wire, laid out.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WireLine {
    pub key: WireKey,
    pub points: Vec<(f32, f32)>,
    pub note: bool,
    /// It reads its outlet a tick late: saved so, or a loop's break.
    pub delayed: bool,
    /// The box or tag it leaves.
    pub source: ModSourceId,
}

/// The whole canvas, laid out.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Layout {
    pub nodes: Vec<Node>,
    pub wires: Vec<WireLine>,
    pub width: f32,
    pub height: f32,
}

/// What a press landed on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Hit {
    Outlet(Jack),
    Inlet(Jack),
    /// The inlet of a route's assignment tag, which nothing wires by hand.
    RouteInlet(NodeKey),
    /// A box's fold arrow.
    Fold(ModSourceId),
    Node(NodeKey),
    Wire(WireKey),
    Empty,
}

/// What a route's tag says: its parameter, its device, and its depth.
pub(crate) struct RouteLabel {
    pub name: String,
    pub device: String,
}

/// How a box reads on the canvas: [`mooloop_core::box_text::spell`].
pub(crate) fn spelling(params: &ModulatorParams, text: &str) -> String {
    mooloop_core::box_text::spell(params, text)
}

fn text_width(text: &str, char_width: f32) -> f32 {
    text.chars().count() as f32 * char_width
}

/// A tag's width for what it says: padding, the arrow's point, each piece
/// and the gaps between them, as `PatchTag` lays them out.
fn tag_width(kind: &str, name: &str, depth: &str, unbound: bool) -> f32 {
    let mut width = 10.0 + 22.0;
    let mut pieces = 0;
    for (text, char_width) in [(kind, SMALL_CHAR), (name, LABEL_CHAR)] {
        if !text.is_empty() {
            width += text_width(text, char_width);
            pieces += 1;
        }
    }
    if unbound {
        width += 26.0;
        pieces += 1;
    }
    if !depth.is_empty() {
        width += text_width(depth, BADGE_CHAR) + 8.0;
        pieces += 1;
    }
    width + 7.0 * (pieces - 1).max(0) as f32
}

/// Lay out `song`'s patch. `channel_name` names a channel by id,
/// `inlet_kind` says what an inlet tag's source reads, `label_of`
/// names a route's destination, and `canvas` is the gesture under way: the
/// nodes a drag holds are moved by how far it has gone and a bend being
/// dragged is where the pointer has it, so a drag is drawn before it is a
/// song edit.
pub(crate) fn lay_out(
    song: &SongModulation,
    channel_name: impl Fn(mooloop_core::ChannelId) -> Option<String>,
    inlet_kind: impl Fn(InletSource) -> Option<String>,
    label_of: impl Fn(ParamAddr) -> RouteLabel,
    delayed: impl Fn(Jack) -> bool,
    canvas: &CanvasState,
) -> Layout {
    let offset = |key| canvas.offset(key);
    let mut nodes = Vec::new();
    for module in &song.modules {
        let ports = module.params.ports();
        let spelling = spelling(&module.params, &module.text);
        let unknown = module.params == ModulatorParams::Unknown;
        let jacks = ports.inlets.len().max(ports.outlets.len()) as f32;
        let key = NodeKey::Node(module.id);
        let (dx, dy) = offset(key);
        let (x, y) = (module.at.x as f32 + dx, module.at.y as f32 + dy);
        let params = if module.open {
            crate::patch_face::face_params(&module.params)
        } else {
            Vec::new()
        };
        let columns = match params.len() {
            count @ 0..=4 => count,
            5..=12 => 4,
            _ => 8,
        };
        let rows = params.len().div_ceil(columns.max(1));
        let width = (26.0 + text_width(&spelling, MONO_CHAR) + FOLD_WIDTH)
            .max(64.0)
            .max(jacks * 24.0)
            .max(columns as f32 * CELL_WIDTH + 2.0 * FACE_PAD);
        let height = match (module.open, rows) {
            (false, _) => BOX_HEIGHT,
            (true, 0) => BOX_HEIGHT + EMPTY_FACE,
            (true, rows) => BOX_HEIGHT + rows as f32 * CELL_HEIGHT + FACE_PAD,
        };
        let knobs = params
            .into_iter()
            .enumerate()
            .map(|(index, param)| FaceKnob {
                param,
                x: x + FACE_PAD + (index % columns) as f32 * CELL_WIDTH,
                y: y + BOX_HEIGHT + (index / columns) as f32 * CELL_HEIGHT,
            })
            .collect();
        nodes.push(Node {
            key,
            x,
            y,
            width,
            height,
            face: Face::Box {
                spelling,
                unknown,
                open: module.open,
                knobs,
            },
            inlets: ports.inlets.to_vec(),
            outlets: ports.outlets.to_vec(),
        });
    }
    for tag in &song.tags {
        let channel = tag.kind.channel().and_then(&channel_name);
        let (points_right, kind, note) = match tag.kind {
            TagKind::Inlet { bind } => (
                true,
                bind.and_then(&inlet_kind).unwrap_or_else(|| "inlet".to_string()),
                false,
            ),
            TagKind::NotesIn { .. } => (true, "notes".to_string(), true),
            TagKind::NotesOut { .. } => (false, "notes to".to_string(), true),
        };
        // A transport tag reads the song, not a channel.
        let transport = matches!(
            tag.kind,
            TagKind::Inlet { bind: Some(source) } if source.channel().is_none()
        );
        let unbound = channel.is_none() && !transport;
        let name = if transport {
            "song".to_string()
        } else {
            channel.unwrap_or_default()
        };
        // An outlet reaches the patch a block late, and its tag says so.
        let depth = match tag.kind {
            TagKind::Inlet { bind: Some(source) } if source.late() && !unbound => "late".to_string(),
            _ => String::new(),
        };
        let key = NodeKey::Node(tag.id);
        let (dx, dy) = offset(key);
        let ports = tag.kind.ports();
        nodes.push(Node {
            key,
            x: tag.at.x as f32 + dx,
            y: tag.at.y as f32 + dy,
            width: tag_width(&kind, &name, &depth, unbound),
            height: TAG_HEIGHT,
            face: Face::Tag {
                points_right,
                kind,
                name,
                depth,
                note,
                unbound,
            },
            inlets: ports.inlets.to_vec(),
            outlets: ports.outlets.to_vec(),
        });
    }

    // An assignment tag for every route from a box or a tag, where it was
    // put, or stacked under its box or beside its tag.
    let mut stacked: Vec<(ModSourceId, usize)> = Vec::new();
    let mut route_wires = Vec::new();
    for route in &song.routes {
        let ModSourceRef::Id(id) = route.source else {
            continue;
        };
        let from_box = song.module(id).map(|module| module.at);
        let from_tag = song.tag(id).map(|tag| tag.at);
        let Some(source_at) = from_box.or(from_tag) else {
            continue;
        };
        let source_node = nodes.iter().find(|node| node.key == NodeKey::Node(id));
        let below_box = source_node.map_or(BOX_HEIGHT, |node| node.height);
        let source_width = source_node.map_or(0.0, |node| node.width);
        let key = NodeKey::Route {
            source: route.source,
            destination: route.destination,
        };
        let at = song
            .route_at(route.source, route.destination)
            .unwrap_or_else(|| {
                let below = match stacked.iter_mut().find(|(source, _)| *source == id) {
                    Some((_, count)) => {
                        *count += 1;
                        *count
                    }
                    None => {
                        stacked.push((id, 0));
                        0
                    }
                };
                if from_box.is_some() {
                    CanvasPoint::new(
                        source_at.x + 16,
                        source_at.y + below_box as i32 + 40 + below as i32 * 34,
                    )
                } else {
                    CanvasPoint::new(
                        source_at.x + source_width as i32 + 48,
                        source_at.y + below as i32 * 34,
                    )
                }
            });
        let label = label_of(route.destination);
        let percent = (route.depth * 100.0).round() as i32;
        let depth = if percent > 0 {
            format!("+{percent}%")
        } else {
            format!("{percent}%")
        };
        let (dx, dy) = offset(key);
        nodes.push(Node {
            key,
            x: at.x as f32 + dx,
            y: at.y as f32 + dy,
            width: tag_width(&label.device, &label.name, &depth, false),
            height: TAG_HEIGHT,
            face: Face::Tag {
                points_right: false,
                kind: label.device,
                name: label.name,
                depth,
                note: false,
                unbound: false,
            },
            inlets: vec![Port {
                name: "in",
                sort: JackSort::Control,
            }],
            outlets: Vec::new(),
        });
        route_wires.push((Jack::new(id, 0), key));
    }

    let find = |key: NodeKey| nodes.iter().find(|node| node.key == key);
    let mut wires = Vec::new();
    for wire in &song.wires {
        let (Some(from), Some(to)) = (
            find(NodeKey::Node(wire.from.node)),
            find(NodeKey::Node(wire.to.node)),
        ) else {
            continue;
        };
        let (Some(start), Some(end)) = (
            from.anchor(true, usize::from(wire.from.port)),
            to.anchor(false, usize::from(wire.to.port)),
        ) else {
            continue;
        };
        let note = from
            .outlets
            .get(usize::from(wire.from.port))
            .is_some_and(|port| port.sort == JackSort::Note);
        wires.push(WireLine {
            key: WireKey::Patch(wire.to),
            points: match canvas.bend_of(wire.to, wire.bend) {
                Some(bend) => bent_points(start, end, bend),
                None => route_points(start, end),
            },
            note,
            delayed: wire.late || delayed(wire.to),
            source: wire.from.node,
        });
    }
    for (from_jack, key) in route_wires {
        let (Some(from), Some(to)) = (find(NodeKey::Node(from_jack.node)), find(key)) else {
            continue;
        };
        let (Some(start), Some(end)) = (from.anchor(true, 0), to.anchor(false, 0)) else {
            continue;
        };
        let NodeKey::Route {
            source,
            destination,
        } = key
        else {
            continue;
        };
        wires.push(WireLine {
            key: WireKey::Route {
                source,
                destination,
            },
            points: route_points(start, end),
            note: false,
            delayed: false,
            source: from_jack.node,
        });
    }

    let (mut width, mut height) = MIN_EXTENT;
    for node in &nodes {
        width = width.max(node.x + node.width + MARGIN);
        height = height.max(node.y + node.height + MARGIN);
    }
    Layout {
        nodes,
        wires,
        width,
        height,
    }
}

/// The corners of a wire from `start` to `end`: orthogonal, leaving and
/// entering each jack the way it faces, and round a node rather than
/// through it when the target sits behind the source (the prototype's
/// `route`).
pub(crate) fn route_points(start: Anchor, end: Anchor) -> Vec<(f32, f32)> {
    let (s, t) = (start, end);
    let g = LEAD;
    match (s.facing, t.facing) {
        (Facing::Right, Facing::Up) => {
            if t.x > s.x + g && t.y > s.y + g {
                vec![(s.x, s.y), (t.x, s.y), (t.x, t.y)]
            } else {
                vec![
                    (s.x, s.y),
                    (s.x + g, s.y),
                    (s.x + g, t.y - g),
                    (t.x, t.y - g),
                    (t.x, t.y),
                ]
            }
        }
        (Facing::Down, Facing::Left) => {
            if t.y > s.y + g && t.x > s.x + g {
                vec![(s.x, s.y), (s.x, t.y), (t.x, t.y)]
            } else {
                vec![
                    (s.x, s.y),
                    (s.x, s.y + g),
                    (t.x - g, s.y + g),
                    (t.x - g, t.y),
                    (t.x, t.y),
                ]
            }
        }
        (Facing::Right, Facing::Left) => {
            if t.x > s.x + 2.0 * g {
                let xm = (s.x + t.x) / 2.0;
                vec![(s.x, s.y), (xm, s.y), (xm, t.y), (t.x, t.y)]
            } else {
                let ym = (s.y + t.y) / 2.0;
                vec![
                    (s.x, s.y),
                    (s.x + g, s.y),
                    (s.x + g, ym),
                    (t.x - g, ym),
                    (t.x - g, t.y),
                    (t.x, t.y),
                ]
            }
        }
        _ => {
            if t.y > s.y + 2.0 * g {
                let ym = s.y + ((t.y - s.y) / 2.0).min(44.0);
                vec![(s.x, s.y), (s.x, ym), (t.x, ym), (t.x, t.y)]
            } else {
                let xm = if t.left > s.right {
                    (s.right + t.left) / 2.0
                } else if t.right < s.left {
                    (t.right + s.left) / 2.0
                } else {
                    s.right.max(t.right) + g
                };
                vec![
                    (s.x, s.y),
                    (s.x, s.y + g),
                    (xm, s.y + g),
                    (xm, t.y - g),
                    (t.x, t.y - g),
                    (t.x, t.y),
                ]
            }
        }
    }
}

/// A point `LEAD` out of a jack, the way its wire leaves (`out`) or comes
/// in.
fn lead(anchor: Anchor, out: bool) -> (f32, f32) {
    let g = if out { LEAD } else { -LEAD };
    match anchor.facing {
        Facing::Right | Facing::Left => (anchor.x + g, anchor.y),
        Facing::Down | Facing::Up => (anchor.x, anchor.y + g),
    }
}

/// The corners of a wire whose bend was dragged by hand: straight out of
/// its outlet, across to the bend's line, along it, and into its inlet. The
/// bend is a horizontal run at `at` on the y axis or a vertical one at `at`
/// on the x axis.
pub(crate) fn bent_points(start: Anchor, end: Anchor, bend: Bend) -> Vec<(f32, f32)> {
    let (s, t) = ((start.x, start.y), (end.x, end.y));
    let (a, b) = (lead(start, true), lead(end, false));
    let at = bend.at as f32;
    match bend.axis {
        BendAxis::Horizontal => vec![s, a, (a.0, at), (b.0, at), b, t],
        BendAxis::Vertical => vec![s, a, (at, a.1), (at, b.1), b, t],
    }
}

/// `points` as SVG path commands, each corner rounded (the prototype's
/// `rpath`). Points that repeat or sit on a straight run are dropped first.
pub(crate) fn path_commands(points: &[(f32, f32)]) -> String {
    let mut kept: Vec<(f32, f32)> = Vec::new();
    for &point in points {
        if kept
            .last()
            .is_none_or(|last| (last.0 - point.0).abs() + (last.1 - point.1).abs() > 0.5)
        {
            kept.push(point);
        }
    }
    let mut corners: Vec<(f32, f32)> = Vec::new();
    for (index, &point) in kept.iter().enumerate() {
        if index == 0 || index == kept.len() - 1 {
            corners.push(point);
            continue;
        }
        let a = *corners.last().expect("the first point is kept");
        let c = kept[index + 1];
        let turn = (point.0 - a.0) * (c.1 - point.1) - (point.1 - a.1) * (c.0 - point.0);
        if turn.abs() > 0.01 {
            corners.push(point);
        }
    }
    let Some(&(x, y)) = corners.first() else {
        return String::new();
    };
    let mut d = format!("M {x} {y}");
    for index in 1..corners.len() {
        let b = corners[index];
        if index == corners.len() - 1 {
            d.push_str(&format!(" L {} {}", b.0, b.1));
            break;
        }
        let (a, c) = (corners[index - 1], corners[index + 1]);
        let l1 = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let l2 = ((c.0 - b.0).powi(2) + (c.1 - b.1).powi(2)).sqrt();
        let r = CORNER.min(l1 / 2.0).min(l2 / 2.0);
        let into = (b.0 - (b.0 - a.0) / l1 * r, b.1 - (b.1 - a.1) / l1 * r);
        let out = (b.0 + (c.0 - b.0) / l2 * r, b.1 + (c.1 - b.1) / l2 * r);
        d.push_str(&format!(
            " L {} {} Q {} {} {} {}",
            into.0, into.1, b.0, b.1, out.0, out.1
        ));
    }
    d
}

/// How far `(x, y)` is from the segment `a`-`b`.
fn segment_distance((x, y): (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((x - a.0) * dx + (y - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = (a.0 + t * dx, a.1 + t * dy);
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

impl Layout {
    pub(crate) fn node(&self, key: NodeKey) -> Option<&Node> {
        self.nodes.iter().find(|node| node.key == key)
    }

    /// What is at `(x, y)`: a jack first, since it sits on its node's edge,
    /// then the topmost node, then a wire.
    pub(crate) fn hit(&self, x: f32, y: f32) -> Hit {
        for node in self.nodes.iter().rev() {
            for outlet in [true, false] {
                let count = if outlet {
                    node.outlets.len()
                } else {
                    node.inlets.len()
                };
                for port in 0..count {
                    let Some(anchor) = node.anchor(outlet, port) else {
                        continue;
                    };
                    if (anchor.x - x).abs() > JACK_REACH || (anchor.y - y).abs() > JACK_REACH {
                        continue;
                    }
                    return match (node.key, outlet) {
                        (NodeKey::Node(id), true) => Hit::Outlet(Jack::new(id, port as u8)),
                        (NodeKey::Node(id), false) => Hit::Inlet(Jack::new(id, port as u8)),
                        (key, _) => Hit::RouteInlet(key),
                    };
                }
            }
        }
        if let Some(node) = self.nodes.iter().rev().find(|node| node.contains(x, y)) {
            return match node.key {
                NodeKey::Node(id) if node.on_fold(x, y) => Hit::Fold(id),
                key => Hit::Node(key),
            };
        }
        for wire in self.wires.iter().rev() {
            if wire
                .points
                .windows(2)
                .any(|pair| segment_distance((x, y), pair[0], pair[1]) <= WIRE_REACH)
            {
                return Hit::Wire(wire.key);
            }
        }
        Hit::Empty
    }

    /// The segment of wire `key` a press at `point` is on, if it is on one.
    pub(crate) fn segment_at(
        &self,
        key: WireKey,
        point: (f32, f32),
    ) -> Option<((f32, f32), (f32, f32))> {
        let wire = self.wires.iter().find(|wire| wire.key == key)?;
        wire.points
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .filter(|(a, b)| (a.0 - b.0).abs() + (a.1 - b.1).abs() > 0.5)
            .map(|(a, b)| (segment_distance(point, a, b), (a, b)))
            .filter(|(distance, _)| *distance <= WIRE_REACH)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, segment)| segment)
    }

    /// The nodes a marquee from `a` to `b` touches.
    pub(crate) fn within(&self, a: (f32, f32), b: (f32, f32)) -> Vec<NodeKey> {
        let (left, right) = (a.0.min(b.0), a.0.max(b.0));
        let (top, bottom) = (a.1.min(b.1), a.1.max(b.1));
        self.nodes
            .iter()
            .filter(|node| {
                node.x <= right
                    && node.x + node.width >= left
                    && node.y <= bottom
                    && node.y + node.height >= top
            })
            .map(|node| node.key)
            .collect()
    }
}

/// A wire being drawn: its points, and the inlet under the pointer with
/// whether the song would take the wire there.
pub(crate) type LooseWire = (Vec<(f32, f32)>, Option<(Jack, bool)>);

/// A gesture in progress.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Drag {
    #[default]
    None,
    /// A press that has not travelled far enough to be a drag: what it
    /// means is decided on release, or once it moves.
    Pending {
        hit: Hit,
        from: (f32, f32),
        shift: bool,
    },
    /// Moving the selection by `by`.
    Move { from: (f32, f32), by: (f32, f32) },
    /// A wire from `from` to the pointer, lifted off inlet `lifted` when it
    /// was picked up from one.
    Wire {
        from: Jack,
        lifted: Option<Jack>,
        at: (f32, f32),
    },
    /// The bend of the wire into `inlet`, dragged to `bend`.
    Bend { inlet: Jack, bend: Bend },
    /// A marquee from `from` to `at`, adding to `kept`.
    Marquee {
        from: (f32, f32),
        at: (f32, f32),
        kept: Vec<NodeKey>,
    },
}

/// What the canvas's one menu is open on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Picking {
    /// What feeds this inlet.
    Feed(Jack),
    /// What this inlet tag reads (song patch step 06).
    Bind(ModSourceId),
    /// Which tag to make here: a right-click on empty canvas.
    Make(f32, f32),
}

/// The canvas's menu as it is open: its title, its rows, the one lit, and
/// the canvas point it hangs over.
pub(crate) struct PatchMenu {
    pub title: String,
    pub options: Vec<String>,
    pub current: Option<usize>,
    pub anchor: (f32, f32),
}

/// What a row of the canvas's menu asks of the song.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Chosen {
    Feed(Jack, PatchFeed),
    Bind(ModSourceId, Option<InletSource>),
    Make(TagKind, CanvasPoint),
}

/// The tags a right-click on empty canvas offers to make, in order.
pub(crate) const MADE_TAGS: [&str; 3] = ["Inlet", "Notes in", "Notes out"];

/// The canvas's own state: what is selected, the gesture under way, and
/// what its menu is open on. None of it is in the song.
#[derive(Debug, Clone, Default)]
pub(crate) struct CanvasState {
    pub selected: Vec<NodeKey>,
    pub wire: Option<WireKey>,
    pub drag: Drag,
    pub picking: Option<Picking>,
    pub typing: Option<Typing>,
    /// A note wire is thick for a note just sent, so the pump keeps
    /// refreshing the cables until it is not.
    pub flashing: std::cell::Cell<bool>,
}

/// A box being typed (song patch step 04): where its field is, the box it
/// retypes if any, what is typed so far, and which completion is lit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Typing {
    pub at: (f32, f32),
    pub retype: Option<ModSourceId>,
    pub text: String,
    pub active: usize,
}

impl Typing {
    /// The names the field could complete to: none once a space shows the
    /// name is done.
    pub(crate) fn words(&self) -> Vec<mooloop_core::box_text::Word> {
        if self.text.contains(char::is_whitespace) && !self.text.trim().is_empty() {
            return Vec::new();
        }
        mooloop_core::box_text::completions(&self.text).collect()
    }

    /// What Enter makes: the text as typed once it has settings or names a
    /// box outright, else the lit completion, as the prototype does.
    pub(crate) fn entered(&self) -> String {
        let text = self.text.trim();
        let words = self.words();
        let exact = words.iter().any(|word| word.name == text);
        match words.get(self.active) {
            Some(word) if !exact && !text.contains(char::is_whitespace) => word.name.to_string(),
            _ => text.to_string(),
        }
    }
}

/// What a gesture asks of the song when it ends.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edit {
    /// Nothing for the song: the canvas's own state changed, if anything.
    None,
    /// Move these nodes to these places, as one edit.
    Move(Vec<(NodeKey, CanvasPoint)>),
    /// Wire `from` into `to`, first unplugging `lifted` if the wire came off
    /// another inlet.
    Wire {
        from: Jack,
        to: Jack,
        lifted: Option<Jack>,
    },
    /// The wire was dropped on nothing: unplug it.
    Unplug(Jack),
    /// A wire the song refuses, and why.
    Refused(PatchRefusal),
    /// Arm (or disarm) a source for the assignment gesture.
    Arm(ModSourceRef),
    /// Open the menu on this.
    Pick(Picking),
    /// Select this box in the surface below.
    Select(ModSourceRef),
    /// Open or fold this box's face.
    Fold(ModSourceId),
    /// Set or clear the bend of the wire into this inlet.
    Bend(Jack, Option<Bend>),
}

impl CanvasState {
    /// How far each node a move drag holds is offset now.
    pub(crate) fn offset(&self, key: NodeKey) -> (f32, f32) {
        match self.drag {
            Drag::Move { by, .. } if self.selected.contains(&key) => by,
            _ => (0.0, 0.0),
        }
    }

    /// The bend of the wire into `inlet` as drawn: the one being dragged,
    /// else `saved`.
    pub(crate) fn bend_of(&self, inlet: Jack, saved: Option<Bend>) -> Option<Bend> {
        match self.drag {
            Drag::Bend { inlet: held, bend } if held == inlet => Some(bend),
            _ => saved,
        }
    }

    /// A right-click at `(x, y)`: on empty canvas, the menu of tags to make
    /// there; on an inlet tag, what it reads. Anything else closes the menu.
    pub(crate) fn context(&mut self, layout: &Layout, song: &SongModulation, x: f32, y: f32) {
        self.typing = None;
        self.drag = Drag::None;
        self.picking = match layout.hit(x, y) {
            Hit::Empty => Some(Picking::Make(x, y)),
            Hit::Node(NodeKey::Node(id))
                if matches!(song.tag(id).map(|tag| tag.kind), Some(TagKind::Inlet { .. })) =>
            {
                Some(Picking::Bind(id))
            }
            _ => None,
        };
    }

    /// A press at `(x, y)`.
    pub(crate) fn press(&mut self, layout: &Layout, x: f32, y: f32, shift: bool) -> Edit {
        self.picking = None;
        self.typing = None;
        let hit = layout.hit(x, y);
        let mut edit = Edit::None;
        match hit {
            Hit::Node(key) => {
                self.wire = None;
                if shift {
                    if let Some(index) = self.selected.iter().position(|k| *k == key) {
                        self.selected.remove(index);
                    } else {
                        self.selected.push(key);
                    }
                } else if !self.selected.contains(&key) {
                    self.selected = vec![key];
                }
                edit = match key {
                    NodeKey::Node(id) if layout.node(key).is_some_and(Node::is_box) => {
                        Edit::Select(ModSourceRef::Id(id))
                    }
                    _ => Edit::None,
                };
            }
            Hit::Wire(key) => {
                self.selected.clear();
                self.wire = Some(key);
            }
            Hit::Empty if !shift => {
                self.selected.clear();
                self.wire = None;
            }
            _ => {}
        }
        self.drag = Drag::Pending {
            hit,
            from: (x, y),
            shift,
        };
        edit
    }

    /// The pointer moved to `(x, y)` with the button down.
    pub(crate) fn moved(&mut self, layout: &Layout, song: &SongModulation, x: f32, y: f32) {
        if let Drag::Pending { hit, from, shift } = self.drag.clone() {
            if (x - from.0).abs().max((y - from.1).abs()) < DRAG_THRESHOLD {
                return;
            }
            self.drag = match hit {
                Hit::Outlet(jack) => Drag::Wire {
                    from: jack,
                    lifted: None,
                    at: (x, y),
                },
                Hit::Inlet(jack) => match song.wire_into(jack) {
                    Some(wire) => Drag::Wire {
                        from: wire.from,
                        lifted: Some(jack),
                        at: (x, y),
                    },
                    None => Drag::None,
                },
                Hit::Node(_) if !shift => Drag::Move {
                    from,
                    by: (0.0, 0.0),
                },
                // A patch wire's segment moves along the axis it crosses.
                Hit::Wire(WireKey::Patch(inlet)) if !shift => {
                    match layout.segment_at(WireKey::Patch(inlet), from) {
                        Some((a, b)) if (a.1 - b.1).abs() < 0.5 => Drag::Bend {
                            inlet,
                            bend: Bend {
                                axis: BendAxis::Horizontal,
                                at: a.1.round() as i32,
                            },
                        },
                        Some((a, _)) => Drag::Bend {
                            inlet,
                            bend: Bend {
                                axis: BendAxis::Vertical,
                                at: a.0.round() as i32,
                            },
                        },
                        None => Drag::None,
                    }
                }
                Hit::Empty | Hit::Wire(_) => Drag::Marquee {
                    from,
                    at: (x, y),
                    kept: if shift {
                        self.selected.clone()
                    } else {
                        Vec::new()
                    },
                },
                _ => Drag::None,
            };
        }
        match &mut self.drag {
            Drag::Move { from, by } => *by = ((x - from.0).round(), (y - from.1).round()),
            Drag::Wire { at, .. } => *at = (x, y),
            Drag::Bend { bend, .. } => {
                bend.at = match bend.axis {
                    BendAxis::Horizontal => y.round() as i32,
                    BendAxis::Vertical => x.round() as i32,
                }
            }
            Drag::Marquee { from, at, kept } => {
                *at = (x, y);
                let mut selected = kept.clone();
                for key in layout.within(*from, *at) {
                    if !selected.contains(&key) {
                        selected.push(key);
                    }
                }
                self.selected = selected;
            }
            _ => {}
        }
    }

    /// The button came up at `(x, y)`. `layout` is the canvas as drawn
    /// during the drag.
    pub(crate) fn release(
        &mut self,
        layout: &Layout,
        song: &SongModulation,
        x: f32,
        y: f32,
    ) -> Edit {
        let drag = std::mem::take(&mut self.drag);
        match drag {
            Drag::Pending { hit, shift, .. } => match hit {
                // A box's outlet, or a bound inlet tag's: a source to arm.
                Hit::Outlet(jack)
                    if song.module(jack.node).is_some()
                        || matches!(
                            song.tag(jack.node).map(|tag| tag.kind),
                            Some(TagKind::Inlet { bind: Some(_) })
                        ) =>
                {
                    Edit::Arm(ModSourceRef::Id(jack.node))
                }
                Hit::Inlet(jack) => {
                    self.picking = Some(Picking::Feed(jack));
                    Edit::Pick(Picking::Feed(jack))
                }
                // A click on an inlet tag asks what it reads.
                Hit::Node(NodeKey::Node(id))
                    if !shift
                        && matches!(song.tag(id).map(|tag| tag.kind), Some(TagKind::Inlet { .. })) =>
                {
                    self.picking = Some(Picking::Bind(id));
                    Edit::Pick(Picking::Bind(id))
                }
                Hit::Node(NodeKey::Route { source, .. }) if !shift => Edit::Arm(source),
                Hit::Fold(id) => Edit::Fold(id),
                _ => Edit::None,
            },
            Drag::Move { by, .. } => {
                if by == (0.0, 0.0) {
                    return Edit::None;
                }
                Edit::Move(
                    self.selected
                        .iter()
                        .filter_map(|key| {
                            let node = layout.node(*key)?;
                            Some((
                                *key,
                                CanvasPoint::new(node.x.round() as i32, node.y.round() as i32),
                            ))
                        })
                        .collect(),
                )
            }
            Drag::Wire { from, lifted, .. } => match layout.hit(x, y) {
                Hit::Inlet(to) => {
                    if lifted == Some(to) {
                        return Edit::None;
                    }
                    match song.check_wire(from, to) {
                        Ok(()) => Edit::Wire { from, to, lifted },
                        Err(refusal) => Edit::Refused(PatchRefusal::Wire(refusal)),
                    }
                }
                _ => lifted.map_or(Edit::None, Edit::Unplug),
            },
            Drag::Bend { inlet, bend } => {
                let saved = song.wire_into(inlet).and_then(|wire| wire.bend);
                if saved == Some(bend) {
                    Edit::None
                } else {
                    Edit::Bend(inlet, Some(bend))
                }
            }
            Drag::Marquee { .. } | Drag::None => Edit::None,
        }
    }

    /// A double-click at `(x, y)`: on empty canvas, a field to type a new
    /// box there; on a box, a field over it holding what it says, to retype
    /// it. Anywhere else, nothing. Returns whether a field opened.
    pub(crate) fn double_click(&mut self, layout: &Layout, song: &SongModulation, x: f32, y: f32) -> bool {
        self.typing = match layout.hit(x, y) {
            Hit::Empty => Some(Typing {
                at: (x.round(), y.round()),
                retype: None,
                text: String::new(),
                active: 0,
            }),
            Hit::Node(NodeKey::Node(id)) => song.module(id).map(|module| Typing {
                at: (module.at.x as f32, module.at.y as f32),
                retype: Some(id),
                text: spelling(&module.params, &module.text),
                active: 0,
            }),
            // A jack or a wire: two clicks there are two clicks, so a
            // double-click on an outlet arms and disarms it.
            _ => None,
        };
        if self.typing.is_some() {
            self.drag = Drag::None;
        }
        self.typing.is_some()
    }

    /// A double-click at `(x, y)` on a cable bent by hand straightens it:
    /// the inlet of that wire.
    pub(crate) fn straighten(&self, layout: &Layout, song: &SongModulation, x: f32, y: f32) -> Option<Jack> {
        match layout.hit(x, y) {
            Hit::Wire(WireKey::Patch(inlet)) => song
                .wire_into(inlet)
                .is_some_and(|wire| wire.bend.is_some())
                .then_some(inlet),
            _ => None,
        }
    }

    /// The pointer's wire while one is being drawn, and the inlet it would
    /// land in with whether the song takes it there.
    pub(crate) fn loose_wire(&self, layout: &Layout, song: &SongModulation) -> Option<LooseWire> {
        let Drag::Wire { from, at, .. } = self.drag else {
            return None;
        };
        let start = layout
            .node(NodeKey::Node(from.node))?
            .anchor(true, usize::from(from.port))?;
        let target = match layout.hit(at.0, at.1) {
            Hit::Inlet(to) => Some((to, song.check_wire(from, to).is_ok())),
            _ => None,
        };
        let end = Anchor {
            x: at.0,
            y: at.1,
            facing: Facing::Up,
            left: at.0 - 1.0,
            right: at.0 + 1.0,
        };
        Some((route_points(start, end), target))
    }
}

/// What a refused wire is told.
pub(crate) fn refusal_text(refusal: PatchRefusal) -> &'static str {
    match refusal {
        PatchRefusal::Wire(mooloop_core::WireRefusal::WrongSort) => {
            "Notes and control do not mix: a note outlet goes into a note inlet"
        }
        PatchRefusal::Wire(mooloop_core::WireRefusal::IntoItself) => "A box cannot feed itself",
        PatchRefusal::Wire(mooloop_core::WireRefusal::NoSuchJack) => "That jack is gone",
        PatchRefusal::InletTaken => "That inlet already has a wire",
    }
}

impl UiState {
    /// The canvas as the song and the gesture under way have it now.
    pub(crate) fn patch_layout(&self) -> Layout {
        let session = &self.session;
        let plan = session.modulation_plan();
        let canvas = &self.patch_canvas;
        lay_out(
            &session.modulation,
            |id| {
                session
                    .channel_index(id)
                    .and_then(|seat| session.channels.get(seat))
                    .map(|channel| channel.name.clone())
            },
            |source| session.inlet_source_kind(source),
            |destination| route_label(session, destination),
            |inlet| {
                plan.modules
                    .iter()
                    .find(|module| module.id == inlet.node)
                    .and_then(|module| {
                        module
                            .inlets
                            .get(usize::from(inlet.port))
                            .copied()
                            .flatten()
                    })
                    .is_some_and(|inlet| inlet.delayed)
            },
            canvas,
        )
    }

    /// What row `index` of the menu open on `picking` asks for.
    pub(crate) fn menu_choice(&self, picking: Picking, index: usize) -> Option<Chosen> {
        match picking {
            Picking::Feed(inlet) => {
                let feed = self.session.patch_feed_options(inlet).into_iter().nth(index)?.0;
                Some(Chosen::Feed(inlet, feed))
            }
            Picking::Bind(id) => match index {
                0 => Some(Chosen::Bind(id, None)),
                _ => {
                    let source = self.session.inlet_sources().into_iter().nth(index - 1)?.0;
                    Some(Chosen::Bind(id, Some(source)))
                }
            },
            Picking::Make(x, y) => {
                let kind = match index {
                    0 => TagKind::Inlet { bind: None },
                    1 => TagKind::NotesIn {
                        channel: None,
                        take: false,
                    },
                    2 => TagKind::NotesOut { channel: None },
                    _ => return None,
                };
                Some(Chosen::Make(kind, CanvasPoint::new(x.round() as i32, y.round() as i32)))
            }
        }
    }

    /// The canvas's menu as it is open now.
    pub(crate) fn patch_menu(&self, layout: &Layout) -> Option<PatchMenu> {
        match self.patch_canvas.picking? {
            Picking::Feed(inlet) => {
                let node = layout.node(NodeKey::Node(inlet.node))?;
                let anchor = node.anchor(false, usize::from(inlet.port))?;
                let name = node.inlets.get(usize::from(inlet.port))?.name;
                let options = self
                    .session
                    .patch_feed_options(inlet)
                    .into_iter()
                    .map(|(_, name)| name)
                    .collect();
                Some(PatchMenu {
                    title: format!("Feed {name} from"),
                    options,
                    current: Some(self.session.patch_feed_choice(inlet)),
                    anchor: (anchor.x, anchor.y),
                })
            }
            Picking::Bind(id) => {
                let node = layout.node(NodeKey::Node(id))?;
                let TagKind::Inlet { bind } = self.session.modulation.tag(id)?.kind else {
                    return None;
                };
                let sources = self.session.inlet_sources();
                let current = bind.map_or(Some(0), |bind| {
                    sources.iter().position(|(source, _)| *source == bind).map(|at| at + 1)
                });
                let options = std::iter::once("None".to_string())
                    .chain(sources.into_iter().map(|(_, name)| name))
                    .collect();
                Some(PatchMenu {
                    title: "Read from".to_string(),
                    options,
                    current,
                    anchor: (node.x, node.y),
                })
            }
            Picking::Make(x, y) => Some(PatchMenu {
                title: "Make a tag".to_string(),
                options: MADE_TAGS.iter().map(|name| name.to_string()).collect(),
                current: None,
                anchor: (x, y),
            }),
        }
    }

    /// Draw the canvas: its nodes, jacks, wires, the loose wire and the
    /// marquee of a gesture under way, and the inlet picker.
    pub(crate) fn publish_patch_canvas(&self, window: &MainWindow) {
        let layout = self.patch_layout();
        let canvas = &self.patch_canvas;
        let armed = self.session.modulation_armed.get();
        let patch = window.global::<crate::PatchView>();

        let loose = canvas.loose_wire(&layout, &self.session.modulation);
        let hot = loose.as_ref().and_then(|(_, target)| *target);

        let mut nodes = Vec::with_capacity(layout.nodes.len());
        let mut jacks = Vec::new();
        let mut knobs = Vec::new();
        for node in &layout.nodes {
            let selected = canvas.selected.contains(&node.key);
            let source = match node.key {
                NodeKey::Node(id) => Some(ModSourceRef::Id(id)),
                NodeKey::Route { source, .. } => Some(source),
            };
            let armed_here = armed.is_some()
                && armed == source
                && (node.is_box() || matches!(node.key, NodeKey::Route { .. }));
            let mut row = crate::PatchNodeRow {
                x: node.x,
                y: node.y,
                width: node.width,
                height: node.height,
                selected,
                armed: armed_here,
                ..Default::default()
            };
            match &node.face {
                Face::Box {
                    spelling,
                    unknown,
                    open,
                    knobs: face,
                } => {
                    row.is_box = true;
                    row.spelling = spelling.into();
                    row.unknown = *unknown;
                    row.open = *open;
                    row.empty_face = *open && face.is_empty();
                    if let NodeKey::Node(id) = node.key {
                        knobs.extend(face.iter().filter_map(|knob| self.face_knob_row(id, *knob)));
                    }
                }
                Face::Tag {
                    points_right,
                    kind,
                    name,
                    depth,
                    note,
                    unbound,
                } => {
                    row.points_right = *points_right;
                    row.kind = kind.into();
                    row.name = name.into();
                    row.depth = depth.into();
                    row.note = *note;
                    row.unbound = *unbound;
                }
            }
            nodes.push(row);
            for outlet in [false, true] {
                let ports = if outlet { &node.outlets } else { &node.inlets };
                for (port, info) in ports.iter().enumerate() {
                    let Some(anchor) = node.anchor(outlet, port) else {
                        continue;
                    };
                    let is_hot = !outlet
                        && matches!((node.key, hot), (NodeKey::Node(id), Some((jack, _))) if jack == Jack::new(id, port as u8));
                    jacks.push(crate::PatchJackRow {
                        x: anchor.x,
                        y: anchor.y,
                        note: info.sort == JackSort::Note,
                        outlet,
                        dot: !node.is_box(),
                        name: info.name.into(),
                        named: node.is_box() && (selected || is_hot),
                        right: node.is_box()
                            && !outlet
                            && ports.len() > 1
                            && port == ports.len() - 1,
                        hot: is_hot && hot.is_some_and(|(_, ok)| ok),
                        refused: is_hot && hot.is_some_and(|(_, ok)| !ok),
                    });
                }
            }
        }
        let wires: Vec<crate::PatchWireRow> = layout
            .wires
            .iter()
            .map(|wire| {
                let last = wire.points.last().copied().unwrap_or_default();
                let (level, seen) = self.session.patch_node_activity(wire.source);
                crate::PatchWireRow {
                    d: path_commands(&wire.points).into(),
                    note: wire.note,
                    selected: canvas.wire == Some(wire.key),
                    delayed: wire.delayed,
                    mark_x: last.0,
                    mark_y: last.1,
                    source: wire.source.0 as i32,
                    level: level.abs(),
                    flash: 0,
                    seen: seen as i32,
                }
            })
            .collect();
        patch.set_nodes(ModelRc::new(VecModel::from(nodes)));
        patch.set_assigning(armed.is_some());
        // In place when the faces on show are the same knobs, so a knob
        // being dragged is not rebuilt under the pointer.
        patch.set_knobs(crate::models::rows_in_place(Some(patch.get_knobs()), knobs));
        patch.set_jacks(ModelRc::new(VecModel::from(jacks)));
        patch.set_wires(ModelRc::new(VecModel::from(wires)));
        patch.set_canvas_width(layout.width);
        patch.set_canvas_height(layout.height);
        match loose {
            Some((points, target)) => {
                patch.set_loose_wire(path_commands(&points).into());
                patch.set_loose_refused(target.is_some_and(|(_, ok)| !ok));
            }
            None => {
                patch.set_loose_wire(SharedString::new());
                patch.set_loose_refused(false);
            }
        }
        match &canvas.drag {
            Drag::Marquee { from, at, .. } => {
                patch.set_marquee(true);
                patch.set_marquee_x(from.0.min(at.0));
                patch.set_marquee_y(from.1.min(at.1));
                patch.set_marquee_width((at.0 - from.0).abs());
                patch.set_marquee_height((at.1 - from.1).abs());
            }
            _ => patch.set_marquee(false),
        }
        match self.patch_menu(&layout) {
            Some(PatchMenu {
                title,
                options,
                current,
                anchor,
            }) => {
                let options: Vec<SharedString> = options.into_iter().map(Into::into).collect();
                patch.set_menu_title(title.into());
                patch.set_menu_options(ModelRc::new(VecModel::from(options)));
                patch.set_menu_current(current.map_or(-1, |at| at as i32));
                patch.set_menu_x(anchor.0);
                patch.set_menu_y(anchor.1);
                patch.set_menu_open(true);
            }
            None => patch.set_menu_open(false),
        }
        match &canvas.typing {
            Some(typing) => {
                let words: Vec<crate::PatchWordRow> = typing
                    .words()
                    .into_iter()
                    .map(|word| crate::PatchWordRow {
                        name: word.name.into(),
                        says: word.says.into(),
                    })
                    .collect();
                patch.set_words(ModelRc::new(VecModel::from(words)));
                patch.set_word_active(typing.active as i32);
                patch.set_typing_x(typing.at.0);
                patch.set_typing_y(typing.at.1);
                patch.set_typing(true);
            }
            None => patch.set_typing(false),
        }
        patch.set_empty(layout.nodes.is_empty() && canvas.typing.is_none());
    }
}

impl UiState {
    /// One knob of box `id`'s face as the canvas draws it: where it is, what
    /// it reads, and what the assignment gesture would do to it.
    /// The cables' activity and the open faces' live offsets, from the
    /// levels the pump last read: only the rows that moved are written, in
    /// place. Run by the pump when a level moved or a cable is flashing.
    pub(crate) fn refresh_patch_activity(&self, window: &MainWindow) {
        let patch = window.global::<crate::PatchView>();
        let knobs = patch.get_knobs();
        for index in 0..knobs.row_count() {
            let Some(mut row) = knobs.row_data(index) else {
                continue;
            };
            let (Ok(module), Ok(param)) = (u32::try_from(row.module), u32::try_from(row.param)) else {
                continue;
            };
            let id = ModSourceId(module);
            let Some(descriptor) = self
                .session
                .modulation
                .module(id)
                .and_then(|module| crate::patch_face::descriptor(&module.params, param))
            else {
                continue;
            };
            let policy = mooloop_core::ModDestinationDescriptor::for_param(descriptor);
            let offset = self.session.live_offset(ParamAddr::modulator(id, param), &policy);
            if row.offset != offset {
                row.offset = offset;
                knobs.set_row_data(index, row);
            }
        }
        let flashing = &self.patch_canvas.flashing;
        flashing.set(false);
        if window.global::<crate::DisplayPrefs>().get_cable_activity() == 0 {
            return;
        }
        let wires = patch.get_wires();
        for index in 0..wires.row_count() {
            let Some(mut row) = wires.row_data(index) else {
                continue;
            };
            let Ok(source) = u32::try_from(row.source) else {
                continue;
            };
            let (level, seen) = self.session.patch_node_activity(ModSourceId(source));
            let (level, seen) = (level.abs(), seen as i32);
            let mut changed = false;
            if row.note && seen != row.seen {
                row.seen = seen;
                row.flash = FLASH_TICKS;
                changed = true;
            } else if row.flash > 0 {
                row.flash -= 1;
                changed = true;
            }
            if !row.note && (row.level - level).abs() > 0.002 {
                row.level = level;
                changed = true;
            }
            flashing.set(flashing.get() || row.flash > 0);
            if changed {
                wires.set_row_data(index, row);
            }
        }
    }

    fn face_knob_row(&self, id: ModSourceId, knob: FaceKnob) -> Option<crate::PatchKnobRow> {
        let session = &self.session;
        let module = session.modulation.module(id)?;
        let params = module.params;
        let descriptor = crate::patch_face::descriptor(&params, knob.param)?;
        let value = params.get(knob.param)?;
        let address = ParamAddr::modulator(id, knob.param);
        let policy = mooloop_core::ModDestinationDescriptor::for_param(descriptor);
        let armed = session.modulation_armed.get();
        Some(crate::PatchKnobRow {
            x: knob.x,
            y: knob.y,
            module: id.0 as i32,
            param: knob.param as i32,
            label: crate::patch_face::label(&params, knob.param).into(),
            value_text: crate::patch_face::readout(&params, knob.param).into(),
            value: descriptor.to_normalized(value),
            default_value: descriptor.to_normalized(descriptor.default),
            steps: crate::patch_face::steps(descriptor).map_or(0, i32::from),
            allowed: policy.allowed && armed != Some(ModSourceRef::Id(id)),
            offset: session.live_offset(address, &policy),
            routes: session.route_count(address) as i32,
            depth: armed.map_or(0.0, |source| session.modulation_depth_for(source, address)),
        })
    }
}

/// What a route's assignment tag calls its destination: the parameter, and
/// the device it is on.
fn route_label(session: &Session, destination: ParamAddr) -> RouteLabel {
    if let Some(row) = session
        .plugin_destinations()
        .into_iter()
        .find(|row| row.address == destination)
    {
        return RouteLabel {
            name: row.name,
            device: row.device,
        };
    }
    match session.modulation_destination(destination) {
        Some((device, descriptor)) => RouteLabel {
            name: descriptor.name.to_string(),
            device,
        },
        None => RouteLabel {
            name: "Unavailable".into(),
            device: String::new(),
        },
    }
}

/// Carry out what a gesture asks of the song, one undo step each, and say
/// what happened in the status bar.
fn apply(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    edit: Edit,
) {
    let edited = |label: &'static str, change: &dyn Fn(&mut UiState) -> bool| {
        let before = project_snapshot(&state.borrow(), window);
        let changed = {
            let mut st = state.borrow_mut();
            let changed = change(&mut st);
            if changed {
                st.modulation_edited(window);
            }
            changed
        };
        if changed {
            record_project_history(commands, before, state, window, label);
        }
        changed
    };
    match edit {
        Edit::None => {}
        Edit::Select(source) => {
            let mut st = state.borrow_mut();
            if let Some(index) = st.session.modulation_source_index(source) {
                st.session.select_modulation_source(index as i32);
                st.refresh_modulation(window);
                return;
            }
        }
        Edit::Move(moves) => {
            edited("Move in patch", &|st| {
                let nodes: Vec<_> = moves
                    .iter()
                    .filter_map(|(key, at)| match key {
                        NodeKey::Node(id) => Some((*id, *at)),
                        NodeKey::Route { .. } => None,
                    })
                    .collect();
                let routes: Vec<_> = moves
                    .iter()
                    .filter_map(|(key, at)| match key {
                        NodeKey::Route {
                            source,
                            destination,
                        } => st
                            .session
                            .modulation
                            .routes
                            .iter()
                            .position(|route| {
                                route.source == *source && route.destination == *destination
                            })
                            .map(|index| (index, *at)),
                        NodeKey::Node(_) => None,
                    })
                    .collect();
                let moved = st.session.move_patch_nodes(&nodes);
                st.session.place_patch_routes(&routes) | moved
            });
        }
        Edit::Wire { from, to, lifted } => {
            edited("Patch wire", &|st| {
                let unplugged = lifted.is_some_and(|inlet| st.session.disconnect_patch(inlet));
                st.session.rewire_patch(from, to).unwrap_or(false) | unplugged
            });
        }
        Edit::Unplug(inlet) => {
            edited("Patch wire removed", &|st| {
                st.session.disconnect_patch(inlet)
            });
        }
        Edit::Refused(refusal) => window.set_status_message(refusal_text(refusal).into()),
        Edit::Arm(source) => {
            let mut st = state.borrow_mut();
            let armed = st.session.toggle_patch_arm(source);
            st.refresh_modulation(window);
            window.set_status_message(match armed {
                Some(name) => format!(
                    "Assigning {name} \u{2014} drag any control: up adds depth, down inverts"
                )
                .into(),
                None => "Modulation assignment off \u{2014} controls edit their base values".into(),
            });
            return;
        }
        Edit::Pick(_) => {}
        Edit::Fold(id) => {
            let open = state
                .borrow()
                .session
                .modulation
                .module(id)
                .is_some_and(|module| module.open);
            edited(if open { "Fold a box" } else { "Open a box" }, &|st| {
                st.session.set_patch_box_open(id, !open)
            });
        }
        Edit::Bend(inlet, bend) => {
            let label = if bend.is_some() {
                "Bend a cable"
            } else {
                "Straighten a cable"
            };
            edited(label, &|st| st.session.set_wire_bend(inlet, bend));
        }
    }
    state.borrow_mut().publish_patch_canvas(window);
}

/// Remove what is selected on the canvas, as one edit.
fn delete_selection(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
) -> bool {
    let (nodes, inlets, routes) = {
        let st = state.borrow();
        let canvas = &st.patch_canvas;
        let route_index = |source: ModSourceRef, destination: ParamAddr| {
            st.session
                .modulation
                .routes
                .iter()
                .position(|route| route.source == source && route.destination == destination)
        };
        let mut nodes = Vec::new();
        let mut routes = Vec::new();
        for key in &canvas.selected {
            match *key {
                NodeKey::Node(id) => nodes.push(id),
                NodeKey::Route {
                    source,
                    destination,
                } => routes.extend(route_index(source, destination)),
            }
        }
        let mut inlets = Vec::new();
        match canvas.wire {
            Some(WireKey::Patch(inlet)) => inlets.push(inlet),
            Some(WireKey::Route {
                source,
                destination,
            }) => routes.extend(route_index(source, destination)),
            None => {}
        }
        (nodes, inlets, routes)
    };
    if nodes.is_empty() && inlets.is_empty() && routes.is_empty() {
        return false;
    }
    let before = project_snapshot(&state.borrow(), window);
    let changed = {
        let mut st = state.borrow_mut();
        let changed = st.session.remove_patch_selection(&nodes, &inlets, &routes);
        st.patch_canvas.selected.clear();
        st.patch_canvas.wire = None;
        if changed {
            st.modulation_edited(window);
        } else {
            st.publish_patch_canvas(window);
        }
        changed
    };
    if changed {
        record_project_history(commands, before, state, window, "Delete from patch");
    }
    true
}

/// Make what the field holds, as one edit: a new box, or the box being
/// retyped made over. Says in the status bar what came of it.
fn commit_typing(
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
    window: &MainWindow,
    text: &str,
) {
    let Some(typing) = state.borrow_mut().patch_canvas.typing.take() else {
        return;
    };
    let text = text.trim().to_string();
    let before = project_snapshot(&state.borrow(), window);
    let made = {
        let mut st = state.borrow_mut();
        let made = match typing.retype {
            Some(id) => st.session.retype_patch_box(id, &text),
            None => st.session.type_patch_box(
                &text,
                CanvasPoint::new(typing.at.0 as i32, typing.at.1 as i32),
            ),
        };
        match made {
            Some(id) => {
                st.patch_canvas.selected = vec![NodeKey::Node(id)];
                st.patch_canvas.wire = None;
                st.modulation_edited(window);
            }
            None => st.publish_patch_canvas(window),
        }
        made
    };
    let Some(id) = made else {
        return;
    };
    let label = if typing.retype.is_some() {
        "Retype a box"
    } else {
        "Type a box"
    };
    record_project_history(commands, before, state, window, label);
    let st = state.borrow();
    let Some(module) = st.session.modulation.module(id) else {
        return;
    };
    let spelled = spelling(&module.params, &module.text);
    window.set_status_message(
        if module.params == ModulatorParams::Unknown {
            format!("\u{201c}{spelled}\u{201d} is not a box this build knows, so it does nothing")
        } else {
            format!("Made {spelled}")
        }
        .into(),
    );
}

/// Install the canvas's handlers: the pointer, the keys it takes while it
/// has the focus, and the inlet picker.
pub(crate) fn wire(
    window: &MainWindow,
    state: &Rc<RefCell<UiState>>,
    commands: &Rc<RefCell<CommandState>>,
) {
    let patch = window.global::<crate::PatchView>();
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_pressed(move |x, y, shift| {
            let Some(window) = weak.upgrade() else { return };
            let edit = {
                let mut state = st.borrow_mut();
                let layout = state.patch_layout();
                state.patch_canvas.press(&layout, x, y, shift)
            };
            apply(&st, &commands, &window, edit);
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        patch.on_moved(move |x, y| {
            let Some(window) = weak.upgrade() else { return };
            let mut state = st.borrow_mut();
            let layout = state.patch_layout();
            let UiState {
                patch_canvas,
                session,
                ..
            } = &mut *state;
            patch_canvas.moved(&layout, &session.modulation, x, y);
            state.publish_patch_canvas(&window);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_released(move |x, y| {
            let Some(window) = weak.upgrade() else { return };
            let edit = {
                let mut state = st.borrow_mut();
                let layout = state.patch_layout();
                let UiState {
                    patch_canvas,
                    session,
                    ..
                } = &mut *state;
                patch_canvas.release(&layout, &session.modulation, x, y)
            };
            apply(&st, &commands, &window, edit);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_key(move |text| {
            let Some(window) = weak.upgrade() else {
                return false;
            };
            match text.as_str() {
                "\u{7f}" | "\u{8}" => delete_selection(&st, &commands, &window),
                "\u{1b}" => {
                    let mut state = st.borrow_mut();
                    let busy = state.patch_canvas.drag != Drag::None
                        || state.patch_canvas.picking.is_some();
                    state.patch_canvas.drag = Drag::None;
                    state.patch_canvas.picking = None;
                    let armed = state.session.modulation_armed.get().is_some();
                    if armed && !busy {
                        state.session.modulation_armed.set(None);
                        state.refresh_modulation(&window);
                        window.set_status_message(
                            "Modulation assignment off \u{2014} controls edit their base values"
                                .into(),
                        );
                        return true;
                    }
                    state.publish_patch_canvas(&window);
                    busy
                }
                _ => false,
            }
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_picked(move |index| {
            let Some(window) = weak.upgrade() else { return };
            let chosen = {
                let mut state = st.borrow_mut();
                let picking = state.patch_canvas.picking.take();
                usize::try_from(index)
                    .ok()
                    .zip(picking)
                    .and_then(|(index, picking)| state.menu_choice(picking, index))
            };
            let Some(chosen) = chosen else {
                st.borrow_mut().publish_patch_canvas(&window);
                return;
            };
            let before = project_snapshot(&st.borrow(), &window);
            let changed = {
                let mut state = st.borrow_mut();
                let changed = match chosen {
                    Chosen::Feed(inlet, feed) => state.session.feed_patch_inlet(inlet, feed),
                    Chosen::Bind(id, bind) => state.session.bind_patch_tag(id, bind),
                    Chosen::Make(kind, at) => match state.session.add_patch_tag(kind, at) {
                        Some(id) => {
                            // A new inlet is empty: ask straight away what
                            // it reads.
                            if matches!(kind, TagKind::Inlet { .. }) {
                                state.patch_canvas.picking = Some(Picking::Bind(id));
                            }
                            state.patch_canvas.selected = vec![NodeKey::Node(id)];
                            true
                        }
                        None => false,
                    },
                };
                if changed {
                    state.modulation_edited(&window);
                } else {
                    state.publish_patch_canvas(&window);
                }
                changed
            };
            if changed {
                let label = match chosen {
                    Chosen::Feed(_, PatchFeed::None) => "Patch wire removed",
                    Chosen::Feed(..) => "Patch wire",
                    Chosen::Bind(..) => "Patch tag source",
                    Chosen::Make(..) => "Patch tag",
                };
                record_project_history(&commands, before, &st, &window, label);
            }
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        patch.on_context(move |x, y| {
            let Some(window) = weak.upgrade() else { return };
            let mut state = st.borrow_mut();
            let layout = state.patch_layout();
            let UiState {
                patch_canvas,
                session,
                ..
            } = &mut *state;
            patch_canvas.context(&layout, &session.modulation, x, y);
            state.publish_patch_canvas(&window);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_double_clicked(move |x, y| {
            let Some(window) = weak.upgrade() else { return };
            let straighten = {
                let state = st.borrow();
                let layout = state.patch_layout();
                state
                    .patch_canvas
                    .straighten(&layout, &state.session.modulation, x, y)
            };
            if let Some(inlet) = straighten {
                st.borrow_mut().patch_canvas.drag = Drag::None;
                apply(&st, &commands, &window, Edit::Bend(inlet, None));
                return;
            }
            let mut state = st.borrow_mut();
            let layout = state.patch_layout();
            let UiState {
                patch_canvas,
                session,
                ..
            } = &mut *state;
            if patch_canvas.double_click(&layout, &session.modulation, x, y) {
                let text = patch_canvas.typing.as_ref().map(|typing| typing.text.clone());
                window
                    .global::<crate::PatchView>()
                    .set_typing_text(text.unwrap_or_default().into());
                state.publish_patch_canvas(&window);
            }
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        patch.on_typing_edited(move |text| {
            let Some(window) = weak.upgrade() else { return };
            let mut state = st.borrow_mut();
            if let Some(typing) = &mut state.patch_canvas.typing {
                typing.text = text.to_string();
                typing.active = 0;
            }
            state.publish_patch_canvas(&window);
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        patch.on_typing_key(move |key| {
            let Some(window) = weak.upgrade() else {
                return false;
            };
            let mut state = st.borrow_mut();
            let Some(typing) = &mut state.patch_canvas.typing else {
                return false;
            };
            let words = typing.words();
            match key.as_str() {
                // Slint's Up and Down arrows.
                "\u{f700}" => typing.active = typing.active.saturating_sub(1),
                "\u{f701}" => typing.active = (typing.active + 1).min(words.len().saturating_sub(1)),
                "\t" => {
                    let Some(word) = words.get(typing.active) else {
                        return true;
                    };
                    typing.text = format!("{} ", word.name);
                    let text = typing.text.clone();
                    window
                        .global::<crate::PatchView>()
                        .set_typing_text(text.into());
                }
                "\u{1b}" => state.patch_canvas.typing = None,
                _ => return false,
            }
            state.publish_patch_canvas(&window);
            true
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_typing_accepted(move |text| {
            let Some(window) = weak.upgrade() else { return };
            let entered = {
                let mut state = st.borrow_mut();
                let Some(typing) = &mut state.patch_canvas.typing else {
                    return;
                };
                typing.text = text.to_string();
                typing.entered()
            };
            commit_typing(&st, &commands, &window, &entered);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_word_picked(move |index| {
            let Some(window) = weak.upgrade() else { return };
            let word = st
                .borrow()
                .patch_canvas
                .typing
                .as_ref()
                .and_then(|typing| typing.words().get(index as usize).copied());
            if let Some(word) = word {
                commit_typing(&st, &commands, &window, word.name);
            }
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_knob_changed(move |module, param, value| {
            let (Some(window), Ok(module), Ok(param)) =
                (weak.upgrade(), u32::try_from(module), u32::try_from(param))
            else {
                return;
            };
            let id = ModSourceId(module);
            // A drag is one undo step: the knob's own gesture holds it open.
            with_gesture_history(&st, &commands, &window, "Box setting", || {
                let mut state = st.borrow_mut();
                let Some(params) = state.session.modulation.module(id).map(|module| module.params)
                else {
                    return false;
                };
                let Some(descriptor) = crate::patch_face::descriptor(&params, param) else {
                    return false;
                };
                let mut natural = descriptor.from_normalized(value);
                if crate::patch_face::steps(descriptor).is_some() {
                    natural = natural.round();
                }
                if !state.session.set_patch_box_param(id, param, natural) {
                    return false;
                }
                state.modulation_edited(&window);
                true
            });
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        patch.on_knob_depth_started(move || {
            let Some(window) = weak.upgrade() else { return };
            st.borrow_mut().begin_gesture(&window);
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_knob_depth_changed(move |module, param, depth| {
            let (Some(window), Ok(module), Ok(param)) =
                (weak.upgrade(), u32::try_from(module), u32::try_from(param))
            else {
                return;
            };
            let destination = ParamAddr::modulator(ModSourceId(module), param);
            with_gesture_history(&st, &commands, &window, "Modulation depth", || {
                let mut state = st.borrow_mut();
                if !state.set_armed_modulation_depth(&window, destination, depth) {
                    state.refresh_modulation(&window);
                    return false;
                }
                true
            });
        });
    }
    {
        let st = state.clone();
        let commands = commands.clone();
        let weak = window.as_weak();
        patch.on_knob_depth_finished(move || {
            let Some(window) = weak.upgrade() else { return };
            let before = st.borrow_mut().session.finish_gesture();
            if let Some(before) = before {
                record_project_history(&commands, before, &st, &window, "Modulation route changed");
            }
        });
    }
    {
        let st = state.clone();
        let weak = window.as_weak();
        patch.on_menu_closed(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut state = st.borrow_mut();
            if state.patch_canvas.picking.take().is_some() {
                state.publish_patch_canvas(&window);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mooloop_core::{ChannelId, ModLfoParams, ModMathOp, ModMathParams, ModulatorKind, SongModule, Wire};

    fn module(id: u32, params: ModulatorParams, at: (i32, i32)) -> SongModule {
        SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            at: CanvasPoint::new(at.0, at.1),
            open: false,
            rack: None,
            params,
            text: String::new(),
        }
    }

    /// The prototype's first example: the kick's gate into an LFO's
    /// retrigger, and `* -0.5` after it.
    fn example() -> SongModulation {
        let mut song = SongModulation {
            modules: vec![
                module(0, ModulatorParams::Lfo(ModLfoParams::default()), (204, 72)),
                module(
                    1,
                    ModulatorParams::Math(ModMathParams {
                        operand: -0.5,
                        ..ModMathParams::default()
                    }),
                    (494, 152),
                ),
            ],
            next_source_id: 2,
            ..SongModulation::default()
        };
        let gate = song.add_tag(
            TagKind::Inlet {
                bind: Some(InletSource::Gate(ChannelId(0))),
            },
            CanvasPoint::new(24, 24),
        );
        song.connect(Jack::new(gate, 0), Jack::new(ModSourceId(0), 1))
            .unwrap();
        song.connect(Jack::new(ModSourceId(0), 0), Jack::new(ModSourceId(1), 0))
            .unwrap();
        song
    }

    fn laid(song: &SongModulation, canvas: &CanvasState) -> Layout {
        lay_out(
            song,
            |_| Some("Kick 1".into()),
            |source| Some(match source {
                InletSource::Gate(_) => "gate".into(),
                InletSource::Beat => "beat".into(),
                _ => "outlet".into(),
            }),
            |_| RouteLabel {
                name: "Cutoff".into(),
                device: "ML-M1 9".into(),
            },
            |_| false,
            canvas,
        )
    }

    fn anchor_of(layout: &Layout, id: u32, outlet: bool, port: usize) -> (f32, f32) {
        let anchor = layout
            .node(NodeKey::Node(ModSourceId(id)))
            .unwrap()
            .anchor(outlet, port)
            .unwrap();
        (anchor.x, anchor.y)
    }

    #[test]
    fn enter_takes_the_lit_name_until_the_typing_is_a_box_of_its_own() {
        let typing = |text: &str, active| Typing {
            at: (0.0, 0.0),
            retype: None,
            text: text.into(),
            active,
        };
        assert_eq!(typing("s", 1).entered(), "select", "the lit completion");
        assert_eq!(typing("step", 0).entered(), "step", "a whole name stands");
        assert_eq!(typing("min", 1).entered(), "min", "even when another is lit");
        assert_eq!(typing("counter 8", 0).entered(), "counter 8", "settings: as typed");
        assert_eq!(typing("chord", 0).entered(), "chord", "nothing to complete");
        assert!(typing("counter 8", 0).words().is_empty(), "the name is done");
        assert_eq!(typing("", 0).words().len(), mooloop_core::box_text::VOCABULARY.len());
    }

    #[test]
    fn boxes_spell_what_they_do() {
        let math = |op, operand| {
            spelling(&ModulatorParams::Math(ModMathParams {
                op,
                operand,
                ..ModMathParams::default()
            }), "")
        };
        assert_eq!(math(ModMathOp::Multiply, -0.5), "* -0.5");
        assert_eq!(math(ModMathOp::Add, 1.0), "+ 1");
        assert_eq!(math(ModMathOp::Min, 0.25), "min 0.25");
        assert_eq!(math(ModMathOp::Divide, -0.0001), "/ 0");
        assert_eq!(
            spelling(&ModulatorParams::Lfo(ModLfoParams::default()), ""),
            "lfo"
        );
        assert_eq!(ModulatorKind::Lfo.ports().inlets.len(), 2);
    }

    #[test]
    fn the_example_lays_out_with_jacks_on_the_edges_and_wires_between_them() {
        let song = example();
        let layout = laid(&song, &CanvasState::default());
        assert_eq!(layout.nodes.len(), 3);
        let lfo = layout.node(NodeKey::Node(ModSourceId(0))).unwrap();
        assert_eq!((lfo.x, lfo.y, lfo.height), (204.0, 72.0, BOX_HEIGHT));
        // Two inlets spread across the top edge, one outlet at the bottom.
        assert_eq!(anchor_of(&layout, 0, false, 0), (210.0, 72.0));
        assert_eq!(
            anchor_of(&layout, 0, false, 1),
            (204.0 + lfo.width - 6.0, 72.0)
        );
        assert_eq!(anchor_of(&layout, 0, true, 0), (210.0, 72.0 + BOX_HEIGHT));
        assert_eq!(layout.wires.len(), 2);
        // The gate leaves its tag's point to the right and turns down into
        // the retrigger inlet.
        let gate = &layout.wires[0];
        let tag = &layout.nodes[2];
        assert_eq!(
            gate.points.first(),
            Some(&(tag.x + tag.width, 24.0 + TAG_HEIGHT / 2.0))
        );
        assert_eq!(gate.points.last(), Some(&anchor_of(&layout, 0, false, 1)));
        assert_eq!(gate.points.len(), 3, "one corner");
        assert!(layout.width >= MIN_EXTENT.0 && layout.height >= MIN_EXTENT.1);
    }

    #[test]
    fn a_wire_path_rounds_its_corners() {
        let d = path_commands(&[(0.0, 0.0), (0.0, 50.0), (0.0, 100.0), (80.0, 100.0)]);
        assert_eq!(d, "M 0 0 L 0 93 Q 0 100 7 100 L 80 100");
        assert_eq!(path_commands(&[(1.0, 2.0), (1.0, 2.0)]), "M 1 2");
    }

    #[test]
    fn assignment_tags_stack_under_their_box_until_placed() {
        use mooloop_core::{
            EffectTarget, ModPolarity, ModRoute, STRIP_PARAM_PAN, STRIP_PARAM_VOLUME,
        };
        let mut song = example();
        let volume = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_VOLUME);
        let pan = ParamAddr::strip(EffectTarget::Channel(0), STRIP_PARAM_PAN);
        for (destination, depth) in [(volume, 0.4), (pan, -0.5)] {
            song.routes.push(ModRoute::from_module(
                ModSourceId(0),
                destination,
                depth,
                ModPolarity::Bipolar,
            ));
        }
        let layout = laid(&song, &CanvasState::default());
        let source = ModSourceRef::Id(ModSourceId(0));
        let first = layout
            .node(NodeKey::Route {
                source,
                destination: volume,
            })
            .unwrap();
        let second = layout
            .node(NodeKey::Route {
                source,
                destination: pan,
            })
            .unwrap();
        assert_eq!(
            (first.x, first.y),
            (220.0, 72.0 + BOX_HEIGHT.round() + 40.0)
        );
        assert_eq!(second.y - first.y, 34.0);
        assert!(matches!(&second.face, Face::Tag { depth, .. } if depth == "-50%"));
        assert!(matches!(&first.face, Face::Tag { depth, .. } if depth == "+40%"));
        assert_eq!(layout.wires.len(), 4, "a wire to each assignment tag");

        song.place_route(source, pan, CanvasPoint::new(600, 300));
        let layout = laid(&song, &CanvasState::default());
        let second = layout
            .node(NodeKey::Route {
                source,
                destination: pan,
            })
            .unwrap();
        assert_eq!((second.x, second.y), (600.0, 300.0));
    }

    #[test]
    fn a_press_finds_the_jack_before_its_box_and_the_box_before_a_wire() {
        let song = example();
        let layout = laid(&song, &CanvasState::default());
        let (x, y) = anchor_of(&layout, 0, false, 1);
        assert_eq!(
            layout.hit(x + 3.0, y - 2.0),
            Hit::Inlet(Jack::new(ModSourceId(0), 1))
        );
        let (x, y) = anchor_of(&layout, 0, true, 0);
        assert_eq!(
            layout.hit(x, y + 4.0),
            Hit::Outlet(Jack::new(ModSourceId(0), 0))
        );
        assert_eq!(
            layout.hit(250.0, 86.0),
            Hit::Node(NodeKey::Node(ModSourceId(0)))
        );
        // The LFO's wire down to the Math box, on its horizontal run.
        let wire = &layout.wires[1];
        let (a, b) = (wire.points[1], wire.points[2]);
        assert_eq!(
            layout.hit((a.0 + b.0) / 2.0, a.1 + 2.0),
            Hit::Wire(WireKey::Patch(Jack::new(ModSourceId(1), 0)))
        );
        assert_eq!(layout.hit(1000.0, 500.0), Hit::Empty);
    }

    #[test]
    fn dragging_a_selection_moves_every_box_in_it_by_the_same_amount() {
        let song = example();
        let mut canvas = CanvasState::default();
        let layout = laid(&song, &canvas);
        canvas.press(&layout, 1000.0, 10.0, false);
        canvas.moved(&layout, &song, 600.0, 300.0);
        canvas.moved(&layout, &song, 150.0, 160.0);
        assert_eq!(canvas.release(&layout, &song, 150.0, 160.0), Edit::None);
        assert_eq!(
            canvas.selected.len(),
            2,
            "the marquee took both boxes: {:?}",
            canvas.selected
        );

        let edit = canvas.press(&layout, 250.0, 86.0, false);
        assert_eq!(edit, Edit::Select(ModSourceRef::Id(ModSourceId(0))));
        assert_eq!(
            canvas.selected.len(),
            2,
            "pressing a selected box keeps the selection"
        );
        canvas.moved(&layout, &song, 260.0, 96.0);
        canvas.moved(&layout, &song, 290.0, 126.0);
        let dragged = laid(&song, &canvas);
        assert_eq!(
            dragged.node(NodeKey::Node(ModSourceId(1))).unwrap().x,
            534.0,
            "drawn moved mid-drag"
        );
        let edit = canvas.release(&dragged, &song, 290.0, 126.0);
        assert_eq!(
            edit,
            Edit::Move(vec![
                (NodeKey::Node(ModSourceId(0)), CanvasPoint::new(244, 112)),
                (NodeKey::Node(ModSourceId(1)), CanvasPoint::new(534, 192)),
            ])
        );
    }

    #[test]
    fn a_wire_drawn_onto_an_inlet_asks_for_it_and_a_wrong_one_is_refused() {
        let mut song = example();
        let notes = song.add_tag(
            TagKind::NotesIn {
                channel: None,
                take: false,
            },
            CanvasPoint::new(24, 300),
        );
        let mut canvas = CanvasState::default();
        let layout = laid(&song, &canvas);
        let out = anchor_of(&layout, 1, true, 0);
        let rate = anchor_of(&layout, 0, false, 0);
        canvas.press(&layout, out.0, out.1, false);
        canvas.moved(&layout, &song, out.0 + 20.0, out.1 + 20.0);
        canvas.moved(&layout, &song, rate.0, rate.1);
        let (_, target) = canvas.loose_wire(&layout, &song).unwrap();
        assert_eq!(
            target,
            Some((Jack::new(ModSourceId(0), 0), true)),
            "the rate inlet lights"
        );
        assert_eq!(
            canvas.release(&layout, &song, rate.0, rate.1),
            Edit::Wire {
                from: Jack::new(ModSourceId(1), 0),
                to: Jack::new(ModSourceId(0), 0),
                lifted: None
            },
            "a wire that closes a loop is allowed"
        );

        let tag = layout
            .node(NodeKey::Node(notes))
            .unwrap()
            .anchor(true, 0)
            .unwrap();
        canvas.press(&layout, tag.x, tag.y, false);
        canvas.moved(&layout, &song, rate.0, rate.1);
        assert_eq!(
            canvas.loose_wire(&layout, &song).unwrap().1,
            Some((Jack::new(ModSourceId(0), 0), false))
        );
        assert_eq!(
            canvas.release(&layout, &song, rate.0, rate.1),
            Edit::Refused(PatchRefusal::Wire(mooloop_core::WireRefusal::WrongSort))
        );
    }

    #[test]
    fn a_wire_picked_off_its_inlet_moves_or_comes_out() {
        let song = example();
        let mut canvas = CanvasState::default();
        let layout = laid(&song, &canvas);
        let retrigger = anchor_of(&layout, 0, false, 1);
        let rate = anchor_of(&layout, 0, false, 0);
        canvas.press(&layout, retrigger.0, retrigger.1, false);
        canvas.moved(&layout, &song, rate.0, rate.1);
        let gate = song.tags[0].id;
        assert_eq!(
            canvas.release(&layout, &song, rate.0, rate.1),
            Edit::Wire {
                from: Jack::new(gate, 0),
                to: Jack::new(ModSourceId(0), 0),
                lifted: Some(Jack::new(ModSourceId(0), 1))
            }
        );
        canvas.press(&layout, retrigger.0, retrigger.1, false);
        canvas.moved(&layout, &song, 900.0, 500.0);
        assert_eq!(
            canvas.release(&layout, &song, 900.0, 500.0),
            Edit::Unplug(Jack::new(ModSourceId(0), 1))
        );
    }

    #[test]
    fn dragging_a_cables_middle_bends_it_and_a_double_click_straightens_it() {
        let mut song = example();
        let mut canvas = CanvasState::default();
        let layout = laid(&song, &canvas);
        let inlet = Jack::new(ModSourceId(1), 0);
        let points = layout
            .wires
            .iter()
            .find(|wire| wire.key == WireKey::Patch(inlet))
            .unwrap()
            .points
            .clone();
        assert!(points.len() >= 4, "the wire has a middle: {points:?}");
        let (a, b) = (points[1], points[2]);
        let middle = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let across = (a.1 - b.1).abs() < 0.5;
        let to = if across {
            (middle.0, middle.1 + 30.0)
        } else {
            (middle.0 + 30.0, middle.1)
        };
        canvas.press(&layout, middle.0, middle.1, false);
        canvas.moved(&layout, &song, to.0, to.1);
        let bend = if across {
            Bend {
                axis: BendAxis::Horizontal,
                at: to.1.round() as i32,
            }
        } else {
            Bend {
                axis: BendAxis::Vertical,
                at: to.0.round() as i32,
            }
        };
        // The drag is drawn before it is an edit: the run sits where the
        // pointer has it.
        let dragged = laid(&song, &canvas);
        let run = &dragged
            .wires
            .iter()
            .find(|wire| wire.key == WireKey::Patch(inlet))
            .unwrap()
            .points;
        assert!(run.windows(2).any(|pair| match bend.axis {
            BendAxis::Horizontal => pair[0].1 == to.1.round() && pair[1].1 == to.1.round(),
            BendAxis::Vertical => pair[0].0 == to.0.round() && pair[1].0 == to.0.round(),
        }));
        assert_eq!(
            canvas.release(&dragged, &song, to.0, to.1),
            Edit::Bend(inlet, Some(bend))
        );
        // Saved, it lays out the same; a double-click on it asks to
        // straighten it, and on an unbent cable does nothing.
        assert!(song.bend_wire(inlet, Some(bend)));
        let bent = laid(&song, &canvas);
        assert_eq!(bent.wires, dragged.wires);
        let on = run[2];
        assert_eq!(canvas.straighten(&bent, &song, on.0, on.1), Some(inlet));
        assert!(song.bend_wire(inlet, None));
        let straight = laid(&song, &canvas);
        assert_eq!(straight, layout);
        assert_eq!(canvas.straighten(&straight, &song, middle.0, middle.1), None);
    }

    #[test]
    fn clicks_arm_an_outlet_and_open_an_inlets_picker() {
        let song = example();
        let mut canvas = CanvasState::default();
        let layout = laid(&song, &canvas);
        let out = anchor_of(&layout, 0, true, 0);
        canvas.press(&layout, out.0, out.1, false);
        assert_eq!(
            canvas.release(&layout, &song, out.0, out.1),
            Edit::Arm(ModSourceRef::Id(ModSourceId(0)))
        );
        let rate = anchor_of(&layout, 0, false, 0);
        canvas.press(&layout, rate.0, rate.1, false);
        assert_eq!(
            canvas.release(&layout, &song, rate.0, rate.1),
            Edit::Pick(Picking::Feed(Jack::new(ModSourceId(0), 0)))
        );
        assert_eq!(canvas.picking, Some(Picking::Feed(Jack::new(ModSourceId(0), 0))));
        canvas.press(&layout, 1000.0, 500.0, false);
        assert_eq!(canvas.picking, None, "a press elsewhere closes it");
    }

    #[test]
    fn a_saved_late_wire_is_drawn_delayed() {
        let mut song = example();
        song.wires.push(Wire {
            from: Jack::new(ModSourceId(1), 0),
            to: Jack::new(ModSourceId(0), 0),
            bend: None,
            late: true,
        });
        let layout = laid(&song, &CanvasState::default());
        assert!(layout.wires.iter().any(|wire| wire.delayed));
    }
}
