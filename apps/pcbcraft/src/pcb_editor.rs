//! PCB content for Solidcraft's shared shell. Controls follow upstream
//! ui_kit/layout.rs and fields.rs; gesture previews commit on release, following
//! drag_snap.rs. The root owns the native Solidcraft viewport and its camera.
use crate::kit::{self, Tone};
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Sense, Shape, Stroke, Ui, Vec2};
use pcbcraft_core::{
    Engine, Id, Layer, Operation, Placement, Point, Project, Track, instances,
    layout::{self, LayoutCommand, RouteRequest, WorldPad},
    schematic,
    updates::Change,
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};

/// Bound on typed placement coordinates (mm); the board is far smaller.
const COORDINATE_LIMIT: f64 = 10_000.0;
/// Board-check findings listed in the card; the rest are counted.
const MAX_LISTED_ISSUES: usize = 40;

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

    /// Tabs are the 2D layout and 3D views of one board, with shared
    /// selection and undo; groups are commands.
    pub fn ribbon(&mut self, engine: &Engine) -> crate::shell::Ribbon {
        use crate::shell::{Group, Ribbon, Tab, Tool as Button};
        self.sync(engine);
        let reviewing = self.update.is_open();
        let tabs = vec![
            Tab {
                id: "layout",
                label: "Layout",
                active: !self.view_3d,
            },
            Tab {
                id: "3d",
                label: "3D",
                active: self.view_3d,
            },
        ];
        if reviewing {
            return Ribbon {
                tabs,
                groups: vec![],
                notice: Some("Review the update in Properties"),
            };
        }
        let pending = !layout::sync_preview(engine.project()).is_empty();
        Ribbon {
            tabs,
            groups: vec![
                Group {
                    label: "Schematic",
                    tools: vec![
                        Button::new("update", "component", "Update PCB")
                            .enabled(pending)
                            .tip("Preview bringing schematic changes to the board"),
                    ],
                },
                Group {
                    label: "Edit",
                    tools: vec![
                        Button::new("select", "move", "Select")
                            .selected(self.tool == Tool::Select)
                            .tip("Select and move placements"),
                        Button::new("route", "trace", "Route")
                            .enabled(!self.view_3d)
                            .selected(self.tool == Tool::Route)
                            .tip("Route copper from a pad; switch to Layout to route"),
                    ],
                },
            ],
            notice: None,
        }
    }

    pub fn on_tab(&mut self, tab: &str) {
        self.set_3d(tab == "3d");
    }

    pub fn on_tool(&mut self, tool: &str, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        match tool {
            "update" => {
                self.update.begin(engine, Change::SyncPcb {});
                self.tool = Tool::Select;
                self.route_start = None;
                self.cancel_drag();
            }
            "select" => {
                self.tool = Tool::Select;
                self.route_start = None;
            }
            "route" => self.tool = Tool::Route,
            _ => {}
        }
        None
    }

    pub fn header(&self, engine: &Engine) -> crate::shell::Header {
        use crate::shell::Header;
        let project = engine.project();
        if self.update.is_open() {
            return Header::new("component", "Update PCB", "Review");
        }
        if let Some(instance) = self.instance.and_then(|id| project.instance(id)) {
            return Header::new("component", instance.designator.clone(), "Placement");
        }
        if let Some(track) = self.track {
            return Header::new("trace", format!("Track {track}"), "Track");
        }
        Header::new("plane", "Board", "PCB")
    }

    /// Return the layout view to its fitted scale.
    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    /// Properties card body. The card header already names the selection, so
    /// the body starts with its first section.
    pub fn inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if self.update.is_open() {
            return self.update.show(ui, engine);
        }
        let project = engine.project();
        let mut operation = None;
        if let Some(placement) = project
            .placements
            .iter()
            .find(|p| Some(p.instance) == self.instance)
        {
            operation = self.placement_section(ui, placement);
        } else if let Some(track) = project.tracks.iter().find(|t| Some(t.id) == self.track) {
            operation = track_section(ui, project, track);
        } else {
            kit::section(ui, "pcb-board", "Board", true, None, |ui| {
                kit::value_row(
                    ui,
                    "Size",
                    &format!(
                        "{} × {} mm",
                        kit::number_text(project.board_size[0]),
                        kit::number_text(project.board_size[1])
                    ),
                );
                kit::value_row(ui, "Copper", "2 layers");
            });
        }
        self.routing_section(ui, project);
        self.checks_section(ui);
        operation
    }

    fn placement_section(&mut self, ui: &mut Ui, placement: &Placement) -> Option<Operation> {
        let locked = placement.locked;
        let mut action = None;
        kit::section(ui, "pcb-placement", "Placement", true, None, |ui| {
            ui.add_enabled_ui(!locked, |ui| {
                kit::pair_row(
                    ui,
                    "Position",
                    &mut self.position,
                    "mm",
                    -COORDINATE_LIMIT..=COORDINATE_LIMIT,
                );
                kit::number_row(ui, "Rotation", &mut self.rotation, "°", 0.0..=360.0);
                kit::choice_row(ui, "Side", &mut self.placement_layer, &layer_options());
            });
            let mut lock = locked;
            if kit::check_row(ui, "Locked", &mut lock) {
                action = Some(LayoutCommand::SetPlacementLock {
                    instance: placement.instance,
                    locked: lock,
                });
            }
            if locked {
                kit::note(ui, "Unlock the placement to move, rotate or flip it.");
            }
            let edit = placement_edit(
                placement,
                self.position,
                self.rotation,
                self.placement_layer,
            );
            kit::actions(ui, |ui| {
                let apply = kit::primary(ui, "Apply", edit.is_some());
                if apply.clicked() && edit.is_some() {
                    action = edit;
                }
                if ui_kit::button(ui, "Flip", Button::Enabled(!locked)).clicked() && !locked {
                    action = Some(LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: placement.rotation_deg,
                        layer: opposite(placement.layer),
                    });
                }
                if ui_kit::button(ui, "Rotate 90°", Button::Enabled(!locked)).clicked() && !locked
                {
                    action = Some(LayoutCommand::Move {
                        instance: placement.instance,
                        position: placement.position,
                        rotation_deg: (placement.rotation_deg + 90.0).rem_euclid(360.0),
                        layer: placement.layer,
                    });
                }
            });
        });
        action.map(Operation::Layout)
    }

    fn routing_section(&mut self, ui: &mut Ui, project: &Project) {
        kit::section(ui, "pcb-routing", "Routing", true, None, |ui| {
            kit::number_row(ui, "Width", &mut self.width, "mm", 0.05..=10.0);
            if kit::choice_row(ui, "Layer", &mut self.layer, &layer_options()) {
                self.route_start = None;
            }
            let mut nets = vec![(None, "Select a net or pad".to_owned())];
            nets.extend(
                schematic::nets(project)
                    .iter()
                    .map(|net| (Some(net.id), net.name.clone())),
            );
            if kit::choice_row(ui, "Net", &mut self.net, &nets) {
                self.route_start = None;
                self.highlighted = self.net;
            }
            let mut highlight = self.highlighted.is_some();
            if kit::check_row(ui, "Highlight complete net", &mut highlight) {
                self.highlight_net(if highlight { self.net } else { None });
            }
            kit::check_row(ui, "Show airwires", &mut self.show_airwires);
            kit::note(
                ui,
                if self.route_start.is_some() {
                    "Click a point or pad. Escape ends the route."
                } else {
                    "Route: click a pad, then a point or pad."
                },
            );
        });
    }

    fn checks_section(&self, ui: &mut Ui) {
        let count = self.issues.len();
        kit::section(
            ui,
            "pcb-checks",
            "Board checks",
            count > 0,
            Some(count),
            |ui| {
                if self.issues.is_empty() {
                    kit::status(ui, "No issues found by these checks.", Tone::Dim);
                }
                for issue in self.issues.iter().take(MAX_LISTED_ISSUES) {
                    kit::status(ui, &issue.message, Tone::Error);
                }
                if count > MAX_LISTED_ISSUES {
                    kit::note(ui, &format!("{} more issues", count - MAX_LISTED_ISSUES));
                }
                kit::note(
                    ui,
                    "Connectivity, outline and 0.2 mm copper clearance only; not a full manufacturing DRC.",
                );
            },
        );
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
                    .map(|i| i.designator.as_str())
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
fn layer_name(layer: Layer) -> &'static str {
    match layer {
        Layer::Top => "Top",
        Layer::Bottom => "Bottom",
    }
}
fn layer_options() -> [(Layer, String); 2] {
    [Layer::Top, Layer::Bottom].map(|layer| (layer, layer_name(layer).to_owned()))
}
/// The Move that the placement form's Apply button issues: `None` while the
/// placement is locked or the form still matches it.
fn placement_edit(
    placement: &Placement,
    position: Point,
    rotation_deg: f64,
    layer: Layer,
) -> Option<LayoutCommand> {
    let unchanged = placement.position == position
        && placement.rotation_deg == rotation_deg
        && placement.layer == layer;
    (!placement.locked && !unchanged).then_some(LayoutCommand::Move {
        instance: placement.instance,
        position,
        rotation_deg,
        layer,
    })
}
/// Read-only track properties with lock and delete.
fn track_section(ui: &mut Ui, project: &Project, track: &Track) -> Option<Operation> {
    let mut action = None;
    kit::section(ui, "pcb-track", "Track", true, None, |ui| {
        kit::value_row(ui, "Net", project.net_name(track.net));
        kit::value_row(
            ui,
            "Width",
            &format!("{} mm", kit::number_text(track.width)),
        );
        kit::value_row(ui, "Layer", layer_name(track.layer));
        let mut locked = track.locked;
        if kit::check_row(ui, "Locked", &mut locked) {
            action = Some(LayoutCommand::SetTrackLock {
                id: track.id,
                locked,
            });
        }
        kit::actions(ui, |ui| {
            if ui_kit::button(ui, "Delete track", Button::Enabled(!track.locked)).clicked()
                && !track.locked
            {
                action = Some(LayoutCommand::DeleteTrack { id: track.id });
            }
        });
    });
    action.map(Operation::Layout)
}
fn snap(p: Point) -> Point {
    p.map(|v| (v * 2.0).round() / 2.0)
}
fn body_polygon(project: &Project, placement: &Placement) -> Option<[Point; 4]> {
    let instance = project.instance(placement.instance)?;
    let package = &instances::resolve(&project.library, instance)
        .ok()?
        .physical?
        .package
        .definition;
    let [x, y, _] = package.body_size;
    let [dx, dy, _] = package.model_offset;
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

    fn ids(engine: &Engine) -> (Id, Id) {
        let id = |d| {
            engine
                .project()
                .instance_by_designator(d)
                .map_or(0, |i| i.id)
        };
        (id("R1"), id("R2"))
    }

    fn routed_board() -> pcbcraft_core::Result<Engine> {
        let mut engine = Engine::demo()?;
        let net = engine
            .project()
            .net_of(pcbcraft_core::Endpoint {
                instance: ids(&engine).0,
                terminal: pcbcraft_core::TerminalId(2),
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
        let (_, r2) = ids(&engine);
        let original = engine.project().clone();
        let revision = engine.revision();
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        assert!(editor.visible_airwires().is_empty());
        let placement = engine
            .project()
            .placements
            .iter()
            .find(|p| p.instance == r2)
            .cloned()
            .ok_or_else(|| pcbcraft_core::Error::Invalid("fixture placement missing".into()))?;
        editor.drag = Some((placement.clone(), placement.position));
        editor.drag_position = Some([20.0, 13.0]);
        editor.refresh_drag_airwires(&engine);
        let first = editor.visible_airwires().to_vec();
        assert_eq!(first.len(), 1);
        let moved = [&first[0].0, &first[0].1]
            .into_iter()
            .find(|pad| pad.instance == r2)
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
                instance: r2,
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
        let mut output = ctx.run_ui(
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
        // No renderer in tests: drop font-atlas uploads instead of applying them.
        output.textures_delta.clear();
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
    fn apply_issues_the_move_the_engine_accepts_and_lock_disables_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let placement = |engine: &Engine| {
            engine
                .project()
                .placements
                .iter()
                .find(|p| p.instance == r1)
                .cloned()
                .ok_or("fixture placement missing")
        };
        let before = placement(&engine)?;
        assert!(
            placement_edit(&before, before.position, before.rotation_deg, before.layer).is_none()
        );
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(r1), None, None);
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(editor.position, before.position);
        // The form edits that the Position, Rotation and Side rows produce.
        editor.position = [12.0, 9.5];
        editor.rotation = 90.0;
        editor.placement_layer = Layer::Bottom;
        let expected = placement_edit(&before, [12.0, 9.5], 90.0, Layer::Bottom)
            .ok_or("moved placement has no edit")?;
        let LayoutCommand::Move {
            instance,
            position,
            rotation_deg,
            layer,
        } = expected
        else {
            return Err("Apply is not a move".into());
        };
        assert_eq!(
            (instance, position, rotation_deg, layer),
            (r1, [12.0, 9.5], 90.0, Layer::Bottom)
        );
        let operation = click(&mut editor, &engine, &ctx, "Apply")?.ok_or("Apply did nothing")?;
        let Operation::Layout(LayoutCommand::Move {
            instance: i,
            position: p,
            rotation_deg: r,
            layer: l,
        }) = &operation
        else {
            return Err("Apply issued another command".into());
        };
        assert_eq!((*i, *p, *r, *l), (instance, position, rotation_deg, layer));
        engine.apply(engine.revision(), operation)?;
        let after = placement(&engine)?;
        assert_eq!(
            (after.position, after.rotation_deg, after.layer),
            (position, rotation_deg, layer)
        );
        // The engine rejects moving a locked placement, so the form offers none.
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::SetPlacementLock {
                instance: r1,
                locked: true,
            }),
        )?;
        let locked = placement(&engine)?;
        assert!(placement_edit(&locked, [1.0, 1.0], 0.0, Layer::Top).is_none());
        editor.position = [1.0, 1.0];
        assert!(click(&mut editor, &engine, &ctx, "Apply")?.is_none());
        Ok(())
    }

    #[test]
    fn sync_preview_cancels_cleanly_and_requires_recompute_after_document_edit()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let first = engine.project().instances[0].clone();
        engine.apply(
            engine.revision(),
            Operation::Instance(pcbcraft_core::InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    component: first.component,
                    designator: Some("R3".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: [30.0, 0.0],
                    physical: first.physical,
                },
            )),
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
            Operation::Instance(pcbcraft_core::InstanceCommand::SetDesignator {
                instance: first.id,
                designator: "R9".into(),
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
