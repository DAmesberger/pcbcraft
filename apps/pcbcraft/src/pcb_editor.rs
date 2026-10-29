//! PCB content for Solidcraft's shared shell. Controls follow upstream
//! ui_kit/layout.rs and fields.rs; gesture previews commit on release, following
//! drag_snap.rs. The root owns the native Solidcraft viewport and its camera.
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Sense, Shape, Stroke, Ui, Vec2};
use pcbcraft_core::{
    Engine, Id, Layer, Operation, Placement, Point, Project,
    layout::{self, LayoutCommand, RouteRequest, WorldPad},
    schematic,
    updates::Change,
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Select,
    Route,
}

struct DragAirwires {
    key: (u64, Id, Point),
    connections: Vec<(WorldPad, WorldPad)>,
}

pub struct PcbEditor {
    view_3d: bool,
    tool: Tool,
    instance: Option<Id>,
    track: Option<Id>,
    net: Option<Id>,
    highlighted: Option<Id>,
    layer: Layer,
    width: f64,
    route_start: Option<Point>,
    property_key: Option<(u64, Option<Id>)>,
    position: Point,
    rotation: f64,
    placement_layer: Layer,
    update: crate::update_preview::UpdatePreview,
    zoom: f32,
    pan: Vec2,
    drag: Option<(Placement, Point)>,
    drag_position: Option<Point>,
    drag_airwires: Option<DragAirwires>,
    show_airwires: bool,
    cached_revision: Option<u64>,
    world_pads: Vec<WorldPad>,
    airwires: Vec<(WorldPad, WorldPad)>,
    issues: Vec<pcbcraft_core::Finding>,
}

impl Default for PcbEditor {
    fn default() -> Self {
        Self {
            view_3d: false,
            tool: Tool::Select,
            instance: None,
            track: None,
            net: None,
            highlighted: None,
            layer: Layer::Top,
            width: 0.3,
            route_start: None,
            property_key: None,
            position: [0.0; 2],
            rotation: 0.0,
            placement_layer: Layer::Top,
            update: Default::default(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            drag: None,
            drag_position: None,
            drag_airwires: None,
            show_airwires: true,
            cached_revision: None,
            world_pads: vec![],
            airwires: vec![],
            issues: vec![],
        }
    }
}

impl PcbEditor {
    pub fn is_3d(&self) -> bool {
        self.view_3d
    }
    pub fn set_3d(&mut self, enabled: bool) {
        self.view_3d = enabled;
        self.route_start = None;
        self.cancel_drag();
    }
    pub fn selected_instance(&self) -> Option<Id> {
        self.instance
    }
    pub fn selected_track(&self) -> Option<Id> {
        self.track
    }
    pub fn highlighted_net(&self) -> Option<Id> {
        self.highlighted
    }
    pub fn highlight_net(&mut self, net: Option<Id>) {
        self.highlighted = net;
    }
    pub fn set_selection(&mut self, instance: Option<Id>, track: Option<Id>, net: Option<Id>) {
        self.instance = instance;
        self.track = track;
        self.net = net;
        self.highlighted = net;
        self.property_key = None;
    }

    fn sync(&mut self, engine: &Engine) {
        if self.cached_revision != Some(engine.revision()) {
            self.cancel_drag();
            self.route_start = None;
            self.world_pads = layout::pads(engine.project());
            self.airwires = layout::airwires(engine.project());
            self.issues = layout::checks(engine.project());
            self.cached_revision = Some(engine.revision());
            if self
                .instance
                .is_some_and(|id| !engine.project().placements.iter().any(|p| p.instance == id))
            {
                self.instance = None;
            }
            if self
                .track
                .is_some_and(|id| !engine.project().tracks.iter().any(|t| t.id == id))
            {
                self.track = None;
            }
            if self
                .net
                .is_some_and(|id| engine.project().net(id).is_none())
            {
                self.net = None;
                self.highlighted = None;
                self.route_start = None;
            }
        }
        let key = (engine.revision(), self.instance);
        if self.property_key != Some(key) {
            if let Some(p) = engine
                .project()
                .placements
                .iter()
                .find(|p| Some(p.instance) == self.instance)
            {
                self.position = p.position;
                self.rotation = p.rotation_deg;
                self.placement_layer = p.layer;
            }
            self.property_key = Some(key);
        }
    }

