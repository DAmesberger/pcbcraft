//! Edge-relative placement: the Board edge section of the Placement card
//! and the dimension annotations that show a constrained part's distance
//! along and off its edge on the Layout canvas.
//!
//! "Constrain to edge" (the Placement section's state row) ties the
//! selected part to the nearest named board edge at its current position
//! (`board.constrain`, one undo step), so the part does not move. Edge,
//! anchor, along, offset, alignment and turning
//! with the edge then commit live, each as one `board.constraint_edit`;
//! along and offset are expressions over the board parameters
//! (`width / 2 - 3 mm`), so editing a parameter moves the part when the
//! board re-solves (`layout::outline`).
use super::board_shape_ui::sketch_of;
use super::{BoardView, PcbEditor};
use crate::kit;
use crate::refusal::{self, Origin};
use egui::{Align2, Pos2, Stroke, Ui, vec2};
use pcbcraft_core::layout::outline::{BoardCommand, edge_pose};
use pcbcraft_core::library::footprint_source::{
    Expr, FootprintParam, FootprintSource, ParamName, ParamUnit, ParamValue,
};
use pcbcraft_core::model::board::{
    ConstraintAlign, ConstraintTarget, EdgeAnchor, EdgeConstraint, EdgeConstraintSpec, EdgeName,
    SolvedBoard, SolvedEdge,
};
use pcbcraft_core::{Id, Operation, Placement, Point, Project};
use pcbcraft_solidcraft::footprint::view as sketch_view;
use solidcraft_ui_egui::{fonts, theme::Tokens};

