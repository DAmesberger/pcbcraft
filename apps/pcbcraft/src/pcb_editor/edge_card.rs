//! Parts held to the board by dimensions (docs/board-sketch-references.md
//! §5.2): the Dimensions section of the Placement card and the dimension
//! annotations that show a dimensioned part's distances to board edges on
//! the Layout canvas.
//!
//! "Dimension to edge" (the Placement section's state row) adds one
//! distance from the selected part's origin to the nearest named board edge
//! at its current distance (`layout.dimension_add`, one undo step), so the
//! part does not move; it then slides along that edge when dragged. Each
//! dimension's value commits live as one `layout.dimension_edit`: a length
//! (`5 mm`) or an expression over the board parameters (`width / 2 - 3
//! mm`), so editing a parameter moves the part when the board re-solves.
//! "Delete" removes one dimension (`layout.dimension_delete`) and leaves the
//! part where it is.
use super::board_shape_ui::sketch_of;
use super::{BoardView, PcbEditor};
use crate::kit;
use crate::refusal::{self, Origin};
use egui::{Align2, Pos2, Stroke, Ui, vec2};
use pcbcraft_core::geometry::Transform2;
use pcbcraft_core::layout::dimensions::{
    self, DimensionCommand, FeatureGeometry, PartFreedom, features,
};
use pcbcraft_core::layout::outline::BoardShapeEvaluator;
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintParam, FootprintSource, ParamName, ParamUnit, ParamValue,
    SketchRef,
};
use pcbcraft_core::model::board::{BoardSketch, ConstraintTarget, SolvedBoard, SolvedEdge};
use pcbcraft_core::model::dimension::{
    BoardRef, FeatureRef, PartDimension, PartDimensionSpec, PartRelation,
};
use pcbcraft_core::{Id, Layer, Operation, Placement, Point, Project};
use pcbcraft_solidcraft::footprint::view as sketch_view;
use solidcraft_theme::Tokens;
use solidcraft_ui_egui::ui_kit::type_face;

/// The Dimensions section's text buffers, and what the part's dimensions
/// leave free, for one document revision.
#[derive(Clone, Debug, Default)]
pub(super) struct EdgeEdit {
    key: Option<(u64, Id)>,
    values: Vec<(Id, String)>,
    /// The dimension a refusal or invalid text belongs to, and why.
    error: Option<(Id, String)>,
    sent: Option<Id>,
    show_all: bool,
    /// The selected part's hold: (revision, instance) → why a move is
    /// refused, and what its dimensions leave free.
    hold: Option<((u64, Id), Hold)>,
    /// The dimension labels drawn on the canvas last frame, for clicks.
    labels: Vec<(Id, egui::Rect)>,
}

/// Why a hand move is refused, and what the dimensions leave free.
type Hold = (Option<String>, Option<PartFreedom>);

fn field_id(dimension: Id) -> egui::Id {
    egui::Id::unique(("pcb-dimension", dimension))
}

/// The dimensions naming a placement.
pub(super) fn dimensions_of(project: &Project, instance: Id) -> Vec<&PartDimension> {
    dimensions::dimensions_of(project, ConstraintTarget::Placement(instance))
}

/// Where `p` is relative to a straight `edge`: the distance from `a` along
/// it and the distance off it along the inward normal.
pub(super) fn along_offset(edge: &SolvedEdge, p: Point) -> Option<(f64, f64)> {
    let d = [edge.b[0] - edge.a[0], edge.b[1] - edge.a[1]];
    let len = d[0].hypot(d[1]);
    if !(len.is_finite() && len > 1e-9) || edge.bulge != 0.0 {
        return None;
    }
    let u = [d[0] / len, d[1] / len];
    let r = [p[0] - edge.a[0], p[1] - edge.a[1]];
    Some((r[0] * u[0] + r[1] * u[1], u[0] * r[1] - u[1] * r[0]))
}

fn edge_length(edge: &SolvedEdge) -> f64 {
    (edge.b[0] - edge.a[0]).hypot(edge.b[1] - edge.a[1])
}

/// The straight named edge nearest `p`.
pub(super) fn nearest_edge(board: &SolvedBoard, p: Point) -> Option<&SolvedEdge> {
    board
        .edges
        .iter()
        .filter_map(|e| {
            let (s, o) = along_offset(e, p)?;
            let len = edge_length(e);
            let outside = if s < 0.0 {
                -s
            } else if s > len {
                s - len
            } else {
                0.0
            };
            Some((o.abs().hypot(outside), e))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, e)| e)
}

