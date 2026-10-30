//! Symbol drawing tools as a small state machine, adapted from Solidcraft's
//! sketch tools (`crates/ui-egui/src/tools.rs`, MIT OR Apache-2.0; see
//! ATTRIBUTION.md): clicked points collect until the n-th completes the
//! shape, a line chains from its last end, Esc or a right click ends the
//! chain and then the tool, the tool stays armed after a shape, and a
//! centre-point arc runs the way the cursor went round its centre
//! (`follow_arc_turn`). Points are millimetres in the unit frame; snapping
//! happens before a point reaches the tool. Nothing here touches the
//! document: a finished gesture returns what to add, and an unfinished
//! shape (a polyline mid-chain) never enters the working copy.
use pcbcraft_core::Point;
use pcbcraft_core::geometry::{Shape, distance, fit_corner_radii};
use pcbcraft_core::library::graphics::MIN_RADIUS_MM;

/// One ribbon tool of the Symbol view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Line,
    Polyline,
    Rect,
    RoundedRect,
    Circle,
    Arc,
    Polygon,
    Text,
    Pin,
    Field,
}

/// A keyboard shortcut: the key and whether Shift is held. Chosen against
/// Solidcraft's keymap and the schematic's keys; see docs/editor-evidence.md.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub key: egui::Key,
    pub shift: bool,
}

const fn key(key: egui::Key) -> Option<Key> {
    Some(Key { key, shift: false })
}
const fn shift(key: egui::Key) -> Option<Key> {
    Some(Key { key, shift: true })
}

impl Tool {
    /// Draw tools, in ribbon order.
    pub const DRAW: [Tool; 8] = [
        Tool::Line,
        Tool::Polyline,
        Tool::Rect,
        Tool::RoundedRect,
        Tool::Circle,
        Tool::Arc,
        Tool::Polygon,
        Tool::Text,
    ];
    /// Pin tools, in ribbon order.
    pub const PINS: [Tool; 2] = [Tool::Pin, Tool::Field];
    pub const ALL: [Tool; 11] = [
        Tool::Select,
        Tool::Line,
        Tool::Polyline,
        Tool::Rect,
        Tool::RoundedRect,
        Tool::Circle,
        Tool::Arc,
        Tool::Polygon,
        Tool::Text,
        Tool::Pin,
        Tool::Field,
    ];

    /// The ribbon tool ID.
    pub fn id(self) -> &'static str {
        match self {
            Self::Select => "draw-select",
            Self::Line => "draw-line",
            Self::Polyline => "draw-polyline",
            Self::Rect => "draw-rect",
            Self::RoundedRect => "draw-rounded-rect",
            Self::Circle => "draw-circle",
            Self::Arc => "draw-arc",
            Self::Polygon => "draw-polygon",
            Self::Text => "draw-text",
            Self::Pin => "draw-pin",
            Self::Field => "draw-field",
        }
    }
    pub fn from_id(id: &str) -> Option<Tool> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Line => "Line",
            Self::Polyline => "Polyline",
            Self::Rect => "Rectangle",
            Self::RoundedRect => "Rounded",
            Self::Circle => "Circle",
            Self::Arc => "Arc",
            Self::Polygon => "Polygon",
            Self::Text => "Text",
            Self::Pin => "Pin",
            Self::Field => "Field",
        }
    }
    /// The Solidcraft icon master drawn on the ribbon button.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Line => "line",
            Self::Polyline => "trace",
            Self::Rect => "rect",
            Self::RoundedRect => "rect_rounded",
            Self::Circle => "circle",
            Self::Arc => "arc_center",
            Self::Polygon => "polygon",
            Self::Text => "text",
            Self::Pin => "point",
            Self::Field => "params",
        }
    }
    pub fn key(self) -> Option<Key> {
        use egui::Key as K;
        match self {
            Self::Select => None,
            Self::Line => key(K::L),
            Self::Polyline => shift(K::L),
            Self::Rect => key(K::R),
            Self::RoundedRect => None,
            Self::Circle => key(K::C),
            Self::Arc => key(K::A),
            Self::Polygon => shift(K::P),
            Self::Text => shift(K::T),
            Self::Pin => key(K::N),
            Self::Field => shift(K::F),
        }
    }
    pub fn tip(self) -> &'static str {
        match self {
            Self::Select => "Select, box-select and drag (Esc)",
            Self::Line => "Line: two clicks; chains from its end (L)",
            Self::Polyline => {
                "Polyline: one multi-segment line; Enter, double-click or Finish ends it (Shift+L)"
            }
            Self::Rect => "Rectangle from two corners (R)",
            Self::RoundedRect => "Rectangle with rounded corners; set the radius in Properties",
            Self::Circle => "Circle: centre, then radius (C)",
            Self::Arc => "Arc: centre, start, end; runs the way the pointer turns (A)",
            Self::Polygon => {
                "Closed polygon: click the first point, press Enter or double-click (Shift+P)"
            }
            Self::Text => "Text: click to place, then type in Properties (Shift+T)",
            Self::Pin => "Pin: place an unplaced terminal's connection end, or a new one (N)",
            Self::Field => "Field: place designator, name or a field value (Shift+F)",
        }
    }
    /// Whether the tool collects points until finished explicitly.
    pub fn is_chain(self) -> bool {
        matches!(self, Self::Polyline | Self::Polygon)
    }
}

