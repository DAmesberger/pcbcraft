//! The Board shape mode of Layout: the board outline as a parametric
//! SolidCraft sketch, edited with the Footprint tab's sketch vocabulary
//! (Line, Arc, Circle, Rectangle with a corner radius, Fillet, Dimension
//! bound to named parameters) plus the board's own features: cutouts,
//! milled slots, mounting holes and named edges.
//!
//! This module is the pure part: tools, their options and the gesture
//! builders that turn clicks into the [`BoardEdit`]s of one `board.edit`
//! (one undo step). `board_shape_ui` draws the sketch and the card and
//! feeds the canvas into it. A gesture that needs the ids of entities it
//! creates in the same commit (a cutout's curves, a fillet's arc ends)
//! stages its prefix with the host evaluator's deterministic
//! `apply_sketch`, as the Footprint tools do (`footprint_tools::Build`),
//! so the engine allocates the same ids when the commit is applied.
//!
//! The sketch is read through `pcbcraft_solidcraft::footprint::view`
//! (the board sketch is the same SolidCraft sketch a footprint keeps), via
//! [`source_of`], and the dimension helpers are the Footprint tab's
//! (`part_editor::footprint_dims`).
use crate::part_editor::footprint_dims::{self as dims, DimInput, DimPicks};
use pcbcraft_core::Point;
use pcbcraft_core::diagnostics::{Error, Result};
use pcbcraft_core::layout::outline::{BoardEdit, BoardShapeEvaluator};
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintEdit, FootprintParam, FootprintSource, GeomConstraint,
    ParamName, ParamUnit, SketchBlob, SketchConstraintId, SketchCurveId, SketchEdit, SketchPointId,
    SketchRef,
};
use pcbcraft_core::model::board::{BoardSketch, CutoutKind, EdgeName};
use pcbcraft_solidcraft::footprint::view::{self, SketchView, ViewGeometry};

/// The host's board sketch engine, which `board.start_sketch` and
/// `board.edit` need. `None` until the integration crate's
/// `SketchBoardEvaluator` (docs/board-contract.md, task 7) is linked here;
/// the app's apply path and the Board shape mode both read it.
pub(crate) fn host_evaluator() -> Option<&'static dyn BoardShapeEvaluator> {
    None
}

/// One ribbon tool of the Board shape mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ShapeTool {
    #[default]
    Select,
    Line,
    Arc,
    Circle,
    /// Two corners; rounded when the corner radius is above zero.
    Rect,
    /// Round the corner two lines meet at.
    Fillet,
    Dimension,
    /// A rectangular or round cutout through the board.
    Cutout,
    /// A milled slot between two clicked ends.
    Slot,
    /// A drilled mounting hole.
    Hole,
    /// Name the clicked edge, so placements can refer to it.
    NameEdge,
}

/// The shape a Cutout click draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CutoutShape {
    #[default]
    Rect,
    Circle,
}

impl ShapeTool {
    /// Every tool, in ribbon order.
    pub(crate) const ALL: [Self; 11] = [
        Self::Select,
        Self::Line,
        Self::Arc,
        Self::Circle,
        Self::Rect,
        Self::Fillet,
        Self::Dimension,
        Self::Cutout,
        Self::Slot,
        Self::Hole,
        Self::NameEdge,
    ];

