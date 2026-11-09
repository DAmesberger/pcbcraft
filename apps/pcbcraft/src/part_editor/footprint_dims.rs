//! On-canvas dimensions and constraints of the Footprint sketch (slice F4):
//! driving dimensions bound to named parameters through Solidcraft's
//! `Constraint.param`, inline value editing, constraint glyphs, under- and
//! over-constrained feedback and the drag preview. Reads the sketch through
//! `pcbcraft_solidcraft::footprint::view`; every change is one
//! `library.edit_footprint` commit.
//!
//! See docs/component-model.md (F4/F5 as built). This module holds the pure parts (picking, what a pick sequence
//! dimensions or constrains, labels, measurements and the loaded-sketch
//! cache); `footprint_dims_ui` draws and drives them on the canvas.
use pcbcraft_core::Point;
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintEdit, FootprintSource, GeomConstraint, PackageValue,
    ParamName, ParamUnit, SketchConstraintId, SketchCurveId, SketchEdit, SketchPointId, SketchRef,
    SolveStatus,
};
use pcbcraft_solidcraft::footprint::view::{
    self as sketch, LiveSketch, SketchView, SolveReport, ViewConstraint, ViewGeometry,
};

/// What the author typed into a dimension's value field.
#[derive(Clone, Debug, PartialEq)]
pub enum DimInput {
    /// A plain number in the dimension's unit (mm or degrees).
    Value(f64),
    /// An existing parameter by name.
    Param(ParamName),
    /// An expression (`2*pitch`); it becomes a new parameter named after
    /// the dimension, since a bound dimension names one parameter.
    Expr(Expr),
    /// `name = expr`: define (or redefine) a parameter and bind to it.
    Define { name: ParamName, expr: Expr },
}

/// Parse the value field: a number, a bare parameter name, `name = expr`,
/// or any other expression. The grammar itself is the evaluator's; this
/// only splits the forms.
pub fn parse_input(text: &str) -> Result<DimInput, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type a value, a parameter or name = expression".into());
    }
    if let Some((name, expr)) = text.split_once('=') {
        let name = ParamName(name.trim().into());
        let expr = Expr(expr.trim().into());
        if !name.valid() {
            return Err(format!("{} is not a parameter name", name.0));
        }
        if name.is_package() {
            return Err("Package values (pkg_…) are read-only".into());
        }
        if !expr.valid() {
            return Err("The expression is empty or too long".into());
        }
        return Ok(DimInput::Define { name, expr });
    }
    if let Ok(v) = text.parse::<f64>() {
        return if v.is_finite() {
            Ok(DimInput::Value(v))
        } else {
            Err("The value must be finite".into())
        };
    }
    let name = ParamName(text.into());
    if name.valid() {
        return Ok(DimInput::Param(name));
    }
    let expr = Expr(text.into());
    if expr.valid() {
        Ok(DimInput::Expr(expr))
    } else {
        Err("The expression is empty or too long".into())
    }
}

/// The edits that set dimension `constraint` from `input`; `fallback` names
/// the parameter an anonymous expression creates.
pub fn input_edits(
    constraint: &SketchConstraintId,
    kind: DimKind,
    input: DimInput,
    fallback: ParamName,
) -> Vec<FootprintEdit> {
    let unit = if kind == DimKind::Angle {
        ParamUnit::Deg
    } else {
        ParamUnit::Mm
    };
    let set = |value| {
        FootprintEdit::Sketch(SketchEdit::SetDimension {
            constraint: constraint.clone(),
            value,
        })
    };
    match input {
        DimInput::Value(v) => vec![set(DimValue::Fixed(v))],
        DimInput::Param(name) => vec![set(DimValue::Param(name))],
        DimInput::Expr(expr) => vec![
            FootprintEdit::SetParameter {
                name: fallback.clone(),
                expr,
                unit,
            },
            set(DimValue::Param(fallback)),
        ],
        DimInput::Define { name, expr } => vec![
            FootprintEdit::SetParameter {
                name: name.clone(),
                expr,
                unit,
            },
            set(DimValue::Param(name)),
        ],
    }
}

/// A dimension label being edited inline (double-click on its label).
#[derive(Clone, Debug, PartialEq)]
pub struct InlineEdit {
    pub constraint: SketchConstraintId,
    pub kind: DimKind,
    pub text: String,
    /// The parse error shown under the field.
    pub error: Option<String>,
}