/// What a finished gesture adds.
#[derive(Clone, Debug, PartialEq)]
pub enum Placed {
    Shape(Shape),
    /// Literal text anchored here.
    Text(Point),
    /// A pin's connection end.
    Pin(Point),
    /// A field placeholder anchored here.
    Field(Point),
}

/// One click on the canvas, already snapped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub at: Point,
    /// How close a click must come to a polygon's first point to close it.
    pub tolerance_mm: f64,
}

/// The armed tool and its pending points.
#[derive(Clone, Debug)]
pub struct Drawing {
    tool: Tool,
    /// The last drawing tool armed, for Shift+R.
    last: Option<Tool>,
    points: Vec<Point>,
    /// Signed degrees the pointer went round an arc's centre since its
    /// start was placed.
    arc_turn: f64,
    /// Corner radius of the Rounded rectangle tool, mm.
    rounded_radius: f64,
    /// Corner radius of the Polyline tool, mm; sharp by default.
    polyline_radius: f64,
    /// Corner radius of the Polygon tool, mm; sharp by default.
    polygon_radius: f64,
}

impl Default for Drawing {
    fn default() -> Self {
        Self {
            tool: Tool::Select,
            last: None,
            points: Vec::new(),
            arc_turn: 0.0,
            rounded_radius: ROUNDED_RADIUS_MM,
            polyline_radius: 0.0,
            polygon_radius: 0.0,
        }
    }
}

/// Default radius of the Rounded rectangle tool, mm.
pub const ROUNDED_RADIUS_MM: f64 = 0.5;

fn angle(center: Point, p: Point) -> f64 {
    (p[1] - center[1]).atan2(p[0] - center[0]).to_degrees()
}

fn same(a: Point, b: Point) -> bool {
    distance(a, b) <= 1e-9
}

impl Drawing {
    pub fn tool(&self) -> Tool {
        self.tool
    }
    pub fn pending(&self) -> &[Point] {
        &self.points
    }
    /// The armed tool's own corner radius, mm, when it rounds corners.
    /// Each tool keeps its own, so rounding one never carries to another.
    pub fn corner_radius(&self) -> Option<f64> {
        match self.tool {
            Tool::RoundedRect => Some(self.rounded_radius),
            Tool::Polyline => Some(self.polyline_radius),
            Tool::Polygon => Some(self.polygon_radius),
            _ => None,
        }
    }
    /// The armed tool's corner radius for editing, when it has one.
    pub fn corner_radius_mut(&mut self) -> Option<&mut f64> {
        match self.tool {
            Tool::RoundedRect => Some(&mut self.rounded_radius),
            Tool::Polyline => Some(&mut self.polyline_radius),
            Tool::Polygon => Some(&mut self.polygon_radius),
            _ => None,
        }
    }
    /// Whether an unfinished shape is in progress.
    pub fn busy(&self) -> bool {
        !self.points.is_empty()
    }

