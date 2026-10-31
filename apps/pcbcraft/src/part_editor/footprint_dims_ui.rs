//! The Footprint sketch overlay on the canvas (slice F4): sketch curves and
//! points coloured by the solver (Solidcraft's free, defined and conflict
//! colours), dimensions laid out by Solidcraft's `dim_layout` with their
//! values and bound parameters, constraint glyphs, the Dimension and
//! Constrain tools, inline value editing and the live point drag
//! (`LiveSketch::drag`, one `Budget::frame` per frame). Nothing here writes
//! the document: every finished gesture queues the edits of one
//! `library.edit_footprint` in the sketch tools' outbox, which
//! [`PartEditor::footprint_tool_operation`] sends.
//!
//! The glyph icons and the dimension look follow Solidcraft's
//! `sketch_tools.rs` and `dim_view.rs` (pinned revision; see
//! ATTRIBUTION.md).
use super::PartEditor;
use super::footprint_dims::{
    self as dims, ConstrainPicks, DimInput, DimPicks, InlineEdit, NewDimension, PointDrag,
    ValueField,
};
use super::footprint_tools::FootprintTool;
use crate::symbol_view::View;
use egui::{Align2, Pos2, Rect, Response, Stroke, Ui, vec2};
use pcbcraft_core::Point;
use pcbcraft_core::library::footprint_edit;
use pcbcraft_core::library::footprint_source::{
    FootprintEdit, SketchConstraintId, SketchEdit, SketchRef, SolveStatus, package_values,
};
use pcbcraft_solidcraft::footprint::SketchEvaluator;
use pcbcraft_solidcraft::footprint::view::ViewGeometry;
use solidcraft_ui_egui::{fonts, icons, theme::Tokens};

/// Pick distance around sketch points and curves, in pixels.
const PICK_PX: f32 = 6.0;
/// Constraint glyph size, in pixels (Solidcraft's `GLYPH`).
const GLYPH: f32 = 15.0;
/// Most glyphs drawn in one frame.
const MAX_GLYPHS: usize = 512;
/// Width of the inline value field, in pixels.
const FIELD_W: f32 = 150.0;
/// Arrowhead length and half width, in pixels.
const ARROW: f32 = 8.0;
const ARROW_HALF: f32 = 2.6;

/// Solidcraft's glyph icon for a constraint type (`icons/markers.rs`).
fn glyph_icon(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "Parallel" => "c_parallel",
        "Perpendicular" => "c_perpendicular",
        "Collinear" => "c_collinear",
        "Tangent" => "c_tangent",
        "Equal" => "c_equal",
        "Concentric" => "c_concentric",
        "MidPoint" => "c_midpoint",
        "Symmetry" => "c_symmetry",
        "Fix" => "c_fix",
        _ => return None,
    })
}

/// Solidcraft's glyph letter where there is no icon.
fn glyph_letter(kind: &str) -> &'static str {
    match kind {
        "Horizontal" => "-",
        "Vertical" => "|",
        _ => "?",
    }
}

/// A Constrain tool's ribbon look: id, icon, label, tip.
fn constrain_look(tool: FootprintTool) -> Option<(&'static str, &'static str, &'static str)> {
    use pcbcraft_core::library::footprint_source::GeomConstraint as G;
    let FootprintTool::Constrain(kind) = tool else {
        return match tool {
            FootprintTool::Dimension => Some((
                "dimension",
                "Dimension",
                "Driving dimension: pick points, a line, two lines or a curve, place it, type a value, a parameter or name = expression",
            )),
            _ => None,
        };
    };
    Some(match kind {
        G::Coincident => ("c_coincident", "Coincident", dims::hint(kind)),
        G::Horizontal => ("line", "Horizontal", dims::hint(kind)),
        G::Vertical => ("centerline", "Vertical", dims::hint(kind)),
        G::Parallel => ("c_parallel", "Parallel", dims::hint(kind)),
        G::Perpendicular => ("c_perpendicular", "Perpendicular", dims::hint(kind)),
        G::Equal => ("c_equal", "Equal", dims::hint(kind)),
        G::Tangent => ("c_tangent", "Tangent", dims::hint(kind)),
        G::Midpoint => ("c_midpoint", "Midpoint", dims::hint(kind)),
        G::PointOnCurve => ("c_coincident", "On curve", dims::hint(kind)),
        G::Concentric => ("c_concentric", "Concentric", dims::hint(kind)),
        G::Symmetric => ("c_symmetry", "Symmetric", dims::hint(kind)),
        G::Fixed => ("c_fix", "Fix", dims::hint(kind)),
    })
}

