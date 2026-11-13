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
//! The dimensions are a `kit::list` (list-system spec): each row reads
//! "Front face → bottom edge" with its decimal-aligned value (`fx expr =
//! value` when driven) and a Lost or Inexact badge. Double-click, Enter or
//! F2 edits the value in its row (one `layout.dimension_edit`); the row's
//! delete action or ⌫ removes it (`layout.dimension_delete`) and leaves
//! the part where it is; the grip or ⌥↑/⌥↓ reorders the part's dimensions
//! (`layout.dimension_order`, document data, one undo step). Hovering a
//! row lights its dimension on the canvas and in 3D; clicking it selects
//! the dimension there, and a dimension selected there selects its row.
use super::PcbEditor;
use super::board_shape_ui::sketch_of;
use crate::kit;
use crate::kit::list::{
    self, Action, Columns, Effect, Layout, Line, ListState, ListTokens, ListView, Row, RowKey,
    Value,
};
use crate::refusal::{self, Origin};
use egui::Ui;
use sparksmith_core::layout::dimensions::{self, DimensionCommand, PartFreedom, view};
use sparksmith_core::layout::outline::BoardShapeEvaluator;
use sparksmith_core::library::footprint_source::{
    DimKind, DimValue, Expr, FootprintParam, FootprintSource, ParamName, ParamUnit, ParamValue,
    SketchRef,
};
use sparksmith_core::model::board::{BoardSketch, ConstraintTarget, SolvedBoard, SolvedEdge};
use sparksmith_core::model::dimension::{
    BoardRef, FeatureRef, PartDimension, PartDimensionSpec, PartRelation, ReferenceStatus,
};
use sparksmith_core::{Id, Operation, Placement, Point, Project};
use sparksmith_solidcraft::footprint::view as sketch_view;

/// The Dimensions section's text buffers, and what the part's dimensions
/// leave free, for one document revision.
#[derive(Clone, Debug, Default)]
pub(super) struct EdgeEdit {
    key: Option<(u64, Id)>,
    /// The dimension whose value is being edited in its row, the text, and
    /// whether the field takes the keyboard this frame.
    editing: Option<(Id, String, bool)>,
    /// The dimension a refusal or invalid text belongs to, and why.
    error: Option<(Id, String)>,
    sent: Option<Id>,
    /// The selected part's hold: (revision, instance) → why a move is
    /// refused, and what its dimensions leave free.
    hold: Option<((u64, Id), Hold)>,
    /// The dimension whose row is under the pointer, and the frame.
    hover: Option<(Id, u64)>,
}

/// The Dimensions list's id: its selection, cursor and fold state.
fn list_id() -> egui::Id {
    egui::Id::unique("pcb-dimensions-list")
}

/// A dimension's row.
fn row_key(dimension: Id) -> RowKey {
    RowKey::of(("dimension", dimension))
}

fn delete(_ctx: &egui::Context, id: Id) -> Operation {
    Operation::Dimension(DimensionCommand::Delete { id })
}