    /// Arm `tool`, dropping any unfinished shape.
    pub fn arm(&mut self, tool: Tool) {
        if tool != Tool::Select {
            self.last = Some(tool);
        }
        self.tool = tool;
        self.points.clear();
        self.arc_turn = 0.0;
    }

    /// Arm the last drawing tool again (Shift+R, Solidcraft's repeat).
    pub fn repeat(&mut self) -> bool {
        match self.last {
            Some(tool) => {
                self.arm(tool);
                true
            }
            None => false,
        }
    }

    /// Esc or right click: end the chain, or the tool when nothing is
    /// pending. Returns whether anything changed.
    pub fn cancel(&mut self) -> bool {
        if self.busy() {
            self.points.clear();
            self.arc_turn = 0.0;
            true
        } else if self.tool != Tool::Select {
            self.tool = Tool::Select;
            true
        } else {
            false
        }
    }

    /// Cmd+Z while drawing: drop the last pending point. Returns whether a
    /// point was dropped (otherwise undo goes to the document).
    pub fn undo_point(&mut self) -> bool {
        let popped = self.points.pop().is_some();
        if popped && self.points.len() < 2 {
            self.arc_turn = 0.0;
        }
        popped
    }

    /// Follow the pointer round an arc's centre, so the arc runs the way
    /// the hand went; each step is under half a turn and a full turn wraps.
    pub fn hover(&mut self, p: Point) {
        if self.tool != Tool::Arc || self.points.len() != 2 {
            self.arc_turn = 0.0;
            return;
        }
        let (c, a) = (self.points[0], self.points[1]);
        if distance(c, p) <= 1e-9 || distance(c, a) <= 1e-9 {
            return;
        }
        let at = angle(c, a) + self.arc_turn;
        let step = (angle(c, p) - at + 180.0).rem_euclid(360.0) - 180.0;
        if step.is_finite() {
            self.arc_turn = (self.arc_turn + step) % 360.0;
        }
    }

    /// The arc sweep from the start to `end`: counter-clockwise unless the
    /// pointer went round clockwise.
    fn arc_sweep(&self, end: Point) -> f64 {
        let (c, a) = (self.points[0], self.points[1]);
        let ccw = (angle(c, end) - angle(c, a)).rem_euclid(360.0);
        let ccw = if ccw <= 1e-9 { 360.0 } else { ccw };
        if self.arc_turn < 0.0 {
            ccw - 360.0
        } else {
            ccw
        }
    }

    /// Corner radii for a path drawn with the tool's corner radius,
    /// reduced to what fits.
    fn radii(&self, points: &[Point], closed: bool) -> Vec<f64> {
        let radius = self.corner_radius().unwrap_or(0.0);
        if radius <= 0.0 || !radius.is_finite() {
            return vec![];
        }
        fit_corner_radii(points, &vec![radius; points.len()], closed)
    }

    fn rect(&self, a: Point, b: Point) -> Shape {
        let radius = if self.tool == Tool::RoundedRect {
            let half = (a[0] - b[0]).abs().min((a[1] - b[1]).abs()) / 2.0;
            let radius = self.rounded_radius;
            if radius.is_finite() {
                radius.min(half * (1.0 - 1e-9)).max(0.0)
            } else {
                0.0
            }
        } else {
            0.0
        };
        Shape::Rect { a, b, radius }
    }