/// A new dimension waiting for its value (after its picks were placed).
#[derive(Clone, Debug, PartialEq)]
pub struct NewDimension {
    pub kind: DimKind,
    pub refs: Vec<SketchRef>,
    /// Where the value field sits, in footprint millimetres.
    pub at: Point,
    pub text: String,
    pub error: Option<String>,
}

/// The value field open on the canvas: an existing dimension's, or a new
/// one's.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueField {
    Existing(InlineEdit),
    New(NewDimension),
}

impl ValueField {
    pub fn text_mut(&mut self) -> &mut String {
        match self {
            Self::Existing(e) => &mut e.text,
            Self::New(n) => &mut n.text,
        }
    }
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Existing(e) => e.error.as_deref(),
            Self::New(n) => n.error.as_deref(),
        }
    }
    pub fn set_error(&mut self, error: Option<String>) {
        match self {
            Self::Existing(e) => e.error = error,
            Self::New(n) => n.error = error,
        }
    }
}

/// A sketch point being dragged: the live (uncommitted) sketch of the last
/// frame, which the next frame continues from.
#[derive(Clone, Debug)]
pub struct PointDrag {
    pub point: SketchPointId,
    pub live: LiveSketch,
    pub view: SketchView,
    /// The dragged point's position after the last frame.
    pub at: Point,
    pub moved: bool,
}

/// Drag-preview frame times, for the status line and the 16 ms target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameTimes {
    pub frames: u32,
    pub last_ms: f64,
    pub max_ms: f64,
}

impl FrameTimes {
    pub fn add(&mut self, ms: f64) {
        self.frames = self.frames.saturating_add(1);
        self.last_ms = ms;
        self.max_ms = self.max_ms.max(ms);
    }
}

/// What the canvas shows of the loaded sketch, recomputed only when the
/// source or the package values change (never per frame).
#[derive(Clone, Debug)]
pub struct Loaded {
    pub source: FootprintSource,
    pub package: Vec<PackageValue>,
    /// `Err` when the parameters do not evaluate: the view then shows the
    /// stored positions, and drags are off.
    pub live: Result<LiveSketch, String>,
    pub view: SketchView,
    pub report: Option<SolveReport>,
    /// The parameters panel's rows and per-parameter errors.
    pub rows: Vec<super::footprint_params::ParamRow>,
    pub errors: std::collections::BTreeMap<ParamName, String>,
}

impl Loaded {
    pub fn load(source: &FootprintSource, package: &[PackageValue]) -> Self {
        let live = LiveSketch::load(source, package).map_err(|e| e.to_string());
        let view = match &live {
            Ok(l) => l.view(),
            Err(_) => sketch::sketch_view(source).unwrap_or_default(),
        };
        let report = live.as_ref().ok().map(LiveSketch::report);
        Self {
            source: source.clone(),
            package: package.to_vec(),
            live,
            view,
            report,
            rows: super::footprint_params::rows(Some(source), package),
            errors: super::footprint_params::errors(source, package),
        }
    }

    pub fn is_for(&self, source: &FootprintSource, package: &[PackageValue]) -> bool {
        self.source == *source && self.package == package
    }
}

/// The Footprint tab's dimension and constraint state.
#[derive(Clone, Debug, Default)]
pub struct DimsState {
    pub loaded: Option<Loaded>,
    /// Entities picked so far by the Dimension or a Constrain tool.
    pub picks: Vec<SketchRef>,
    /// The value field open on the canvas.
    pub field: Option<ValueField>,
    /// Frames left in which the field takes the keyboard (the click that
    /// opened it must not close it).
    pub fresh: u8,
    pub drag: Option<PointDrag>,
    pub times: FrameTimes,
    /// The selected dimension or constraint (Delete removes it).
    pub selected: Option<SketchConstraintId>,
    /// A parameter the panel highlights the readers of.
    pub highlight: Option<ParamName>,
    /// Dimension labels and constraint glyphs as drawn last frame (screen
    /// rectangles), for picking them on the next.
    pub labels: Vec<(SketchConstraintId, egui::Rect)>,
    pub glyphs: Vec<(SketchConstraintId, egui::Rect)>,
    /// The parameters panel's text buffers.
    pub panel: super::footprint_params_ui::Panel,
    /// How many dimensions the canvas labels.
    pub detail: DimDetail,
    /// The constraint tool the Constrain split button shows.
    pub last_constraint: Option<super::footprint_tools::FootprintTool>,
}