/// Sample a curve in footprint millimetres.
fn polyline(geometry: &ViewGeometry) -> Vec<Point> {
    let arc = |c: Point, r: f64, a0: f64, sweep: f64| -> Vec<Point> {
        let n = ((sweep.abs() / 0.1).ceil() as usize).clamp(8, 256);
        (0..=n)
            .map(|i| {
                let a = a0 + sweep * i as f64 / n as f64;
                [c[0] + r * a.cos(), c[1] + r * a.sin()]
            })
            .collect()
    };
    match *geometry {
        ViewGeometry::Line { a, b } => vec![a, b],
        ViewGeometry::Circle { center, radius } => arc(center, radius, 0.0, std::f64::consts::TAU),
        ViewGeometry::Arc { center, start, end } => {
            let r = (start[0] - center[0]).hypot(start[1] - center[1]);
            let a0 = (start[1] - center[1]).atan2(start[0] - center[0]);
            let a1 = (end[1] - center[1]).atan2(end[0] - center[0]);
            let mut sweep = (a1 - a0).rem_euclid(std::f64::consts::TAU);
            if sweep <= 1e-12 {
                sweep = std::f64::consts::TAU;
            }
            arc(center, r, a0, sweep)
        }
        ViewGeometry::Other => Vec::new(),
    }
}

/// Where a curve's glyphs start: its middle, and the unit normal (screen)
/// they step away along.
fn glyph_base(view: View, geometry: &ViewGeometry) -> Option<(Pos2, egui::Vec2, f32)> {
    let line = polyline(geometry);
    let (first, last) = (line.first()?, line.last()?);
    let mid = line.get(line.len() / 2)?;
    let (a, b) = (view.screen(*first), view.screen(*last));
    let along = b - a;
    let length = along.length();
    let normal = if length > 1e-3 {
        vec2(-along.y, along.x) / length
    } else {
        vec2(0.0, -1.0)
    };
    let mid = match geometry {
        ViewGeometry::Line { a, b } => view.screen([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]),
        _ => view.screen(*mid),
    };
    Some((mid, normal, length.max(GLYPH * 2.0)))
}

fn dashed(painter: &egui::Painter, points: &[Pos2], stroke: Stroke) {
    for pair in points.windows(2) {
        if let [a, b] = pair {
            painter.extend(egui::Shape::dashed_line(&[*a, *b], stroke, 5.0, 3.0));
        }
    }
}

impl PartEditor {
    /// Load the sketch overlay for the current working copy, once per
    /// source or package change. Clears it when the footprint has no source.
    pub(super) fn refresh_footprint_sketch(&mut self) {
        let Some(set) = &self.draft else {
            self.footprint_dims = Default::default();
            return;
        };
        let Some(source) = set.footprint.source.as_deref() else {
            if self.footprint_dims.loaded.is_some() {
                self.footprint_dims = Default::default();
            }
            return;
        };
        let package = package_values(&set.package);
        if !self
            .footprint_dims
            .loaded
            .as_ref()
            .is_some_and(|l| l.is_for(source, &package))
        {
            self.footprint_dims.loaded = Some(dims::Loaded::load(source, &package));
            self.footprint_dims.drag = None;
            self.footprint_dims.panel.reload();
        }
    }

