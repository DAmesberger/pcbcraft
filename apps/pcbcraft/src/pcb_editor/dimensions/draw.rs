//! How a part dimension is drawn, in either view: extension lines, the
//! dimension line with its arrows and the label, laid out by Solidcraft's
//! sketch dimension layout, stacked clear of each other, with the hover and
//! selection colours. A view hands its board → screen mapping ([`Surface`]):
//! the Layout view's (z ignored) or the 3D camera's.
use super::state::{
    self, Drawn, board_centre, height, label, part_held, to_vec, words_with_body, worse,
};
use crate::pcb_editor::{PcbEditor, Tool};
use egui::{FontId, Pos2, Rect, Shape, Stroke, Ui, vec2};
use pcbcraft_core::Project;
use pcbcraft_core::layout::dimensions::view::{self, Shown};
use pcbcraft_core::layout::dimensions::{self, FeatureGeometry};
use pcbcraft_core::library::footprint_source::DimKind;
use pcbcraft_core::model::dimension::{BoardRef, PartRelation, ReferenceStatus};
use pcbcraft_solidcraft::dims::{DimFrame, default_text, default_text_offset, dim_layout_reaching};
use pcbcraft_solidcraft::geom::Vec2;
use pcbcraft_ui::theme::Tokens;

/// A view's mapping of a board point (board mm, z up) to the screen.
pub(in crate::pcb_editor) trait Surface {
    fn to_screen(&self, p: [f64; 3]) -> Option<Pos2>;
}

/// Solidcraft's dimension arrowhead (its `markers::dimension_arrow`).
fn arrow(painter: &egui::Painter, tip: Pos2, back: Pos2, colour: egui::Color32) {
    let d = (tip - back).normalized();
    let n = vec2(-d.y, d.x);
    painter.add(Shape::convex_polygon(
        vec![tip, tip - d * 9.0 + n * 3.0, tip - d * 9.0 - n * 3.0],
        colour,
        Stroke::NONE,
    ));
}

/// How one dimension is drawn: its label, colour and line style.
pub(super) struct Look<'a> {
    pub(super) label: &'a str,
    pub(super) colour: egui::Color32,
    pub(super) dashed: bool,
}

/// Draw one linear dimension in the plane `z` above the board, laid out
/// by Solidcraft's sketch dimension layout: extension lines from the
/// measured points, the dimension line with its arrows, and the label
/// level on screen. A label that would land on one already drawn this
/// frame (`placed`) steps to the next row across the dimension line, as
/// Solidcraft's `dim_view::show` stacks sketch dimension texts. Its text
/// box and its lines on screen.
pub(super) fn draw(
    painter: &egui::Painter,
    surface: &dyn Surface,
    shown: &Shown,
    z: f64,
    away: [f64; 2],
    look: &Look<'_>,
    placed: &mut Vec<Rect>,
) -> Option<(Rect, Vec<[Pos2; 2]>)> {
    let t = Tokens::get();
    let colour = look.colour;
    let to = |p: Vec2| surface.to_screen([p.x, p.y, z]);
    let (p0, p1) = (to_vec(shown.from), to_vec(shown.to));
    let frame = DimFrame::Linear {
        p0,
        p1,
        dir: to_vec(shown.along),
    };
    let anchor = (p0 + p1) * 0.5;
    let (a, b) = (to(anchor)?, to(anchor + Vec2::new(1.0, 0.0))?);
    let (c, d) = (to(anchor), to(anchor + Vec2::new(0.0, 1.0)));
    // Millimetres per point at the dimension, the larger of the two axes'
    // (one may be foreshortened).
    let across = c.zip(d).map_or(0.0, |(c, d)| c.distance(d));
    let px = 1.0 / f64::from(a.distance(b).max(across)).max(1e-9);
    if !px.is_finite() {
        return None;
    }
    let font = FontId::monospace(t.font_body);
    let galley = painter.layout_no_wrap(look.label.to_owned(), font, colour);
    let size = galley.size() + vec2(6.0, 2.0);
    let half = Vec2::new(f64::from(size.x) * 0.5 * px, f64::from(size.y) * 0.5 * px);
    let mut text = default_text(&frame, to_vec(away), px, half);
    let mut lay = dim_layout_reaching(&frame, text, px, half, [None, None]);
    for _ in 0..6 {
        let Some(at) = to(lay.text) else { break };
        let here = Rect::from_center_size(at, size).expand(-0.5);
        if !placed.iter().any(|q| q.expand(-0.5).intersects(here)) {
            break;
        }
        let row = (f64::from(size.y) + 2.0) * px;
        let base = text.unwrap_or_else(|| default_text_offset(&frame, px, half));
        let side = if base.y < 0.0 { -1.0 } else { 1.0 };
        text = Some(base + Vec2::new(0.0, row * side));
        lay = dim_layout_reaching(&frame, text, px, half, [None, None]);
    }
    let stroke = Stroke::new(1.0, colour);
    // A dark rim under each line keeps it readable over the copper and
    // the parts' bodies.
    let rim = Stroke::new(3.0, t.viewport_top.gamma_multiply(0.6));
    let mut segs = Vec::new();
    for line in &lay.lines {
        let points: Vec<Pos2> = line.iter().filter_map(|q| to(*q)).collect();
        segs.extend(points.windows(2).map(|w| [w[0], w[1]]));
        painter.add(Shape::line(points.clone(), rim));
        if look.dashed {
            painter.extend(Shape::dashed_line(&points, stroke, 4.0, 3.0));
        } else {
            painter.add(Shape::line(points, stroke));
        }
    }
    for (tip, dir) in &lay.arrows {
        if let (Some(tip), Some(back)) = (to(*tip), to(*tip - *dir * px)) {
            arrow(painter, tip, back, colour);
        }
    }
    let centre = to(lay.text)?;
    let rect = Rect::from_center_size(centre, size);
    placed.push(rect);
    painter.rect_filled(
        rect,
        f32::from(t.radius_sm) / 2.0,
        t.viewport_top.gamma_multiply(0.85),
    );
    painter.galley(rect.min + vec2(3.0, 1.0), galley, colour);
    Some((rect, segs))
}