/// The dimension that keeps a part at `position` relative to `edge`: its
/// origin's distance to the edge's curve, as it is now.
pub(super) fn spec_at(
    board: &BoardSketch,
    instance: Id,
    edge: &SolvedEdge,
    position: Point,
) -> Option<PartDimensionSpec> {
    let (_, offset) = along_offset(edge, position)?;
    let curve = board
        .edges
        .iter()
        .find(|e| e.name == edge.name)?
        .curve
        .clone();
    Some(PartDimensionSpec {
        kind: PartRelation::Dim(DimKind::Distance),
        refs: vec![
            BoardRef::Part(FeatureRef::Origin {
                target: ConstraintTarget::Placement(instance),
            }),
            BoardRef::Board(SketchRef::Curve(curve)),
        ],
        value: Some(DimValue::Fixed(round_um(offset.abs()))),
        reference: false,
    })
}

/// A length to the micrometre, as the card shows it.
fn round_um(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// The board parameters expressions read: the sketch's, or on a plain
/// rectangle board the `width` and `height` of the sketch the dimension
/// solve holds it as ([`dimensions::board_sketch`]).
fn parameters(project: &Project) -> Vec<FootprintParam> {
    if let Some(board) = sketch_of(project) {
        return board.parameters.clone();
    }
    super::board_shape::host_evaluator()
        .and_then(|ev| dimensions::board_sketch(project, ev).ok())
        .map(|b| b.parameters.clone())
        .unwrap_or_default()
}

fn source(params: Vec<FootprintParam>) -> FootprintSource {
    super::board_shape::source_with(Default::default(), params)
}

/// Why `expr` is not a length over the board parameters.
pub(super) fn expression_error(project: &Project, expr: &str) -> Option<String> {
    let expr = expr.trim();
    if !Expr(expr.into()).valid() {
        return Some("Type a distance, like 5 mm or width / 2".into());
    }
    let name = ParamName("edge_value".into());
    sketch_view::expression_error(
        &source(parameters(project)),
        &[],
        &name,
        expr,
        ParamUnit::Mm,
    )
}

/// The value of `expr` (mm) over the board parameters.
pub(super) fn evaluate(project: &Project, expr: &Expr) -> Option<f64> {
    if let Some(v) = kit::parse_number(&expr.0, "mm") {
        return Some(v);
    }
    let name = ParamName("edge_value_".into());
    let mut params = parameters(project);
    params.push(FootprintParam {
        name: name.clone(),
        expr: expr.clone(),
        unit: ParamUnit::Mm,
        comment: String::new(),
    });
    sketch_view::parameter_values(&source(params), &[])
        .ok()?
        .into_iter()
        .find(|v| v.name == name)
        .and_then(|v| match v.value {
            Ok(ParamValue::Length(mm)) => Some(mm),
            _ => None,
        })
}

/// A dimension value's length in mm, where it reads as one.
pub(super) fn length(project: &Project, value: &DimValue) -> Option<f64> {
    match value {
        DimValue::Fixed(v) => Some(*v),
        DimValue::Param(p) => evaluate(project, &Expr(p.0.clone())),
        DimValue::Expr(e) => evaluate(project, e),
    }
}

fn is_angle(d: &PartDimension) -> bool {
    matches!(d.kind, PartRelation::Dim(DimKind::Angle))
}

/// A dimension's value as the card shows it.
fn value_text(d: &PartDimension) -> String {
    let unit = if is_angle(d) { "°" } else { "mm" };
    match &d.value {
        Some(DimValue::Fixed(v)) if unit == "°" => format!("{}°", kit::number_text(*v)),
        Some(DimValue::Fixed(v)) => format!("{} mm", kit::number_text(*v)),
        Some(DimValue::Param(p)) => p.0.clone(),
        Some(DimValue::Expr(e)) => e.0.clone(),
        None => String::new(),
    }
}

/// The value typed into a dimension's field, or why it is not one.
fn typed_value(
    project: &Project,
    d: &PartDimension,
    text: &str,
) -> std::result::Result<DimValue, String> {
    let text = text.trim();
    if is_angle(d) {
        return kit::parse_number(text, "°")
            .map(DimValue::Fixed)
            .ok_or_else(|| "Type an angle, like 90°".to_owned());
    }
    length_value(project, text)
}

/// A distance typed as a value: a length (at least 0 mm) or an expression
/// over the board parameters, or why it is neither.
pub(super) fn length_value(project: &Project, text: &str) -> std::result::Result<DimValue, String> {
    let text = text.trim();
    if let Some(v) = kit::parse_number(text, "mm") {
        return if v >= 0.0 {
            Ok(DimValue::Fixed(v))
        } else {
            Err("A distance is at least 0 mm".into())
        };
    }
    match expression_error(project, text) {
        None => Ok(DimValue::Expr(Expr(text.to_owned()))),
        Some(why) => Err(why),
    }
}

/// What a dimension of `instance` holds, from the part's side: "Origin to
/// the right edge", "Pad 1 to the bottom edge", "X axis to the top edge".
fn label(
    project: &Project,
    board: Option<&BoardSketch>,
    instance: Id,
    d: &PartDimension,
) -> String {
    let words = |r: &BoardRef| -> String {
        match r {
            BoardRef::Board(SketchRef::Curve(c)) => board
                .and_then(|b| dimensions::edge_name(b, c))
                .map_or_else(|| "a board curve".into(), dimensions::edge_words),
            BoardRef::Board(_) => "a board point".into(),
            BoardRef::Housing(_) => "the housing".into(),
            BoardRef::Part(f) if f.target() != ConstraintTarget::Placement(instance) => {
                match f.target() {
                    ConstraintTarget::Placement(id) => project
                        .instance(id)
                        .map_or_else(|| "a part".into(), |i| i.designator.clone()),
                    ConstraintTarget::Fill(_) => "a fill".into(),
                    ConstraintTarget::Keepout(_) => "a keepout".into(),
                }
            }
            BoardRef::Part(f) => match f {
                FeatureRef::Origin { .. } => "origin".into(),
                FeatureRef::Axis { axis, .. } => match axis {
                    pcbcraft_core::model::dimension::PartAxis::X => "x axis".into(),
                    pcbcraft_core::model::dimension::PartAxis::Y => "y axis".into(),
                },
                FeatureRef::Pad { pad, .. } => format!("pad {}", pad.0),
                FeatureRef::Courtyard { .. } => "courtyard".into(),
                FeatureRef::Anchor { name, .. } => format!("‘{}’", name.0),
                // "front face", "top-front edge": where on the model.
                _ => dimensions::model::feature_term(project, f, None)
                    .unwrap_or_else(|| "3D model".into()),
            },
        }
    };
    let text = d.refs.iter().map(words).collect::<Vec<_>>().join(" to ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().chain(chars).collect::<String>()
    })
}