/// The Board edge section's text buffers for one constraint.
#[derive(Clone, Debug, Default)]
pub(super) struct EdgeEdit {
    key: Option<(u64, Id)>,
    along: String,
    offset: String,
    /// The field (`"along"`/`"offset"`) a refusal or invalid text belongs
    /// to, and why.
    error: Option<(&'static str, String)>,
    sent: Option<&'static str>,
}

fn field_id(field: &'static str) -> egui::Id {
    egui::Id::new(("pcb-edge", field))
}

/// The constraint on a placement, if any.
pub(super) fn constraint_of(project: &Project, instance: Id) -> Option<&EdgeConstraint> {
    project
        .edge_constraints
        .iter()
        .find(|c| c.target == ConstraintTarget::Placement(instance))
}

fn mm(v: f64) -> Expr {
    Expr(format!("{} mm", kit::number_text(v)))
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

fn anchor_at(edge: &SolvedEdge, anchor: EdgeAnchor) -> f64 {
    let len = edge_length(edge);
    match anchor {
        EdgeAnchor::Start => 0.0,
        EdgeAnchor::Middle => len / 2.0,
        EdgeAnchor::End => len,
        EdgeAnchor::Fraction(f) => f * len,
    }
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

/// The constraint that keeps a part at `position` relative to `edge`.
pub(super) fn spec_at(
    instance: Id,
    edge: &SolvedEdge,
    position: Point,
) -> Option<EdgeConstraintSpec> {
    let (along, offset) = along_offset(edge, position)?;
    Some(EdgeConstraintSpec {
        target: ConstraintTarget::Placement(instance),
        edge: edge.name.clone(),
        anchor: EdgeAnchor::Start,
        along: mm(along),
        offset: mm(offset),
        align: ConstraintAlign::Origin,
        follow_edge: false,
        rotation_deg: 0.0,
    })
}

/// The board parameters expressions read (none on a rectangular board).
fn parameters(project: &Project) -> Vec<FootprintParam> {
    sketch_of(project)
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

fn anchor_options() -> [(EdgeAnchor, String); 3] {
    [
        (EdgeAnchor::Start, "Edge start"),
        (EdgeAnchor::Middle, "Edge middle"),
        (EdgeAnchor::End, "Edge end"),
    ]
    .map(|(v, l)| (v, l.to_owned()))
}

fn align_options() -> [(ConstraintAlign, String); 3] {
    [
        (ConstraintAlign::Origin, "Part origin"),
        (ConstraintAlign::NearSide, "Courtyard near side"),
        (ConstraintAlign::Centre, "Courtyard centre"),
    ]
    .map(|(v, l)| (v, l.to_owned()))
}

impl PcbEditor {
    /// BOARD EDGE, for a constrained part only: the constraint's edge,
    /// anchor, along, offset, alignment and turning, each committed live.
    /// "Constrain to edge" and "Free part" are the Placement section's
    /// state row.
    pub(super) fn edge_section(
        &mut self,
        ui: &mut Ui,
        revision: u64,
        project: &Project,
        placement: &Placement,
    ) -> Option<Operation> {
        if let Some(field) = self.edge.sent
            && let Some(text) = refusal::claim(ui.ctx(), Origin::Field(field_id(field)))
        {
            self.edge.error = Some((field, text));
            self.edge.sent = None;
        }
        let board = project.board();
        // A free part's "Constrain to edge" is the Placement section's
        // state row; this section edits a constraint that exists.
        let c = constraint_of(project, placement.instance).cloned()?;
        let mut operation = None;
        let mut sent = None;
        kit::section(ui, "pcb-edge", "Board edge", true, None, |ui| {
            let key = (revision, c.id);
            if self.edge.key != Some(key) {
                self.edge.key = Some(key);
                self.edge.along = c.along.0.clone();
                self.edge.offset = c.offset.0.clone();
                if self.edge.sent.is_none() {
                    self.edge.error = None;
                }
            }
            let edit = |f: &dyn Fn(&mut EdgeConstraint)| {
                let mut next = c.clone();
                f(&mut next);
                (next != c).then_some(Operation::Board(BoardCommand::EditConstraint {
                    constraint: next,
                }))
            };
            // The edge: switching keeps the part where it is.
            let mut edge = c.edge.clone();
            let options: Vec<(EdgeName, String)> = board
                .edges
                .iter()
                .map(|e| (e.name.clone(), e.name.0.clone()))
                .chain(
                    (!board.edges.iter().any(|e| e.name == c.edge))
                        .then(|| (c.edge.clone(), format!("{} (missing)", c.edge.0))),
                )
                .collect();
            if kit::choice_row(ui, "Edge", &mut edge, &options)
                && let Some(e) = board.edge(&edge)
                && let Some(spec) = spec_at(placement.instance, e, placement.position)
            {
                operation = edit(&|n| {
                    n.edge = spec.edge.clone();
                    n.anchor = EdgeAnchor::Start;
                    n.along = spec.along.clone();
                    n.offset = spec.offset.clone();
                });
            }
            let mut anchor = match c.anchor {
                EdgeAnchor::Fraction(_) => EdgeAnchor::Start,
                a => a,
            };
            if kit::choice_row(ui, "Measured from", &mut anchor, &anchor_options()) {
                // A plain distance is kept in place; an expression is kept.
                let moved = board.edge(&c.edge).and_then(|e| {
                    kit::parse_number(&c.along.0, "mm")
                        .map(|along| mm(along + anchor_at(e, c.anchor) - anchor_at(e, anchor)))
                });
                operation = edit(&|n| {
                    n.anchor = anchor;
                    if let Some(along) = &moved {
                        n.along = along.clone();
                    }
                });
            }
            for (field, label) in [("along", "Along"), ("offset", "Offset")] {
                let text = if field == "along" {
                    &mut self.edge.along
                } else {
                    &mut self.edge.offset
                };
                let stored = if field == "along" {
                    &c.along
                } else {
                    &c.offset
                };
                let invalid = self.edge.error.as_ref().is_some_and(|(f, _)| *f == field);
                let response = kit::checked_text_row(ui, label, text, "distance", invalid);
                if response.lost_focus() && text.trim() != stored.0 {
                    match expression_error(project, text) {
                        None => {
                            let value = Expr(text.trim().to_owned());
                            operation = edit(&|n| {
                                if field == "along" {
                                    n.along = value.clone();
                                } else {
                                    n.offset = value.clone();
                                }
                            });
                            self.edge.error = None;
                            sent = Some(field);
                        }
                        Some(why) => self.edge.error = Some((field, why)),
                    }
                }
                if let Some((f, why)) = &self.edge.error
                    && *f == field
                {
                    kit::field_error(ui, &response, why, true);
                } else if let Some(v) = evaluate(project, stored)
                    && kit::parse_number(&stored.0, "mm").is_none()
                {
                    kit::note(ui, &format!("= {} mm", kit::number_text(v)));
                }
            }
            let mut align = c.align;
            if kit::choice_row(ui, "Aligns", &mut align, &align_options()) {
                operation = edit(&|n| n.align = align);
            }
            let mut follow = c.follow_edge;
            if kit::check_row(ui, "Turn with edge", &mut follow) {
                operation = edit(&|n| n.follow_edge = follow);
            }
            if c.follow_edge {
                let mut rotation = c.rotation_deg;
                if kit::number_row(ui, "Extra turn", &mut rotation, "°", -360.0..=360.0) {
                    operation = edit(&|n| n.rotation_deg = rotation);
                }
            }
            kit::note(ui, "Follows the edge when the board changes");
        });
        if let Some(field) = sent {
            refusal::mark(ui.ctx(), Origin::Field(field_id(field)));
            self.edge.sent = Some(field);
        }
        operation
    }

    /// The selected part's edge constraint (every part's in the Board
    /// shape mode) as two dimensions: along the edge from its anchor, and
    /// off the edge to the part.
    pub(super) fn paint_edge_constraints(
        &self,
        painter: &egui::Painter,
        view: &BoardView,
        project: &Project,
    ) {
        let t = Tokens::get();
        let board = project.board();
        let font = fonts::medium(t.font_small);
        for c in &project.edge_constraints {
            let ConstraintTarget::Placement(instance) = c.target else {
                continue;
            };
            if !(self.shape.active || self.instance == Some(instance)) {
                continue;
            }
            let Some(edge) = board.edge(&c.edge) else {
                continue;
            };
            let (Some(along), Some(offset)) =
                (evaluate(project, &c.along), evaluate(project, &c.offset))
            else {
                continue;
            };
            let Ok(pose) = edge_pose(edge, c.anchor, along, offset) else {
                continue;
            };
            let Ok(foot) = edge_pose(edge, c.anchor, along, 0.0) else {
                continue;
            };
            let Ok(start) = edge_pose(edge, c.anchor, 0.0, 0.0) else {
                continue;
            };
            let color = t.sketch_accent;
            let stroke = Stroke::new(1.0, color);
            let (s, f, p) = (
                view.screen(start.position),
                view.screen(foot.position),
                view.screen(pose.position),
            );
            dimension(painter, s, f, stroke);
            dimension(painter, f, p, stroke);
            painter.circle_stroke(s, 3.0, stroke);
            let label = |a: Pos2, b: Pos2, text: &str| {
                if (b - a).length() > 12.0 {
                    let mid = a + (b - a) * 0.5;
                    let galley = painter.layout_no_wrap(text.to_owned(), font.clone(), color);
                    let r = egui::Rect::from_center_size(mid, galley.size() + vec2(6.0, 2.0));
                    painter.rect_filled(r, 2.0, t.panel);
                    painter.text(mid, Align2::CENTER_CENTER, text, font.clone(), color);
                }
            };
            label(s, f, &c.along.0);
            label(f, p, &c.offset.0);
        }
    }
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