/// Trace a reference's geometry in the plane `z`: a line or arc along
/// itself, a circle round, a point as a dot.
pub(super) fn trace_reference(
    painter: &egui::Painter,
    surface: &dyn Surface,
    g: &FeatureGeometry,
    z: f64,
    stroke: Stroke,
) {
    let to = |p: [f64; 2]| surface.to_screen([p[0], p[1], z]);
    let points: Vec<[f64; 2]> = match *g {
        FeatureGeometry::Point { at } => {
            if let Some(p) = to(at) {
                painter.circle_filled(p, stroke.width + 1.0, stroke.color);
            }
            return;
        }
        FeatureGeometry::Line { a, b } => vec![a, b],
        FeatureGeometry::Arc { a, b, bulge } => arc_points(a, b, bulge),
        FeatureGeometry::Circle { center, radius } => (0..=48)
            .map(|i| {
                let t = std::f64::consts::TAU * f64::from(i) / 48.0;
                [center[0] + radius * t.cos(), center[1] + radius * t.sin()]
            })
            .collect(),
    };
    let points: Vec<Pos2> = points.into_iter().filter_map(to).collect();
    painter.add(Shape::line(points, stroke));
}

/// Points along a bulge arc from `a` to `b` (`library::Segment::Arc`:
/// the bulge is the tangent of a quarter of the counter-clockwise sweep).
pub(in crate::pcb_editor) fn arc_points(a: [f64; 2], b: [f64; 2], bulge: f64) -> Vec<[f64; 2]> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let chord = dx.hypot(dy);
    if !(chord > 1e-12 && bulge.is_finite() && bulge != 0.0) {
        return vec![a, b];
    }
    let sweep = 4.0 * bulge.atan();
    let radius = chord / (2.0 * (sweep / 2.0).sin());
    // The centre: from the chord's middle, square to it to the left of
    // a → b for a counter-clockwise sweep under half a turn.
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let off = radius * (sweep / 2.0).cos();
    let n = [-dy / chord, dx / chord];
    let c = [mid[0] + n[0] * off, mid[1] + n[1] * off];
    let start = (a[1] - c[1]).atan2(a[0] - c[0]);
    let r = radius.abs();
    (0..=24)
        .map(|i| {
            let t = start + sweep * f64::from(i) / 24.0;
            [c[0] + r * t.cos(), c[1] + r * t.sin()]
        })
        .collect()
}

/// The point a dimension's label is put away from: its middle mirrored
/// through the board's centre, so the label sits on the board's inner
/// side, readable over the board and clear of the edges the tool picks.
fn inward(centre: [f64; 2], shown: &Shown) -> [f64; 2] {
    let mid = [
        (shown.from[0] + shown.to[0]) / 2.0,
        (shown.from[1] + shown.to[1]) / 2.0,
    ];
    [2.0 * mid[0] - centre[0], 2.0 * mid[1] - centre[1]]
}