    /// The ribbon id (prefixed `bs-`, distinct from Layout's own tools).
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Select => "bs-select",
            Self::Line => "bs-line",
            Self::Arc => "bs-arc",
            Self::Circle => "bs-circle",
            Self::Rect => "bs-rect",
            Self::Fillet => "bs-fillet",
            Self::Dimension => "bs-dimension",
            Self::Cutout => "bs-cutout",
            Self::Slot => "bs-slot",
            Self::Hole => "bs-hole",
            Self::NameEdge => "bs-name-edge",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Line => "Line",
            Self::Arc => "Arc",
            Self::Circle => "Circle",
            Self::Rect => "Rectangle",
            Self::Fillet => "Fillet",
            Self::Dimension => "Dimension",
            Self::Cutout => "Cutout",
            Self::Slot => "Slot",
            Self::Hole => "Mounting hole",
            Self::NameEdge => "Name edge",
        }
    }

    /// The Solidcraft icon master on the ribbon button.
    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::Select => "select",
            Self::Line => "line",
            Self::Arc => "arc_center",
            Self::Circle => "circle",
            Self::Rect => "rect_rounded",
            Self::Fillet => "fillet",
            Self::Dimension => "dimension",
            Self::Cutout => "cut",
            Self::Slot => "slot",
            Self::Hole => "hole",
            Self::NameEdge => "text",
        }
    }

    /// The tool's key: Solidcraft's sketch keys (L, R, C, D) and the
    /// Footprint tab's (A arc, H hole, Shift+H slot), plus F fillet,
    /// U cutout and E name edge.
    pub(crate) fn key(self) -> Option<(egui::Key, bool)> {
        use egui::Key as K;
        match self {
            Self::Line => Some((K::L, false)),
            Self::Arc => Some((K::A, false)),
            Self::Circle => Some((K::C, false)),
            Self::Rect => Some((K::R, false)),
            Self::Fillet => Some((K::F, false)),
            Self::Dimension => Some((K::D, false)),
            Self::Cutout => Some((K::U, false)),
            Self::Slot => Some((K::H, true)),
            Self::Hole => Some((K::H, false)),
            Self::NameEdge => Some((K::E, false)),
            Self::Select => None,
        }
    }

    /// The key cap the tool card shows.
    pub(crate) fn key_text(self) -> &'static str {
        match self {
            Self::Line => "L",
            Self::Arc => "A",
            Self::Circle => "C",
            Self::Rect => "R",
            Self::Fillet => "F",
            Self::Dimension => "D",
            Self::Cutout => "U",
            Self::Slot => "⇧H",
            Self::Hole => "H",
            Self::NameEdge => "E",
            Self::Select => "",
        }
    }

    /// One line of help for the tool card and the ribbon tip.
    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Select => "Click to select, drag a point, Delete removes; Esc leaves",
            Self::Line => "Click both ends; it chains from its end; Esc ends the chain",
            Self::Arc => "Click the centre, the start, then the end (counter-clockwise)",
            Self::Circle => "Click the centre, then a point on the circle",
            Self::Rect => "Click two corners; a corner radius rounds them",
            Self::Fillet => "Click the corner where two lines meet",
            Self::Dimension => "Pick a line, two points, a point and a line, or a curve",
            Self::Cutout => "Click two corners (rectangle) or centre and radius (circle)",
            Self::Slot => "Click both ends of the slot",
            Self::Hole => "Click where the mounting hole goes",
            Self::NameEdge => "Click an edge to give it the name below",
        }
    }

    /// Clicks one gesture of this tool takes.
    pub(crate) fn clicks(self) -> usize {
        match self {
            Self::Line | Self::Circle | Self::Rect | Self::Slot | Self::Cutout => 2,
            Self::Arc => 3,
            _ => 1,
        }
    }
}

/// The tools' options, shown in the tool card while the tool is armed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ShapeOptions {
    /// Rectangle corner radius, mm; 0 draws sharp corners.
    pub corner_radius: f64,
    pub fillet_radius: f64,
    pub cutout: CutoutShape,
    pub slot_width: f64,
    pub slot_plated: bool,
    pub hole_drill: f64,
    pub hole_plated: bool,
    /// Copper ring diameter of a plated hole, mm.
    pub hole_pad: f64,
    /// The name the Name edge tool gives.
    pub edge_name: String,
    /// Draw construction geometry (reference only, never board edge).
    pub construction: bool,
}

impl Default for ShapeOptions {
    fn default() -> Self {
        Self {
            corner_radius: 0.0,
            fillet_radius: 2.0,
            cutout: CutoutShape::Rect,
            slot_width: 1.0,
            slot_plated: false,
            hole_drill: 3.2,
            hole_plated: false,
            hole_pad: 6.0,
            edge_name: String::new(),
            construction: false,
        }
    }
}

/// A board sketch as the footprint view functions read it: its sketch and
/// parameters, with no pads or package values.
pub(crate) fn source_of(board: &BoardSketch) -> FootprintSource {
    source_with(board.sketch.clone(), board.parameters.clone())
}

pub(crate) fn source_with(sketch: SketchBlob, parameters: Vec<FootprintParam>) -> FootprintSource {
    FootprintSource {
        evaluator: view_ref(),
        sketch,
        parameters,
        pads: Vec::new(),
        arrays: Vec::new(),
        graphics: Vec::new(),
        keys: Vec::new(),
        overrides: Vec::new(),
        solve: Default::default(),
        datum: None,
    }
}

fn view_ref() -> pcbcraft_core::library::footprint_source::EvaluatorRef {
    pcbcraft_solidcraft::footprint::evaluator_ref()
}

/// The view of a staged sketch.
fn view_of(sketch: &SketchBlob) -> Result<SketchView> {
    view::sketch_view(&source_with(sketch.clone(), Vec::new()))
}