    /// What the edits would do to the working copy, through the same
    /// `library.edit_footprint` path the engine runs: `Err` with the
    /// refusal, so a field can keep its text and show why.
    pub(super) fn try_footprint_edits(&self, edits: &[FootprintEdit]) -> Result<(), String> {
        let set = self.draft.as_ref().ok_or("There is no component draft")?;
        footprint_edit::edit(&set.footprint, &set.package, edits, &SketchEvaluator)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// The Footprint ribbon's Constrain group: Dimension and the geometric
    /// constraints, enabled on a sketched footprint.
    pub(super) fn constrain_group(&self) -> crate::shell::Group {
        use crate::shell::{Group, Tool};
        let sourced = self
            .draft
            .as_ref()
            .is_some_and(|set| set.footprint.source.is_some());
        let armed = self.footprint_tool.tool;
        let tools = FootprintTool::ALL
            .into_iter()
            .filter_map(|tool| {
                let (icon, label, tip) = constrain_look(tool)?;
                Some(
                    Tool::new(tool.id(), icon, label)
                        .enabled(sourced)
                        .selected(armed == Some(tool))
                        .tip(tip),
                )
            })
            .collect();
        Group {
            label: "Constrain",
            tools,
        }
    }

    /// Canvas input for the sketch overlay, before the pad picking: the
    /// Dimension and Constrain tools, label and glyph picks, the value
    /// field and point drags. True when it took the pointer this frame.
    pub(super) fn footprint_sketch_input(
        &mut self,
        ui: &mut Ui,
        response: &Response,
        view: View,
        panning: bool,
    ) -> bool {
        self.refresh_footprint_sketch();
        if self.footprint_dims.loaded.is_none() {
            return false;
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        let busy = self.footprint_dims.field.is_some()
            || self.footprint_dims.drag.is_some()
            || !self.footprint_dims.picks.is_empty();
        if busy && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.footprint_dims.cancel();
            return true;
        }
        if !typing
            && let Some(selected) = self.footprint_dims.selected.clone()
            && ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
            })
        {
            self.footprint_dims.selected = None;
            self.queue_footprint_edits(vec![FootprintEdit::Sketch(SketchEdit::Delete {
                ids: vec![SketchRef::Constraint(selected)],
            })]);
            return true;
        }
        if panning {
            return false;
        }
        let tolerance = f64::from(PICK_PX / view.scale);
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        let world = pointer.map(|p| view.world(p));
        match self.footprint_tool.tool {
            Some(FootprintTool::Dimension) => {
                if response.clicked()
                    && let Some(at) = world
                {
                    self.dimension_click(at, tolerance);
                }
                response.clicked() || response.drag_started()
            }
            Some(FootprintTool::Constrain(kind)) => {
                if response.clicked()
                    && let Some(at) = world
                {
                    self.constrain_click(kind, at, tolerance);
                }
                response.clicked() || response.drag_started()
            }
            None | Some(FootprintTool::Select) => self.select_input(ui, response, view, tolerance),
            Some(_) => false,
        }
    }

    fn dimension_click(&mut self, at: Point, tolerance: f64) {
        let Some(view) = self.footprint_dims.view().cloned() else {
            return;
        };
        let state = &mut self.footprint_dims;
        // A complete pick set: this click places it and opens its field.
        let ready = match dims::dimension_picks(&view, &state.picks) {
            DimPicks::Ready { refs, linear } => Some((refs, linear)),
            _ => None,
        };
        let picked = dims::pick(&view, at, tolerance);
        let place = match (ready, picked) {
            (Some(ready), _) => Some(ready),
            // A lone line placed on empty canvas: its length.
            (None, None) => match state.picks.as_slice() {
                [SketchRef::Curve(_)] => Some((state.picks.clone(), true)),
                _ => None,
            },
            (None, Some(r)) => {
                if !state.picks.contains(&r) {
                    state.picks.push(r);
                }
                match dims::dimension_picks(&view, &state.picks) {
                    DimPicks::Wrong(why) => {
                        state.picks.clear();
                        self.notice = why.into();
                    }
                    DimPicks::Ready { .. } | DimPicks::More => {}
                }
                None
            }
        };
        if let Some((refs, linear)) = place {
            let kind = dims::dimension_kind(&view, &refs, linear, at);
            let value = dims::measure(&view, kind, &refs).unwrap_or(0.0);
            state.picks.clear();
            state.field = Some(ValueField::New(NewDimension {
                kind,
                refs,
                at,
                text: crate::kit::number_text(value),
                error: None,
            }));
            state.fresh = 3;
        }
    }

    fn constrain_click(
        &mut self,
        kind: pcbcraft_core::library::footprint_source::GeomConstraint,
        at: Point,
        tolerance: f64,
    ) {
        let Some(view) = self.footprint_dims.view().cloned() else {
            return;
        };
        let Some(r) = dims::pick(&view, at, tolerance) else {
            return;
        };
        let state = &mut self.footprint_dims;
        if !state.picks.contains(&r) {
            state.picks.push(r);
        }
        match dims::constrain_picks(&view, kind, &state.picks) {
            ConstrainPicks::More => {}
            ConstrainPicks::Wrong(why) => {
                state.picks.clear();
                self.notice = why.into();
            }
            ConstrainPicks::Ready => {
                let refs = std::mem::take(&mut state.picks);
                let edits = vec![FootprintEdit::Sketch(SketchEdit::AddConstraint {
                    kind,
                    refs,
                })];
                match self.try_footprint_edits(&edits) {
                    Ok(()) => self.queue_footprint_edits(edits),
                    Err(e) => self.notice = e,
                }
            }
        }
    }

    /// With no tool (or Select): labels and glyphs select, a double-click
    /// on a label edits it, and a free point drags.
    fn select_input(&mut self, ui: &Ui, response: &Response, view: View, tolerance: f64) -> bool {
        let pressed_at = if response.drag_started() {
            ui.input(|i| i.pointer.press_origin())
        } else {
            response.interact_pointer_pos()
        };
        let hit = |list: &[(SketchConstraintId, Rect)], p: Pos2| {
            list.iter()
                .find(|(_, r)| r.contains(p))
                .map(|(id, _)| id.clone())
        };
        if response.double_clicked()
            && let Some(p) = pressed_at
            && let Some(id) = hit(&self.footprint_dims.labels, p)
        {
            self.open_field(&id);
            return true;
        }
        if response.clicked()
            && let Some(p) = pressed_at
        {
            let picked =
                hit(&self.footprint_dims.labels, p).or_else(|| hit(&self.footprint_dims.glyphs, p));
            let took = picked.is_some();
            self.footprint_dims.selected = picked;
            if took {
                self.footprint_tool.selection.clear();
                return true;
            }
        }
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(p) = pressed_at
            && let Some(loaded) = &self.footprint_dims.loaded
            && let Ok(live) = &loaded.live
            && let Some(point) = dims::pick_free_point(
                &loaded.view,
                loaded.report.as_ref(),
                view.world(p),
                tolerance,
            )
        {
            let at = loaded.view.point(&point).map_or(view.world(p), |q| q.at);
            self.footprint_dims.drag = Some(PointDrag {
                point,
                live: live.clone(),
                view: loaded.view.clone(),
                at,
                moved: false,
            });
            self.footprint_dims.times = Default::default();
            // The pointer has already left the press point: this frame
            // previews too.
        }
        if self.footprint_dims.drag.is_some() {
            if response.dragged_by(egui::PointerButton::Primary)
                && let Some(p) = response.interact_pointer_pos()
            {
                self.drag_frame(view.world(p));
            }
            if !response.dragged() {
                self.finish_drag();
            }
            return true;
        }
        false
    }

    /// One live drag frame from the last one, timed.
    fn drag_frame(&mut self, to: Point) {
        let Some(drag) = &mut self.footprint_dims.drag else {
            return;
        };
        if (drag.at[0] - to[0]).hypot(drag.at[1] - to[1]) < 1e-9 {
            return;
        }
        let clock = std::time::Instant::now();
        match drag.live.drag(&drag.point, to) {
            Ok(frame) => {
                drag.view = frame.sketch.view();
                drag.at = frame.at;
                drag.moved |= frame.moved;
                drag.live = frame.sketch;
            }
            Err(e) => self.notice = e.to_string(),
        }
        let ms = clock.elapsed().as_secs_f64() * 1e3;
        self.footprint_dims.times.add(ms);
    }

    /// The release: one `SketchEdit::Drag` to where the point got to.
    fn finish_drag(&mut self) {
        let Some(drag) = self.footprint_dims.drag.take() else {
            return;
        };
        if drag.moved {
            self.queue_footprint_edits(vec![FootprintEdit::Sketch(SketchEdit::Drag {
                point: drag.point,
                to: drag.at,
            })]);
        }
    }

    fn open_field(&mut self, id: &SketchConstraintId) {
        let Some(constraint) = self
            .footprint_dims
            .view()
            .and_then(|v| v.constraints.iter().find(|k| k.id == *id))
            .cloned()
        else {
            return;
        };
        if constraint.value.is_none() || constraint.driven {
            return;
        }
        self.footprint_dims.selected = Some(id.clone());
        self.footprint_dims.field = Some(ValueField::Existing(InlineEdit {
            constraint: id.clone(),
            kind: dims::stored_kind(&constraint),
            text: dims::field_text(&constraint),
            error: None,
        }));
        self.footprint_dims.fresh = 3;
    }

    /// Draw the sketch overlay: curves and points by the solver's verdict,
    /// dimensions, glyphs, picks and the status line.
    pub(super) fn paint_footprint_sketch(
        &mut self,
        painter: &egui::Painter,
        rect: Rect,
        view: View,
        hover: Option<Pos2>,
    ) {
        let t = Tokens::get();
        let Some(loaded) = &self.footprint_dims.loaded else {
            return;
        };
        let state = &self.footprint_dims;
        let drawn = state.view().unwrap_or(&loaded.view);
        let report = loaded.report.as_ref();
        let conflicting = report.map(|r| r.conflicting.as_slice()).unwrap_or_default();
        let redundant = report.map(|r| r.redundant.as_slice()).unwrap_or_default();
        let conflict_curves: Vec<_> = drawn
            .constraints
            .iter()
            .filter(|k| conflicting.contains(&k.id))
            .flat_map(|k| k.curves.iter().cloned())
            .collect();
        let conflict_points: Vec<_> = drawn
            .constraints
            .iter()
            .filter(|k| conflicting.contains(&k.id))
            .flat_map(|k| k.points.iter().cloned())
            .collect();
        let picked = |r: SketchRef| state.picks.contains(&r);
        let highlighted: Vec<SketchConstraintId> = state
            .highlight
            .as_ref()
            .and_then(|name| loaded.rows.iter().find(|row| row.name == *name))
            .map(|row| row.used_by.clone())
            .unwrap_or_default();
        // Curves.
        for curve in &drawn.curves {
            let line: Vec<Pos2> = polyline(&curve.geometry)
                .into_iter()
                .map(|p| view.screen(p))
                .collect();
            let color = if picked(SketchRef::Curve(curve.id.clone())) {
                t.accent
            } else if conflict_curves.contains(&curve.id) {
                t.sketch_conflict
            } else if report.is_none_or(|r| r.free_curves.contains(&curve.id)) {
                t.sketch
            } else {
                t.sketch_fixed
            };
            let width = if color == t.accent { 2.5 } else { 1.25 };
            let stroke = Stroke::new(width, color);
            if curve.construction {
                dashed(painter, &line, stroke);
            } else {
                painter.add(egui::Shape::line(line, stroke));
            }
        }
        // Points (the origin is the datum cross).
        for point in drawn.points.iter().filter(|p| p.id.0 != "origin") {
            let at = view.screen(point.at);
            let free = report.is_some_and(|r| r.free_points.contains(&point.id));
            let color = if picked(SketchRef::Point(point.id.clone())) {
                t.accent
            } else if conflict_points.contains(&point.id) {
                t.sketch_conflict
            } else if free {
                t.sketch_point_free
            } else {
                t.sketch_point
            };
            painter.circle_filled(at, if free { 2.5 } else { 2.0 }, color);
        }
        if let Some(drag) = &state.drag {
            painter.circle_stroke(view.screen(drag.at), 5.0, Stroke::new(1.5, t.accent));
        }
        // Dimensions.
        let px = f64::from(1.0 / view.scale);
        let font = fonts::medium(t.font_small);
        let labels: Vec<(SketchConstraintId, String)> = drawn
            .constraints
            .iter()
            .filter(|k| k.value.is_some())
            .map(|k| (k.id.clone(), dims::label(k)))
            .collect();
        let char_px = t.font_small * 0.56;
        let half_of = |id: &SketchConstraintId| -> Point {
            let chars = labels
                .iter()
                .find(|(k, _)| k == id)
                .map_or(4, |(_, s)| s.chars().count());
            [
                f64::from(chars as f32 * char_px / 2.0 + 3.0) * px,
                f64::from(t.font_small / 2.0 + 2.0) * px,
            ]
        };
        let live = state
            .drag
            .as_ref()
            .map(|d| &d.live)
            .or(loaded.live.as_ref().ok());
        let layouts = live.map(|l| l.dimensions(px, &half_of)).unwrap_or_default();
        let hovered_label = hover.and_then(|h| {
            state
                .labels
                .iter()
                .find(|(_, r)| r.contains(h))
                .map(|(id, _)| id.clone())
        });
        let mut label_rects = Vec::new();
        for layout in &layouts {
            let Some(constraint) = drawn.constraints.iter().find(|k| k.id == layout.id) else {
                continue;
            };
            let lit = state.selected.as_ref() == Some(&layout.id)
                || hovered_label.as_ref() == Some(&layout.id)
                || highlighted.contains(&layout.id);
            let color = if conflicting.contains(&layout.id) {
                t.sketch_conflict
            } else if lit {
                t.accent
            } else if constraint.driven {
                t.dimension_driven
            } else if redundant.contains(&layout.id) {
                t.dimension.gamma_multiply(0.5)
            } else {
                t.dimension
            };
            let stroke = Stroke::new(1.0, color);
            for line in &layout.lines {
                let points: Vec<Pos2> = line.iter().map(|p| view.screen(*p)).collect();
                painter.add(egui::Shape::line(points, stroke));
            }
            for (tip, dir) in &layout.arrows {
                let tip = view.screen(*tip);
                let d = vec2(dir[0] as f32, -(dir[1] as f32));
                let n = vec2(-d.y, d.x);
                let back = tip - d * ARROW;
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, back + n * ARROW_HALF, back - n * ARROW_HALF],
                    color,
                    Stroke::NONE,
                ));
            }
            let editing =
                matches!(&state.field, Some(ValueField::Existing(e)) if e.constraint == layout.id);
            let text = labels
                .iter()
                .find(|(k, _)| *k == layout.id)
                .map_or_else(String::new, |(_, s)| s.clone());
            let center = view.screen(layout.text);
            let galley = painter.layout_no_wrap(text, font.clone(), color);
            let r = Rect::from_center_size(center, galley.size() + vec2(6.0, 2.0));
            if !editing {
                painter.rect_filled(r, t.radius_sm, t.glyph_bg.gamma_multiply(0.85));
                painter.galley(center - galley.size() / 2.0, galley, color);
            }
            label_rects.push((layout.id.clone(), r));
        }
        // Constraint glyphs beside their entities, stepping along where
        // they would overlap.
        let mut glyph_rects: Vec<(SketchConstraintId, Rect)> = Vec::new();
        let mut taken: Vec<Rect> = label_rects.iter().map(|(_, r)| r.expand(2.0)).collect();
        for constraint in drawn
            .constraints
            .iter()
            .filter(|k| k.value.is_none() && k.kind != "Coincident")
        {
            if glyph_rects.len() >= MAX_GLYPHS {
                break;
            }
            let base = match (constraint.curves.first(), constraint.points.first()) {
                (Some(c), _) => drawn
                    .curve(c)
                    .and_then(|c| glyph_base(view, &c.geometry))
                    .filter(|(_, _, length)| *length >= GLYPH * 1.5),
                (None, Some(p)) => drawn
                    .point(p)
                    .map(|p| (view.screen(p.at), vec2(0.7, -0.7), GLYPH * 2.0)),
                _ => None,
            };
            let Some((base, normal, _)) = base else {
                continue;
            };
            let along = vec2(normal.y, -normal.x);
            let mut at = base + normal * GLYPH;
            for _ in 0..8 {
                let r = Rect::from_center_size(at, vec2(GLYPH, GLYPH));
                if !taken.iter().any(|q| q.intersects(r)) {
                    break;
                }
                at += along * (GLYPH + 2.0);
            }
            let r = Rect::from_center_size(at, vec2(GLYPH, GLYPH));
            if !rect.intersects(r) {
                continue;
            }
            let failing = conflicting.contains(&constraint.id);
            let lit = state.selected.as_ref() == Some(&constraint.id)
                || hover.is_some_and(|h| r.contains(h));
            let ink = if failing {
                t.sketch_conflict
            } else if lit {
                t.accent
            } else if redundant.contains(&constraint.id) {
                t.glyph_text.gamma_multiply(0.45)
            } else {
                t.glyph_text
            };
            painter.rect_filled(
                r,
                t.radius_sm.saturating_sub(1),
                if lit { t.accent_soft } else { t.glyph_bg },
            );
            painter.rect_stroke(
                r,
                t.radius_sm.saturating_sub(1),
                Stroke::new(1.0, if failing || lit { ink } else { t.glyph_edge }),
                egui::StrokeKind::Inside,
            );
            match glyph_icon(constraint.kind) {
                Some(icon) => {
                    icons::paint_icon(painter, r.shrink(2.0), icon, f32::from(ink.a()) / 255.0);
                }
                None => {
                    painter.text(
                        at,
                        Align2::CENTER_CENTER,
                        glyph_letter(constraint.kind),
                        fonts::semibold(t.font_small),
                        ink,
                    );
                }
            }
            taken.push(r.expand(1.0));
            glyph_rects.push((constraint.id.clone(), r));
        }
        // A placed-pick dimension follows the pointer until it is placed.
        if self.footprint_tool.tool == Some(FootprintTool::Dimension)
            && let Some(h) = hover
            && let DimPicks::Ready { refs, linear } = dims::dimension_picks(drawn, &state.picks)
        {
            let at = view.world(h);
            let kind = dims::dimension_kind(drawn, &refs, linear, at);
            if let Some(v) = dims::measure(drawn, kind, &refs) {
                painter.text(
                    h + vec2(12.0, -12.0),
                    Align2::LEFT_BOTTOM,
                    crate::kit::number_text(v),
                    font.clone(),
                    t.accent,
                );
            }
        }
        self.paint_sketch_status(painter, rect);
        self.footprint_dims.labels = label_rects;
        self.footprint_dims.glyphs = glyph_rects;
    }

    fn paint_sketch_status(&self, painter: &egui::Painter, rect: Rect) {
        let t = Tokens::get();
        let Some(loaded) = &self.footprint_dims.loaded else {
            return;
        };
        let (mut text, ink) = match (&loaded.live, &loaded.report) {
            (Err(e), _) => (e.clone(), t.error),
            (Ok(_), Some(r)) => (
                dims::status_text(r),
                match r.status {
                    SolveStatus::Solved => t.text_dim,
                    SolveStatus::UnderConstrained => t.text_dim,
                    SolveStatus::OverConstrained | SolveStatus::Failed => t.error,
                },
            ),
            (Ok(_), None) => (String::new(), t.text_dim),
        };
        let times = self.footprint_dims.times;
        if times.frames > 0 {
            text.push_str(&format!(
                "   drag {:.1} ms (max {:.1})",
                times.last_ms, times.max_ms
            ));
        }
        painter.text(
            rect.right_bottom() + vec2(-t.space_16, -t.space_16),
            Align2::RIGHT_BOTTOM,
            text,
            fonts::medium(t.font_small),
            ink,
        );
        let hint = match self.footprint_tool.tool {
            Some(FootprintTool::Constrain(kind)) => Some(dims::hint(kind)),
            Some(FootprintTool::Dimension) => Some(
                "Dimension: pick two points, a point and a line, two lines or a curve; click to place",
            ),
            _ => None,
        };
        if let Some(hint) = hint {
            painter.text(
                rect.center_bottom() + vec2(0.0, -t.space_16 * 2.5),
                Align2::CENTER_BOTTOM,
                hint,
                fonts::medium(t.font_small),
                t.text_dim,
            );
        }
    }

    /// The inline value field over the canvas: Enter commits (after the
    /// same edit is tried on the working copy, so a refusal keeps the text
    /// and shows why), Esc or a click elsewhere closes it.
    pub(super) fn footprint_value_field(&mut self, ui: &mut Ui, view: View) {
        let Some(field) = &self.footprint_dims.field else {
            return;
        };
        let center = match field {
            ValueField::New(n) => view.screen(n.at),
            ValueField::Existing(e) => self
                .footprint_dims
                .labels
                .iter()
                .find(|(id, _)| *id == e.constraint)
                .map_or_else(|| ui.max_rect().center(), |(_, r)| r.center()),
        };
        let t = Tokens::get();
        let r = Rect::from_center_size(center, vec2(FIELD_W, t.control_height));
        let invalid = field.error().is_some();
        let error = field.error().map(str::to_owned);
        let mut text = match &mut self.footprint_dims.field {
            Some(f) => f.text_mut().clone(),
            None => return,
        };
        let response = ui
            .scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
                crate::kit::text_field(
                    ui,
                    "fp-dimension-value",
                    &mut text,
                    "value, name or name = expr",
                    invalid,
                )
            })
            .inner;
        if let Some(f) = &mut self.footprint_dims.field {
            *f.text_mut() = text;
        }
        // Read before any focus request below, which would take the focus
        // straight back and hide Enter's surrender.
        let lost = response.lost_focus();
        if self.footprint_dims.fresh > 0 {
            self.footprint_dims.fresh -= 1;
            if !lost {
                response.request_focus();
            }
        }
        if let Some(error) = error {
            let painter = ui.painter();
            let galley = painter.layout(error, fonts::medium(t.font_small), t.error, FIELD_W * 2.0);
            let at = Pos2::new(r.left(), r.bottom() + 2.0);
            painter.rect_filled(
                Rect::from_min_size(at, galley.size() + vec2(6.0, 2.0)),
                t.radius_sm,
                t.panel,
            );
            painter.galley(at + vec2(3.0, 1.0), galley, t.error);
        }
        let (enter, escape) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if lost && enter {
            self.commit_field();
        } else if escape || (lost && self.footprint_dims.fresh == 0) {
            self.footprint_dims.field = None;
        }
    }

    /// Enter in the value field: parse, try, then queue one commit.
    fn commit_field(&mut self) {
        let Some(field) = self.footprint_dims.field.clone() else {
            return;
        };
        let source = self
            .draft
            .as_ref()
            .and_then(|s| s.footprint.source.as_deref().cloned())
            .unwrap_or_default();
        let parsed = match &field {
            ValueField::Existing(e) => dims::parse_input(&e.text),
            ValueField::New(n) => dims::parse_input(&n.text),
        };
        let input: DimInput = match parsed {
            Ok(input) => input,
            Err(e) => {
                self.keep_field(field, e);
                return;
            }
        };
        let edits = match &field {
            ValueField::Existing(e) => {
                let bound = self
                    .footprint_dims
                    .view()
                    .and_then(|v| v.constraints.iter().find(|k| k.id == e.constraint))
                    .and_then(|k| k.param.clone());
                let fallback = dims::fallback_name(&source, bound.as_deref());
                dims::input_edits(&e.constraint, e.kind, input, fallback)
            }
            ValueField::New(n) => {
                let fallback = dims::fallback_name(&source, None);
                dims::new_dimension_edits(n.kind, n.refs.clone(), input, fallback)
            }
        };
        match self.try_footprint_edits(&edits) {
            Ok(()) => {
                self.footprint_dims.field = None;
                self.queue_footprint_edits(edits);
            }
            Err(e) => self.keep_field(field, e),
        }
    }

    fn keep_field(&mut self, mut field: ValueField, error: String) {
        field.set_error(Some(error));
        self.footprint_dims.field = Some(field);
        self.footprint_dims.fresh = 1;
    }
}
