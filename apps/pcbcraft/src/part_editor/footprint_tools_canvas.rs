//! The Footprint tab's sketch tools on the canvas, the ribbon and in
//! Properties (slice F4): snapping (the Symbol tab's `snap.rs` over sketch
//! points, pad centres and lands), tool clicks into
//! [`footprint_tools::ToolState`], Select picks, drags and Delete, the
//! keymap, previews (rubber bands, a ghost pad, array ghosts, a moving
//! selection) and the one commit each finished gesture sends.
use super::PartEditor;
use super::footprint_tools::{
    self, ActiveLayer, ArrayTool, FootprintTool, Gesture, MoveDrag, Pick, Scene,
};
use super::snap::{self, SNAP_PX, Snap, Targets};
use crate::kit;
use crate::symbol_view::{self, View};
use egui::{Align2, Color32, Rect, Response, Stroke, Ui, vec2};
use pcbcraft_core::geometry::{Shape, distance};
use pcbcraft_core::library::footprint_source::{FootprintEdit, PadTemplate, Pin1Position};
use pcbcraft_core::library::{PadKind, PadSide};
use pcbcraft_core::{Engine, Operation, Point};
use pcbcraft_solidcraft::footprint::SketchEvaluator;
use pcbcraft_solidcraft::footprint::view::{SketchView, ViewGeometry};
use pcbcraft_ui::theme::Tokens;
use pcbcraft_ui::ui_kit::type_face;

/// Pick reach around sketch entities and lands, in pixels.
const PICK_PX: f32 = 4.0;
/// Curves are flattened to this chord error, in pixels.
const CHORD_PX: f32 = 0.25;

fn shape_of(g: &ViewGeometry) -> Option<Shape> {
    match g {
        ViewGeometry::Line { a, b } => Some(Shape::Line { a: *a, b: *b }),
        ViewGeometry::Circle { center, radius } => Some(Shape::Circle {
            center: *center,
            radius: *radius,
        }),
        ViewGeometry::Arc { center, start, end } => {
            let a0 = (start[1] - center[1])
                .atan2(start[0] - center[0])
                .to_degrees();
            let a1 = (end[1] - center[1]).atan2(end[0] - center[0]).to_degrees();
            let sweep = (a1 - a0).rem_euclid(360.0);
            Some(Shape::Arc {
                center: *center,
                radius: distance(*center, *start),
                start_deg: a0,
                sweep_deg: if sweep <= 1e-9 { 360.0 } else { sweep },
            })
        }
        ViewGeometry::FreeForm { .. } => None,
    }
}

fn offset(shape: &Shape, d: Point) -> Shape {
    let m = |p: &Point| [p[0] + d[0], p[1] + d[1]];
    match shape {
        Shape::Line { a, b } => Shape::Line { a: m(a), b: m(b) },
        Shape::Polyline { points, radii } => Shape::Polyline {
            points: points.iter().map(m).collect(),
            radii: radii.clone(),
        },
        Shape::Rect { a, b, radius } => Shape::Rect {
            a: m(a),
            b: m(b),
            radius: *radius,
        },
        Shape::Circle { center, radius } => Shape::Circle {
            center: m(center),
            radius: *radius,
        },
        Shape::Arc {
            center,
            radius,
            start_deg,
            sweep_deg,
        } => Shape::Arc {
            center: m(center),
            radius: *radius,
            start_deg: *start_deg,
            sweep_deg: *sweep_deg,
        },
        Shape::Polygon { points, radii } => Shape::Polygon {
            points: points.iter().map(m).collect(),
            radii: radii.clone(),
        },
    }
}

/// Snap targets of the sketch and the pads, leaving out what a drag moves.
fn targets(
    view: &SketchView,
    pads: &[pcbcraft_core::library::Pad],
    chord: f64,
    skip_pads: &[pcbcraft_core::PadId],
    pending: &[Point],
) -> Targets {
    let mut t = Targets::default();
    for p in &view.points {
        if p.id.0 != "origin" {
            t.vertices.push(p.at);
        }
    }
    for c in &view.curves {
        match &c.geometry {
            ViewGeometry::Line { a, b } => {
                t.midpoints.push([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]);
            }
            ViewGeometry::Circle { center, .. } | ViewGeometry::Arc { center, .. } => {
                t.centers.push(*center);
            }
            ViewGeometry::FreeForm { .. } => {}
        }
        if let Some(shape) = shape_of(&c.geometry) {
            t.curves.push((shape.coarse_bounds(0.0), shape));
        }
    }
    for pad in pads.iter().filter(|p| !skip_pads.contains(&p.id)) {
        t.centers.push(pad.position());
        let land = pad.land(chord);
        t.curves.push((land.coarse_bounds(0.0), land));
    }
    t.vertices.extend_from_slice(pending);
    t
}