/// The ribbon toggle between [`DimDetail::Key`] and [`DimDetail::All`].
pub const ALL_DIMS_TOOL: &str = "fp-dims-all";

/// How many dimensions the canvas labels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DimDetail {
    /// The independent parameters' dimensions, each parameter once, the
    /// unbound driving ones, and everything the selection touches.
    #[default]
    Key,
    /// Every dimension, driven and derived included.
    All,
}

/// What the canvas labels whatever the detail: the selected dimension,
/// the readers of a highlighted parameter and the dimensions touching
/// the selected sketch entities.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DimFocus {
    pub constraints: Vec<SketchConstraintId>,
    pub points: Vec<SketchPointId>,
    pub curves: Vec<SketchCurveId>,
}

/// The dimensions the canvas labels, in sketch order. At
/// [`DimDetail::Key`] a dimension bound to an expression (a pad's corner
/// radius) or to a derived parameter (one reading other parameters, as a
/// template's seed and paste-grid positions) is left out, and each
/// parameter is labelled once, unless the focus touches it.
pub fn shown_dimensions(
    view: &SketchView,
    source: &FootprintSource,
    detail: DimDetail,
    focus: &DimFocus,
) -> Vec<SketchConstraintId> {
    let dimensions = view.constraints.iter().filter(|k| k.value.is_some());
    if detail == DimDetail::All {
        return dimensions.map(|k| k.id.clone()).collect();
    }
    let independent = |name: &str| {
        source
            .parameters
            .iter()
            .find(|p| p.name.0 == name)
            .is_none_or(|p| {
                !source
                    .parameters
                    .iter()
                    .any(|q| q.name != p.name && mentions(&p.expr.0, &q.name.0))
            })
    };
    let mut seen = std::collections::BTreeSet::new();
    dimensions
        .filter(|k| {
            let focused = focus.constraints.contains(&k.id)
                || k.points.iter().any(|p| focus.points.contains(p))
                || k.curves.iter().any(|c| focus.curves.contains(c));
            if focused {
                return true;
            }
            if k.driven {
                return false;
            }
            match &k.param {
                None => true,
                Some(param) if ParamName(param.clone()).valid() => {
                    independent(param) && seen.insert(param.clone())
                }
                Some(_) => false,
            }
        })
        .map(|k| k.id.clone())
        .collect()
}

/// Whether the canvas draws constraint `k`'s glyph. At
/// [`DimDetail::Key`] the glyphs of a pad's own template, drill and paste
/// curves are left out unless the focus touches them; the author's own
/// sketch constraints always show.
pub fn shows_glyph(
    k: &ViewConstraint,
    source: &FootprintSource,
    detail: DimDetail,
    focus: &DimFocus,
) -> bool {
    use pcbcraft_core::library::footprint_source::{PadGeometry, PasteRuleExpr};
    if detail == DimDetail::All
        || focus.constraints.contains(&k.id)
        || k.points.iter().any(|p| focus.points.contains(p))
        || k.curves.iter().any(|c| focus.curves.contains(c))
        || k.curves.is_empty()
    {
        return true;
    }
    let owned = |c: &SketchCurveId| {
        source.pads.iter().any(|r| {
            matches!(&r.geometry, PadGeometry::Template { owned_curves, .. } if owned_curves.contains(c))
                || r.hole.as_ref().is_some_and(|h| h.curves.contains(c))
                || matches!(&r.paste, PasteRuleExpr::Windows(w) if w.contains(c))
        })
    };
    !k.curves.iter().all(owned)
}

/// Whether `expr` names the identifier `name` (whole words only).
fn mentions(expr: &str, name: &str) -> bool {
    expr.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|w| w == name)
}

impl DimsState {
    /// Esc: drop the field, then the picks. True when something was dropped.
    pub fn cancel(&mut self) -> bool {
        if self.field.take().is_some() {
            return true;
        }
        if self.drag.take().is_some() {
            return true;
        }
        !std::mem::take(&mut self.picks).is_empty()
    }

    #[cfg(test)]
    pub fn panel_error(&self) -> Option<&str> {
        self.panel.error_text()
    }

    /// The live view: the drag's, else the loaded one.
    pub fn view(&self) -> Option<&SketchView> {
        self.drag
            .as_ref()
            .map(|d| &d.view)
            .or_else(|| self.loaded.as_ref().map(|l| &l.view))
    }
}