impl PcbEditor {
    /// Why a hand move of the selected part is refused (locked, or fully
    /// held by its dimensions) and what its dimensions leave free, solved
    /// once per document revision.
    pub(super) fn hold_of(
        &mut self,
        revision: u64,
        project: &Project,
        instance: Id,
    ) -> (Option<String>, Option<PartFreedom>) {
        let key = (revision, instance);
        if let Some((k, (why, free))) = &self.edge.hold
            && *k == key
        {
            return (why.clone(), *free);
        }
        let why = pcbcraft_core::layout::move_refusal(project, instance, self.shape.evaluator)
            .map(|e| refusal::shown_error(&e));
        let free = self.shape.evaluator.and_then(|ev| {
            dimensions::freedom(project, ConstraintTarget::Placement(instance), ev)
                .and_then(|r| r.ok())
                .map(|(f, _)| f)
        });
        self.edge.hold = Some((key, (why.clone(), free)));
        (why, free)
    }

    /// DIMENSIONS, for a dimensioned part only: each dimension's value,
    /// committed live, and its Delete. "Dimension to edge" and "Free part"
    /// are the Placement section's state row.
    pub(super) fn edge_section(
        &mut self,
        ui: &mut Ui,
        revision: u64,
        project: &Project,
        placement: &Placement,
    ) -> Option<Operation> {
        if let Some(id) = self.edge.sent
            && let Some(text) = refusal::claim(ui.ctx(), Origin::Field(field_id(id)))
        {
            self.edge.error = Some((id, text));
            self.edge.sent = None;
        }
        let dims: Vec<PartDimension> = dimensions_of(project, placement.instance)
            .into_iter()
            .cloned()
            .collect();
        if dims.is_empty() {
            return None;
        }
        let key = (revision, placement.instance);
        if self.edge.key != Some(key) {
            if self.edge.key.map(|k| k.1) != Some(placement.instance) {
                self.edge.show_all = false;
            }
            self.edge.key = Some(key);
            // The document changed (an edit applied, an undo): the fields
            // show its values, so no earlier text's error stands. A refused
            // edit leaves the revision as it was, so its error is kept.
            self.edge.values = dims.iter().map(|d| (d.id, value_text(d))).collect();
            self.edge.error = None;
            self.edge.sent = None;
        }
        let board = self
            .shape
            .evaluator
            .and_then(|ev: &dyn BoardShapeEvaluator| dimensions::board_sketch(project, ev).ok());
        let mut operation = None;
        let mut sent = None;
        let mut select = None;
        let selected = self.dimension;
        let max = if self.edge.show_all {
            dims.len()
        } else {
            kit::LIST_MAX_ROWS
        };
        kit::section(
            ui,
            "pcb-dimensions",
            "Dimensions",
            true,
            Some(dims.len()),
            |ui| {
                for d in dims.iter().take(max) {
                    // The selected dimension's row (2D and 3D share it) on
                    // the selection's background, laid behind the row.
                    let behind = ui.painter().add(egui::Shape::Noop);
                    let top = ui.cursor().top();
                    ui.push_id(d.id, |ui| {
                        let title = label(project, board.as_deref(), placement.instance, d);
                        if d.reference || d.value.is_none() {
                            kit::value_row(
                                ui,
                                &title,
                                if d.reference { "Reference" } else { "Held" },
                            );
                        } else if let Some((_, text)) =
                            self.edge.values.iter_mut().find(|(id, _)| *id == d.id)
                        {
                            let invalid =
                                self.edge.error.as_ref().is_some_and(|(id, _)| *id == d.id);
                            let response =
                                kit::checked_text_row(ui, &title, text, "value", invalid);
                            if response.gained_focus() {
                                select = Some(d.id);
                            }
                            if response.lost_focus() && text.trim() != value_text(d) {
                                match typed_value(project, d, text) {
                                    Ok(value) => {
                                        let mut next = d.clone();
                                        next.value = Some(value);
                                        operation =
                                            Some(Operation::Dimension(DimensionCommand::Edit {
                                                dimension: next,
                                            }));
                                        self.edge.error = None;
                                        sent = Some(d.id);
                                    }
                                    Err(why) => self.edge.error = Some((d.id, why)),
                                }
                            }
                            if let Some((id, why)) = &self.edge.error
                                && *id == d.id
                            {
                                kit::field_error(ui, &response, why, true);
                            } else if let Some(DimValue::Expr(_) | DimValue::Param(_)) = &d.value
                                && let Some(v) = d.value.as_ref().and_then(|v| length(project, v))
                            {
                                kit::note(ui, &format!("= {} mm", kit::number_text(v)));
                            }
                        }
                        if kit::summary_row(ui, "", Some("Delete")) {
                            operation =
                                Some(Operation::Dimension(DimensionCommand::Delete { id: d.id }));
                        }
                    });
                    if selected == Some(d.id) {
                        let t = Tokens::get();
                        let row = egui::Rect::from_x_y_ranges(
                            ui.max_rect().x_range(),
                            top..=ui.cursor().top(),
                        );
                        ui.painter().set(
                            behind,
                            egui::Shape::rect_filled(row, t.radius_sm, t.selected_bg),
                        );
                    }
                }
                if dims.len() > max
                    && kit::summary_row(
                        ui,
                        &format!("{max} of {}", dims.len()),
                        Some("Show all \u{203a}"),
                    )
                {
                    self.edge.show_all = true;
                }
                kit::note(ui, "Follows the board when it changes");
            },
        );
        if let Some(id) = sent {
            refusal::mark(ui.ctx(), Origin::Field(field_id(id)));
            self.edge.sent = Some(id);
        }
        if select.is_some() {
            self.dimension = select;
        }
        operation
    }