impl PartEditor {
    /// Whether the Footprint tab draws with the sketch tools: a sourced
    /// footprint, or one without explicit pads to start a sketch on.
    pub(super) fn footprint_sketching(&self) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|s| s.footprint.source.is_some() || s.footprint.pads.is_empty())
    }

    /// Arm a Footprint tool, taking its options from the sketch's
    /// parameters.
    pub(super) fn footprint_arm(&mut self, tool: FootprintTool) {
        self.footprint_tool.arm(tool);
        self.footprint_dims.picks.clear();
        if matches!(tool, FootprintTool::Constrain(_)) {
            self.footprint_dims.last_constraint = Some(tool);
        }
        let source = self
            .draft
            .as_ref()
            .and_then(|s| s.footprint.source.as_deref());
        self.footprint_tool.adopt(source);
    }

    /// A Layer chooser button; false when `id` is not one.
    pub(super) fn footprint_layer_tool(&mut self, id: &str) -> bool {
        match ActiveLayer::from_id(id) {
            Some(layer) => {
                self.footprint_tool.layer = layer;
                true
            }
            None => false,
        }
    }

    /// The Footprint tab's Sketch, Pads and Layer groups.
    pub(super) fn footprint_sketch_groups(&self) -> Vec<crate::shell::Group> {
        use crate::shell::{Group, Tool};
        let enabled = self.footprint_sketching();
        let armed = self.footprint_tool.armed();
        let tool = |t: FootprintTool| {
            Tool::new(t.id(), t.icon(), t.label())
                .enabled(enabled)
                .selected(armed == t)
                .tip(t.tip())
        };
        let family = |i: usize| {
            let variants = FootprintTool::FAMILIES.get(i).copied().unwrap_or(&[]);
            let shown = if variants.contains(&armed) {
                armed
            } else {
                self.footprint_tool
                    .shown
                    .get(i)
                    .copied()
                    .unwrap_or(FootprintTool::Select)
            };
            Tool::family(tool(shown), variants.iter().copied().map(tool).collect())
        };
        let layer = self.footprint_tool.layer;
        vec![
            Group {
                label: "Sketch",
                tools: FootprintTool::SKETCH.into_iter().map(tool).collect(),
            },
            Group {
                label: "Pads",
                tools: vec![
                    family(0),
                    tool(FootprintTool::CustomPad),
                    family(1),
                    family(2),
                ],
            },
            Group {
                label: "Layer",
                tools: ActiveLayer::ALL
                    .into_iter()
                    .map(|l| {
                        Tool::new(l.id(), l.icon(), l.label())
                            .enabled(enabled)
                            .selected(layer == l)
                            .tip(l.tip())
                    })
                    .collect(),
            },
        ]
    }

    /// Queue one finished gesture's edits (a tool, dimension, constraint,
    /// drag or parameter edit) as the next `library.edit_footprint`.
    pub(super) fn queue_footprint_edits(&mut self, edits: Vec<FootprintEdit>) {
        if !self.footprint_tool.queue(edits) {
            let busy = footprint_tools::OUTBOX_BUSY;
            self.footprint_tool.refuse(busy, busy);
        }
    }

    /// The commit of a finished gesture: the working copy goes first when
    /// it has unsent edits, so neither reload discards the other.
    pub(super) fn footprint_tool_operation(&mut self, engine: &Engine) -> Option<Operation> {
        let edits = self.footprint_tool.outbox.take()?;
        if self.awaiting {
            self.footprint_tool.outbox = Some(edits);
            return None;
        }
        // The gesture was staged against the working copy sent before it:
        // if the engine refused that copy, the ids no longer match.
        if let Some(expected) = self.footprint_tool.held_for.take()
            && self.draft.as_ref().map(|s| &s.footprint) != Some(&expected)
        {
            let dropped = "Sketch change dropped: the edit before it was refused";
            self.footprint_tool.refuse(dropped, dropped);
            return None;
        }
        if self.draft != self.baseline {
            self.footprint_tool.held_for = self
                .draft
                .as_ref()
                .map(|s| s.retargeted(engine.drafts()).footprint);
            self.footprint_tool.outbox = Some(edits);
            return self.send(engine);
        }
        let draft = self.draft.as_ref()?.ids.footprint;
        self.awaiting = true;
        Some(footprint_tools::commit(draft, edits))
    }

    /// Handle one canvas frame of the sketch tools and paint their
    /// overlay. `taken` is true when the dimension overlay
    /// (`footprint_dims_ui`) already took the pointer this frame: a label,
    /// glyph, Dimension or Constrain pick or a live point drag.
    pub(super) fn footprint_tools_frame(
        &mut self,
        ui: &Ui,
        rect: Rect,
        response: &Response,
        view: View,
        grid: f64,
        taken: bool,
    ) {
        if !self.footprint_sketching() {
            return;
        }
        self.footprint_tool_keys(ui);
        let chord = f64::from(CHORD_PX / view.scale);
        let tolerance = f64::from(PICK_PX / view.scale);
        let reach = f64::from(SNAP_PX / view.scale);
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (self.space_held && response.dragged_by(egui::PointerButton::Primary));
        let suppress = ui.input(|i| i.modifiers.command || i.modifiers.alt);
        let shift_held = ui.input(|i| i.modifiers.shift);
        let Self {
            draft,
            footprint_tool: state,
            footprint_dims: dims,
            selected_pad,
            ..
        } = self;
        let Some(set) = draft.as_ref() else {
            return;
        };
        let sketch = state.view(set.footprint.source.as_deref());
        let scene = Scene {
            footprint: &set.footprint,
            package: &set.package,
            view: &sketch,
            evaluator: &SketchEvaluator,
            tolerance,
            chord,
        };
        let tool = state.armed();
        let moving_pads: Vec<_> = match state.drag {
            Some(_) => state
                .selection
                .iter()
                .filter_map(|p| match p {
                    Pick::Pad(id) => Some(*id),
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        };
        let targets = targets(
            &sketch,
            &set.footprint.pads,
            chord,
            &moving_pads,
            &state.pending,
        );
        let snap_at = |p: Point| snap::snap(p, reach, grid, &targets, !suppress);
        let cursor: Option<Snap> = response
            .hover_pos()
            .filter(|p| rect.contains(*p))
            .map(|p| snap_at(view.world(p)));
        if let Some(c) = &cursor {
            state.hover(c.at);
        }
        let report = |result: pcbcraft_core::Result<Gesture>,
                      state: &mut footprint_tools::ToolState| {
            match result {
                Ok(Gesture::Commit(edits)) if !edits.is_empty() => {
                    if !state.queue(edits) {
                        let busy = footprint_tools::OUTBOX_BUSY;
                        state.refuse(busy, busy);
                    }
                }
                Ok(_) => {}
                Err(e) => state.refuse(
                    &crate::refusal::shown_error(&e),
                    "That sketch change is not accepted",
                ),
            }
        };
        if !panning && !self.space_held && !taken {
            if response.secondary_clicked() {
                state.cancel();
                state.drag = None;
            } else if response.clicked()
                && let Some(p) = response.interact_pointer_pos()
            {
                let raw = view.world(p);
                if tool == FootprintTool::Select {
                    // One selection: a pad or sketch pick drops a selected
                    // dimension or constraint.
                    dims.selected = None;
                    state.select(raw, &scene, shift_held);
                    *selected_pad = state.selection.iter().find_map(|p| match p {
                        Pick::Pad(id) => Some(*id),
                        _ => None,
                    });
                } else {
                    let at = snap_at(raw).at;
                    let result = state.press(at, &scene);
                    report(result, state);
                }
            }
            // Select: drag the picked (or selected) things.
            if tool == FootprintTool::Select {
                if response.drag_started_by(egui::PointerButton::Primary)
                    && let Some(p) = ui.input(|i| i.pointer.press_origin())
                {
                    let start = view.world(p);
                    let picked = state.pick(start, &scene);
                    if let Some(pick) = picked {
                        if !state.selection.contains(&pick) {
                            state.selection = vec![pick.clone()];
                        }
                        let reference = match &pick {
                            Pick::Pad(id) => set
                                .footprint
                                .pads
                                .iter()
                                .find(|q| q.id == *id)
                                .map_or(start, |q| q.position()),
                            Pick::Point(id) => sketch.point(id).map_or(start, |q| q.at),
                            Pick::Curve(_) => start,
                        };
                        state.drag = Some(MoveDrag {
                            start,
                            reference,
                            delta: [0.0, 0.0],
                        });
                    }
                }
                if let Some(drag) = &mut state.drag
                    && response.dragged_by(egui::PointerButton::Primary)
                    && let Some(p) = response.interact_pointer_pos()
                {
                    let raw = view.world(p);
                    let to = [
                        drag.reference[0] + raw[0] - drag.start[0],
                        drag.reference[1] + raw[1] - drag.start[1],
                    ];
                    let snapped = snap_at(to).at;
                    drag.delta = [
                        snapped[0] - drag.reference[0],
                        snapped[1] - drag.reference[1],
                    ];
                }
                if response.drag_stopped()
                    && let Some(drag) = state.drag.take()
                {
                    let result = state.move_edits(drag.delta, &scene);
                    report(result, state);
                }
            }
        }
        if !response.dragged() && !response.drag_stopped() {
            state.drag = None;
        }
        // Delete from the keyboard, resolved against this frame's scene.
        if state.delete_requested {
            state.delete_requested = false;
            let result = state.delete_edits(&scene);
            if matches!(result, Ok(Gesture::Commit(_))) {
                state.selection.clear();
                *selected_pad = None;
            }
            report(result, state);
        }
        let painter = ui.painter().with_clip_rect(rect);
        paint(&painter, rect, view, state, &scene, cursor.as_ref());
    }

    /// Keys of the sketch tools. Plain letters only act while no text
    /// field has the keyboard.
    fn footprint_tool_keys(&mut self, ui: &Ui) {
        use crate::keys::{pressed, pressed_tool};
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let ctx = ui.ctx().clone();
        // A held Space pans (navigation); its press turns the next pad.
        if crate::keys::pressed_once(&ctx, "footprint.turn") {
            self.footprint_tool.turn();
        }
        if pressed(&ctx, "footprint.repeat") {
            self.footprint_tool.repeat();
        }
        for tool in FootprintTool::ALL {
            if pressed_tool(&ctx, "footprint", tool.id()) {
                self.footprint_arm(tool);
            }
        }
        if pressed(&ctx, "footprint.layer") {
            self.footprint_tool.layer = if self.footprint_tool.layer == ActiveLayer::Construction {
                ActiveLayer::Silkscreen
            } else {
                ActiveLayer::Construction
            };
        }
        if !self.footprint_tool.selection.is_empty() && pressed(&ctx, "footprint.delete") {
            self.footprint_tool.delete_requested = true;
        }
        if pressed(&ctx, "footprint.cancel") {
            self.footprint_tool.cancel();
            self.footprint_tool.drag = None;
        }
    }

    /// Post the gesture refusals collected since the last frame as error
    /// toasts (the canvas and the card both drain them).
    pub(super) fn post_footprint_refusals(&mut self, ctx: &egui::Context) {
        for text in self.footprint_tool.refusals.drain(..) {
            crate::kit::toast::post(ctx, crate::kit::severity::Level::Error, text, Vec::new());
        }
    }

    /// The armed sketch tool other than Select, if any: while one is armed
    /// its tool card replaces the Properties card.
    pub(super) fn footprint_tool_armed(&self) -> Option<FootprintTool> {
        let tool = self.footprint_tool.armed();
        (self.footprint_sketching() && tool != FootprintTool::Select).then_some(tool)
    }

    /// The armed tool's card: its header (name, key, Done), its options
    /// and one hint line. Returns whether a tool card was drawn.
    pub(super) fn footprint_tool_card(&mut self, ui: &mut Ui) -> bool {
        let Some(tool) = self.footprint_tool_armed() else {
            return false;
        };
        let keymap = crate::keys::registry(ui.ctx());
        let key = keymap
            .get_parts("footprint", tool.id())
            .and_then(|c| keymap.shortcut(c))
            .unwrap_or_default();
        let (hint, tip) = tool_hint(tool);
        let card = kit::ToolCard {
            icon: tool.icon(),
            name: tool_name(tool),
            key: &key,
            hint,
            tip,
            finish: None,
        };
        let state = &mut self.footprint_tool;
        let count = |ui: &mut Ui, label: &str, n: &mut u16| {
            let mut v = f64::from(*n);
            if kit::number_row(ui, label, &mut v, "", 1.0..=1024.0) {
                *n = v.round().clamp(1.0, 1024.0) as u16;
            }
        };
        let response = kit::tool_card(ui, &card, |ui| {
            let o = &mut state.options;
            if matches!(tool, FootprintTool::Array(_)) {
                kit::choice_row(
                    ui,
                    "Pad",
                    &mut o.array.template,
                    &[
                        (PadTemplate::Rect, "Rectangle".into()),
                        (PadTemplate::RoundRect, "Rounded".into()),
                        (PadTemplate::Circle, "Round".into()),
                        (PadTemplate::Oblong, "Oblong".into()),
                    ],
                );
            }
            if let FootprintTool::Pad(_) | FootprintTool::CustomPad | FootprintTool::Array(_) = tool
            {
                let template = match tool {
                    FootprintTool::Pad(t) => t,
                    FootprintTool::Array(_) => o.array.template,
                    _ => PadTemplate::Rect,
                };
                kit::choice_row(
                    ui,
                    "Kind",
                    &mut o.pad.kind,
                    &[
                        (PadKind::Smd, "SMD".into()),
                        (PadKind::Mechanical, "Mechanical".into()),
                        (PadKind::Fiducial, "Fiducial".into()),
                    ],
                );
                kit::choice_row(
                    ui,
                    "Side",
                    &mut o.pad.side,
                    &[
                        (PadSide::Top, "Top".into()),
                        (PadSide::Bottom, "Bottom".into()),
                    ],
                );
                if template == PadTemplate::Circle {
                    kit::number_row(ui, "Diameter", &mut o.pad.diameter, "mm", 0.01..=100.0);
                } else {
                    kit::pair_row(ui, "Size", &mut o.pad.size, "mm", 0.01..=100.0);
                }
                if template == PadTemplate::RoundRect {
                    kit::number_row(ui, "Corner", &mut o.pad.radius_ratio, "", 0.0..=0.5);
                }
            }
            if let FootprintTool::Array(kind) = tool {
                let a = &mut o.array;
                match kind {
                    ArrayTool::Linear => {
                        count(ui, "Count", &mut a.count);
                        kit::number_row(ui, "Pitch", &mut a.pitch, "mm", 0.01..=100.0);
                    }
                    ArrayTool::DualRow => {
                        count(ui, "Per row", &mut a.per_row);
                        kit::number_row(ui, "Pitch", &mut a.pitch, "mm", 0.01..=100.0);
                        kit::number_row(ui, "Row span", &mut a.row_span, "mm", 0.01..=200.0);
                    }
                    ArrayTool::Quad => {
                        count(ui, "Bottom/top", &mut a.nx);
                        count(ui, "Left/right", &mut a.ny);
                        kit::number_row(ui, "Pitch", &mut a.pitch, "mm", 0.01..=100.0);
                        kit::pair_row(ui, "Span", &mut a.span, "mm", 0.01..=200.0);
                        kit::choice_row(
                            ui,
                            "Pin 1",
                            &mut a.pin1,
                            &[
                                (Pin1Position::Left, "Left".into()),
                                (Pin1Position::Bottom, "Bottom".into()),
                                (Pin1Position::Right, "Right".into()),
                                (Pin1Position::Top, "Top".into()),
                            ],
                        );
                    }
                    ArrayTool::Grid => {
                        count(ui, "Rows", &mut a.rows);
                        count(ui, "Columns", &mut a.cols);
                        kit::pair_row(ui, "Pitch", &mut a.grid_pitch, "mm", 0.01..=100.0);
                        kit::check_row(ui, "Stagger odd rows", &mut a.stagger);
                        kit::check_row(ui, "Letter rows (A1)", &mut a.alpha);
                        let mut void = [f64::from(a.void[0]), f64::from(a.void[1])];
                        if kit::pair_row(ui, "Centre void", &mut void, "", 0.0..=1024.0) {
                            a.void = void.map(|v| v.round().clamp(0.0, 1024.0) as u16);
                        }
                        let mut ring = f64::from(a.perimeter);
                        if kit::number_row(ui, "Rings", &mut ring, "", 0.0..=1024.0) {
                            a.perimeter = ring.round().clamp(0.0, 1024.0) as u16;
                        }
                    }
                }
            }
            if matches!(tool, FootprintTool::Drill | FootprintTool::Slot) {
                kit::number_row(ui, "Drill", &mut o.drill, "mm", 0.05..=50.0);
            }
            if tool.draws() {
                kit::choice_row(
                    ui,
                    "Layer",
                    &mut state.layer,
                    &ActiveLayer::ALL.map(|l| (l, l.label().to_owned())),
                );
            }
        });
        if response.done {
            self.footprint_arm(FootprintTool::Select);
        }
        true
    }
}

/// The tool card's name: the family, then the variant.
fn tool_name(tool: FootprintTool) -> &'static str {
    match tool {
        FootprintTool::Pad(PadTemplate::Rect) => "Pad · rectangle",
        FootprintTool::Pad(PadTemplate::RoundRect) => "Pad · rounded",
        FootprintTool::Pad(PadTemplate::Circle) => "Pad · round",
        FootprintTool::Pad(PadTemplate::Oblong) => "Pad · oblong",
        FootprintTool::CustomPad => "Pad · custom",
        FootprintTool::Array(ArrayTool::Linear) => "Pad row",
        FootprintTool::Array(ArrayTool::DualRow) => "Dual row",
        FootprintTool::Array(ArrayTool::Quad) => "Quad array",
        FootprintTool::Array(ArrayTool::Grid) => "Grid array",
        t => t.label(),
    }
}