/// Footprint dimension edits as board edits (the Footprint tab's
/// dimension field produces `SetParameter` and sketch edits only).
pub(crate) fn board_edits(edits: Vec<FootprintEdit>) -> Result<Vec<BoardEdit>> {
    edits
        .into_iter()
        .map(|e| match e {
            FootprintEdit::Sketch(edit) => Ok(BoardEdit::Sketch { edit }),
            FootprintEdit::SetParameter { name, expr, unit } => Ok(BoardEdit::SetParameter {
                param: FootprintParam {
                    name,
                    expr,
                    unit,
                    comment: String::new(),
                },
            }),
            FootprintEdit::RemoveParameter { name } => Ok(BoardEdit::RemoveParameter { name }),
            _ => Err(invalid("board.edit: that edit is not a board edit")),
        })
        .collect()
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

fn mm(v: f64) -> Expr {
    Expr(format!("{} mm", crate::kit::number_text(v)))
}

fn dist(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn scale(a: Point, k: f64) -> Point {
    [a[0] * k, a[1] * k]
}

fn unit(v: Point) -> Option<Point> {
    let l = v[0].hypot(v[1]);
    (l.is_finite() && l > 1e-9).then(|| [v[0] / l, v[1] / l])
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// A commit being built: board edits, with the sketch they stage, so
/// later edits can name what earlier ones created.
pub(crate) struct Stage<'a> {
    evaluator: &'a dyn BoardShapeEvaluator,
    sketch: SketchBlob,
    pub edits: Vec<BoardEdit>,
}

impl<'a> Stage<'a> {
    pub(crate) fn new(evaluator: &'a dyn BoardShapeEvaluator, sketch: &SketchBlob) -> Self {
        Self {
            evaluator,
            sketch: sketch.clone(),
            edits: Vec::new(),
        }
    }

    /// Stage one sketch edit; returns what it created.
    pub(crate) fn sketch(&mut self, edit: SketchEdit) -> Result<Vec<SketchRef>> {
        let out = self.evaluator.apply_sketch(&self.sketch, &edit)?;
        self.sketch = out.sketch;
        self.edits.push(BoardEdit::Sketch { edit });
        Ok(out.created)
    }

    /// The sketch with every staged edit applied.
    #[cfg(test)]
    pub(crate) fn staged(&self) -> &SketchBlob {
        &self.sketch
    }

    pub(crate) fn push(&mut self, edit: BoardEdit) {
        self.edits.push(edit);
    }

    fn view(&self) -> Result<SketchView> {
        view_of(&self.sketch)
    }

    /// The curve an edit created and its defining points (a line's ends;
    /// an arc's centre, start and end; a circle's centre).
    fn created_curve(&self, created: &[SketchRef]) -> Result<(SketchCurveId, Vec<SketchPointId>)> {
        let id = created
            .iter()
            .rev()
            .find_map(|r| match r {
                SketchRef::Curve(c) => Some(c.clone()),
                _ => None,
            })
            .ok_or_else(|| invalid("board.edit: the sketch created no curve"))?;
        let points = self
            .view()?
            .curve(&id)
            .map(|c| c.points.clone())
            .unwrap_or_default();
        Ok((id, points))
    }

    fn line(
        &mut self,
        a: Point,
        b: Point,
        construction: bool,
    ) -> Result<(SketchCurveId, [SketchPointId; 2])> {
        let created = self.sketch(SketchEdit::AddLine { a, b, construction })?;
        match self.created_curve(&created)? {
            (id, points) if points.len() == 2 => Ok((id, [points[0].clone(), points[1].clone()])),
            _ => Err(invalid("board.edit: a line needs two ends")),
        }
    }

    fn arc(
        &mut self,
        center: Point,
        start: Point,
        end: Point,
        construction: bool,
    ) -> Result<(SketchCurveId, [SketchPointId; 3])> {
        let created = self.sketch(SketchEdit::AddArc {
            center,
            start,
            end,
            construction,
        })?;
        match self.created_curve(&created)? {
            (id, p) if p.len() == 3 => Ok((id, [p[0].clone(), p[1].clone(), p[2].clone()])),
            _ => Err(invalid("board.edit: an arc needs a centre and two ends")),
        }
    }

    fn constrain(&mut self, kind: GeomConstraint, refs: Vec<SketchRef>) -> Result<()> {
        self.sketch(SketchEdit::AddConstraint { kind, refs })
            .map(|_| ())
    }

    fn coincident(&mut self, p: &SketchPointId, q: &SketchPointId) -> Result<()> {
        self.constrain(
            GeomConstraint::Coincident,
            vec![SketchRef::Point(p.clone()), SketchRef::Point(q.clone())],
        )
    }

    fn tangent(&mut self, a: &SketchCurveId, b: &SketchCurveId) -> Result<()> {
        self.constrain(
            GeomConstraint::Tangent,
            vec![SketchRef::Curve(a.clone()), SketchRef::Curve(b.clone())],
        )
    }

    /// Tie a new end to the existing point it was snapped to.
    fn tie(&mut self, end: &SketchPointId, snapped: Option<&SketchPointId>) -> Result<()> {
        match snapped {
            Some(existing) => self.coincident(end, existing),
            None => Ok(()),
        }
    }
}

/// A clicked point, snapped to an existing sketch point within the pick
/// tolerance: the new geometry is tied to it with a Coincident.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Click {
    pub at: Point,
    pub snapped: Option<SketchPointId>,
}

/// Snap `at` to the nearest sketch point within `tolerance` mm.
pub(crate) fn snap(view: &SketchView, at: Point, tolerance: f64) -> Click {
    view.points
        .iter()
        .filter(|p| p.id.0 != "origin")
        .map(|p| (dist(p.at, at), p))
        .filter(|(d, _)| *d <= tolerance)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Click { at, snapped: None }, |(_, p)| Click {
            at: p.at,
            snapped: Some(p.id.clone()),
        })
}