/// `layout.dimension_order` moving the dimension at `from` by `by` places
/// among the part's dimensions; `None` when it stays where it is.
fn reorder(dims: &[PartDimension], from: usize, by: i32) -> Option<Operation> {
    let to = (from as i64 + i64::from(by)).clamp(0, dims.len() as i64 - 1) as usize;
    if to == from {
        return None;
    }
    let mut ids: Vec<Id> = dims.iter().map(|d| d.id).collect();
    let moved = ids.remove(from);
    ids.insert(to, moved);
    Some(Operation::Dimension(DimensionCommand::Order { ids }))
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
pub(super) fn value_text(d: &PartDimension) -> String {
    let unit = if is_angle(d) { "°" } else { "mm" };
    match &d.value {
        Some(DimValue::Fixed(v)) if unit == "°" => format!("{}°", kit::number_text(*v)),
        Some(DimValue::Fixed(v)) => format!("{} mm", kit::number_text(*v)),
        Some(DimValue::Param(p)) => p.0.clone(),
        Some(DimValue::Expr(e)) => e.0.clone(),
        None => String::new(),
    }
}

/// What an expression or parameter drives a dimension to ("20 mm"); `None`
/// for a plain number.
fn driven(project: &Project, d: &PartDimension) -> Option<String> {
    match &d.value {
        Some(v @ (DimValue::Expr(_) | DimValue::Param(_))) => {
            length(project, v).map(|mm| format!("{} mm", kit::number_text(mm)))
        }
        _ => None,
    }
}

/// The value typed into a dimension's field, or why it is not one.
pub(super) fn typed_value(
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

/// What a dimension of `instance` holds, from the part's side, as the two
/// references it runs between: ["Origin", "right edge"], ["Pad 1",
/// "bottom edge"], ["Front face", "U3"]. The card joins them with an
/// arrow ("Origin → right edge").
pub(super) fn name_parts(
    project: &Project,
    board: Option<&BoardSketch>,
    instance: Id,
    d: &PartDimension,
) -> Vec<String> {
    let words = |r: &BoardRef| -> String {
        match r {
            BoardRef::Board(SketchRef::Curve(c)) => {
                board.and_then(|b| dimensions::edge_name(b, c)).map_or_else(
                    || "board curve".into(),
                    |n| {
                        dimensions::edge_words(n)
                            .trim_start_matches("the ")
                            .to_owned()
                    },
                )
            }
            BoardRef::Board(_) => "board point".into(),
            BoardRef::FollowedFace(_) => "followed face".into(),
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
                    sparksmith_core::model::dimension::PartAxis::X => "x axis".into(),
                    sparksmith_core::model::dimension::PartAxis::Y => "y axis".into(),
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
    let mut parts: Vec<String> = d.refs.iter().map(words).collect();
    if let Some(first) = parts.first_mut() {
        let mut chars = first.chars();
        *first = chars.next().map_or_else(String::new, |c| {
            c.to_uppercase().chain(chars).collect::<String>()
        });
    }
    // A horizontal and a vertical dimension between the same references
    // would read alike: the last part names the axis.
    let axis = match d.kind {
        PartRelation::Dim(DimKind::HorizontalDistance) => Some("x"),
        PartRelation::Dim(DimKind::VerticalDistance) => Some("y"),
        _ => None,
    };
    if let (Some(axis), Some(last)) = (axis, parts.last_mut()) {
        last.push_str(" · ");
        last.push_str(axis);
    }
    parts
}

/// The worse status of a dimension's references, where they read as one
/// (a lost or inexact reference shows as a badge on its row).
fn reference_status(
    project: &Project,
    evaluator: Option<&dyn BoardShapeEvaluator>,
    d: &PartDimension,
) -> ReferenceStatus {
    use ReferenceStatus::*;
    d.refs
        .iter()
        .filter_map(|r| view::reference_geometry(project, evaluator, r).map(|(_, s)| s))
        .fold(Resolved, |a, b| match (a, b) {
            (Lost, _) | (_, Lost) => Lost,
            (Inexact, _) | (_, Inexact) => Inexact,
            _ => Resolved,
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
        let why = sparksmith_core::layout::move_refusal(project, instance, self.shape.evaluator)
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
        // Another part: its list starts bounded and not editing.
        let fresh = self.edge.key.map(|k| k.1) != Some(placement.instance);
        if fresh {
            self.edge.editing = None;
        }
        if self.edge.key != Some(key) {
            self.edge.key = Some(key);
            // The document changed (an edit applied, an undo): no earlier
            // text's error stands. A refused edit leaves the revision as it
            // was, so its error is kept.
            self.edge.error = None;
            self.edge.sent = None;
            self.edge.editing = None;
        }
        let board = self
            .shape
            .evaluator
            .and_then(|ev: &dyn BoardShapeEvaluator| dimensions::board_sketch(project, ev).ok());
        let names: Vec<Vec<String>> = dims
            .iter()
            .map(|d| name_parts(project, board.as_deref(), placement.instance, d))
            .collect();
        let joined: Vec<String> = names.iter().map(|n| n.join(list::row::ARROW)).collect();
        let texts: Vec<String> = dims.iter().map(value_text).collect();
        let results: Vec<Option<String>> = dims.iter().map(|d| driven(project, d)).collect();
        let keys: Vec<RowKey> = dims.iter().map(|d| row_key(d.id)).collect();
        let evaluator = self.shape.evaluator;
        let badges: Vec<Option<(kit::severity::Level, &str)>> = dims
            .iter()
            .map(|d| match reference_status(project, evaluator, d) {
                ReferenceStatus::Lost => Some((kit::severity::Level::Error, "Lost")),
                ReferenceStatus::Inexact => Some((kit::severity::Level::Warning, "Inexact")),
                ReferenceStatus::Resolved => None,
            })
            .collect();
        let deletes: Vec<String> = joined
            .iter()
            .map(|n| format!("Delete dimension {n}"))
            .collect();
        let id = list_id();
        let ctx = ui.ctx().clone();
        // The one selection, shared with 2D and 3D: the list shows it.
        let selected = self.dimension;
        let mut state = ListState::load(&ctx, id);
        let selected_key = selected
            .filter(|s| dims.iter().any(|d| d.id == *s))
            .map(row_key);
        if state.selection.iter().next().copied() != selected_key {
            state.selection = selected_key.into_iter().collect();
            if let Some(k) = selected_key {
                state.cursor = Some(k);
                state.anchor = Some(k);
                // Selected in 2D or 3D past the first rows: the whole list.
                if keys.iter().position(|x| *x == k) >= Some(kit::LIST_MAX_ROWS) {
                    state.show_all = true;
                }
            }
        }
        if fresh {
            state.show_all = false;
        }
        state.store(&ctx, id);
        let editing = self.edge.editing.as_ref().map(|(d, _, _)| *d);
        let mut operation = None;
        let mut sent = None;
        let mut select = None;
        kit::section(
            ui,
            "pcb-dimensions",
            "Dimensions",
            true,
            Some(dims.len()),
            |ui| {
                let l = ListTokens::get(ui.ctx());
                let values: Vec<Value<'_>> = dims
                    .iter()
                    .zip(&results)
                    .zip(&texts)
                    .map(|((d, r), text)| match r {
                        Some(r) => Value::Expr {
                            expr: text,
                            value: r,
                        },
                        None if d.reference => Value::Text("Reference"),
                        None if d.value.is_none() => Value::Text("Held"),
                        None => Value::Number(text),
                    })
                    .collect();
                let cols = Columns::measure(ui, &l, values.iter().copied());
                let lines: Vec<Line<'_>> = keys
                    .iter()
                    .zip(&joined)
                    .map(|(k, n)| Line::item(*k, n))
                    .collect();
                let view = ListView {
                    multiple: false,
                    reorder: true,
                    delete: true,
                    editing: editing.is_some(),
                    ..ListView::new(
                        id,
                        "Dimensions",
                        Layout::Bounded {
                            rows: kit::LIST_MAX_ROWS,
                        },
                    )
                };
                let out = view.show(ui, &lines, |ui, rect, line, cx| {
                    let i = keys.iter().position(|k| *k == line.key).unwrap_or(0);
                    let parts: Vec<&str> = names
                        .get(i)
                        .map(|n| n.iter().map(String::as_str).collect())
                        .unwrap_or_default();
                    let actions = [Action::delete(deletes.get(i).map_or("", String::as_str))];
                    let mut row = Row::new(line.key, &parts);
                    row.grip = true;
                    row.selected = cx.selected;
                    row.cursor = cx.cursor;
                    row.badge = badges.get(i).copied().flatten();
                    row.value = if dims.get(i).map(|d| d.id) == editing {
                        Value::None
                    } else {
                        values.get(i).copied().unwrap_or_default()
                    };
                    row.actions = &actions;
                    row.role = egui::Role::ListBoxOption;
                    row.flash = list::motion::flash(ui.ctx(), id, line.key);
                    list::row::paint(ui, cx.id, rect, &row, &cols)
                });
                let at = |k: &RowKey| keys.iter().position(|x| x == k);
                for effect in &out.effects {
                    match effect {
                        Effect::Selected => {
                            select = Some(
                                out.state
                                    .selection
                                    .iter()
                                    .next()
                                    .and_then(&at)
                                    .and_then(|i| dims.get(i))
                                    .map(|d| d.id),
                            );
                        }
                        Effect::Activate(k) => {
                            if let Some(d) = at(k).and_then(|i| dims.get(i))
                                && !(d.reference || d.value.is_none())
                            {
                                select = Some(Some(d.id));
                                self.edge.editing = Some((d.id, value_text(d), true));
                            }
                        }
                        Effect::CancelEdit => {
                            self.edge.editing = None;
                            self.edge.error = None;
                        }
                        Effect::Delete(gone) => {
                            if let Some(d) = gone.first().and_then(at).and_then(|i| dims.get(i)) {
                                operation = Some(delete(ui.ctx(), d.id));
                            }
                        }
                        Effect::Reorder(k, by) => {
                            if let Some(from) = at(k) {
                                operation = reorder(&dims, from, *by);
                            }
                        }
                        _ => {}
                    }
                }
                for (k, _) in &out.actions {
                    if let Some(d) = at(k).and_then(|i| dims.get(i)) {
                        operation = Some(delete(ui.ctx(), d.id));
                    }
                }
                self.edge.hover = out
                    .hovered
                    .and_then(|k| at(&k))
                    .and_then(|i| dims.get(i))
                    .map(|d| (d.id, ui.ctx().cumulative_frame_nr()));
                // The value being edited, in its row.
                if let Some((dim, text, start)) = &mut self.edge.editing
                    && let Some((_, rect)) =
                        out.value_rects.iter().find(|(k, _)| *k == row_key(*dim))
                {
                    let invalid = self.edge.error.as_ref().is_some_and(|(e, _)| e == dim);
                    let label = dims
                        .iter()
                        .position(|d| d.id == *dim)
                        .and_then(|i| joined.get(i))
                        .cloned()
                        .unwrap_or_default();
                    let edit =
                        list::edit::field(ui, id.with(*dim), *rect, text, invalid, start, &label);
                    if edit != list::edit::Edit::Open {
                        // The rows take the keyboard back.
                        view.request_focus(ui.ctx());
                    }
                    let d = dims.iter().find(|d| d.id == *dim);
                    match (edit, d) {
                        (list::edit::Edit::Cancel, _) | (_, None) => {
                            self.edge.editing = None;
                            self.edge.error = None;
                        }
                        (list::edit::Edit::Open, _) => {}
                        (commit, Some(d)) => {
                            let text = text.clone();
                            if text.trim() == value_text(d) {
                                self.edge.editing = None;
                                self.edge.error = None;
                            } else {
                                match typed_value(project, d, &text) {
                                    Ok(value) => {
                                        let mut next = d.clone();
                                        next.value = Some(value);
                                        operation =
                                            Some(Operation::Dimension(DimensionCommand::Edit {
                                                dimension: next,
                                            }));
                                        list::motion::start_flash(ui.ctx(), id, row_key(d.id));
                                        self.edge.error = None;
                                        self.edge.editing = None;
                                        sent = Some(d.id);
                                    }
                                    Err(why) => {
                                        self.edge.error = Some((d.id, why));
                                        // Keep the text, back in the field.
                                        if let Some(e) = &mut self.edge.editing {
                                            e.2 = true;
                                        }
                                    }
                                }
                            }
                            // Tab: the next editable row.
                            if commit == list::edit::Edit::CommitNext
                                && let Some(next) = dims
                                    .iter()
                                    .skip_while(|x| x.id != d.id)
                                    .skip(1)
                                    .find(|x| !(x.reference || x.value.is_none()))
                                && self.edge.error.is_none()
                            {
                                select = Some(Some(next.id));
                                self.edge.editing = Some((next.id, value_text(next), true));
                            }
                        }
                    }
                }
                // Why the value was refused, under the list, naming the row.
                if let Some((dim, why)) = &self.edge.error
                    && let Some(i) = dims.iter().position(|d| d.id == *dim)
                {
                    let name = joined.get(i).map_or("", String::as_str);
                    list::row::field_error(ui, &format!("{name}: {why}"), 0.0);
                }
                kit::note(ui, "Follows the board when it changes");
            },
        );
        if let Some(id) = sent {
            refusal::mark(ui.ctx(), Origin::Field(field_id(id)));
            self.edge.sent = Some(id);
        }
        if let Some(select) = select {
            self.dimension = select;
        }
        operation
    }

    /// The dimension whose Properties row is under the pointer (this frame
    /// or the last), lit in 2D and 3D as if selected.
    pub(super) fn hovered_dimension(&self, ctx: &egui::Context) -> Option<Id> {
        self.edge
            .hover
            .filter(|(_, frame)| frame + 1 >= ctx.cumulative_frame_nr())
            .map(|(id, _)| id)
    }
}