/// The tool card's one hint line and its hover text.
fn tool_hint(tool: FootprintTool) -> (&'static str, &'static str) {
    match tool {
        FootprintTool::Pad(_) | FootprintTool::CustomPad => (
            "Click to place; Space turns the next pad",
            "Sizes become the pad_l, pad_w and pad_d parameters; Esc ends the tool",
        ),
        FootprintTool::Array(ArrayTool::DualRow) => (
            "Pad 1, then the row direction",
            "The second row faces the first, to the left of the row direction (counter-clockwise numbering)",
        ),
        FootprintTool::Array(ArrayTool::Grid) => (
            "Click pad A1; Select and Delete skips one",
            "Delete an instance with Select to skip it",
        ),
        FootprintTool::Drill | FootprintTool::Slot => (
            "On a pad: a plated hole; elsewhere a bare hole",
            "On a pad the hole is plated and bound to the drill parameter; elsewhere it is a non-plated (NPTH) hole",
        ),
        FootprintTool::Constrain(_) => (
            "Pick the entities to constrain",
            "Pick points or curves; the constraint applies once the picks are complete",
        ),
        t => (t.tip(), ""),
    }
}

/// The tools' overlay: sketch geometry, the selection, the gesture in
/// progress, the snap marker and a hint.
fn paint(
    painter: &egui::Painter,
    rect: Rect,
    view: View,
    state: &footprint_tools::ToolState,
    scene: &Scene<'_>,
    cursor: Option<&Snap>,
) {
    let t = Tokens::get();
    let colors = crate::palette::board();
    let sketch = scene.view;
    let selected_curve = |id: &pcbcraft_core::library::footprint_source::SketchCurveId| {
        state
            .selection
            .iter()
            .any(|p| matches!(p, Pick::Curve(c) if c == id))
    };
    // The sketch itself is drawn by the dimension overlay
    // (`footprint_dims_ui`) in the solver's colours; only the selection is
    // drawn here.
    for c in sketch.curves.iter().filter(|c| selected_curve(&c.id)) {
        let Some(shape) = shape_of(&c.geometry) else {
            continue;
        };
        let stroke = Stroke::new(2.0, colors.selected);
        if c.construction {
            symbol_view::paint_dashed(painter, view, &shape, stroke, 5.0, 3.0);
        } else {
            symbol_view::paint_shape(painter, view, &shape, None, Some(stroke));
        }
    }
    for p in &sketch.points {
        let selected = state
            .selection
            .iter()
            .any(|s| *s == Pick::Point(p.id.clone()));
        if selected {
            painter.circle_filled(view.screen(p.at), 4.0, colors.selected);
        }
    }
    for pick in &state.selection {
        if let Pick::Pad(id) = pick
            && let Some(pad) = scene.footprint.pads.iter().find(|q| q.id == *id)
        {
            symbol_view::paint_shape(
                painter,
                view,
                &pad.land(f64::from(CHORD_PX / view.scale)),
                None,
                Some(Stroke::new(2.0, colors.selected)),
            );
        }
    }
    // A moving selection, offset by the drag.
    if let Some(drag) = state.drag {
        let ghost = Stroke::new(1.5, t.preview_sketch);
        for pick in &state.selection {
            match pick {
                Pick::Pad(id) => {
                    if let Some(pad) = scene.footprint.pads.iter().find(|q| q.id == *id) {
                        let land = pad.land(f64::from(CHORD_PX / view.scale));
                        symbol_view::paint_shape(
                            painter,
                            view,
                            &offset(&land, drag.delta),
                            None,
                            Some(ghost),
                        );
                    }
                }
                Pick::Curve(id) => {
                    if let Some(shape) = sketch.curve(id).and_then(|c| shape_of(&c.geometry)) {
                        symbol_view::paint_shape(
                            painter,
                            view,
                            &offset(&shape, drag.delta),
                            None,
                            Some(ghost),
                        );
                    }
                }
                Pick::Point(id) => {
                    if let Some(p) = sketch.point(id) {
                        let at = [p.at[0] + drag.delta[0], p.at[1] + drag.delta[1]];
                        painter.circle_stroke(view.screen(at), 4.0, ghost);
                    }
                }
            }
        }
    }
    let tool = state.armed();
    if let Some(c) = cursor.filter(|_| tool != FootprintTool::Select) {
        for [a, b] in &c.guides {
            painter.add(egui::Shape::dashed_line(
                &[view.screen(*a), view.screen(*b)],
                Stroke::new(1.0, t.accent_soft),
                4.0,
                3.0,
            ));
        }
        paint_preview(painter, view, state, c.at);
        for p in &state.pending {
            painter.circle_filled(view.screen(*p), 3.0, t.preview_sketch);
        }
        let at = view.screen(c.at);
        let marker = match c.kind {
            snap::Kind::Free | snap::Kind::Grid => t.text_dim,
            _ => t.accent,
        };
        painter.circle_stroke(at, 4.0, Stroke::new(1.5, marker));
        let label = c.kind.label();
        if !label.is_empty() {
            painter.text(
                at + vec2(7.0, -7.0),
                Align2::LEFT_BOTTOM,
                label,
                egui::FontId::proportional(t.font_caption),
                t.accent,
            );
        }
    }
    // The armed tool and the active layer, under the view title.
    if tool != FootprintTool::Select || !state.selection.is_empty() {
        let hint = if tool == FootprintTool::Select {
            format!(
                "{} selected · drag to move · Delete removes",
                state.selection.len()
            )
        } else if tool.draws() {
            format!(
                "{} on {} · {}",
                tool.label(),
                state.layer.label(),
                tool.tip()
            )
        } else {
            format!("{} · {}", tool.label(), tool.tip())
        };
        painter.text(
            rect.left_top() + vec2(t.space_16, t.space_16 + t.font_panel * 1.6),
            Align2::LEFT_TOP,
            hint,
            type_face::medium(t.font_small),
            t.text_dim,
        );
    }
}