/// A closed polygon of separate lines whose corners are tied with
/// Coincident constraints (so Fillet can open any corner later). Returns
/// the lines in order.
fn closed_lines(
    stage: &mut Stage<'_>,
    corners: &[Point],
    construction: bool,
) -> Result<Vec<SketchCurveId>> {
    let mut lines = Vec::with_capacity(corners.len());
    let mut ends = Vec::with_capacity(corners.len());
    for (k, a) in corners.iter().enumerate() {
        let b = corners[(k + 1) % corners.len()];
        let (id, [s, e]) = stage.line(*a, b, construction)?;
        lines.push(id);
        ends.push((s, e));
    }
    for k in 0..ends.len() {
        let next = (k + 1) % ends.len();
        stage.coincident(&ends[k].1, &ends[next].0)?;
    }
    Ok(lines)
}

/// The fresh parameter name `base`, `base2`, … not in `taken`.
pub(crate) fn fresh_name(taken: &[FootprintParam], base: &str) -> ParamName {
    let free = |n: &str| !taken.iter().any(|p| p.name.0 == n);
    if free(base) {
        return ParamName(base.into());
    }
    (2u32..)
        .map(|i| format!("{base}{i}"))
        .find(|n| free(n))
        .map_or_else(|| ParamName(base.into()), ParamName)
}