fn dist(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Distance from `p` to segment `a`–`b`.
fn segment_dist(p: Point, a: Point, b: Point) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    if l2.is_nan() || l2 <= 1e-24 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
    dist(p, [a[0] + d[0] * t, a[1] + d[1] * t])
}

/// Counter-clockwise sweep from `start` to `end` about `c`, in (0, 2π].
fn sweep(c: Point, start: Point, end: Point) -> (f64, f64) {
    let a0 = (start[1] - c[1]).atan2(start[0] - c[0]);
    let a1 = (end[1] - c[1]).atan2(end[0] - c[0]);
    let mut s = (a1 - a0).rem_euclid(std::f64::consts::TAU);
    if s <= 1e-12 {
        s = std::f64::consts::TAU;
    }
    (a0, s)
}

/// Distance from `p` to a curve's drawn geometry.
pub fn curve_distance(geometry: &ViewGeometry, p: Point) -> Option<f64> {
    match *geometry {
        ViewGeometry::Line { a, b } => Some(segment_dist(p, a, b)),
        ViewGeometry::Circle { center, radius } => Some((dist(p, center) - radius).abs()),
        ViewGeometry::Arc { center, start, end } => {
            let r = dist(start, center);
            let (a0, s) = sweep(center, start, end);
            let a = (p[1] - center[1]).atan2(p[0] - center[0]);
            let within = (a - a0).rem_euclid(std::f64::consts::TAU) <= s;
            Some(if within {
                (dist(p, center) - r).abs()
            } else {
                dist(p, start).min(dist(p, end))
            })
        }
        ViewGeometry::FreeForm { .. } => None,
    }
}

/// The sketch entity at `at`: the nearest point within `tolerance` (mm),
/// else the nearest curve within it.
pub fn pick(view: &SketchView, at: Point, tolerance: f64) -> Option<SketchRef> {
    let nearest_point = view
        .points
        .iter()
        .map(|p| (dist(p.at, at), p))
        .filter(|(d, _)| *d <= tolerance)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, p)) = nearest_point {
        return Some(SketchRef::Point(p.id.clone()));
    }
    view.curves
        .iter()
        .filter_map(|c| Some((curve_distance(&c.geometry, at)?, c)))
        .filter(|(d, _)| *d <= tolerance)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, c)| SketchRef::Curve(c.id.clone()))
}

/// A movable point at `at` (for a drag): free in the report and not fixed.
pub fn pick_free_point(
    view: &SketchView,
    report: Option<&SolveReport>,
    at: Point,
    tolerance: f64,
) -> Option<SketchPointId> {
    view.points
        .iter()
        .filter(|p| !p.fixed && p.id.0 != "origin")
        .filter(|p| report.is_none_or(|r| r.free_points.contains(&p.id)))
        .map(|p| (dist(p.at, at), p))
        .filter(|(d, _)| *d <= tolerance)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p.id.clone())
}

fn is_line(view: &SketchView, id: &SketchCurveId) -> bool {
    view.curve(id)
        .is_some_and(|c| matches!(c.geometry, ViewGeometry::Line { .. }))
}

fn is_round(view: &SketchView, id: &SketchCurveId) -> bool {
    view.curve(id).is_some_and(|c| {
        matches!(
            c.geometry,
            ViewGeometry::Circle { .. } | ViewGeometry::Arc { .. }
        )
    })
}