    /// A click. Returns the shape or placement it finished, if any.
    pub fn click(&mut self, click: Click) -> Option<Placed> {
        let p = click.at;
        match self.tool {
            Tool::Select => None,
            Tool::Text => Some(Placed::Text(p)),
            Tool::Pin => Some(Placed::Pin(p)),
            Tool::Field => Some(Placed::Field(p)),
            Tool::Line => match self.points.first().copied() {
                None => {
                    self.points.push(p);
                    None
                }
                Some(a) if same(a, p) => None,
                Some(a) => {
                    // Chain: the end starts the next segment.
                    self.points = vec![p];
                    Some(Placed::Shape(Shape::Line { a, b: p }))
                }
            },
            Tool::Rect | Tool::RoundedRect => match self.points.first().copied() {
                None => {
                    self.points.push(p);
                    None
                }
                Some(a) if (a[0] - p[0]).abs() <= 1e-9 || (a[1] - p[1]).abs() <= 1e-9 => None,
                Some(a) => {
                    self.points.clear();
                    Some(Placed::Shape(self.rect(a, p)))
                }
            },
            Tool::Circle => match self.points.first().copied() {
                None => {
                    self.points.push(p);
                    None
                }
                Some(c) if distance(c, p) < MIN_RADIUS_MM => None,
                Some(c) => {
                    self.points.clear();
                    Some(Placed::Shape(Shape::Circle {
                        center: c,
                        radius: distance(c, p),
                    }))
                }
            },
            Tool::Arc => match self.points.len() {
                0 => {
                    self.points.push(p);
                    None
                }
                1 if distance(self.points[0], p) < MIN_RADIUS_MM => None,
                1 => {
                    self.points.push(p);
                    self.arc_turn = 0.0;
                    None
                }
                _ => {
                    let (c, a) = (self.points[0], self.points[1]);
                    if distance(c, p) <= 1e-9 {
                        return None;
                    }
                    let shape = Shape::Arc {
                        center: c,
                        radius: distance(c, a),
                        start_deg: angle(c, a),
                        sweep_deg: self.arc_sweep(p),
                    };
                    self.points.clear();
                    self.arc_turn = 0.0;
                    Some(Placed::Shape(shape))
                }
            },
            Tool::Polyline => {
                if self.points.last().is_none_or(|q| !same(*q, p)) {
                    self.points.push(p);
                }
                None
            }
            Tool::Polygon => {
                if self.points.len() >= 3
                    && self
                        .points
                        .first()
                        .is_some_and(|first| distance(*first, p) <= click.tolerance_mm)
                {
                    return self.finish();
                }
                if self.points.last().is_none_or(|q| !same(*q, p)) {
                    self.points.push(p);
                }
                None
            }
        }
    }

    /// Enter, a double click or the Finish button: complete a polyline or
    /// polygon from the points so far. Too few points finish nothing.
    pub fn finish(&mut self) -> Option<Placed> {
        let (needed, closed) = match self.tool {
            Tool::Polyline => (2, false),
            Tool::Polygon => (3, true),
            _ => return None,
        };
        if self.points.len() < needed {
            return None;
        }
        let points = std::mem::take(&mut self.points);
        let radii = self.radii(&points, closed);
        Some(Placed::Shape(if closed {
            Shape::Polygon { points, radii }
        } else {
            Shape::Polyline { points, radii }
        }))
    }