/// The edits of an axis-aligned rectangle from corner `a` to corner `b`,
/// counter-clockwise; with `radius` > 0 its corners are tangent arcs whose
/// radius is the parameter `radius_param` (added here when given as
/// `Some((name, true))`). Returns the curves of its loop in order.
pub(crate) fn rectangle(
    stage: &mut Stage<'_>,
    a: Point,
    b: Point,
    radius: f64,
    radius_param: Option<(ParamName, bool)>,
    construction: bool,
) -> Result<Vec<SketchCurveId>> {
    let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
    let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
    let (w, h) = (x1 - x0, y1 - y0);
    if !(w > 1e-6 && h > 1e-6) {
        return Err(invalid("board.edit: the rectangle has no area"));
    }
    if radius <= 0.0 {
        let lines = closed_lines(
            stage,
            &[[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
            construction,
        )?;
        for (k, l) in lines.iter().enumerate() {
            let kind = if k % 2 == 0 {
                GeomConstraint::Horizontal
            } else {
                GeomConstraint::Vertical
            };
            stage.constrain(kind, vec![SketchRef::Curve(l.clone())])?;
        }
        return Ok(lines);
    }
    if !(radius.is_finite() && 2.0 * radius < w.min(h)) {
        return Err(invalid(
            "board.edit: the corner radius must be under half the shorter side",
        ));
    }
    let r = radius;
    // Sides, counter-clockwise from the bottom, each between its corners'
    // tangent points; then the corner arcs, each from the side before it
    // to the side after it.
    let sides = [
        ([x0 + r, y0], [x1 - r, y0]),
        ([x1, y0 + r], [x1, y1 - r]),
        ([x1 - r, y1], [x0 + r, y1]),
        ([x0, y1 - r], [x0, y0 + r]),
    ];
    let centres = [
        [x1 - r, y0 + r],
        [x1 - r, y1 - r],
        [x0 + r, y1 - r],
        [x0 + r, y0 + r],
    ];
    let mut lines = Vec::with_capacity(4);
    for (k, (s, e)) in sides.iter().enumerate() {
        let (id, ends) = stage.line(*s, *e, construction)?;
        let kind = if k % 2 == 0 {
            GeomConstraint::Horizontal
        } else {
            GeomConstraint::Vertical
        };
        stage.constrain(kind, vec![SketchRef::Curve(id.clone())])?;
        lines.push((id, ends));
    }
    if let Some((name, true)) = &radius_param {
        stage.push(BoardEdit::SetParameter {
            param: FootprintParam {
                name: name.clone(),
                expr: mm(r),
                unit: ParamUnit::Mm,
                comment: "Board corner radius".into(),
            },
        });
    }
    let mut curves = Vec::with_capacity(8);
    for k in 0..4 {
        let next = (k + 1) % 4;
        let (line_id, [_, line_end]) = lines[k].clone();
        let (next_id, [next_start, _]) = lines[next].clone();
        let (arc, [_, arc_start, arc_end]) =
            stage.arc(centres[k], sides[k].1, sides[next].0, construction)?;
        stage.coincident(&arc_start, &line_end)?;
        stage.coincident(&arc_end, &next_start)?;
        stage.tangent(&line_id, &arc)?;
        stage.tangent(&next_id, &arc)?;
        let value = match &radius_param {
            Some((name, _)) => DimValue::Param(name.clone()),
            None => DimValue::Fixed(r),
        };
        stage.sketch(SketchEdit::AddDimension {
            kind: DimKind::Radius,
            refs: vec![SketchRef::Curve(arc.clone())],
            value,
        })?;
        curves.push(line_id);
        curves.push(arc);
    }
    Ok(curves)
}

/// A slot (obround) between `a` and `b` of `width`, counter-clockwise:
/// two side lines and two end arcs, tied and tangent. Returns its loop.
pub(crate) fn slot(
    stage: &mut Stage<'_>,
    a: Point,
    b: Point,
    width: f64,
) -> Result<Vec<SketchCurveId>> {
    let u = unit(sub(b, a)).ok_or_else(|| invalid("board.edit: the slot ends coincide"))?;
    if !(width.is_finite() && width > 0.0) {
        return Err(invalid("board.edit: the slot needs a width"));
    }
    let h = width / 2.0;
    let n = [-u[1] * h, u[0] * h];
    let (a_r, b_r) = (sub(a, n), sub(b, n));
    let (a_l, b_l) = (add(a, n), add(b, n));
    let (low, [low_a, low_b]) = stage.line(a_r, b_r, false)?;
    let (end_b, [_, eb_s, eb_e]) = stage.arc(b, b_r, b_l, false)?;
    let (high, [high_b, high_a]) = stage.line(b_l, a_l, false)?;
    let (end_a, [_, ea_s, ea_e]) = stage.arc(a, a_l, a_r, false)?;
    stage.coincident(&low_b, &eb_s)?;
    stage.coincident(&eb_e, &high_b)?;
    stage.coincident(&high_a, &ea_s)?;
    stage.coincident(&ea_e, &low_a)?;
    for (line, arc) in [
        (&low, &end_b),
        (&high, &end_b),
        (&high, &end_a),
        (&low, &end_a),
    ] {
        stage.tangent(line, arc)?;
    }
    stage.sketch(SketchEdit::AddDimension {
        kind: DimKind::Diameter,
        refs: vec![SketchRef::Curve(end_a.clone())],
        value: DimValue::Fixed(width),
    })?;
    stage.constrain(
        GeomConstraint::Equal,
        vec![
            SketchRef::Curve(end_a.clone()),
            SketchRef::Curve(end_b.clone()),
        ],
    )?;
    Ok(vec![low, end_b, high, end_a])
}

/// A fillet of `radius` at the corner whose Coincident constraint joins
/// the ends of two lines near `at`: the Coincident goes, both ends move to
/// the tangent points and a tangent arc of that radius joins them.
pub(crate) fn fillet(
    stage: &mut Stage<'_>,
    view: &SketchView,
    at: Point,
    tolerance: f64,
    radius: f64,
) -> Result<()> {
    if !(radius.is_finite() && radius > 0.0) {
        return Err(invalid("board.edit: the fillet needs a radius"));
    }
    let point_of = |id: &SketchPointId| view.point(id).map(|p| p.at);
    // The line ending at a point, and its other end.
    let line_of = |p: &SketchPointId| {
        view.curves
            .iter()
            .find_map(|c| match (&c.geometry, c.points.as_slice()) {
                (ViewGeometry::Line { a, b }, [pa, pb]) if pa == p => Some((c.id.clone(), *b, *a)),
                (ViewGeometry::Line { a, b }, [pa, pb]) if pb == p => Some((c.id.clone(), *a, *b)),
                _ => None,
            })
    };
    let corner = view
        .constraints
        .iter()
        .filter(|k| k.kind == "Coincident" && k.curves.is_empty() && k.points.len() == 2)
        .filter_map(|k| {
            let (p, q) = (&k.points[0], &k.points[1]);
            let d = dist(point_of(p)?, at);
            (d <= tolerance).then_some((d, k, p, q))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let Some((_, k, p, q)) = corner else {
        let shared = view
            .points
            .iter()
            .filter(|pt| dist(pt.at, at) <= tolerance)
            .any(|pt| {
                view.curves
                    .iter()
                    .filter(|c| c.points.contains(&pt.id))
                    .count()
                    >= 2
            });
        return Err(invalid(if shared {
            "board.fillet: these lines share one corner point; redraw them with Line or Rectangle"
        } else {
            "board.fillet: click the corner where two lines meet"
        }));
    };
    let (Some((l1, far1, c1)), Some((l2, far2, _))) = (line_of(p), line_of(q)) else {
        return Err(invalid("board.fillet: a fillet joins two lines"));
    };
    if l1 == l2 {
        return Err(invalid("board.fillet: a fillet joins two lines"));
    }
    let corner = c1;
    let (u1, u2) = match (unit(sub(far1, corner)), unit(sub(far2, corner))) {
        (Some(u1), Some(u2)) => (u1, u2),
        _ => return Err(invalid("board.fillet: a line at this corner has no length")),
    };
    let cos = (u1[0] * u2[0] + u1[1] * u2[1]).clamp(-1.0, 1.0);
    let theta = cos.acos();
    if !(theta > 1e-3 && theta < std::f64::consts::PI - 1e-3) {
        return Err(invalid(
            "board.fillet: the lines are in line; there is no corner",
        ));
    }
    let back = radius / (theta / 2.0).tan();
    if back >= dist(far1, corner) || back >= dist(far2, corner) {
        return Err(invalid(
            "board.fillet: the radius is too large for these lines",
        ));
    }
    let t1 = add(corner, scale(u1, back));
    let t2 = add(corner, scale(u2, back));
    let bisector = unit(add(u1, u2)).ok_or_else(|| invalid("board.fillet: no corner"))?;
    let centre = add(corner, scale(bisector, radius / (theta / 2.0).sin()));
    stage.sketch(SketchEdit::Delete {
        ids: vec![SketchRef::Constraint(k.id.clone())],
    })?;
    stage.sketch(SketchEdit::Drag {
        point: p.clone(),
        to: t1,
    })?;
    stage.sketch(SketchEdit::Drag {
        point: q.clone(),
        to: t2,
    })?;
    // Counter-clockwise, the short way round.
    let ((start, start_at), (end, end_at)) = if cross(sub(t1, centre), sub(t2, centre)) > 0.0 {
        ((p, t1), (q, t2))
    } else {
        ((q, t2), (p, t1))
    };
    let (arc, [_, a_s, a_e]) = stage.arc(centre, start_at, end_at, false)?;
    stage.coincident(&a_s, start)?;
    stage.coincident(&a_e, end)?;
    stage.tangent(&l1, &arc)?;
    stage.tangent(&l2, &arc)?;
    stage.sketch(SketchEdit::AddDimension {
        kind: DimKind::Radius,
        refs: vec![SketchRef::Curve(arc)],
        value: DimValue::Fixed(radius),
    })?;
    Ok(())
}

/// What one finished gesture commits, and what to select after it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Commit {
    pub edits: Vec<BoardEdit>,
    /// The new dimension, selected so its value field shows.
    pub select: Option<SketchConstraintId>,
}

/// Where a gesture stands after a click.
#[derive(Clone, Debug)]
pub(crate) enum Step {
    /// More clicks needed (or nothing to do).
    More,
    Commit(Commit),
}

/// The context a gesture builds in.
pub(crate) struct Scene<'a> {
    pub board: &'a BoardSketch,
    pub view: &'a SketchView,
    pub evaluator: &'a dyn BoardShapeEvaluator,
    /// Pick and snap tolerance, mm.
    pub tolerance: f64,
}

/// The drawing tools' gesture: clicked points collect until the tool's
/// count, then one commit; the Dimension tool's picks.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Gesture {
    pub pending: Vec<Click>,
    pub picks: Vec<SketchRef>,
    /// A dimension whose picks are complete, following the pointer until
    /// a click places it (linear dimensions).
    pub placing: Option<Vec<SketchRef>>,
}

impl Gesture {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.picks.is_empty() && self.placing.is_none()
    }
}