    fn cancel_drag(&mut self) {
        self.drag = None;
        self.drag_position = None;
        self.drag_airwires = None;
    }

    /// Recompute only when the snapped position changes. The same bounded
    /// connectivity kernel used by the committed board detects newly detached
    /// routed pads as well as moving existing airwire endpoints. The candidate
    /// never enters Engine history, and is discarded on release or Escape.
    fn refresh_drag_airwires(&mut self, engine: &Engine) {
        let (Some((placement, _)), Some(position)) = (&self.drag, self.drag_position) else {
            self.drag_airwires = None;
            return;
        };
        let key = (engine.revision(), placement.instance, position);
        if self
            .drag_airwires
            .as_ref()
            .is_some_and(|preview| preview.key == key)
        {
            return;
        }
        let mut candidate = engine.project().clone();
        if let Some(target) = candidate
            .placements
            .iter_mut()
            .find(|p| p.instance == placement.instance)
        {
            target.position = position;
            self.drag_airwires = Some(DragAirwires {
                key,
                connections: layout::airwires(&candidate),
            });
        }
    }

    fn visible_airwires(&self) -> &[(WorldPad, WorldPad)] {
        self.drag_airwires
            .as_ref()
            .map(|preview| preview.connections.as_slice())
            .unwrap_or(&self.airwires)
    }

