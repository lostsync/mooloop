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

use crate::{project_snapshot, record_project_history, CommandState, MainWindow, UiState};
use mooloop_core::{
    CanvasPoint, InletSource, Jack, JackSort, ModMathOp, ModSourceId, ModSourceRef,
    ModulatorParams, ParamAddr, Port, SongModulation, TagKind,
};
use mooloop_session::modulation::{PatchFeed, PatchRefusal};
use mooloop_session::session::Session;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

/// A box's height, and a tag's.
pub(crate) const BOX_HEIGHT: f32 = 28.0;
pub(crate) const TAG_HEIGHT: f32 = 26.0;
/// A box's jack is a bar this wide on its edge.
pub(crate) const JACK_WIDTH: f32 = 12.0;
/// How far from a jack a press still takes it.
const JACK_REACH: f32 = 8.0;
/// How far from a wire a press still selects it.
const WIRE_REACH: f32 = 5.0;
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
    /// A box: its spelling in the mono face.
    Box { spelling: String },
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
    Node(NodeKey),
    Wire(WireKey),
    Empty,
}

/// What a route's tag says: its parameter, its device, and its depth.
pub(crate) struct RouteLabel {
    pub name: String,
    pub device: String,
}

/// How `params` reads on its box: the five kinds by name, a Math box as the
/// sum it does, `* -0.5`.
pub(crate) fn spelling(params: &ModulatorParams) -> String {
    match params {
        ModulatorParams::Lfo(_) => "lfo".into(),
        ModulatorParams::Envelope(_) => "env".into(),
        ModulatorParams::Step(_) => "step".into(),
        ModulatorParams::Random(_) => "random".into(),
        ModulatorParams::Math(math) => {
            let operand = format_number(math.operand);
            match math.op {
                ModMathOp::Add => format!("+ {operand}"),
                ModMathOp::Subtract => format!("- {operand}"),
                ModMathOp::Multiply => format!("* {operand}"),
                ModMathOp::Divide => format!("/ {operand}"),
                ModMathOp::Min => format!("min {operand}"),
                ModMathOp::Max => format!("max {operand}"),
                ModMathOp::Clamp => format!(
                    "clamp {} {}",
                    format_number(math.clamp_low),
                    format_number(math.clamp_high)
                ),
            }
        }
    }
}