/// A click of `tool` at `at`.
pub(crate) fn press(
    tool: ShapeTool,
    options: &ShapeOptions,
    gesture: &mut Gesture,
    at: Point,
    scene: &Scene<'_>,
) -> Result<Step> {
    match tool {
        ShapeTool::Select => Ok(Step::More),
        ShapeTool::Dimension => dimension_press(gesture, at, scene),
        ShapeTool::Fillet => {
            let mut stage = Stage::new(scene.evaluator, &scene.board.sketch);
            fillet(
                &mut stage,
                scene.view,
                at,
                scene.tolerance,
                options.fillet_radius,
            )?;
            Ok(commit(stage))
        }
        ShapeTool::Hole => {
            let mut stage = Stage::new(scene.evaluator, &scene.board.sketch);
            let created = stage.sketch(SketchEdit::AddPoint { at })?;
            let center = created
                .iter()
                .find_map(|r| match r {
                    SketchRef::Point(p) => Some(p.clone()),
                    _ => None,
                })
                .ok_or_else(|| invalid("board.edit: the hole point was not created"))?;
            stage.push(BoardEdit::AddMountingHole {
                center,
                drill: mm(options.hole_drill),
                plated: options.hole_plated,
                pad_diameter: options.hole_plated.then(|| mm(options.hole_pad)),
            });
            Ok(commit(stage))
        }
        ShapeTool::NameEdge => {
            let name = EdgeName(options.edge_name.trim().to_owned());
            if !edge_name_valid(&name) {
                return Err(invalid(
                    "board.edge_name: type an edge name: letters, digits and _",
                ));
            }
            let curve = scene
                .view
                .curves
                .iter()
                .filter(|c| !c.construction)
                .filter_map(|c| Some((dims::curve_distance(&c.geometry, at)?, c)))
                .filter(|(d, _)| *d <= scene.tolerance)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, c)| c.id.clone())
                .ok_or_else(|| invalid("board.edge_name: click an edge of the board"))?;
            Ok(Step::Commit(Commit {
                edits: vec![BoardEdit::NameEdge { name, curve }],
                select: None,
            }))
        }
        _ => {
            gesture.pending.push(snap(scene.view, at, scene.tolerance));
            if gesture.pending.len() < tool.clicks() {
                return Ok(Step::More);
            }
            // Snap again against this revision's sketch: a chained line's
            // start finds the end the last commit made.
            let clicks: Vec<Click> = std::mem::take(&mut gesture.pending)
                .into_iter()
                .map(|c| match c.snapped {
                    Some(_) => c,
                    None => {
                        let again = snap(scene.view, c.at, 1e-6);
                        Click { at: c.at, ..again }
                    }
                })
                .collect();
            let step = draw(tool, options, &clicks, scene)?;
            // A line chains from its end.
            if tool == ShapeTool::Line
                && let Some(last) = clicks.last()
            {
                gesture.pending.push(Click {
                    at: last.at,
                    snapped: None,
                });
            }
            Ok(step)
        }
    }
}

fn commit(stage: Stage<'_>) -> Step {
    Step::Commit(Commit {
        edits: stage.edits,
        select: None,
    })
}