/// The shape being drawn and ghost pads.
fn paint_preview(
    painter: &egui::Painter,
    view: View,
    state: &footprint_tools::ToolState,
    at: Point,
) {
    let t = Tokens::get();
    let colors = crate::palette::board();
    let ink = Stroke::new(1.5, t.preview_sketch);
    let ghost_fill = Color32::from_rgba_unmultiplied(
        colors.copper_top.r(),
        colors.copper_top.g(),
        colors.copper_top.b(),
        90,
    );
    let o = &state.options;
    let turned = state.quarter_turns % 2 == 1;
    let pad_shape = |template: PadTemplate, c: Point, turned: bool| {
        let [mut w, mut h] = o.pad.size;
        if turned {
            std::mem::swap(&mut w, &mut h);
        }
        match template {
            PadTemplate::Circle => Shape::Circle {
                center: c,
                radius: o.pad.diameter * 0.5,
            },
            PadTemplate::Oblong => Shape::Rect {
                a: [c[0] - w / 2.0, c[1] - h / 2.0],
                b: [c[0] + w / 2.0, c[1] + h / 2.0],
                radius: w.min(h) / 2.0 * (1.0 - 1e-9),
            },
            PadTemplate::RoundRect => Shape::Rect {
                a: [c[0] - w / 2.0, c[1] - h / 2.0],
                b: [c[0] + w / 2.0, c[1] + h / 2.0],
                radius: w.min(h) * o.pad.radius_ratio.clamp(0.0, 0.5) * (1.0 - 1e-9),
            },
            PadTemplate::Rect => Shape::Rect {
                a: [c[0] - w / 2.0, c[1] - h / 2.0],
                b: [c[0] + w / 2.0, c[1] + h / 2.0],
                radius: 0.0,
            },
        }
    };
    let pending = &state.pending;
    match state.armed() {
        FootprintTool::Pad(template) => {
            symbol_view::paint_shape(
                painter,
                view,
                &pad_shape(template, at, turned),
                Some(ghost_fill),
                Some(ink),
            );
        }
        FootprintTool::CustomPad => {
            symbol_view::paint_shape(
                painter,
                view,
                &pad_shape(PadTemplate::Rect, at, turned),
                None,
                Some(ink),
            );
        }
        FootprintTool::Array(kind) => {
            let centres = footprint_tools::ghost_centres(state, at);
            let across = match (kind, pending.first()) {
                // Until the pointer leaves pad 1 the row runs down.
                (ArrayTool::Linear | ArrayTool::DualRow, Some(first))
                    if distance(*first, at) > 1e-9 =>
                {
                    (at[1] - first[1]).abs() > (at[0] - first[0]).abs()
                }
                (ArrayTool::Linear | ArrayTool::DualRow, Some(_)) => true,
                (ArrayTool::Linear | ArrayTool::DualRow, None) => true,
                _ => false,
            };
            let seed_turned = match kind {
                ArrayTool::Linear | ArrayTool::DualRow => across == turned,
                ArrayTool::Quad => {
                    matches!(o.array.pin1, Pin1Position::Bottom | Pin1Position::Top) != turned
                }
                ArrayTool::Grid => turned,
            };
            for (i, c) in centres.iter().enumerate() {
                // Quad sides alternate their long axis.
                let turn = match kind {
                    ArrayTool::Quad => {
                        let ny = usize::from(o.array.ny.min(256));
                        (i >= 2 * ny) != turned
                    }
                    _ => seed_turned,
                };
                let fill = if i == 0 { Some(ghost_fill) } else { None };
                symbol_view::paint_shape(
                    painter,
                    view,
                    &pad_shape(o.array.template, *c, turn),
                    fill,
                    Some(ink),
                );
            }
        }
        FootprintTool::Drill => {
            symbol_view::paint_shape(
                painter,
                view,
                &Shape::Circle {
                    center: at,
                    radius: o.drill * 0.5,
                },
                None,
                Some(ink),
            );
        }
        FootprintTool::Slot => {
            if let Some(a) = pending.first() {
                symbol_view::paint_shape(
                    painter,
                    view,
                    &Shape::Line { a: *a, b: at },
                    None,
                    Some(Stroke::new(
                        (o.drill as f32 * view.scale).max(1.5),
                        t.preview_sketch.gamma_multiply(0.5),
                    )),
                );
            }
        }
        FootprintTool::Line => {
            if let Some(a) = pending.first() {
                symbol_view::paint_shape(
                    painter,
                    view,
                    &Shape::Line { a: *a, b: at },
                    None,
                    Some(ink),
                );
            }
        }
        FootprintTool::Rect => {
            if let Some(a) = pending.first() {
                symbol_view::paint_shape(
                    painter,
                    view,
                    &Shape::Rect {
                        a: *a,
                        b: at,
                        radius: 0.0,
                    },
                    None,
                    Some(ink),
                );
            }
        }
        FootprintTool::Circle => {
            if let Some(c) = pending.first() {
                let r = distance(*c, at);
                if r > 1e-9 {
                    symbol_view::paint_shape(
                        painter,
                        view,
                        &Shape::Circle {
                            center: *c,
                            radius: r,
                        },
                        None,
                        Some(ink),
                    );
                }
            }
        }
        FootprintTool::Arc => match pending.as_slice() {
            [c] => {
                symbol_view::paint_shape(
                    painter,
                    view,
                    &Shape::Line { a: *c, b: at },
                    None,
                    Some(ink),
                );
            }
            [c, a] => {
                let r = distance(*c, *a);
                let a0 = (a[1] - c[1]).atan2(a[0] - c[0]).to_degrees();
                let a1 = (at[1] - c[1]).atan2(at[0] - c[0]).to_degrees();
                let ccw = (a1 - a0).rem_euclid(360.0);
                let sweep = if state.arc_clockwise() {
                    ccw - 360.0
                } else {
                    ccw
                };
                if r > 1e-9 && sweep.abs() > 1e-6 {
                    symbol_view::paint_shape(
                        painter,
                        view,
                        &Shape::Arc {
                            center: *c,
                            radius: r,
                            start_deg: a0,
                            sweep_deg: sweep,
                        },
                        None,
                        Some(ink),
                    );
                }
            }
            _ => {}
        },
        FootprintTool::Select | FootprintTool::Dimension | FootprintTool::Constrain(_) => {}
    }
}