/// Where a dimension's picks stand.
#[derive(Clone, Debug, PartialEq)]
pub enum DimPicks {
    /// Pick more (or place a single line's length).
    More,
    /// Complete: the dimension follows the pointer until a click places it.
    /// `linear` dimensions choose aligned, horizontal or vertical from
    /// where they are placed.
    Ready { refs: Vec<SketchRef>, linear: bool },
    /// The picks do not make a dimension.
    Wrong(&'static str),
}

/// What the Dimension tool's picks make: two points or a point and a line
/// (a distance), two lines (an angle), a circle (diameter) or an arc
/// (radius). A single line waits: a second pick makes it a point–line
/// distance or an angle, a click on empty canvas places its length.
pub fn dimension_picks(view: &SketchView, picks: &[SketchRef]) -> DimPicks {
    use SketchRef::{Curve, Point as P};
    match picks {
        [] | [P(_)] => DimPicks::More,
        [Curve(c)] if is_round(view, c) => DimPicks::Ready {
            refs: picks.to_vec(),
            linear: false,
        },
        [Curve(c)] if is_line(view, c) => DimPicks::More,
        [P(_), P(_)] => DimPicks::Ready {
            refs: picks.to_vec(),
            linear: true,
        },
        [P(p), Curve(c)] | [Curve(c), P(p)] if is_line(view, c) => DimPicks::Ready {
            refs: vec![P(p.clone()), Curve(c.clone())],
            linear: false,
        },
        [Curve(a), Curve(b)] if is_line(view, a) && is_line(view, b) && a != b => DimPicks::Ready {
            refs: picks.to_vec(),
            linear: false,
        },
        _ => DimPicks::Wrong("Dimension two points, a point and a line, two lines or a curve"),
    }
}

/// The ends a linear dimension measures between.
fn linear_ends(view: &SketchView, refs: &[SketchRef]) -> Option<(Point, Point)> {
    match refs {
        [SketchRef::Point(p), SketchRef::Point(q)] => Some((view.point(p)?.at, view.point(q)?.at)),
        [SketchRef::Curve(c)] => match view.curve(c)?.geometry {
            ViewGeometry::Line { a, b } => Some((a, b)),
            _ => None,
        },
        _ => None,
    }
}

/// Aligned, horizontal or vertical, by where a linear dimension is placed
/// (as in Solidcraft): beside the measured span, it measures across it;
/// above or below, along x; left or right, along y.
pub fn linear_kind(view: &SketchView, refs: &[SketchRef], place: Point) -> DimKind {
    let Some((a, b)) = linear_ends(view, refs) else {
        return DimKind::Distance;
    };
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    let inside_x = place[0] > x0 && place[0] < x1;
    let inside_y = place[1] > y0 && place[1] < y1;
    let flat = (y1 - y0) < 1e-9;
    let upright = (x1 - x0) < 1e-9;
    match (inside_x, inside_y) {
        _ if flat || upright => DimKind::Distance,
        (true, false) => DimKind::HorizontalDistance,
        (false, true) => DimKind::VerticalDistance,
        _ => DimKind::Distance,
    }
}

/// The kind of a ready dimension placed at `place`.
pub fn dimension_kind(
    view: &SketchView,
    refs: &[SketchRef],
    linear: bool,
    place: Point,
) -> DimKind {
    if linear {
        return linear_kind(view, refs, place);
    }
    match refs {
        [SketchRef::Curve(c)] => match view.curve(c).map(|c| &c.geometry) {
            Some(ViewGeometry::Circle { .. }) => DimKind::Diameter,
            Some(ViewGeometry::Arc { .. }) => DimKind::Radius,
            _ => DimKind::Distance,
        },
        [SketchRef::Curve(_), SketchRef::Curve(_)] => DimKind::Angle,
        _ => DimKind::Distance,
    }
}

fn direction(view: &SketchView, c: &SketchCurveId) -> Option<Point> {
    match view.curve(c)?.geometry {
        ViewGeometry::Line { a, b } => {
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0].hypot(d[1]);
            (l > 1e-12).then(|| [d[0] / l, d[1] / l])
        }
        _ => None,
    }
}

/// What a new dimension measures now (mm, or degrees for angles): the
/// value its field starts with.
pub fn measure(view: &SketchView, kind: DimKind, refs: &[SketchRef]) -> Option<f64> {
    use SketchRef::{Curve, Point as P};
    match (kind, refs) {
        (DimKind::Distance, [P(p), P(q)]) => Some(dist(view.point(p)?.at, view.point(q)?.at)),
        (DimKind::HorizontalDistance | DimKind::VerticalDistance, _) => {
            let (a, b) = linear_ends(view, refs)?;
            let axis = usize::from(kind == DimKind::VerticalDistance);
            Some((b[axis] - a[axis]).abs())
        }
        (DimKind::Distance, [Curve(c)]) => {
            let (a, b) = linear_ends(view, refs)?;
            is_line(view, c).then(|| dist(a, b))
        }
        (DimKind::Distance, [P(p), Curve(c)]) => {
            let at = view.point(p)?.at;
            match view.curve(c)?.geometry {
                ViewGeometry::Line { a, b } => {
                    let d = direction(view, c)?;
                    Some(((at[0] - a[0]) * d[1] - (at[1] - a[1]) * d[0]).abs()).filter(|_| {
                        // A degenerate line has no direction.
                        dist(a, b) > 1e-12
                    })
                }
                _ => None,
            }
        }
        (DimKind::Radius | DimKind::Diameter, [Curve(c)]) => {
            let r = match view.curve(c)?.geometry {
                ViewGeometry::Circle { radius, .. } => radius,
                ViewGeometry::Arc { center, start, .. } => dist(center, start),
                _ => return None,
            };
            Some(if kind == DimKind::Diameter {
                2.0 * r
            } else {
                r
            })
        }
        (DimKind::Angle, [Curve(a), Curve(b)]) => {
            let (u, v) = (direction(view, a)?, direction(view, b)?);
            let cos = (u[0] * v[0] + u[1] * v[1]).clamp(-1.0, 1.0);
            Some(cos.acos().to_degrees())
        }
        _ => None,
    }
}