/// Edge names are identifiers, as parameter names are.
pub(crate) fn edge_name_valid(name: &EdgeName) -> bool {
    ParamName(name.0.clone()).valid()
}

/// The next free `edgeN` name.
pub(crate) fn next_edge_name(board: &BoardSketch) -> String {
    (1u32..)
        .map(|i| format!("edge{i}"))
        .find(|n| !board.edges.iter().any(|e| &e.name.0 == n))
        .unwrap_or_else(|| "edge".into())
}

/// The commit of a drawing tool's complete clicks.
fn draw(
    tool: ShapeTool,
    options: &ShapeOptions,
    clicks: &[Click],
    scene: &Scene<'_>,
) -> Result<Step> {
    let mut stage = Stage::new(scene.evaluator, &scene.board.sketch);
    let construction = options.construction;
    match (tool, clicks) {
        (ShapeTool::Line, [a, b]) => {
            if dist(a.at, b.at) <= 1e-9 {
                return Ok(Step::More);
            }
            let (_, [s, e]) = stage.line(a.at, b.at, construction)?;
            stage.tie(&s, a.snapped.as_ref())?;
            stage.tie(&e, b.snapped.as_ref())?;
        }
        (ShapeTool::Arc, [c, s, e]) => {
            let r = dist(c.at, s.at);
            let end = unit(sub(e.at, c.at))
                .map(|u| add(c.at, scale(u, r)))
                .ok_or_else(|| invalid("board.edit: the arc end is at its centre"))?;
            let (_, [pc, ps, pe]) = stage.arc(c.at, s.at, end, construction)?;
            stage.tie(&pc, c.snapped.as_ref())?;
            stage.tie(&ps, s.snapped.as_ref())?;
            if dist(end, e.at) <= 1e-9 {
                stage.tie(&pe, e.snapped.as_ref())?;
            }
        }
        (ShapeTool::Circle, [c, p]) => {
            let radius = dist(c.at, p.at);
            if radius <= 1e-9 {
                return Err(invalid("board.edit: the circle has no radius"));
            }
            stage.sketch(SketchEdit::AddCircle {
                center: c.at,
                radius,
                construction,
            })?;
        }
        (ShapeTool::Rect, [a, b]) => {
            let param = (options.corner_radius > 0.0)
                .then(|| (fresh_name(&scene.board.parameters, "corner_r"), true));
            rectangle(
                &mut stage,
                a.at,
                b.at,
                options.corner_radius,
                param,
                construction,
            )?;
        }
        (ShapeTool::Cutout, [a, b]) => {
            let curves = match options.cutout {
                CutoutShape::Rect => rectangle(&mut stage, a.at, b.at, 0.0, None, false)?,
                CutoutShape::Circle => {
                    let radius = dist(a.at, b.at);
                    if radius <= 1e-9 {
                        return Err(invalid("board.edit: the cutout has no radius"));
                    }
                    let created = stage.sketch(SketchEdit::AddCircle {
                        center: a.at,
                        radius,
                        construction: false,
                    })?;
                    vec![stage.created_curve(&created)?.0]
                }
            };
            stage.push(BoardEdit::AddCutout {
                kind: CutoutKind::Cutout,
                curves,
            });
        }
        (ShapeTool::Slot, [a, b]) => {
            let curves = slot(&mut stage, a.at, b.at, options.slot_width)?;
            stage.push(BoardEdit::AddCutout {
                kind: CutoutKind::Slot {
                    plated: options.slot_plated,
                },
                curves,
            });
        }
        _ => return Ok(Step::More),
    }
    Ok(commit(stage))
}

fn dimension_press(gesture: &mut Gesture, at: Point, scene: &Scene<'_>) -> Result<Step> {
    if let Some(refs) = gesture.placing.take() {
        gesture.picks.clear();
        return new_dimension(scene, refs, true, at);
    }
    match dims::pick(scene.view, at, scene.tolerance) {
        Some(pick) => gesture.picks.push(pick),
        // A single line's length is placed by a click on empty canvas.
        None if matches!(gesture.picks.as_slice(), [SketchRef::Curve(_)]) => {
            let refs = std::mem::take(&mut gesture.picks);
            return new_dimension(scene, refs, true, at);
        }
        None => return Ok(Step::More),
    }
    match dims::dimension_picks(scene.view, &gesture.picks) {
        DimPicks::More => Ok(Step::More),
        DimPicks::Ready { refs, linear: true } => {
            gesture.placing = Some(refs);
            Ok(Step::More)
        }
        DimPicks::Ready {
            refs,
            linear: false,
        } => {
            gesture.picks.clear();
            new_dimension(scene, refs, false, at)
        }
        DimPicks::Wrong(why) => {
            gesture.picks.clear();
            Err(invalid(format!("board.dimension: {why}")))
        }
    }
}