impl PcbEditor {
    /// The dimensions drawn, in either view: the selected part's and the
    /// selected one, every one while the Dimension tool is on, and the one
    /// being edited; then the tool's draft. `live` is false while a newer
    /// board model is made: no hover then.
    pub(super) fn paint_dimensions(
        &mut self,
        ui: &Ui,
        painter: &egui::Painter,
        surface: &dyn Surface,
        project: &Project,
        live: bool,
    ) {
        let t = Tokens::get();
        let evaluator = self.shape.evaluator;
        let hover = ui.input(|i| i.pointer.hover_pos()).filter(|p| {
            live && painter.clip_rect().contains(*p)
                && ui.rect_contains_pointer(painter.clip_rect())
        });
        let armed = self.tool == Tool::Dimension;
        let hovered = hover.and_then(|p| {
            if armed {
                self.dims.hit_text(p)
            } else {
                self.dims.hit(p)
            }
        });
        self.dims.hovered = hovered;
        // Under the pointer here, or its Properties row under the pointer.
        let row = self.hovered_dimension(ui.ctx());
        let hot_hover = hovered.or(row);
        let editing = self.dims.edit.as_ref().map(|e| e.id);
        let mut drawn = Vec::new();
        let mut placed = Vec::new();
        let centre = board_centre(project);
        let findings = if project.part_dimensions.is_empty() {
            Vec::new()
        } else {
            dimensions::checks(project)
        };
        for d in project.part_dimensions.iter() {
            if !matches!(d.kind, PartRelation::Dim(DimKind::Distance)) {
                continue;
            }
            let part = part_held(d);
            let shows = armed
                || self.dimension == Some(d.id)
                || editing == Some(d.id)
                || row == Some(d.id)
                || part.is_some_and(|p| self.instance == Some(p) || self.also.contains(&p));
            if !shows {
                continue;
            }
            let (Some(a), Some(b)) = (d.refs.first(), d.refs.get(1)) else {
                continue;
            };
            let (Some((ga, sa)), Some((gb, sb))) = (
                view::reference_geometry(project, evaluator, a),
                view::reference_geometry(project, evaluator, b),
            ) else {
                continue;
            };
            let Some(shown) = view::shown(&ga, &gb) else {
                continue;
            };
            let status = worse(sa, sb);
            let z = height(project, a).max(height(project, b));
            let away = inward(centre, &shown);
            let mut text = label(shown.value, Some(d));
            let (colour, dashed) = match status {
                ReferenceStatus::Lost => {
                    text.push_str(" · lost");
                    (t.error, true)
                }
                ReferenceStatus::Inexact => {
                    text.push_str(" · inexact");
                    (t.warning, true)
                }
                ReferenceStatus::Resolved => (t.dimension, false),
            };
            let hot =
                self.dimension == Some(d.id) || hot_hover == Some(d.id) || editing == Some(d.id);
            let colour = if hot && status == ReferenceStatus::Resolved {
                t.accent
            } else {
                colour
            };
            if hot {
                // What it measures between, traced in its colour.
                for g in [&ga, &gb] {
                    trace_reference(painter, surface, g, z, Stroke::new(2.0, colour));
                }
            }
            let look = Look {
                label: &text,
                colour,
                dashed,
            };
            let Some((rect, segs)) = draw(painter, surface, &shown, z, away, &look, &mut placed)
            else {
                continue;
            };
            if live {
                // On hover: what it measures between, each by name with the
                // body its face is on; a lost or inexact reference says why.
                let mut why: Vec<String> = vec![format!(
                    "{} to {}",
                    state::capital(&words_with_body(project, evaluator, a, None)),
                    words_with_body(project, evaluator, b, None)
                )];
                if status != ReferenceStatus::Resolved
                    && let Some(part) = part
                {
                    why.extend(
                        findings
                            .iter()
                            .filter(|f| {
                                f.code == dimensions::REFERENCE_LOST
                                    || f.code == dimensions::REFERENCE_INEXACT
                            })
                            .filter(|f| {
                                f.objects.iter().any(|o| {
                                    matches!(o, pcbcraft_core::ObjectRef::Instance { id } if *id == part)
                                })
                            })
                            .map(|f| f.message.clone()),
                    );
                }
                ui.interact(
                    rect,
                    egui::Id::unique(("pcb-dimension-why", d.id)),
                    egui::Sense::hover(),
                )
                .on_hover_text(why.join("\n"));
            }
            drawn.push(Drawn {
                id: d.id,
                text: rect,
                segs,
                ends: [
                    [shown.from[0], shown.from[1], z],
                    [shown.to[0], shown.to[1], z],
                ],
                label: text,
                hot,
            });
        }
        self.dims.drawn = drawn;
        // The draft: the picks so far, and their dimension once measured.
        self.dims.draft_at = None;
        if armed && live {
            for p in &self.dims.picks {
                match p.geometry {
                    FeatureGeometry::Point { at } | FeatureGeometry::Circle { center: at, .. } => {
                        if let Some(s) = surface.to_screen([at[0], at[1], p.z]) {
                            painter.circle_stroke(s, 5.0, Stroke::new(1.5, t.accent));
                        }
                    }
                    ref g => trace_reference(painter, surface, g, p.z, Stroke::new(2.0, t.accent)),
                }
            }
            if let (Some(shown), [a, b]) = (self.draft_shown(), self.dims.picks.as_slice()) {
                let typed = self
                    .dims
                    .value
                    .as_deref()
                    .and_then(|v| crate::pcb_editor::edge_card::length_value(project, v).ok())
                    .and_then(|v| crate::pcb_editor::edge_card::length(project, &v));
                let text = label(typed.unwrap_or(shown.value), None);
                let z = a.z.max(b.z);
                let away = match (&a.reference, &b.reference) {
                    (r @ BoardRef::Part(_), _) | (_, r @ BoardRef::Part(_)) => state::part_of(r)
                        .and_then(|id| project.placement_of(id))
                        .map(|p| p.position),
                    _ => None,
                }
                .unwrap_or(shown.from);
                let look = Look {
                    label: &text,
                    colour: t.accent,
                    dashed: false,
                };
                self.dims.draft_at = draw(painter, surface, &shown, z, away, &look, &mut placed)
                    .map(|(r, _)| r.center());
            }
        }
    }
}