    pub fn tools(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if crate::shell::tool(
            ui,
            "component",
            "Update PCB",
            !layout::sync_preview(engine.project()).is_empty() && !self.update.is_open(),
            self.update.is_open(),
        )
        .clicked()
        {
            self.update.begin(engine, Change::SyncPcb {});
            self.tool = Tool::Select;
            self.route_start = None;
            self.cancel_drag();
        }
        if crate::shell::tool(ui, "move", "Select", true, self.tool == Tool::Select).clicked() {
            self.tool = Tool::Select;
            self.route_start = None;
        }
        if crate::shell::tool(
            ui,
            "trace",
            "Route",
            !self.view_3d && !self.update.is_open(),
            self.tool == Tool::Route,
        )
        .clicked()
        {
            self.tool = Tool::Route;
        }
        ui.separator();
        if crate::shell::tool(ui, "sketch", "2D", true, !self.view_3d).clicked() {
            self.set_3d(false);
        }
        if crate::shell::tool(ui, "box", "3D", true, self.view_3d).clicked() {
            self.set_3d(true);
        }
        if crate::shell::tool(ui, "fit", "Fit", !self.view_3d, false).clicked() {
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
        None
    }

    pub fn inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if let Some(operation) = self.update.show(ui, engine) {
            return Some(operation);
        }
        if self.update.is_open() {
            return None;
        }
        let t = Tokens::get();
        let project = engine.project();
        ui.label(egui::RichText::new("Board").font(fonts::semibold(t.font_panel)));
        ui.weak(format!(
            "{} × {} mm · 2 copper layers",
            project.board_size[0], project.board_size[1]
        ));
        if let Some(placement) = project
            .placements
            .iter()
            .find(|p| Some(p.instance) == self.instance)
        {
            let reference = project
                .instance(placement.instance)
                .map(|i| i.reference.as_str())
                .unwrap_or("Component");
            ui.separator();
            ui.label(egui::RichText::new(reference).font(fonts::semibold(t.font_panel)));
            ui.add_enabled_ui(!placement.locked, |ui| {
                number_row(ui, "X", &mut self.position[0], " mm");
                number_row(ui, "Y", &mut self.position[1], " mm");
                number_row(ui, "Rotation", &mut self.rotation, "°");
                layer_row(ui, &mut self.placement_layer);
            });
            if ui_kit::button(ui, "Apply placement", Button::Normal).clicked() && !placement.locked
            {
                return Some(Operation::Layout(LayoutCommand::Move {
                    instance: placement.instance,
                    position: self.position,
                    rotation_deg: self.rotation,
                    layer: self.placement_layer,
                }));
            }
            let mut action = None;
            ui.horizontal(|ui| {
                if ui_kit::button(ui, "Rotate 90°", Button::Small).clicked() && !placement.locked {
                    action = Some(LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: (placement.rotation_deg + 90.0).rem_euclid(360.0),
                        layer: placement.layer,
                    });
                }
                if ui_kit::button(ui, "Flip", Button::Small).clicked() && !placement.locked {
                    action = Some(LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: placement.rotation_deg,
                        layer: opposite(placement.layer),
                    });
                }
            });
            if let Some(action) = action {
                return Some(Operation::Layout(action));
            }
            let mut locked = placement.locked;
            if ui.checkbox(&mut locked, "Lock placement").changed() {
                return Some(Operation::Layout(LayoutCommand::SetPlacementLock {
                    instance: placement.instance,
                    locked,
                }));
            }
        }
        if let Some(track) = project.tracks.iter().find(|p| Some(p.id) == self.track) {
            ui.separator();
            ui.label(format!("Track {}", track.id));
            ui.weak(format!(
                "{} · {:.2} mm · {:?}",
                project.net_name(track.net),
                track.width,
                track.layer
            ));
            let mut locked = track.locked;
            if ui.checkbox(&mut locked, "Lock track").changed() {
                return Some(Operation::Layout(LayoutCommand::SetTrackLock {
                    id: track.id,
                    locked,
                }));
            }
            if ui_kit::button(ui, "Delete track", Button::Normal).clicked() && !locked {
                return Some(Operation::Layout(LayoutCommand::DeleteTrack {
                    id: track.id,
                }));
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Routing").font(fonts::semibold(t.font_panel)));
        number_row(ui, "Width", &mut self.width, " mm");
        let previous_layer = self.layer;
        layer_row(ui, &mut self.layer);
        if previous_layer != self.layer {
            self.route_start = None;
        }
        egui::ComboBox::from_id_salt("routing-net")
            .selected_text(
                self.net
                    .map_or("Select a net or pad", |id| project.net_name(id)),
            )
            .show_ui(ui, |ui| {
                for net in schematic::nets(project) {
                    if ui
                        .selectable_value(&mut self.net, Some(net.id), &net.name)
                        .changed()
                    {
                        self.route_start = None;
                        self.highlighted = Some(net.id);
                    }
                }
            });
        let mut highlight = self.highlighted.is_some();
        if ui
            .checkbox(&mut highlight, "Highlight complete net")
            .changed()
        {
            self.highlight_net(if highlight { self.net } else { None });
        }
        ui.checkbox(&mut self.show_airwires, "Show airwires");
        ui.weak(if self.route_start.is_some() {
            "Click the next point or destination pad. Escape ends routing."
        } else {
            "Route: click a pad, then a point or another pad. 0.5 mm grid."
        });
        ui.collapsing(format!("Board checks · {}", self.issues.len()), |ui| {
            ui.weak("Basic physical connectivity, outline and 0.2 mm copper clearance. This is not a complete manufacturing DRC.");
            if self.issues.is_empty() { ui.label("No issues found by these checks."); }
            for issue in self.issues.iter().take(40) { ui.label(&issue.message); }
            if self.issues.len() > 40 { ui.weak(format!("{} more issues", self.issues.len() - 40)); }
        });
        ui.collapsing("Placements", |ui| {
            for placement in &project.placements {
                let label = project
                    .instance(placement.instance)
                    .map(|i| i.reference.as_str())
                    .unwrap_or("Component");
                if ui_kit::button(
                    ui,
                    label,
                    if self.instance == Some(placement.instance) {
                        Button::SmallSelected
                    } else {
                        Button::Ghost
                    },
                )
                .clicked()
                {
                    self.set_selection(Some(placement.instance), None, None);
                }
            }
        });
        None
    }

    pub fn canvas(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        let project = engine.project();
        let t = Tokens::get();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        if response.dragged_by(PointerButton::Middle) {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.2, 12.0);
        }
        let scale = ((rect.width() - 100.0).max(20.0) / project.board_size[0] as f32)
            .min((rect.height() - 100.0).max(20.0) / project.board_size[1] as f32)
            * self.zoom;
        let origin = rect.center()
            + self.pan
            + egui::vec2(-project.board_size[0] as f32, project.board_size[1] as f32) * scale * 0.5;
        let screen = |p: Point| origin + egui::vec2(p[0] as f32, -p[1] as f32) * scale;
        let world = |p: Pos2| {
            [
                f64::from((p.x - origin.x) / scale),
                f64::from((origin.y - p.y) / scale),
            ]
        };
        let painter = ui.painter().with_clip_rect(rect);
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.hover_pos()));
        let cursor = pointer.map(world);
        let mut operation = None;
        if response.hovered() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.route_start = None;
            self.cancel_drag();
            self.tool = Tool::Select;
        }
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.drag_started_by(PointerButton::Primary)
        {
            let start = ui.input(|i| i.pointer.press_origin()).or(pointer);
            if let Some(start) = start {
                let at = world(start);
                if let Some(id) = pick_instance(project, &self.world_pads, at) {
                    self.set_selection(Some(id), None, None);
                    if let Some(placement) = project
                        .placements
                        .iter()
                        .find(|p| p.instance == id && !p.locked)
                    {
                        self.drag = Some((placement.clone(), at));
                    }
                }
            }
        }
        if let (Some((placement, start)), Some(at)) = (&self.drag, cursor) {
            self.drag_position = Some(snap([
                placement.position[0] + at[0] - start[0],
                placement.position[1] + at[1] - start[1],
            ]));
        }
        if response.drag_stopped_by(PointerButton::Primary)
            && let (Some((placement, _)), Some(position)) =
                (self.drag.take(), self.drag_position.take())
            && placement.position != position
        {
            operation = Some(Operation::Layout(LayoutCommand::Move {
                instance: placement.instance,
                position,
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            }));
        }
        if response.clicked_by(PointerButton::Primary)
            && let Some(at) = cursor
        {
            let snapped_pad = self
                .world_pads
                .iter()
                .filter(|p| p.layer == self.layer)
                .filter(|p| {
                    screen(p.position).distance(screen(at)) < 8.0
                        || layout::point_in_polygon(at, &layout::pad_polygon(p))
                })
                .min_by(|a, b| {
                    screen(a.position)
                        .distance(screen(at))
                        .total_cmp(&screen(b.position).distance(screen(at)))
                });
            if self.tool == Tool::Route {
                let destination = snapped_pad.map(|p| p.position).unwrap_or_else(|| snap(at));
                if let Some(start) = self.route_start {
                    if let Some(net) = self.net {
                        operation = Some(Operation::Layout(LayoutCommand::Route {
                            request: RouteRequest {
                                net,
                                start,
                                end: destination,
                                width: self.width,
                                layer: self.layer,
                            },
                        }));
                        // End the speculative gesture even if the engine rejects
                        // it. A later route never starts from uncommitted copper.
                        self.route_start = None;
                    }
                } else {
                    if let Some(pad) = snapped_pad {
                        self.net = Some(pad.net);
                        self.highlighted = self.net;
                    }
                    if self.net.is_some() {
                        self.route_start = Some(destination);
                    }
                }
            } else if let Some(pad) = self
                .world_pads
                .iter()
                .rev()
                .find(|p| layout::point_in_polygon(at, &layout::pad_polygon(p)))
            {
                let (instance, net) = (pad.instance, pad.net);
                self.set_selection(Some(instance), None, Some(net));
            } else if let Some(track) = project
                .tracks
                .iter()
                .rev()
                .find(|track| layout::point_in_polygon(at, &layout::track_polygon(track)))
            {
                self.set_selection(None, Some(track.id), Some(track.net));
            } else {
                self.set_selection(pick_instance(project, &self.world_pads, at), None, None);
            }
        }
        self.refresh_drag_airwires(engine);
        let board = [
            [0.0, 0.0],
            [project.board_size[0], 0.0],
            project.board_size,
            [0.0, project.board_size[1]],
        ];
        painter.add(Shape::convex_polygon(
            board.map(screen).to_vec(),
            t.panel.gamma_multiply(0.35),
            Stroke::new(1.5, t.body_edge),
        ));
        let grid_step = if scale < 6.0 { 5 } else { 1 };
        for x in (0..=project.board_size[0] as usize).step_by(grid_step) {
            painter.line_segment(
                [
                    screen([x as f64, 0.0]),
                    screen([x as f64, project.board_size[1]]),
                ],
                Stroke::new(0.5, t.grid),
            );
        }
        for y in (0..=project.board_size[1] as usize).step_by(grid_step) {
            painter.line_segment(
                [
                    screen([0.0, y as f64]),
                    screen([project.board_size[0], y as f64]),
                ],
                Stroke::new(0.5, t.grid),
            );
        }
        if self.show_airwires {
            for (a, b) in self.visible_airwires() {
                painter.line_segment(
                    [screen(a.position), screen(b.position)],
                    Stroke::new(1.0, t.text_dim.gamma_multiply(0.65)),
                );
            }
        }
        for track in &project.tracks {
            let selected = self.track == Some(track.id) || self.highlighted == Some(track.net);
            let ink = copper_color(track.layer, selected);
            painter.add(Shape::convex_polygon(
                layout::track_polygon(track).map(screen).to_vec(),
                ink,
                Stroke::NONE,
            ));
        }
        for placement in &project.placements {
            let mut shown = placement.clone();
            if self
                .drag
                .as_ref()
                .is_some_and(|(p, _)| p.instance == placement.instance)
                && let Some(position) = self.drag_position
            {
                shown.position = position;
            }
            if let Some(polygon) = body_polygon(project, &shown) {
                let ink = if self.instance == Some(shown.instance) {
                    t.body_selected
                } else {
                    t.text_dim
                };
                let vertices = polygon.map(screen);
                for i in 0..4 {
                    painter
                        .line_segment([vertices[i], vertices[(i + 1) % 4]], Stroke::new(1.0, ink));
                }
                let reference = project
                    .instance(shown.instance)
                    .map(|i| i.reference.as_str())
                    .unwrap_or("?");
                painter.text(
                    screen(shown.position) + egui::vec2(0.0, -12.0),
                    Align2::CENTER_BOTTOM,
                    reference,
                    fonts::semibold(t.font_label),
                    ink,
                );
            }
        }
        for pad in &self.world_pads {
            let mut shown = pad.clone();
            if let (Some((placement, _)), Some(position)) = (&self.drag, self.drag_position)
                && placement.instance == shown.instance
            {
                shown.position[0] += position[0] - placement.position[0];
                shown.position[1] += position[1] - placement.position[1];
            }
            let selected =
                self.instance == Some(shown.instance) || self.highlighted == Some(shown.net);
            painter.add(Shape::convex_polygon(
                layout::pad_polygon(&shown).map(screen).to_vec(),
                copper_color(shown.layer, selected),
                Stroke::NONE,
            ));
            if scale > 7.0 {
                painter.text(
                    screen(shown.position),
                    Align2::CENTER_CENTER,
                    shown.pad.to_string(),
                    FontId::proportional(t.font_small),
                    t.on_accent,
                );
            }
        }
        if let (Some(start), Some(at)) = (self.route_start, cursor) {
            let end = self
                .world_pads
                .iter()
                .find(|p| p.layer == self.layer && screen(p.position).distance(screen(at)) < 8.0)
                .map(|p| p.position)
                .unwrap_or_else(|| snap(at));
            painter.line_segment(
                [screen(start), screen(end)],
                Stroke::new((self.width as f32 * scale).max(1.0), t.body_selected),
            );
            painter.circle_stroke(screen(end), 5.0, Stroke::new(1.5, t.body_selected));
        }
        painter.text(
            rect.left_bottom() + egui::vec2(t.space_16, -t.space_16),
            Align2::LEFT_BOTTOM,
            format!(
                "{} · {:?} copper · grid 0.5 mm · {} airwires",
                if self.tool == Tool::Route {
                    "Route"
                } else {
                    "Select / move"
                },
                self.layer,
                self.visible_airwires().len()
            ),
            FontId::proportional(t.font_small),
            t.text_dim,
        );
        operation
    }
}