/// Where a Constrain tool's picks stand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConstrainPicks {
    More,
    Ready,
    Wrong(&'static str),
}

/// Whether `picks` complete constraint `kind` (the entity sets the
/// evaluator accepts for it).
pub fn constrain_picks(
    view: &SketchView,
    kind: GeomConstraint,
    picks: &[SketchRef],
) -> ConstrainPicks {
    use ConstrainPicks::{More, Ready, Wrong};
    use GeomConstraint as G;
    use SketchRef::{Curve, Point as P};
    let line = |c: &SketchCurveId| is_line(view, c);
    let round = |c: &SketchCurveId| is_round(view, c);
    match (kind, picks) {
        (_, []) => More,
        (G::Fixed, [P(_)]) => Ready,
        (G::Horizontal | G::Vertical, [Curve(c)]) if line(c) => Ready,
        (G::Horizontal | G::Vertical | G::Coincident, [P(_)]) => More,
        (G::Horizontal | G::Vertical | G::Coincident, [P(p), P(q)]) if p != q => Ready,
        (G::Coincident | G::PointOnCurve, [P(_), Curve(_)]) => Ready,
        (G::PointOnCurve | G::Midpoint, [P(_)]) => More,
        (G::Midpoint, [P(_), Curve(c)]) if line(c) => Ready,
        (G::Parallel | G::Perpendicular | G::Equal, [Curve(c)]) if line(c) || kind == G::Equal => {
            More
        }
        (G::Parallel | G::Perpendicular, [Curve(a), Curve(b)]) if a != b && line(a) && line(b) => {
            Ready
        }
        (G::Equal, [Curve(a), Curve(b)]) if a != b && (line(a) == line(b)) => Ready,
        (G::Tangent, [Curve(_)]) => More,
        (G::Tangent, [Curve(a), Curve(b)]) if a != b && (round(a) || round(b)) => Ready,
        (G::Concentric, [Curve(c)]) if round(c) => More,
        (G::Concentric, [Curve(a), Curve(b)]) if a != b && round(a) && round(b) => Ready,
        (G::Symmetric, [P(_)] | [P(_), P(_)]) => More,
        (G::Symmetric, [P(_), P(_), Curve(c)]) if line(c) => Ready,
        _ => Wrong(hint(kind)),
    }
}

/// What a Constrain tool takes, for the status line.
pub fn hint(kind: GeomConstraint) -> &'static str {
    use GeomConstraint as G;
    match kind {
        G::Coincident => "Coincident: two points, or a point and a curve",
        G::Horizontal => "Horizontal: a line, or two points",
        G::Vertical => "Vertical: a line, or two points",
        G::Parallel => "Parallel: two lines",
        G::Perpendicular => "Perpendicular: two lines",
        G::Equal => "Equal: two lines, or two circles or arcs",
        G::Tangent => "Tangent: a circle or arc and a line or another curve",
        G::Midpoint => "Midpoint: a point, then a line",
        G::PointOnCurve => "On curve: a point, then a curve",
        G::Concentric => "Concentric: two circles or arcs",
        G::Symmetric => "Symmetric: two points, then the line of symmetry",
        G::Fixed => "Fix: a point",
    }
}

/// The dimension kind a stored constraint takes values in: angle or not.
pub fn stored_kind(constraint: &ViewConstraint) -> DimKind {
    match constraint.kind {
        "AngularDimension" => DimKind::Angle,
        "RadialDimension" => DimKind::Radius,
        "DiameterDimension" => DimKind::Diameter,
        _ => DimKind::Distance,
    }
}