    /// What the tool would draw with the pointer at `cursor`: the rubber
    /// band, never part of the document.
    pub fn preview(&self, cursor: Point) -> Option<Shape> {
        let first = *self.points.first()?;
        match self.tool {
            Tool::Line => Some(Shape::Line {
                a: first,
                b: cursor,
            }),
            Tool::Rect | Tool::RoundedRect => Some(self.rect(first, cursor)),
            Tool::Circle => Some(Shape::Circle {
                center: first,
                radius: distance(first, cursor),
            }),
            Tool::Arc if self.points.len() == 1 => Some(Shape::Line {
                a: first,
                b: cursor,
            }),
            Tool::Arc => {
                let a = self.points[1];
                Some(Shape::Arc {
                    center: first,
                    radius: distance(first, a),
                    start_deg: angle(first, a),
                    sweep_deg: self.arc_sweep(cursor),
                })
            }
            Tool::Polyline | Tool::Polygon => {
                let mut points = self.points.clone();
                if points.last().is_none_or(|q| !same(*q, cursor)) {
                    points.push(cursor);
                }
                let closed = self.tool == Tool::Polygon && points.len() >= 3;
                let radii = self.radii(&points, closed);
                Some(if closed {
                    Shape::Polygon { points, radii }
                } else {
                    Shape::Polyline { points, radii }
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(d: &mut Drawing, at: Point) -> Option<Placed> {
        d.click(Click {
            at,
            tolerance_mm: 0.5,
        })
    }

    #[test]
    fn lines_chain_one_segment_per_click_until_escape() {
        let mut d = Drawing::default();
        d.arm(Tool::Line);
        assert_eq!(click(&mut d, [0.0, 0.0]), None);
        assert_eq!(
            click(&mut d, [5.0, 0.0]),
            Some(Placed::Shape(Shape::Line {
                a: [0.0, 0.0],
                b: [5.0, 0.0]
            }))
        );
        // The end starts the next segment.
        assert_eq!(d.pending(), &[[5.0, 0.0]]);
        assert!(
            matches!(click(&mut d, [5.0, 5.0]), Some(Placed::Shape(Shape::Line { a, .. })) if a == [5.0, 0.0])
        );
        assert!(d.cancel(), "Esc ends the chain");
        assert_eq!(d.tool(), Tool::Line, "the tool stays armed");
        assert!(d.cancel(), "a second Esc ends the tool");
        assert_eq!(d.tool(), Tool::Select);
        assert!(d.repeat(), "Shift+R arms the last tool");
        assert_eq!(d.tool(), Tool::Line);
    }

    #[test]
    fn a_polyline_is_finished_only_explicitly_and_cmd_z_pops_points() -> Result<(), String> {
        let mut d = Drawing::default();
        d.arm(Tool::Polyline);
        if let Some(r) = d.corner_radius_mut() {
            *r = 1.0;
        }
        for p in [[0.0, 0.0], [5.0, 0.0], [5.0, 5.0], [5.0, 5.0]] {
            assert_eq!(click(&mut d, p), None, "no commit mid-chain");
        }
        assert_eq!(d.pending().len(), 3, "a repeated click adds no point");
        assert!(d.undo_point());
        assert_eq!(d.pending().len(), 2);
        click(&mut d, [10.0, 5.0]);
        let Some(Placed::Shape(Shape::Polyline { points, radii })) = d.finish() else {
            return Err("finished polyline".into());
        };
        assert_eq!(points.len(), 3);
        // Interior corner rounded, open ends sharp.
        assert_eq!(radii.len(), 3);
        assert!(radii[0] == 0.0 && radii[2] == 0.0 && radii[1] > 0.99);
        assert!(!d.busy() && d.tool() == Tool::Polyline);
        assert_eq!(d.finish(), None, "nothing pending finishes nothing");
        Ok(())
    }

    #[test]
    fn polygons_close_on_the_first_point() {
        let mut d = Drawing::default();
        d.arm(Tool::Polygon);
        click(&mut d, [0.0, 0.0]);
        click(&mut d, [5.0, 0.0]);
        // Too few points to close: the click is a new point.
        assert_eq!(click(&mut d, [0.1, 0.1]), None);
        d.undo_point();
        click(&mut d, [2.5, 5.0]);
        let placed = click(&mut d, [0.2, 0.1]);
        assert!(
            matches!(&placed, Some(Placed::Shape(Shape::Polygon { points, radii })) if points.len() == 3 && radii.is_empty()),
            "{placed:?}"
        );
    }

    #[test]
    fn each_tool_keeps_its_own_corner_radius() -> Result<(), String> {
        let mut d = Drawing::default();
        d.arm(Tool::Rect);
        assert_eq!(d.corner_radius(), None, "Rect offers no radius");
        d.arm(Tool::RoundedRect);
        // The radius row edits the value the committed shape uses.
        if let Some(r) = d.corner_radius_mut() {
            *r = 0.75;
        }
        click(&mut d, [0.0, 0.0]);
        assert_eq!(
            click(&mut d, [4.0, 3.0]),
            Some(Placed::Shape(Shape::Rect {
                a: [0.0, 0.0],
                b: [4.0, 3.0],
                radius: 0.75
            }))
        );
        // Rounding a rectangle does not round a polyline drawn after it.
        d.arm(Tool::Polyline);
        assert_eq!(d.corner_radius(), Some(0.0));
        for p in [[0.0, 0.0], [5.0, 0.0], [5.0, 5.0]] {
            click(&mut d, p);
        }
        let Some(Placed::Shape(Shape::Polyline { radii, .. })) = d.finish() else {
            return Err("finished polyline".into());
        };
        assert!(radii.is_empty(), "sharp polyline, got {radii:?}");
        d.arm(Tool::Polygon);
        assert_eq!(d.corner_radius(), Some(0.0));
        d.arm(Tool::RoundedRect);
        assert_eq!(d.corner_radius(), Some(0.75), "the radius is kept");
        Ok(())
    }

    #[test]
    fn rectangles_circles_and_rounded_rectangles_take_two_clicks() {
        let mut d = Drawing::default();
        d.arm(Tool::RoundedRect);
        assert_eq!(d.corner_radius(), Some(ROUNDED_RADIUS_MM));
        click(&mut d, [0.0, 0.0]);
        assert_eq!(
            click(&mut d, [0.0, 4.0]),
            None,
            "a flat rectangle is refused"
        );
        assert_eq!(
            click(&mut d, [4.0, 2.0]),
            Some(Placed::Shape(Shape::Rect {
                a: [0.0, 0.0],
                b: [4.0, 2.0],
                radius: 0.5
            }))
        );
        d.arm(Tool::Rect);
        click(&mut d, [0.0, 0.0]);
        assert_eq!(
            click(&mut d, [1.0, 1.0]),
            Some(Placed::Shape(Shape::rect([0.0, 0.0], [1.0, 1.0])))
        );
        d.arm(Tool::Circle);
        click(&mut d, [1.0, 1.0]);
        assert_eq!(
            click(&mut d, [4.0, 5.0]),
            Some(Placed::Shape(Shape::Circle {
                center: [1.0, 1.0],
                radius: 5.0
            }))
        );
    }

    #[test]
    fn arcs_run_the_way_the_pointer_turned() -> Result<(), String> {
        let mut d = Drawing::default();
        d.arm(Tool::Arc);
        click(&mut d, [0.0, 0.0]);
        click(&mut d, [2.0, 0.0]);
        // Clockwise round to the top: −270°.
        for p in [[1.0, -1.0], [0.0, -2.0], [-2.0, 0.0], [0.0, 2.0]] {
            d.hover(p);
        }
        let Some(Placed::Shape(Shape::Arc {
            radius, sweep_deg, ..
        })) = click(&mut d, [0.0, 2.0])
        else {
            return Err("arc".into());
        };
        assert_eq!(radius, 2.0);
        assert!((sweep_deg + 270.0).abs() < 1e-9, "{sweep_deg}");
        // Counter-clockwise: +90°.
        click(&mut d, [0.0, 0.0]);
        click(&mut d, [2.0, 0.0]);
        d.hover([1.0, 1.0]);
        let Some(Placed::Shape(Shape::Arc { sweep_deg, .. })) = click(&mut d, [0.0, 2.0]) else {
            return Err("arc".into());
        };
        assert!((sweep_deg - 90.0).abs() < 1e-9, "{sweep_deg}");
        Ok(())
    }

    #[test]
    fn tools_have_distinct_ids_keys_and_icons() {
        let mut keys = std::collections::BTreeSet::new();
        for tool in Tool::ALL {
            assert_eq!(Tool::from_id(tool.id()), Some(tool));
            assert!(!tool.icon().is_empty() && !tool.label().is_empty());
            if let Some(k) = tool.key() {
                assert!(keys.insert(format!("{k:?}")), "{tool:?} shares a key");
            }
        }
    }
}