    /// The selected part's distances to named board edges (every part's in
    /// the Board shape mode), as dimension lines from the edge to the
    /// feature, labelled with their values.
    pub(super) fn paint_part_dimensions(
        &mut self,
        painter: &egui::Painter,
        view: &BoardView,
        project: &Project,
    ) {
        self.edge.labels.clear();
        if project.part_dimensions.is_empty() {
            return;
        }
        let Some(evaluator) = self.shape.evaluator else {
            return;
        };
        let Ok(board) = dimensions::board_sketch(project, evaluator) else {
            return;
        };
        let solved = project.board();
        let t = Tokens::get();
        let font = type_face::medium(t.font_small);
        let color = t.sketch_accent;
        let stroke = Stroke::new(1.0, color);
        for d in project.part_dimensions.iter() {
            if !matches!(d.kind, PartRelation::Dim(DimKind::Distance)) {
                continue;
            }
            let (Some(BoardRef::Part(f)), Some(edge)) = (
                d.refs.iter().find(|r| matches!(r, BoardRef::Part(_))),
                d.refs.iter().find_map(|r| match r {
                    BoardRef::Board(SketchRef::Curve(c)) => dimensions::edge_name(&board, c),
                    _ => None,
                }),
            ) else {
                continue;
            };
            let ConstraintTarget::Placement(instance) = f.target() else {
                continue;
            };
            if !(self.shape.active
                || self.instance == Some(instance)
                || self.dimension == Some(d.id))
            {
                continue;
            }
            // The selected dimension in the selection's accent, as in 3D.
            let (color, stroke) = if self.dimension == Some(d.id) {
                (t.accent, Stroke::new(1.5, t.accent))
            } else {
                (color, stroke)
            };
            let (Some(placement), Some(edge)) = (
                project.placement_of(instance),
                solved
                    .edges
                    .iter()
                    .find(|e| e.name.0 == edge && e.bulge == 0.0),
            ) else {
                continue;
            };
            let Some(at) = feature_point(project, placement, f) else {
                continue;
            };
            let Some((along, offset)) = along_offset(edge, at) else {
                continue;
            };
            let len = edge_length(edge).max(1e-9);
            let u = [(edge.b[0] - edge.a[0]) / len, (edge.b[1] - edge.a[1]) / len];
            let foot = [edge.a[0] + u[0] * along, edge.a[1] + u[1] * along];
            let (f_screen, p_screen) = (view.screen(foot), view.screen(at));
            dimension(painter, f_screen, p_screen, stroke);
            if (p_screen - f_screen).length() > 12.0 {
                let mid = f_screen + (p_screen - f_screen) * 0.5;
                let text = format!("{} mm", kit::number_text(round_um(offset.abs())));
                let galley = painter.layout_no_wrap(text.clone(), font.clone(), color);
                let r = egui::Rect::from_center_size(mid, galley.size() + vec2(6.0, 2.0));
                painter.rect_filled(r, 2.0, t.panel);
                painter.text(mid, Align2::CENTER_CENTER, text, font.clone(), color);
                self.edge.labels.push((d.id, r));
            }
        }
    }
}