/// A dimension's label: `R`/`Ø` for radii and diameters, `°` for angles,
/// the bound parameter before its value, and parentheses for a reference
/// (driven) dimension.
pub fn label(constraint: &ViewConstraint) -> String {
    let value = constraint.value.unwrap_or(f64::NAN);
    let number = crate::kit::number_text(value);
    let shown = match stored_kind(constraint) {
        DimKind::Angle => format!("{number}°"),
        DimKind::Radius => format!("R{number}"),
        DimKind::Diameter => format!("Ø{number}"),
        _ => number,
    };
    // A bare parameter names itself; an expression shows its value only
    // (the field and the parameters panel show the text).
    let text = match &constraint.param {
        Some(param) if ParamName(param.clone()).valid() => format!("{param} = {shown}"),
        _ => shown,
    };
    if constraint.driven {
        format!("({text})")
    } else {
        text
    }
}

/// What a dimension's field opens with: its parameter, else its value.
pub fn field_text(constraint: &ViewConstraint) -> String {
    match &constraint.param {
        Some(param) => param.clone(),
        None => crate::kit::number_text(constraint.value.unwrap_or(0.0)),
    }
}

/// The parameter an expression typed into a dimension's field defines: the
/// one it is bound to (redefined), else the first free `dN`.
pub fn fallback_name(source: &FootprintSource, bound: Option<&str>) -> ParamName {
    if let Some(name) = bound.map(|b| ParamName(b.to_string()))
        && name.valid()
        && !name.is_package()
    {
        return name;
    }
    let taken = |n: &str| source.parameters.iter().any(|p| p.name.0 == n);
    (1u32..)
        .map(|i| format!("d{i}"))
        .find(|n| !taken(n))
        .map_or_else(|| ParamName("d".into()), ParamName)
}

/// The edits a new dimension's field commits: the dimension (bound or
/// fixed) and, for an expression or `name = expr`, its parameter first.
pub fn new_dimension_edits(
    kind: DimKind,
    refs: Vec<SketchRef>,
    input: DimInput,
    fallback: ParamName,
) -> Vec<FootprintEdit> {
    let unit = if kind == DimKind::Angle {
        ParamUnit::Deg
    } else {
        ParamUnit::Mm
    };
    let add = |value| {
        FootprintEdit::Sketch(SketchEdit::AddDimension {
            kind,
            refs: refs.clone(),
            value,
        })
    };
    match input {
        DimInput::Value(v) => vec![add(DimValue::Fixed(v))],
        DimInput::Param(name) => vec![add(DimValue::Param(name))],
        DimInput::Expr(expr) => vec![
            FootprintEdit::SetParameter {
                name: fallback.clone(),
                expr,
                unit,
            },
            add(DimValue::Param(fallback)),
        ],
        DimInput::Define { name, expr } => vec![
            FootprintEdit::SetParameter {
                name: name.clone(),
                expr,
                unit,
            },
            add(DimValue::Param(name)),
        ],
    }
}

/// Status-line text for a solve report.
pub fn status_text(report: &SolveReport) -> String {
    match report.status {
        SolveStatus::Solved => "Fully defined".into(),
        SolveStatus::UnderConstrained => match report.dof {
            1 => "1 degree of freedom".into(),
            n => format!("{n} degrees of freedom"),
        },
        SolveStatus::OverConstrained => match report.conflicting.len() {
            1 => "Over-constrained: 1 constraint conflicts".into(),
            n => format!("Over-constrained: {n} constraints conflict"),
        },
        SolveStatus::Failed => "The sketch does not solve".into(),
    }
}

#[cfg(test)]
#[path = "footprint_dims_tests.rs"]
mod more_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_field_splits_numbers_names_definitions_and_expressions() {
        assert_eq!(parse_input(" 1.27 "), Ok(DimInput::Value(1.27)));
        assert_eq!(
            parse_input("pitch"),
            Ok(DimInput::Param(ParamName("pitch".into())))
        );
        assert_eq!(
            parse_input("span = pkg_E1 + pad_l"),
            Ok(DimInput::Define {
                name: ParamName("span".into()),
                expr: Expr("pkg_E1 + pad_l".into()),
            })
        );
        assert_eq!(
            parse_input("2*pitch"),
            Ok(DimInput::Expr(Expr("2*pitch".into())))
        );
        assert!(parse_input("").is_err());
        assert!(parse_input("pkg_e = 1").is_err());
        assert!(parse_input("1x = 2").is_err());
    }
}