/// A number as a box spells it: at most two decimals, no trailing zeros.
fn format_number(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
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

/// Lay out `song`'s patch. `channel_name` names a channel by id, `label_of`
/// names a route's destination, and `offset` moves the nodes a drag holds by
/// how far it has gone, so a drag is drawn before it is a song edit.
pub(crate) fn lay_out(
    song: &SongModulation,
    channel_name: impl Fn(mooloop_core::ChannelId) -> Option<String>,
    label_of: impl Fn(ParamAddr) -> RouteLabel,
    delayed: impl Fn(Jack) -> bool,
    offset: impl Fn(NodeKey) -> (f32, f32),
) -> Layout {
    let mut nodes = Vec::new();
    for module in &song.modules {
        let ports = module.params.kind().ports();
        let spelling = spelling(&module.params);
        let jacks = ports.inlets.len().max(ports.outlets.len()) as f32;
        let width = (26.0 + text_width(&spelling, MONO_CHAR))
            .max(64.0)
            .max(jacks * 24.0);
        let key = NodeKey::Node(module.id);
        let (dx, dy) = offset(key);
        nodes.push(Node {
            key,
            x: module.at.x as f32 + dx,
            y: module.at.y as f32 + dy,
            width,
            height: BOX_HEIGHT,
            face: Face::Box { spelling },
            inlets: ports.inlets.to_vec(),
            outlets: ports.outlets.to_vec(),
        });
    }
    for tag in &song.tags {
        let channel = tag.kind.channel().and_then(&channel_name);
        let (points_right, kind, note) = match tag.kind {
            TagKind::Inlet { bind } => (
                true,
                match bind {
                    Some(InletSource::Gate(_)) => "gate",
                    None => "inlet",
                },
                false,
            ),
            TagKind::NotesIn { .. } => (true, "notes", true),
            TagKind::NotesOut { .. } => (false, "notes to", true),
        };
        let unbound = channel.is_none();
        let name = channel.unwrap_or_default();
        let key = NodeKey::Node(tag.id);
        let (dx, dy) = offset(key);
        let ports = tag.kind.ports();
        nodes.push(Node {
            key,
            x: tag.at.x as f32 + dx,
            y: tag.at.y as f32 + dy,
            width: tag_width(kind, &name, "", unbound),
            height: TAG_HEIGHT,
            face: Face::Tag {
                points_right,
                kind: kind.into(),
                name,
                depth: String::new(),
                note,
                unbound,
            },
            inlets: ports.inlets.to_vec(),
            outlets: ports.outlets.to_vec(),
        });
    }

    // An assignment tag for every route from a box, where it was put or
    // stacked under its box. A route from a channel's outlet or keyboard has
    // nothing on the canvas to come from until step 06's song inlets.
    let mut stacked: Vec<(ModSourceId, usize)> = Vec::new();
    let mut route_wires = Vec::new();
    for route in &song.routes {
        let ModSourceRef::Id(id) = route.source else {
            continue;
        };
        let Some(module) = song.module(id) else {
            continue;
        };
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
                CanvasPoint::new(
                    module.at.x + 16,
                    module.at.y + BOX_HEIGHT as i32 + 40 + below as i32 * 34,
                )
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
            points: route_points(start, end),
            note,
            delayed: wire.late || delayed(wire.to),
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
            return Hit::Node(node.key);
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
    /// A marquee from `from` to `at`, adding to `kept`.
    Marquee {
        from: (f32, f32),
        at: (f32, f32),
        kept: Vec<NodeKey>,
    },
}

/// The canvas's own state: what is selected, the gesture under way, and the
/// inlet whose picker is open. None of it is in the song.
#[derive(Debug, Clone, Default)]
pub(crate) struct CanvasState {
    pub selected: Vec<NodeKey>,
    pub wire: Option<WireKey>,
    pub drag: Drag,
    pub picking: Option<Jack>,
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
    /// Open the picker for this inlet.
    Pick(Jack),
    /// Select this box in the surface below.
    Select(ModSourceRef),
}

impl CanvasState {
    /// How far each node a move drag holds is offset now.
    pub(crate) fn offset(&self, key: NodeKey) -> (f32, f32) {
        match self.drag {
            Drag::Move { by, .. } if self.selected.contains(&key) => by,
            _ => (0.0, 0.0),
        }
    }

    /// A press at `(x, y)`.
    pub(crate) fn press(&mut self, layout: &Layout, x: f32, y: f32, shift: bool) -> Edit {
        self.picking = None;
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
                Hit::Outlet(jack) if song.module(jack.node).is_some() => {
                    Edit::Arm(ModSourceRef::Id(jack.node))
                }
                Hit::Inlet(jack) => {
                    self.picking = Some(jack);
                    Edit::Pick(jack)
                }
                Hit::Node(NodeKey::Route { source, .. }) if !shift => Edit::Arm(source),
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
            Drag::Marquee { .. } | Drag::None => Edit::None,
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
            |key| canvas.offset(key),
        )
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
                Face::Box { spelling } => {
                    row.is_box = true;
                    row.spelling = spelling.into();
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
                crate::PatchWireRow {
                    d: path_commands(&wire.points).into(),
                    note: wire.note,
                    selected: canvas.wire == Some(wire.key),
                    delayed: wire.delayed,
                    mark_x: last.0,
                    mark_y: last.1,
                }
            })
            .collect();
        patch.set_nodes(ModelRc::new(VecModel::from(nodes)));
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
        let picker = canvas.picking.and_then(|inlet| {
            let node = layout.node(NodeKey::Node(inlet.node))?;
            let anchor = node.anchor(false, usize::from(inlet.port))?;
            let name = node.inlets.get(usize::from(inlet.port))?.name;
            Some((inlet, anchor, name))
        });
        match picker {
            Some((inlet, anchor, name)) => {
                let options: Vec<SharedString> = self
                    .session
                    .patch_feed_options(inlet)
                    .into_iter()
                    .map(|(_, name)| name.into())
                    .collect();
                patch.set_menu_title(format!("Feed {name} from").into());
                patch.set_menu_options(ModelRc::new(VecModel::from(options)));
                patch.set_menu_current(self.session.patch_feed_choice(inlet) as i32);
                patch.set_menu_x(anchor.x);
                patch.set_menu_y(anchor.y);
                patch.set_menu_open(true);
            }
            None => patch.set_menu_open(false),
        }
        patch.set_empty(layout.nodes.is_empty());
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
            let picked = {
                let mut state = st.borrow_mut();
                let inlet = state.patch_canvas.picking.take();
                inlet.and_then(|inlet| {
                    let feed = usize::try_from(index)
                        .ok()
                        .and_then(|index| {
                            state
                                .session
                                .patch_feed_options(inlet)
                                .into_iter()
                                .nth(index)
                        })?
                        .0;
                    Some((inlet, feed))
                })
            };
            let Some((inlet, feed)) = picked else {
                st.borrow_mut().publish_patch_canvas(&window);
                return;
            };
            let before = project_snapshot(&st.borrow(), &window);
            let changed = {
                let mut state = st.borrow_mut();
                let changed = state.session.feed_patch_inlet(inlet, feed);
                if changed {
                    state.modulation_edited(&window);
                } else {
                    state.publish_patch_canvas(&window);
                }
                changed
            };
            if changed {
                let label = if feed == PatchFeed::None {
                    "Patch wire removed"
                } else {
                    "Patch wire"
                };
                record_project_history(&commands, before, &st, &window, label);
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
    use mooloop_core::{ChannelId, ModLfoParams, ModMathParams, ModulatorKind, SongModule, Wire};

    fn module(id: u32, params: ModulatorParams, at: (i32, i32)) -> SongModule {
        SongModule {
            id: ModSourceId(id),
            name: String::new(),
            seed: id,
            at: CanvasPoint::new(at.0, at.1),
            open: false,
            rack: None,
            params,
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
            |_| RouteLabel {
                name: "Cutoff".into(),
                device: "ML-M1 9".into(),
            },
            |_| false,
            |key| canvas.offset(key),
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
    fn boxes_spell_what_they_do() {
        let math = |op, operand| {
            spelling(&ModulatorParams::Math(ModMathParams {
                op,
                operand,
                ..ModMathParams::default()
            }))
        };
        assert_eq!(math(ModMathOp::Multiply, -0.5), "* -0.5");
        assert_eq!(math(ModMathOp::Add, 1.0), "+ 1");
        assert_eq!(math(ModMathOp::Min, 0.25), "min 0.25");
        assert_eq!(math(ModMathOp::Divide, -0.0001), "/ 0");
        assert_eq!(
            spelling(&ModulatorParams::Lfo(ModLfoParams::default())),
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
            Edit::Pick(Jack::new(ModSourceId(0), 0))
        );
        assert_eq!(canvas.picking, Some(Jack::new(ModSourceId(0), 0)));
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