/// Same layer colours as the 3D model.
fn copper_color(layer: Layer, selected: bool) -> Color32 {
    let colors = crate::palette::board();
    if selected {
        colors.selected
    } else {
        colors.copper(layer)
    }
}
fn opposite(layer: Layer) -> Layer {
    if layer == Layer::Top {
        Layer::Bottom
    } else {
        Layer::Top
    }
}
fn snap(p: Point) -> Point {
    p.map(|v| (v * 2.0).round() / 2.0)
}
fn number_row(ui: &mut Ui, label: &str, value: &mut f64, suffix: &str) {
    let t = Tokens::get();
    ui.horizontal(|ui| {
        ui.add_sized(
            [t.form_label_width, t.control_height],
            egui::Label::new(label),
        );
        ui.add(egui::DragValue::new(value).speed(0.1).suffix(suffix));
    });
}
fn layer_row(ui: &mut Ui, layer: &mut Layer) {
    ui.horizontal(|ui| {
        for (value, label) in [(Layer::Top, "Top"), (Layer::Bottom, "Bottom")] {
            if ui_kit::button(
                ui,
                label,
                if *layer == value {
                    Button::SmallSelected
                } else {
                    Button::Small
                },
            )
            .clicked()
            {
                *layer = value;
            }
        }
    });
}
fn body_polygon(project: &Project, placement: &Placement) -> Option<[Point; 4]> {
    let instance = project.instance(placement.instance)?;
    let part = project.part(instance.part)?;
    let [x, y, _] = part.package.body_size;
    let [dx, dy, _] = part.package.model_offset;
    Some(
        [
            [-x / 2.0 + dx, -y / 2.0 + dy],
            [x / 2.0 + dx, -y / 2.0 + dy],
            [x / 2.0 + dx, y / 2.0 + dy],
            [-x / 2.0 + dx, y / 2.0 + dy],
        ]
        .map(|p| layout::placed_point(placement, p, [0.0; 2])),
    )
}
fn pick_instance(project: &Project, pads: &[WorldPad], at: Point) -> Option<Id> {
    pads.iter()
        .rev()
        .find(|p| layout::point_in_polygon(at, &layout::pad_polygon(p)))
        .map(|p| p.instance)
        .or_else(|| {
            project
                .placements
                .iter()
                .rev()
                .find(|p| {
                    body_polygon(project, p)
                        .is_some_and(|polygon| layout::point_in_polygon(at, &polygon))
                })
                .map(|p| p.instance)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routed_board() -> pcbcraft_core::Result<Engine> {
        let mut engine = Engine::demo()?;
        let net = engine
            .project()
            .net_of(pcbcraft_core::Endpoint {
                instance: 2,
                terminal: 2,
            })
            .map(|n| n.id)
            .ok_or_else(|| pcbcraft_core::Error::NotFound("demo net".into()))?;
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start: [9.2, 8.0],
                    end: [14.8, 8.0],
                    width: 0.3,
                    layer: Layer::Top,
                },
            }),
        )?;
        Ok(engine)
    }

    #[test]
    fn drag_reconnects_airwires_live_and_cancel_preserves_document_and_history()
    -> pcbcraft_core::Result<()> {
        let mut engine = routed_board()?;
        let original = engine.project().clone();
        let revision = engine.revision();
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        assert!(editor.visible_airwires().is_empty());
        let placement = engine
            .project()
            .placements
            .iter()
            .find(|p| p.instance == 3)
            .cloned()
            .ok_or_else(|| pcbcraft_core::Error::Invalid("fixture placement missing".into()))?;
        editor.drag = Some((placement.clone(), placement.position));
        editor.drag_position = Some([20.0, 13.0]);
        editor.refresh_drag_airwires(&engine);
        let first = editor.visible_airwires().to_vec();
        assert_eq!(first.len(), 1);
        let moved = [&first[0].0, &first[0].1]
            .into_iter()
            .find(|pad| pad.instance == 3)
            .ok_or_else(|| pcbcraft_core::Error::Invalid("drag endpoint missing".into()))?;
        assert!((moved.position[0] - 18.8).abs() < 1e-9);
        assert_eq!(moved.position[1], 13.0);
        editor.drag_position = Some([22.0, 15.0]);
        editor.refresh_drag_airwires(&engine);
        let final_preview = editor.visible_airwires().to_vec();
        assert_ne!(first, final_preview);
        assert_eq!(engine.project(), &original);
        assert_eq!(engine.revision(), revision);
        editor.cancel_drag();
        assert!(editor.visible_airwires().is_empty());
        assert_eq!(engine.project(), &original);
        assert_eq!(engine.revision(), revision);

        engine.apply(
            revision,
            Operation::Layout(LayoutCommand::Move {
                instance: 3,
                position: [22.0, 15.0],
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            }),
        )?;
        editor.sync(&engine);
        assert_eq!(editor.visible_airwires(), final_preview);
        assert_eq!(engine.revision(), revision + 1);
        engine.undo()?;
        editor.sync(&engine);
        assert!(editor.visible_airwires().is_empty());
        assert_eq!(engine.project(), &original);
        Ok(())
    }

    fn frame(
        editor: &mut PcbEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut command = None;
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    egui::vec2(700.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                command = editor.inspector(ui, engine);
            },
        );
        (output, command)
    }

    fn text_position(shape: &Shape, label: &str) -> Option<Pos2> {
        match shape {
            Shape::Text(text) if text.galley.job.text == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_position(shape, label)),
            _ => None,
        }
    }

    fn click(
        editor: &mut PcbEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
    ) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let (output, _) = frame(editor, engine, ctx, vec![]);
        let point = output
            .shapes
            .iter()
            .find_map(|shape| text_position(&shape.shape, label))
            .ok_or_else(|| format!("missing button {label}"))?;
        frame(
            editor,
            engine,
            ctx,
            vec![
                egui::Event::PointerMoved(point),
                egui::Event::PointerButton {
                    pos: point,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        Ok(frame(
            editor,
            engine,
            ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
        .1)
    }

    #[test]
    fn sync_preview_cancels_cleanly_and_requires_recompute_after_document_edit()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        engine.apply(
            engine.revision(),
            Operation::Schematic(schematic::SchematicCommand::Place {
                part: pcbcraft_core::PartRef { id: 1, revision: 1 },
                reference: "R3".into(),
                value: "1k".into(),
                position: [30.0, 0.0],
            }),
        )?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.update.begin(&engine, Change::SyncPcb {});
        let before = engine.project().clone();
        let revision = engine.revision();
        assert!(click(&mut editor, &engine, &ctx, "Cancel")?.is_none());
        assert!(!editor.update.is_open());
        assert_eq!(engine.project(), &before);
        assert_eq!(engine.revision(), revision);

        editor.update.begin(&engine, Change::SyncPcb {});
        engine.apply(
            engine.revision(),
            Operation::Schematic(schematic::SchematicCommand::Edit {
                instance: 2,
                reference: "R1".into(),
                value: "22k".into(),
            }),
        )?;
        assert!(click(&mut editor, &engine, &ctx, "Apply update")?.is_none());
        assert!(editor.update.is_open());
        assert!(click(&mut editor, &engine, &ctx, "Recompute")?.is_none());
        let operation =
            click(&mut editor, &engine, &ctx, "Apply update")?.ok_or("missing reviewed update")?;
        let Operation::Update(request) = &operation else {
            return Err("sync bypassed the review command".into());
        };
        assert_eq!(request.expected_revision, engine.revision());
        engine.apply(engine.revision(), operation)?;
        assert!(layout::sync_preview(engine.project()).is_empty());
        assert_eq!(engine.project().placements.len(), 3);
        Ok(())
    }
}