impl PcbEditor {
    /// The dimension whose label was drawn at `p` last frame.
    pub(super) fn dimension_label_at(&self, p: Pos2) -> Option<Id> {
        self.edge
            .labels
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(id, _)| *id)
    }
}

/// A part feature's point on the board (a line or arc's start, a circle's
/// centre), through the placement.
fn feature_point(project: &Project, placement: &Placement, f: &FeatureRef) -> Option<Point> {
    let at = match features::geometry(project, f).ok()? {
        FeatureGeometry::Point { at } => at,
        FeatureGeometry::Line { a, .. } | FeatureGeometry::Arc { a, .. } => a,
        FeatureGeometry::Circle { center, .. } => center,
    };
    Some(
        Transform2 {
            translate: placement.position,
            rotation_deg: placement.rotation_deg,
            mirror: placement.layer == Layer::Bottom,
        }
        .apply(at),
    )
}

/// A dimension line from `a` to `b` with arrowheads at both ends.
fn dimension(painter: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    let d = b - a;
    if d.length() < 1.0 {
        return;
    }
    painter.add(egui::Shape::dashed_line(&[a, b], stroke, 4.0, 2.0));
    let u = d.normalized();
    let n = vec2(-u.y, u.x);
    for (tip, dir) in [(a, -u), (b, u)] {
        let back = tip - dir * 7.0;
        painter.add(egui::Shape::convex_polygon(
            vec![tip, back + n * 2.4, back - n * 2.4],
            stroke.color,
            Stroke::NONE,
        ));
    }
}