/// A driving dimension at what it measures now; the new dimension is
/// selected, so its value field takes a number or a parameter next.
fn new_dimension(
    scene: &Scene<'_>,
    refs: Vec<SketchRef>,
    linear: bool,
    place: Point,
) -> Result<Step> {
    let kind = dims::dimension_kind(scene.view, &refs, linear, place);
    let value = dims::measure(scene.view, kind, &refs)
        .ok_or_else(|| invalid("board.dimension: these entities cannot be measured"))?;
    let mut stage = Stage::new(scene.evaluator, &scene.board.sketch);
    let created = stage.sketch(SketchEdit::AddDimension {
        kind,
        refs,
        value: DimValue::Fixed(value),
    })?;
    let select = created.into_iter().find_map(|r| match r {
        SketchRef::Constraint(k) => Some(k),
        _ => None,
    });
    Ok(Step::Commit(Commit {
        edits: stage.edits,
        select,
    }))
}

/// The edits a dimension's value field commits: a number, a parameter, an
/// expression (a new parameter) or `name = expr`.
pub(crate) fn dimension_value_edits(
    board: &BoardSketch,
    view: &SketchView,
    constraint: &SketchConstraintId,
    text: &str,
) -> std::result::Result<Vec<BoardEdit>, String> {
    let input = dims::parse_input(text)?;
    let shown = view
        .constraints
        .iter()
        .find(|k| &k.id == constraint)
        .ok_or_else(|| "The dimension is gone".to_string())?;
    let kind = dims::stored_kind(shown);
    let source = source_of(board);
    let fallback = dims::fallback_name(&source, shown.param.as_deref());
    if let DimInput::Param(name) = &input
        && !board.parameters.iter().any(|p| &p.name == name)
    {
        return Err(format!("No parameter {}", name.0));
    }
    board_edits(dims::input_edits(constraint, kind, input, fallback)).map_err(|e| e.to_string())
}

/// The parameter edit a parameters row commits, checked against the
/// other parameters first (syntax, unknown names, units).
pub(crate) fn parameter_edit(
    board: &BoardSketch,
    name: &ParamName,
    expr: &str,
    unit: ParamUnit,
) -> std::result::Result<BoardEdit, String> {
    let expr = Expr(expr.trim().to_owned());
    if !expr.valid() {
        return Err("Type an expression".into());
    }
    if !name.valid() || name.is_package() {
        return Err(format!("{} is not a parameter name", name.0));
    }
    if let Some(why) = view::expression_error(&source_of(board), &[], name, &expr.0, unit) {
        return Err(why);
    }
    let comment = board
        .parameters
        .iter()
        .find(|p| &p.name == name)
        .map(|p| p.comment.clone())
        .unwrap_or_default();
    Ok(BoardEdit::SetParameter {
        param: FootprintParam {
            name: name.clone(),
            expr,
            unit,
            comment,
        },
    })
}

/// The edits Delete sends for the selection; a named edge's curve is
/// unnamed and a cutout's curves leave the cutout first.
pub(crate) fn delete_edits(board: &BoardSketch, selection: &[SketchRef]) -> Vec<BoardEdit> {
    if selection.is_empty() {
        return Vec::new();
    }
    let mut edits = Vec::new();
    let curves: Vec<&SketchCurveId> = selection
        .iter()
        .filter_map(|r| match r {
            SketchRef::Curve(c) => Some(c),
            _ => None,
        })
        .collect();
    for edge in board.edges.iter().filter(|e| curves.contains(&&e.curve)) {
        edits.push(BoardEdit::UnnameEdge {
            name: edge.name.clone(),
        });
    }
    for cutout in board
        .cutouts
        .iter()
        .filter(|c| c.curves.iter().any(|id| curves.contains(&id)))
    {
        edits.push(BoardEdit::RemoveCutout { id: cutout.id });
    }
    for hole in board.holes.iter().filter(|h| {
        selection
            .iter()
            .any(|r| matches!(r, SketchRef::Point(p) if *p == h.center))
    }) {
        edits.push(BoardEdit::RemoveMountingHole { id: hole.id });
    }
    edits.push(BoardEdit::Sketch {
        edit: SketchEdit::Delete {
            ids: selection.to_vec(),
        },
    });
    edits
}

/// The selection as an inner loop: the selected curves become a cutout or
/// a slot.
pub(crate) fn cutout_edit(selection: &[SketchRef], kind: CutoutKind) -> Option<BoardEdit> {
    let curves: Vec<SketchCurveId> = selection
        .iter()
        .filter_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    (!curves.is_empty()).then_some(BoardEdit::AddCutout { kind, curves })
}

/// The selection as the outer outline.
pub(crate) fn outline_edit(selection: &[SketchRef]) -> Option<BoardEdit> {
    let curves: Vec<SketchCurveId> = selection
        .iter()
        .filter_map(|r| match r {
            SketchRef::Curve(c) => Some(c.clone()),
            _ => None,
        })
        .collect();
    (!curves.is_empty()).then_some(BoardEdit::SetOutline { curves })
}

/// The board command of a commit.
pub(crate) fn operation(edits: Vec<BoardEdit>) -> pcbcraft_core::Operation {
    pcbcraft_core::Operation::Board(pcbcraft_core::layout::outline::BoardCommand::Edit { edits })
}

#[cfg(test)]
#[path = "board_shape_tests.rs"]
mod tests;
