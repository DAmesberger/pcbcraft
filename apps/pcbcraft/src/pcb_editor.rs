//! PCB content for Solidcraft's shared shell. Controls follow upstream
//! ui_kit/layout.rs and fields.rs; gesture previews commit on release, following
//! drag_snap.rs. The root owns the native Solidcraft viewport and its camera.
//!
//! Layout draws only what core places ([`layout::placed_pads`],
//! [`layout::placed_graphics`]) and picks through core queries
//! ([`layout::pick_pad`], [`layout::pick_graphic`]); it never decides pad or
//! graphic geometry. The cached board is rebuilt per document revision, not
//! on selection, layer or camera changes.
use crate::kit::{self, Tone};
use crate::symbol_view::{self, View};
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use pcbcraft_core::{
    Engine, Fabrication, Id, Layer, MaskColour, MaskFinish, Operation, PadFinish, Placement, Point,
    Project, SilkscreenColour, Track,
    geometry::{Shape as CoreShape, Transform2},
    instances,
    layout::{
        self, BoardCopper, BoardDrawing, LayoutCommand, PlacedGraphic, PlacedHole, PlacedItem,
        PlacedPad, RouteRequest, WorldPad,
    },
    library::{PadKind, PlacedText, TextMetric},
    schematic,
    updates::Change,
};
use solidcraft_ui_egui::{
    fonts,
    theme::{self, Tokens},
    ui_kit::{self, Button},
};
use std::collections::BTreeSet;

/// Scroll and pinch zoom limits, relative to the board fitting the canvas.
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.2..=12.0;
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

/// Which board layers Layout draws and picks. Hidden layers do not pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layers {
    copper_top: bool,
    copper_bottom: bool,
    silk_top: bool,
    silk_bottom: bool,
    courtyard: bool,
    assembly: bool,
    holes: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            copper_top: true,
            copper_bottom: true,
            silk_top: true,
            silk_bottom: true,
            courtyard: true,
            assembly: true,
            holes: true,
        }
    }
}

impl Layers {
    fn copper(&self, layer: Layer) -> bool {
        match layer {
            Layer::Top => self.copper_top,
            Layer::Bottom => self.copper_bottom,
        }
    }
    /// A through-hole pad shows while either copper layer does; a bare
    /// hole with the hole layer.
    fn pad(&self, pad: &PlacedPad) -> bool {
        match pad.pad.kind {
            PadKind::PlatedThrough => self.copper_top || self.copper_bottom,
            PadKind::NonPlatedHole => self.holes,
            PadKind::Smd | PadKind::Fiducial | PadKind::Mechanical => self.copper(pad.layer),
        }
    }
    fn drawing(&self, layer: BoardDrawing) -> bool {
        match layer {
            BoardDrawing::Silkscreen(Layer::Top) => self.silk_top,
            BoardDrawing::Silkscreen(Layer::Bottom) => self.silk_bottom,
            BoardDrawing::Courtyard(_) => self.courtyard,
            BoardDrawing::Assembly(_) => self.assembly,
        }
    }
}

/// The board side of a drawing layer.
fn drawing_side(layer: BoardDrawing) -> Layer {
    match layer {
        BoardDrawing::Silkscreen(side)
        | BoardDrawing::Courtyard(side)
        | BoardDrawing::Assembly(side) => side,
    }
}

/// One placed pad with its board outline and holes, cached per revision.
struct BoardPad {
    placed: PlacedPad,
    outline: CoreShape,
    holes: Vec<PlacedHole>,
}

impl BoardPad {
    fn contains(&self, p: Point) -> bool {
        self.outline.distance(p, true) <= 0.0
    }
}

/// Board millimetres to screen pixels, seen from above or (mirrored in x
/// about the board centre) from below.
#[derive(Clone, Copy)]
struct BoardView {
    view: View,
    flip: Option<Transform2>,
}

impl BoardView {
    fn new(view: View, board_width: f64, from_bottom: bool) -> Self {
        Self {
            view,
            // x → width − x: reflect across the x-axis, then turn 180°.
            flip: from_bottom.then_some(Transform2 {
                translate: [board_width, 0.0],
                rotation_deg: 180.0,
                mirror: true,
            }),
        }
    }
    fn seen(&self, p: Point) -> Point {
        self.flip.map_or(p, |f| f.apply(p))
    }
    fn screen(&self, p: Point) -> Pos2 {
        self.view.screen(self.seen(p))
    }
    /// The flip is its own inverse.
    fn world(&self, p: Pos2) -> Point {
        self.seen(self.view.world(p))
    }
    fn shape(&self, shape: &CoreShape, delta: Point) -> CoreShape {
        let moved = if delta == [0.0, 0.0] {
            shape.clone()
        } else {
            shape.translated(delta)
        };
        match self.flip {
            Some(f) => moved.transformed(&f),
            None => moved,
        }
    }
    fn text(&self, text: &PlacedText, delta: Point) -> PlacedText {
        let mut seen = text.clone();
        seen.at = [text.at[0] + delta[0], text.at[1] + delta[1]];
        if let Some(f) = self.flip {
            seen.at = f.apply(seen.at);
            seen.rotation_deg = f.apply_angle(text.rotation_deg).rem_euclid(360.0);
            seen.mirrored = !text.mirrored;
        }
        seen
    }
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
    /// Connected pads with their exact outlines, built once per revision;
    /// the checks, airwires and drag preview read it.
    copper: BoardCopper,
    airwires: Vec<(WorldPad, WorldPad)>,
    issues: Vec<pcbcraft_core::Finding>,
    board_pads: Vec<BoardPad>,
    graphics: Vec<PlacedGraphic>,
    /// Instances whose footprint draws an assembly or courtyard outline;
    /// the others get the package envelope as their outline.
    outlined: BTreeSet<Id>,
    /// Instances whose designator a placed text already shows.
    labelled: BTreeSet<Id>,
    layers: Layers,
    /// PCB 3D's layer toggles; its outer copper and silkscreen entries are
    /// `layers`' ([`PcbEditor::layer_view`]).
    view_layers: crate::layer_view::LayerView,
    from_bottom: bool,
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
            copper: BoardCopper::default(),
            airwires: vec![],
            issues: vec![],
            board_pads: vec![],
            graphics: vec![],
            outlined: BTreeSet::new(),
            labelled: BTreeSet::new(),
            layers: Layers::default(),
            view_layers: Default::default(),
            from_bottom: false,
        }
    }
}

impl PcbEditor {
    /// The layers PCB 3D shows: its own toggles, with the outer copper and
    /// silkscreen toggles it shares with PCB 2D.
    pub fn layer_view(&self) -> crate::layer_view::LayerView {
        let mut view = self.view_layers.clone();
        for (layer, visible) in self.shared_layers() {
            view.set(layer, visible);
        }
        view
    }

    /// Set the layers PCB 3D shows; the shared toggles change in 2D too.
    pub fn set_layer_view(&mut self, view: crate::layer_view::LayerView) {
        use crate::layer_view::ViewLayer;
        let l = &mut self.layers;
        for (layer, flag) in [
            (ViewLayer::Copper(Layer::Top.into()), &mut l.copper_top),
            (
                ViewLayer::Copper(Layer::Bottom.into()),
                &mut l.copper_bottom,
            ),
            (ViewLayer::Silkscreen(Layer::Top), &mut l.silk_top),
            (ViewLayer::Silkscreen(Layer::Bottom), &mut l.silk_bottom),
        ] {
            *flag = view.visible(layer);
        }
        self.view_layers = view;
    }

    fn shared_layers(&self) -> [(crate::layer_view::ViewLayer, bool); 4] {
        use crate::layer_view::ViewLayer;
        let l = &self.layers;
        [
            (ViewLayer::Copper(Layer::Top.into()), l.copper_top),
            (ViewLayer::Copper(Layer::Bottom.into()), l.copper_bottom),
            (ViewLayer::Silkscreen(Layer::Top), l.silk_top),
            (ViewLayer::Silkscreen(Layer::Bottom), l.silk_bottom),
        ]
    }

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
        self.discard_staged();
    }

    /// Drop unapplied Position, Rotation and Side edits; the next sync reloads
    /// them from the document. Called on every tool or subject change.
    fn discard_staged(&mut self) {
        self.property_key = None;
    }

    fn sync(&mut self, engine: &Engine) {
        if self.cached_revision != Some(engine.revision()) {
            self.cancel_drag();
            self.route_start = None;
            self.copper = BoardCopper::of(engine.project());
            self.airwires = self.copper.airwires(&engine.project().tracks);
            self.issues = self.copper.checks(engine.project());
            self.cache_board(engine.project());
            self.cached_revision = Some(engine.revision());
            // An instance without a placement stays selected: the card
            // shows it as not placed.
            if self
                .instance
                .is_some_and(|id| engine.project().instance(id).is_none())
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

    /// Place pads and graphics once per revision.
    fn cache_board(&mut self, project: &Project) {
        self.board_pads = layout::placed_pads(project)
            .into_iter()
            .map(|placed| BoardPad {
                outline: placed.outline(),
                holes: placed.holes(),
                placed,
            })
            .collect();
        self.graphics = layout::placed_graphics(project);
        self.outlined = self
            .graphics
            .iter()
            .filter(|g| {
                matches!(
                    g.layer,
                    BoardDrawing::Assembly(_) | BoardDrawing::Courtyard(_)
                )
            })
            .map(|g| g.instance)
            .collect();
        self.labelled = self
            .graphics
            .iter()
            .filter(|g| {
                matches!(&g.item, PlacedItem::Text(text) if project
                    .instance(g.instance)
                    .is_some_and(|i| !i.designator.is_empty() && text.text == i.designator))
            })
            .map(|g| g.instance)
            .collect();
    }

    fn cancel_drag(&mut self) {
        self.drag = None;
        self.drag_position = None;
        self.drag_airwires = None;
    }

    /// Recompute only when the snapped position changes. The same bounded
    /// connectivity kernel used by the committed board, over the revision's
    /// cached copper with the dragged part's outlines moved, detects newly
    /// detached routed pads as well as moving existing airwire endpoints.
    /// The preview never enters Engine history, and is discarded on release
    /// or Escape.
    fn refresh_drag_airwires(&mut self, engine: &Engine) {
        let (Some((placement, _)), Some(position)) = (&self.drag, self.drag_position) else {
            self.drag_airwires = None;
            return;
        };
        if self.cached_revision != Some(engine.revision()) {
            // The copper belongs to another revision; sync cancels the drag.
            self.drag_airwires = None;
            return;
        }
        let key = (engine.revision(), placement.instance, position);
        if self
            .drag_airwires
            .as_ref()
            .is_some_and(|preview| preview.key == key)
        {
            return;
        }
        let delta = [
            position[0] - placement.position[0],
            position[1] - placement.position[1],
        ];
        self.drag_airwires = Some(DragAirwires {
            key,
            connections: self
                .copper
                .moved(placement.instance, delta)
                .airwires(&engine.project().tracks),
        });
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
        self.discard_staged();
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
            let placed = project.placements.iter().any(|p| p.instance == instance.id);
            let kind = if placed { "Placement" } else { "Not placed" };
            return Header::new("component", instance.designator.clone(), kind);
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
        let operation = if let Some(placement) = project
            .placements
            .iter()
            .find(|p| Some(p.instance) == self.instance)
        {
            self.placement_section(ui, placement)
        } else if let Some(instance) = self.instance.and_then(|id| project.instance(id)) {
            self.unplaced_section(ui, engine, &instance.designator);
            None
        } else if let Some(track) = project.tracks.iter().find(|t| Some(t.id) == self.track) {
            track_section(ui, project, track)
        } else {
            board_section(ui, project)
        };
        self.routing_section(ui, project);
        // PCB 3D shows its layer toggles on the navigation bar instead.
        if !self.view_3d {
            self.layers_section(ui);
        }
        self.checks_section(ui);
        operation
    }

    /// A selected instance without a placement: the header reads "Not
    /// placed", and the body offers the update that places it.
    fn unplaced_section(&mut self, ui: &mut Ui, engine: &Engine, designator: &str) {
        let mut update = false;
        kit::section(ui, "pcb-unplaced", "Placement", true, None, |ui| {
            kit::note(
                ui,
                &format!("{designator} is not on the board yet. Run Update PCB to place it."),
            );
            kit::actions(ui, |ui| {
                update = ui_kit::button(ui, "Update PCB…", Button::Normal)
                    .on_hover_text("Review the schematic changes the board does not have yet")
                    .clicked();
            });
        });
        if update {
            self.update.begin(engine, Change::SyncPcb {});
            self.tool = Tool::Select;
            self.route_start = None;
            self.cancel_drag();
        }
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

    /// Layer visibility and the board side Layout is seen from. Display
    /// state only: nothing here changes the document.
    fn layers_section(&mut self, ui: &mut Ui) {
        kit::section(ui, "pcb-layers", "Layers", true, None, |ui| {
            let l = &mut self.layers;
            kit::check_row(ui, "Top copper", &mut l.copper_top);
            kit::check_row(ui, "Bottom copper", &mut l.copper_bottom);
            kit::check_row(ui, "Top silkscreen", &mut l.silk_top);
            kit::check_row(ui, "Bottom silkscreen", &mut l.silk_bottom);
            kit::check_row(ui, "Courtyard", &mut l.courtyard);
            kit::check_row(ui, "Assembly", &mut l.assembly);
            kit::check_row(ui, "Holes", &mut l.holes);
            kit::check_row(ui, "View from bottom", &mut self.from_bottom);
            kit::note(ui, "Hidden layers are not picked.");
        });
    }

    /// The two parts of each courtyard-overlap finding, with the side.
    fn courtyard_overlaps(&self) -> Vec<(Id, Id)> {
        self.issues
            .iter()
            .filter(|f| f.code == layout::courtyard::COURTYARD_OVERLAP)
            .filter_map(|f| {
                let mut parts = f.objects.iter().filter_map(|o| match o {
                    pcbcraft_core::diagnostics::ObjectRef::Instance { id } => Some(*id),
                    _ => None,
                });
                Some((parts.next()?, parts.next()?))
            })
            .collect()
    }

    fn checks_section(&mut self, ui: &mut Ui) {
        let mut select = None;
        self.checks_list(ui, &mut select);
        if let Some((instance, track, net)) = select {
            self.set_selection(instance, track, net);
        }
    }

    /// The Board checks list. Each row naming a part, pad, track or net is
    /// a button that selects the first such object.
    fn checks_list(&self, ui: &mut Ui, select: &mut Option<Selection>) {
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
                    let target = locate(&issue.objects);
                    let hint = target.map(|(instance, track, _)| match (instance, track) {
                        (Some(_), _) => "Select the part on the board",
                        (None, Some(_)) => "Select the track on the board",
                        (None, None) => "Select the net on the board",
                    });
                    let response = kit::finding_row(ui, &issue.message, Tone::Error, hint);
                    if target.is_some() && response.clicked() {
                        *select = target;
                    }
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
            // Scroll, or a pinch (and Ctrl+scroll), zooms about the hovered
            // board point: it stays under the cursor.
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let zoom = (self.zoom * (scroll * 0.002).exp() * pinch)
                .clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
            if zoom != self.zoom
                && let Some(p) = response.hover_pos()
            {
                self.pan = zoom_pan(rect, project.board_size, self.zoom, zoom, self.pan, p);
            }
            self.zoom = zoom;
        }
        let view = board_view(
            rect,
            project.board_size,
            self.zoom,
            self.pan,
            self.from_bottom,
        );
        let scale = view.view.scale;
        let screen = |p: Point| view.screen(p);
        let world = |p: Pos2| view.world(p);
        let painter = ui.painter().with_clip_rect(rect);
        let metric = symbol_view::metric(&painter);
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.hover_pos()));
        let cursor = pointer.map(world);
        // Strokes and text pick within a few pixels.
        let tolerance = f64::from(4.0 / scale);
        let mut operation = None;
        if response.hovered() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.route_start = None;
            self.cancel_drag();
            self.tool = Tool::Select;
            self.discard_staged();
        }
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.drag_started_by(PointerButton::Primary)
        {
            let start = ui.input(|i| i.pointer.press_origin()).or(pointer);
            if let Some(start) = start {
                let at = world(start);
                if let Some(id) = self.pick_instance(project, at, tolerance, metric) {
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
            if self.tool == Tool::Route {
                let snapped = self.route_snap(at, scale).map(|p| (p.center(), p.net));
                let destination = snapped.map(|(c, _)| c).unwrap_or_else(|| snap(at));
                if let Some(start) = self.route_start {
                    if let Some(net) = self.net {
                        operation = Some(Operation::Layout(LayoutCommand::Route {
                            request: RouteRequest {
                                net,
                                start,
                                end: destination,
                                width: self.width,
                                layer: self.layer.into(),
                            },
                        }));
                        // End the speculative gesture even if the engine rejects
                        // it. A later route never starts from uncommitted copper.
                        self.route_start = None;
                    }
                } else {
                    if let Some((_, Some(net))) = snapped {
                        self.net = Some(net);
                        self.highlighted = self.net;
                    }
                    if self.net.is_some() {
                        self.route_start = Some(destination);
                    }
                }
            } else {
                self.select_at(project, at, tolerance, metric);
            }
        }
        self.refresh_drag_airwires(engine);
        self.paint_board(&painter, view, project);
        if let (Some(start), Some(at)) = (self.route_start, cursor) {
            let end = self
                .route_snap(at, scale)
                .map(PlacedPad::center)
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
                "{} · {:?} copper · grid 0.5 mm · {} airwires{}",
                if self.tool == Tool::Route {
                    "Route"
                } else {
                    "Select / move"
                },
                self.layer,
                self.visible_airwires().len(),
                if self.from_bottom {
                    " · viewed from bottom"
                } else {
                    ""
                }
            ),
            FontId::proportional(t.font_small),
            t.text_dim,
        );
        operation
    }

    /// The pad a route click at `at` snaps to: a visible pad with a net and
    /// copper on the routing layer whose exact outline is under the pointer
    /// or within 8 pixels of it. The nearest outline wins; among pads the
    /// pointer is inside, the nearest centre.
    fn route_snap(&self, at: Point, scale: f32) -> Option<&PlacedPad> {
        let reach = f64::from(8.0 / scale);
        self.board_pads
            .iter()
            .filter(|b| {
                let p = &b.placed;
                p.net.is_some() && p.on_copper(self.layer) && self.layers.pad(p)
            })
            .map(|b| {
                (
                    b,
                    b.outline.distance(at, true),
                    pcbcraft_core::geometry::distance(b.placed.center(), at),
                )
            })
            .filter(|(_, edge, _)| *edge < reach)
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
            .map(|(b, _, _)| &b.placed)
    }

    /// The topmost (last drawn) visible pad under `at`, against the cached
    /// board outlines.
    fn pick_pad(&self, at: Point, visible: impl Fn(&PlacedPad) -> bool) -> Option<&PlacedPad> {
        self.board_pads
            .iter()
            .rev()
            .find(|b| visible(&b.placed) && b.contains(at))
            .map(|b| &b.placed)
    }

    /// Select what a click at `at` hits, topmost first among visible
    /// layers: a pad (and its net), a track, then a part by its graphics or
    /// outline; empty board clears the selection.
    fn select_at(&mut self, project: &Project, at: Point, tolerance: f64, metric: TextMetric) {
        let layers = self.layers;
        if let Some(pad) = self.pick_pad(at, |p| layers.pad(p)) {
            let (instance, net) = (pad.instance, pad.net);
            self.set_selection(Some(instance), None, net);
        } else if let Some(track) = project.tracks.iter().rev().find(|track| {
            track.layer.side().is_some_and(|side| layers.copper(side))
                && layout::point_in_polygon(at, &layout::track_polygon(track))
        }) {
            self.set_selection(None, Some(track.id), Some(track.net));
        } else {
            let instance = self.pick_instance(project, at, tolerance, metric);
            self.set_selection(instance, None, None);
        }
    }

    /// The part under `at`: its visible pads, then its visible graphics,
    /// then its package envelope.
    fn pick_instance(
        &self,
        project: &Project,
        at: Point,
        tolerance: f64,
        metric: TextMetric,
    ) -> Option<Id> {
        let layers = self.layers;
        self.pick_pad(at, |p| layers.pad(p))
            .map(|p| p.instance)
            .or_else(|| {
                layout::pick_graphic(&self.graphics, at, tolerance, metric, |g| {
                    layers.drawing(g.layer)
                })
                .map(|g| g.instance)
            })
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

    /// How far the dragged placement's preview has moved `instance`.
    fn drag_delta(&self, instance: Id) -> Point {
        match (&self.drag, self.drag_position) {
            (Some((placement, _)), Some(position)) if placement.instance == instance => [
                position[0] - placement.position[0],
                position[1] - placement.position[1],
            ],
            _ => [0.0, 0.0],
        }
    }

    /// Paint the board as seen from the chosen side, far side first: its
    /// drawings, then its copper, through-hole copper, the near copper,
    /// holes, then the near drawings, airwires, outlines and labels.
    fn paint_board(&self, painter: &egui::Painter, view: BoardView, project: &Project) {
        let t = Tokens::get();
        let ink = BoardInk::get();
        let scale = view.view.scale;
        let screen = |p: Point| view.screen(p);
        let near = if self.from_bottom {
            Layer::Bottom
        } else {
            Layer::Top
        };
        let far = opposite(near);
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
        // Far side dimmed underneath; then near assembly, copper, holes,
        // silkscreen and courtyard (graphics spec §6 order).
        self.paint_drawings(painter, view, &ink, far, true, |_| true);
        self.paint_copper(painter, view, project, Some(far), true);
        let assembly = |l: BoardDrawing| matches!(l, BoardDrawing::Assembly(_));
        self.paint_drawings(painter, view, &ink, near, false, assembly);
        self.paint_copper(painter, view, project, None, false);
        self.paint_copper(painter, view, project, Some(near), false);
        if self.layers.holes {
            for pad in &self.board_pads {
                let delta = self.drag_delta(pad.placed.instance);
                for hole in &pad.holes {
                    // Always outlined, so holes stay visible with every
                    // copper layer hidden.
                    let edge = if hole.plated {
                        ink.plated
                    } else {
                        ink.unplated
                    };
                    symbol_view::paint_shape(
                        painter,
                        view.view,
                        &view.shape(&hole.shape, delta),
                        Some(ink.drill),
                        Some(Stroke::new(1.0, edge)),
                    );
                }
            }
        }
        let silk = |l: BoardDrawing| matches!(l, BoardDrawing::Silkscreen(_));
        let courtyard = |l: BoardDrawing| matches!(l, BoardDrawing::Courtyard(_));
        self.paint_drawings(painter, view, &ink, near, false, silk);
        self.paint_drawings(painter, view, &ink, near, false, courtyard);
        self.paint_courtyard_overlaps(painter, view, near);
        if self.show_airwires {
            for (a, b) in self.visible_airwires() {
                painter.line_segment(
                    [screen(a.position), screen(b.position)],
                    Stroke::new(1.0, t.text_dim.gamma_multiply(0.65)),
                );
            }
        }
        for placement in &project.placements {
            let selected = self.instance == Some(placement.instance);
            let mut shown = placement.clone();
            let delta = self.drag_delta(placement.instance);
            shown.position = [shown.position[0] + delta[0], shown.position[1] + delta[1]];
            let outlined = self.outlined.contains(&placement.instance);
            let labelled = self.labelled.contains(&placement.instance);
            if outlined && labelled && !selected {
                continue;
            }
            let Some(polygon) = body_polygon(project, &shown) else {
                continue;
            };
            let color = if selected {
                t.body_selected
            } else {
                t.text_dim
            };
            if !outlined || selected {
                let vertices = polygon.map(screen);
                for i in 0..4 {
                    painter.line_segment(
                        [vertices[i], vertices[(i + 1) % 4]],
                        Stroke::new(1.0, color),
                    );
                }
            }
            if !labelled {
                let reference = project
                    .instance(shown.instance)
                    .map(|i| i.designator.as_str())
                    .unwrap_or("?");
                let top = polygon
                    .iter()
                    .map(|p| screen(*p))
                    .fold(f32::INFINITY, |y, p| y.min(p.y));
                painter.text(
                    egui::pos2(screen(shown.position).x, top - 2.0),
                    Align2::CENTER_BOTTOM,
                    reference,
                    fonts::semibold(t.font_label),
                    color,
                );
            }
        }
        if scale > 7.0 {
            for pad in &self.board_pads {
                if pad.placed.pad.number.is_empty() || !self.layers.pad(&pad.placed) {
                    continue;
                }
                let delta = self.drag_delta(pad.placed.instance);
                let c = pad.placed.center();
                painter.text(
                    screen([c[0] + delta[0], c[1] + delta[1]]),
                    Align2::CENTER_CENTER,
                    &pad.placed.pad.number,
                    FontId::proportional(t.font_small),
                    t.on_accent,
                );
            }
        }
    }

    /// Copper of one side (tracks, then surface pads), or with `None` the
    /// through-hole pads that span both.
    fn paint_copper(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        side: Option<Layer>,
        dim: bool,
    ) {
        let shade = |c: Color32| if dim { c.gamma_multiply(0.45) } else { c };
        if let Some(layer) = side {
            if !self.layers.copper(layer) {
                return;
            }
            for track in project.tracks.iter().filter(|t| t.layer == layer) {
                let selected = self.track == Some(track.id) || self.highlighted == Some(track.net);
                painter.add(Shape::convex_polygon(
                    layout::track_polygon(track)
                        .map(|p| view.screen(p))
                        .to_vec(),
                    shade(copper_color(layer, selected)),
                    Stroke::NONE,
                ));
            }
        }
        let ink = BoardInk::get();
        for pad in &self.board_pads {
            let placed = &pad.placed;
            let through = placed.pad.kind.through_hole();
            match side {
                Some(layer) if through || placed.layer != layer => continue,
                None if !through => continue,
                _ => {}
            }
            if !self.layers.pad(placed) {
                continue;
            }
            let selected = self.instance == Some(placed.instance)
                || (placed.net.is_some() && self.highlighted == placed.net);
            let shape = view.shape(&pad.outline, self.drag_delta(placed.instance));
            if placed.pad.kind == PadKind::NonPlatedHole {
                // A bare hole has no copper: outline its keep-out land.
                let color = if selected {
                    copper_color(Layer::Top, true)
                } else {
                    ink.unplated
                };
                symbol_view::paint_shape(
                    painter,
                    view.view,
                    &shape,
                    None,
                    Some(Stroke::new(1.0, color)),
                );
                continue;
            }
            let layer = match side {
                Some(layer) => layer,
                None if self.from_bottom => Layer::Bottom,
                None => Layer::Top,
            };
            symbol_view::paint_shape(
                painter,
                view.view,
                &shape,
                Some(shade(copper_color(layer, selected))),
                None,
            );
        }
    }

    /// The courtyards of parts a `board.courtyard_overlap` finding names,
    /// tinted on the near side so the overlap reads darker where they
    /// cross.
    fn paint_courtyard_overlaps(&self, painter: &egui::Painter, view: BoardView, side: Layer) {
        let pairs = self.courtyard_overlaps();
        if pairs.is_empty() || !self.layers.courtyard {
            return;
        }
        let error = Tokens::get().error;
        for graphic in &self.graphics {
            let BoardDrawing::Courtyard(layer) = graphic.layer else {
                continue;
            };
            if layer != side
                || !pairs
                    .iter()
                    .any(|(a, b)| *a == graphic.instance || *b == graphic.instance)
            {
                continue;
            }
            if let PlacedItem::Shape { shape, .. } = &graphic.item
                && shape.is_closed()
            {
                let delta = self.drag_delta(graphic.instance);
                symbol_view::paint_shape(
                    painter,
                    view.view,
                    &view.shape(shape, delta),
                    Some(error.gamma_multiply(0.22)),
                    Some(Stroke::new(1.5, error)),
                );
            }
        }
    }

    /// Silkscreen, assembly and courtyard drawings on one side; the far
    /// side is dimmed.
    fn paint_drawings(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        ink: &BoardInk,
        side: Layer,
        dim: bool,
        layers: impl Fn(BoardDrawing) -> bool,
    ) {
        let selected_ink = Tokens::get().body_selected;
        for graphic in &self.graphics {
            if drawing_side(graphic.layer) != side
                || !layers(graphic.layer)
                || !self.layers.drawing(graphic.layer)
            {
                continue;
            }
            let mut color = if self.instance == Some(graphic.instance) {
                selected_ink
            } else {
                match graphic.layer {
                    BoardDrawing::Silkscreen(_) => ink.silkscreen,
                    BoardDrawing::Courtyard(_) => ink.courtyard,
                    BoardDrawing::Assembly(_) => ink.assembly,
                }
            };
            if dim {
                color = color.gamma_multiply(0.45);
            }
            let delta = self.drag_delta(graphic.instance);
            match &graphic.item {
                PlacedItem::Shape {
                    shape,
                    width,
                    filled,
                } => {
                    let stroke = Stroke::new((*width as f32 * view.view.scale).max(1.0), color);
                    if matches!(graphic.layer, BoardDrawing::Courtyard(_)) && !*filled {
                        symbol_view::paint_dashed(
                            painter,
                            view.view,
                            &view.shape(shape, delta),
                            stroke,
                            6.0,
                            4.0,
                        );
                        continue;
                    }
                    symbol_view::paint_shape(
                        painter,
                        view.view,
                        &view.shape(shape, delta),
                        filled.then_some(color),
                        Some(stroke),
                    );
                }
                PlacedItem::Text(text) => {
                    symbol_view::paint_text(painter, view.view, &view.text(text, delta), color);
                }
            }
        }
    }
}

/// Theme colours of the non-copper board layers in Layout.
struct BoardInk {
    silkscreen: Color32,
    courtyard: Color32,
    assembly: Color32,
    /// Drilled holes: the canvas seen through the board.
    drill: Color32,
    unplated: Color32,
    /// A plated barrel's edge.
    plated: Color32,
}

impl BoardInk {
    fn get() -> Self {
        let t = Tokens::get();
        let dark = theme::is_dark();
        Self {
            silkscreen: if dark {
                Color32::from_rgb(232, 232, 224)
            } else {
                Color32::from_rgb(70, 70, 76)
            },
            courtyard: if dark {
                Color32::from_rgb(206, 120, 214)
            } else {
                Color32::from_rgb(150, 50, 160)
            },
            assembly: t.text_dim,
            drill: t.viewport_bottom,
            unplated: t.text,
            plated: t.text_dim,
        }
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
/// Board size and layers, and the fabrication choices. Choosing another
/// option issues `layout.set_fabrication` with the other attributes kept.
fn board_section(ui: &mut Ui, project: &Project) -> Option<Operation> {
    let mut fabrication = project.fabrication;
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
        kit::choice_row(
            ui,
            "Soldermask",
            &mut fabrication.mask_colour,
            &mask_colour_options(),
        );
        kit::choice_row(
            ui,
            "Mask finish",
            &mut fabrication.mask_finish,
            &mask_finish_options(),
        );
        kit::choice_row(
            ui,
            "Pad finish",
            &mut fabrication.pad_finish,
            &pad_finish_options(),
        );
        kit::choice_row(
            ui,
            "Silkscreen",
            &mut fabrication.silkscreen_colour,
            &silkscreen_options(),
        );
        kit::note(
            ui,
            "Saved with the design for fabrication. The 3D view shows mask and pad finish; silkscreen is drawn in 2D Layout only for now.",
        );
    });
    fabrication_edit(project.fabrication, fabrication)
}
/// The operation a changed fabrication form issues; `None` when unchanged.
fn fabrication_edit(current: Fabrication, edited: Fabrication) -> Option<Operation> {
    (current != edited).then_some(Operation::Layout(LayoutCommand::SetFabrication(edited)))
}
fn mask_colour_options() -> [(MaskColour, String); 8] {
    [
        (MaskColour::Green, "Green"),
        (MaskColour::Blue, "Blue"),
        (MaskColour::Red, "Red"),
        (MaskColour::Purple, "Purple"),
        (MaskColour::Yellow, "Yellow"),
        (MaskColour::Black, "Black"),
        (MaskColour::White, "White"),
        (MaskColour::Clear, "Clear"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
fn mask_finish_options() -> [(MaskFinish, String); 3] {
    [
        (MaskFinish::Gloss, "Gloss"),
        (MaskFinish::Satin, "Satin"),
        (MaskFinish::Matte, "Matte"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
fn pad_finish_options() -> [(PadFinish, String); 5] {
    [
        (PadFinish::Enig, "ENIG"),
        (PadFinish::Hasl, "HASL (tin-lead)"),
        (PadFinish::LeadFreeHasl, "Lead-free HASL"),
        (PadFinish::Osp, "OSP"),
        (PadFinish::ImmersionSilver, "Immersion silver"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
}
fn silkscreen_options() -> [(SilkscreenColour, String); 3] {
    [
        (SilkscreenColour::White, "White"),
        (SilkscreenColour::Black, "Black"),
        (SilkscreenColour::Yellow, "Yellow"),
    ]
    .map(|(value, label)| (value, label.to_owned()))
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
        kit::value_row(ui, "Layer", &track.layer.name());
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
/// Board millimetres to the canvas `rect`: the board fitted with a 50 px
/// margin and centred, times `zoom`, moved by `pan`.
fn board_view(rect: Rect, board: Point, zoom: f32, pan: Vec2, from_bottom: bool) -> BoardView {
    let scale = fit_scale(rect, board) * zoom;
    let centring = egui::vec2(-board[0] as f32, board[1] as f32) * 0.5;
    let origin = rect.center() + pan + centring * scale;
    BoardView::new(View { origin, scale }, board[0], from_bottom)
}

fn fit_scale(rect: Rect, board: Point) -> f32 {
    ((rect.width() - 100.0).max(20.0) / board[0] as f32)
        .min((rect.height() - 100.0).max(20.0) / board[1] as f32)
}

/// The pan that keeps the board point under `pointer` in place while the
/// zoom changes from `before` to `after`. The bottom view's flip is fixed
/// in board space, so it needs no special case.
fn zoom_pan(rect: Rect, board: Point, before: f32, after: f32, pan: Vec2, pointer: Pos2) -> Vec2 {
    let fit = fit_scale(rect, board);
    let centring = egui::vec2(-board[0] as f32, board[1] as f32) * 0.5;
    let (from, to) = (fit * before, fit * after);
    let origin = rect.center() + pan + centring * from;
    let moved = pointer - (pointer - origin) * (to / from);
    moved - rect.center() - centring * to
}

fn snap(p: Point) -> Point {
    p.map(|v| (v * 2.0).round() / 2.0)
}
/// The package envelope of a placement on the board: the footprint's
/// [`envelope_outline`](pcbcraft_core::library::FootprintDefinition::envelope_outline)
/// through the placement transform, the frame the Component editor draws.
/// An (instance, track, net) selection, as `PcbEditor::set_selection` takes.
type Selection = (Option<Id>, Option<Id>, Option<Id>);

/// The object a finding row locates: the first part (directly, or through
/// a pad or terminal) it names, else its first track, else its first net.
fn locate(objects: &[pcbcraft_core::diagnostics::ObjectRef]) -> Option<Selection> {
    use pcbcraft_core::diagnostics::ObjectRef;
    let instance = objects.iter().find_map(|o| match o {
        ObjectRef::Instance { id }
        | ObjectRef::Pad { instance: id, .. }
        | ObjectRef::Unit { instance: id, .. }
        | ObjectRef::Property { instance: id, .. } => Some(*id),
        ObjectRef::Terminal { endpoint } => Some(endpoint.instance),
        _ => None,
    });
    let track = || {
        objects.iter().find_map(|o| match o {
            ObjectRef::Track { id } => Some(*id),
            _ => None,
        })
    };
    let net = || {
        objects.iter().find_map(|o| match o {
            ObjectRef::Net { id } => Some(*id),
            _ => None,
        })
    };
    match (instance, track()) {
        (Some(id), _) => Some((Some(id), None, None)),
        (None, Some(track)) => Some((None, Some(track), net())),
        (None, None) => net().map(|net| (None, None, Some(net))),
    }
}

fn body_polygon(project: &Project, placement: &Placement) -> Option<[Point; 4]> {
    let instance = project.instance(placement.instance)?;
    let physical = instances::resolve(&project.library, instance)
        .ok()?
        .physical?;
    let footprint = &physical.footprint.definition;
    let envelope = footprint.envelope_outline(&physical.package.definition);
    let t = layout::placement_transform(placement, &footprint.datum);
    Some(envelope.corners().map(|p| t.apply(p)))
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
                    layer: Layer::Top.into(),
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
        // R2's pad 1 sits 0.8 mm left of its origin.
        assert!((moved.position[0] - 19.2).abs() < 1e-9);
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

    /// Choosing a mask colour in the Board section issues
    /// `layout.set_fabrication` with the other attributes kept; the engine
    /// applies it as one undo step and the rebuilt 3D layers take the look.
    #[test]
    fn board_soldermask_row_issues_set_fabrication() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        assert!(fabrication_edit(Fabrication::default(), Fabrication::default()).is_none());
        // Open the Soldermask dropdown, then choose Blue.
        assert!(click(&mut editor, &engine, &ctx, "Green")?.is_none());
        let operation = click(&mut editor, &engine, &ctx, "Blue")?.ok_or("Blue did nothing")?;
        let expected = Fabrication {
            mask_colour: MaskColour::Blue,
            ..Fabrication::default()
        };
        let Operation::Layout(LayoutCommand::SetFabrication(issued)) = &operation else {
            return Err("the row issued another command".into());
        };
        assert_eq!(*issued, expected);
        engine.apply(engine.revision(), operation)?;
        assert_eq!(engine.project().fabrication, expected);
        let model = pcbcraft_solidcraft::project::build(engine.project())?;
        let blue = pcbcraft_solidcraft::material::material(
            pcbcraft_solidcraft::material::BodyKind::MaskTop,
            &expected,
        )
        .surface()?;
        assert!(model.geometry.iter().any(|g| g.kind
            == pcbcraft_solidcraft::material::BodyKind::MaskTop
            && g.surface == blue));
        // The form follows the document: the row now shows Blue, and undo
        // restores Green.
        let (output, none) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(none.is_none());
        assert!(
            output
                .shapes
                .iter()
                .any(|s| text_position(&s.shape, "Blue").is_some())
        );
        engine.undo()?;
        assert_eq!(engine.project().fabrication, Fabrication::default());
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

    /// The demo plus R3, placed on the schematic but not yet on the board.
    fn with_unplaced() -> Result<(Engine, Id), Box<dyn std::error::Error>> {
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
        let r3 = engine
            .project()
            .instance_by_designator("R3")
            .map(|i| i.id)
            .ok_or("R3 missing")?;
        Ok((engine, r3))
    }

    /// B-a: an unplaced instance gets its own header and body, which agree,
    /// and the body's one action opens the Update PCB review.
    #[test]
    fn an_unplaced_instance_reads_not_placed_and_offers_update_pcb()
    -> Result<(), Box<dyn std::error::Error>> {
        let (engine, r3) = with_unplaced()?;
        assert!(!engine.project().placements.iter().any(|p| p.instance == r3));
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(r3), None, None);
        let header = editor.header(&engine);
        assert_eq!((header.title.as_str(), header.kind), ("R3", "Not placed"));
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let has = |label: &str| {
            output
                .shapes
                .iter()
                .any(|s| text_position(&s.shape, label).is_some())
        };
        assert!(has(
            "R3 is not on the board yet. Run Update PCB to place it."
        ));
        // Not the Board body under a placement header.
        assert!(!has("Soldermask"));
        assert!(click(&mut editor, &engine, &ctx, "Update PCB…")?.is_none());
        assert!(editor.update.is_open());
        assert_eq!(editor.header(&engine).title, "Update PCB");
        // A placed instance still reads as a placement.
        let (r1, _) = ids(&engine);
        let mut placed = PcbEditor::default();
        placed.set_selection(Some(r1), None, None);
        assert_eq!(placed.header(&engine).kind, "Placement");
        Ok(())
    }

    /// B-b: a Board checks row is a button that selects the part it names.
    #[test]
    fn a_board_check_row_click_selects_the_named_part() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        let (message, expected) = editor
            .issues
            .iter()
            .find_map(|f| Some((f.message.clone(), locate(&f.objects)?)))
            .ok_or("the demo board has no locatable finding")?;
        assert!(click(&mut editor, &engine, &ctx, &message)?.is_none());
        assert_eq!(
            (
                editor.selected_instance(),
                editor.selected_track(),
                editor.net
            ),
            expected
        );
        Ok(())
    }

    #[test]
    fn finding_rows_locate_pads_tracks_and_nets() {
        use pcbcraft_core::diagnostics::ObjectRef;
        let pad = ObjectRef::Pad {
            instance: 7,
            pad: pcbcraft_core::PadId(1),
        };
        assert_eq!(
            locate(&[ObjectRef::Net { id: 3 }, pad]),
            Some((Some(7), None, None))
        );
        assert_eq!(
            locate(&[ObjectRef::Net { id: 3 }, ObjectRef::Track { id: 5 }]),
            Some((None, Some(5), Some(3)))
        );
        assert_eq!(
            locate(&[ObjectRef::Net { id: 3 }]),
            Some((None, None, Some(3)))
        );
        assert_eq!(locate(&[]), None);
    }

    /// B-c: unapplied Position, Rotation and Side edits are dropped when the
    /// tool or the selected subject changes; the form shows the document.
    #[test]
    fn staged_placement_edits_are_discarded_on_tool_or_subject_change()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let (r1, r2) = ids(&engine);
        let placement = |id| {
            engine
                .project()
                .placements
                .iter()
                .find(|p| p.instance == id)
                .cloned()
                .ok_or("fixture placement missing")
        };
        let before = placement(r1)?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let stage = |editor: &mut PcbEditor| {
            editor.position = [12.0, 9.5];
            editor.rotation = 90.0;
            editor.placement_layer = Layer::Bottom;
        };
        let shown = |editor: &PcbEditor| (editor.position, editor.rotation, editor.placement_layer);
        let document = (before.position, before.rotation_deg, before.layer);
        editor.set_selection(Some(r1), None, None);
        frame(&mut editor, &engine, &ctx, vec![]);
        // Staged edits survive plain redraws until applied.
        stage(&mut editor);
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(shown(&editor), ([12.0, 9.5], 90.0, Layer::Bottom));
        for tool in ["route", "select"] {
            stage(&mut editor);
            editor.on_tool(tool, &engine);
            frame(&mut editor, &engine, &ctx, vec![]);
            assert_eq!(shown(&editor), document, "after the {tool} tool");
        }
        // Another part and back, and a re-selection of the same part.
        stage(&mut editor);
        editor.set_selection(Some(r2), None, None);
        frame(&mut editor, &engine, &ctx, vec![]);
        let other = placement(r2)?;
        assert_eq!(
            shown(&editor),
            (other.position, other.rotation_deg, other.layer)
        );
        editor.set_selection(Some(r1), None, None);
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(shown(&editor), document);
        stage(&mut editor);
        editor.set_selection(Some(r1), None, None);
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(shown(&editor), document);
        Ok(())
    }

    /// The demo with every footprint replaced by a plated through-hole
    /// oblong (slot and via) and a turned custom SMD land, plus silkscreen,
    /// courtyard, assembly and designator graphics; R1 at (20, 20) on the
    /// top and R2 far away at (45, 20) on the bottom.
    fn shaped_board() -> Result<Engine, Box<dyn std::error::Error>> {
        use pcbcraft_core::library::{
            DrawingLayer, Fill, FootprintGraphic, Hole, HoleShape, Item, Outline, PadShape, PadVia,
            Segment, Side, designator_placeholder,
        };
        let engine = Engine::demo()?;
        let mut file = engine.to_file();
        for footprint in &mut file.project.library.footprints {
            let f = &mut footprint.definition;
            if f.pads.len() != 2 {
                continue;
            }
            let pth = &mut f.pads[0];
            pth.kind = PadKind::PlatedThrough;
            pth.position = [-1.5, 0.0];
            pth.shape = PadShape::Oblong { size: [1.6, 2.4] };
            pth.hole = Some(Hole {
                shape: HoleShape::Slot {
                    width: 0.6,
                    length: 1.2,
                    angle_deg: 90.0,
                },
                plated: true,
                offset: [0.0, 0.0],
            });
            pth.vias = vec![PadVia {
                id: pcbcraft_core::library::ApertureId(1),
                at: [0.0, 0.9],
                drill: 0.2,
            }];
            let smd = &mut f.pads[1];
            smd.position = [1.5, 0.0];
            smd.rotation_deg = 30.0;
            smd.shape = PadShape::Custom {
                outline: Outline {
                    start: [-0.6, -0.6],
                    segments: vec![
                        Segment::Line { to: [0.6, -0.6] },
                        Segment::Arc {
                            to: [0.6, 0.6],
                            bulge: 0.6,
                        },
                        Segment::Line { to: [-0.6, 0.6] },
                        Segment::Line { to: [-0.6, -0.6] },
                    ],
                },
            };
            let shape = |id, layer, shape| FootprintGraphic {
                id: pcbcraft_core::GraphicId(id),
                layer,
                side: Side::Component,
                item: Item::Shape {
                    shape,
                    width: 0.15,
                    fill: Fill::None,
                },
            };
            f.graphics = vec![
                shape(
                    10,
                    DrawingLayer::Silkscreen,
                    CoreShape::Line {
                        a: [-2.0, 2.0],
                        b: [2.0, 2.0],
                    },
                ),
                shape(
                    11,
                    DrawingLayer::Courtyard,
                    CoreShape::rect([-3.0, -2.5], [3.0, 2.5]),
                ),
                shape(
                    12,
                    DrawingLayer::Assembly,
                    CoreShape::rect([-2.5, -1.5], [2.5, 1.5]),
                ),
                designator_placeholder(
                    pcbcraft_core::GraphicId(13),
                    [0.0, 3.2],
                    DrawingLayer::Silkscreen,
                ),
            ];
            f.next_local = f.next_local.max(14);
        }
        let mut engine = Engine::from_file(file)?;
        let (r1, r2) = ids(&engine);
        for (instance, position, layer) in [
            (r1, [20.0, 20.0], Layer::Top),
            (r2, [45.0, 20.0], Layer::Bottom),
        ] {
            engine.apply(
                engine.revision(),
                Operation::Layout(LayoutCommand::Move {
                    instance,
                    position,
                    rotation_deg: 0.0,
                    layer,
                }),
            )?;
        }
        Ok(engine)
    }

    /// One Layout frame of the canvas at 900 × 700 pixels.
    fn canvas_frame(
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
                    egui::vec2(900.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                command = editor.canvas(ui, engine);
            },
        );
        output.textures_delta.clear();
        (output, command)
    }

    /// Fill and stroke colours and text painted in one frame.
    #[derive(Default)]
    struct Painted {
        colors: Vec<Color32>,
        texts: Vec<String>,
    }

    fn collect(shape: &Shape, out: &mut Painted) {
        let stroke_color = |c: &egui::epaint::ColorMode| match c {
            egui::epaint::ColorMode::Solid(c) => Some(*c),
            _ => None,
        };
        match shape {
            Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
            Shape::Path(path) => {
                out.colors.push(path.fill);
                out.colors.extend(stroke_color(&path.stroke.color));
            }
            Shape::Mesh(mesh) => out.colors.extend(mesh.vertices.iter().map(|v| v.color)),
            Shape::Circle(circle) => out.colors.push(circle.fill),
            Shape::LineSegment { stroke, .. } => out.colors.push(stroke.color),
            Shape::Text(text) => out.texts.push(text.galley.job.text.clone()),
            _ => {}
        }
    }

    fn painted(output: &egui::FullOutput) -> Painted {
        let mut out = Painted::default();
        for shape in &output.shapes {
            collect(&shape.shape, &mut out);
        }
        out
    }

    /// Layout draws each pad shape, its holes and every graphic layer from
    /// the core placement queries, and the layer toggles hide them without
    /// rebuilding the cached board.
    #[test]
    fn layout_draws_pad_shapes_holes_and_graphic_layers_with_toggles()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = shaped_board()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let ink = BoardInk::get();
        let board = crate::palette::board();
        let all = painted(&output);
        for (what, color) in [
            ("top copper", board.copper_top),
            // Seen from the top, bottom copper is the dimmed far side.
            ("bottom copper", board.copper_bottom.gamma_multiply(0.45)),
            ("drill", ink.drill),
            ("silkscreen", ink.silkscreen),
            ("courtyard", ink.courtyard),
        ] {
            assert!(all.colors.contains(&color), "{what} not painted");
        }
        // Courtyard and silkscreen are drawn above the copper (spec order).
        let last = |c: Color32| all.colors.iter().rposition(|x| *x == c);
        let first = |c: Color32| all.colors.iter().position(|x| *x == c);
        assert!(first(ink.courtyard) > last(board.copper_top));
        assert!(first(ink.silkscreen) > last(board.copper_top));
        // R2 is on the bottom: its silkscreen is the dimmed far side.
        assert!(all.colors.contains(&ink.silkscreen.gamma_multiply(0.45)));
        // The designator graphics label the parts; no extra label is drawn.
        assert_eq!(all.texts.iter().filter(|t| *t == "R1").count(), 1);
        assert_eq!(all.texts.iter().filter(|t| *t == "R2").count(), 1);
        // Every pad, both placements: two holes and a via each for the
        // through-hole pad, the custom land flattened with its arc.
        assert_eq!(editor.board_pads.len(), 4);
        assert!(
            editor
                .board_pads
                .iter()
                .all(|p| { p.placed.pad.kind != PadKind::PlatedThrough || p.holes.len() == 2 })
        );
        let custom = editor
            .board_pads
            .iter()
            .find(|p| p.placed.pad.kind == PadKind::Smd)
            .ok_or("custom pad missing")?;
        assert!(custom.outline.flatten(0.01).len() > 5);

        let cached = editor.board_pads.as_ptr();
        editor.layers.silk_top = false;
        editor.layers.silk_bottom = false;
        editor.layers.courtyard = false;
        editor.layers.holes = false;
        editor.layers.copper_bottom = false;
        editor.set_selection(Some(ids(&engine).0), None, None);
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let hidden = painted(&output);
        for (what, color) in [
            ("bottom copper", board.copper_bottom),
            (
                "dimmed bottom copper",
                board.copper_bottom.gamma_multiply(0.45),
            ),
            ("drill", ink.drill),
            ("silkscreen", ink.silkscreen),
            ("courtyard", ink.courtyard),
        ] {
            assert!(!hidden.colors.contains(&color), "hidden {what} painted");
        }
        // Hidden silkscreen hides the designator; no fallback label
        // replaces it.
        assert_eq!(hidden.texts.iter().filter(|t| *t == "R1").count(), 0);
        // Layers and selection are display state: the board is not rebuilt.
        assert_eq!(editor.board_pads.as_ptr(), cached);
        Ok(())
    }

    /// Clicks pick through the core queries, respect hidden layers and land
    /// on the same board point when the view is seen from the bottom.
    #[test]
    fn clicks_pick_pads_and_graphics_on_visible_layers_from_either_side()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = shaped_board()?;
        let (r1, r2) = ids(&engine);
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let metric = TextMetric::default();
        let project = engine.project();
        // The silkscreen line of R1 sits outside its pads and envelope.
        editor.select_at(project, [20.5, 22.0], 0.05, metric);
        assert_eq!(editor.selected_instance(), Some(r1));
        editor.set_selection(None, None, None);
        editor.layers.silk_top = false;
        editor.layers.courtyard = false;
        editor.layers.assembly = false;
        editor.select_at(project, [20.5, 22.0], 0.05, metric);
        assert_eq!(editor.selected_instance(), None);
        // The bottom part's surface land is on hidden bottom copper.
        editor.layers.copper_bottom = false;
        let r2_smd = editor
            .board_pads
            .iter()
            .map(|b| &b.placed)
            .find(|p| p.instance == r2 && p.pad.kind == PadKind::Smd)
            .map(PlacedPad::center)
            .ok_or("R2 land")?;
        editor.select_at(project, r2_smd, 0.0, metric);
        assert_ne!(editor.selected_instance(), Some(r2));
        editor.layers = Layers::default();
        editor.select_at(project, r2_smd, 0.0, metric);
        assert_eq!(editor.selected_instance(), Some(r2));
        assert_eq!(
            editor.net,
            editor
                .board_pads
                .iter()
                .map(|b| &b.placed)
                .find(|p| p.instance == r2 && p.pad.kind == PadKind::Smd)
                .and_then(|p| p.net)
        );

        // Seen from the bottom, a click on the screen point of R1's
        // through-hole pad still selects R1.
        editor.set_selection(None, None, None);
        editor.from_bottom = true;
        let pth = editor
            .board_pads
            .iter()
            .map(|b| &b.placed)
            .find(|p| p.instance == r1 && p.pad.kind == PadKind::PlatedThrough)
            .map(PlacedPad::center)
            .ok_or("R1 through-hole pad")?;
        let view = BoardView::new(
            View {
                origin: Pos2::new(100.0, 600.0),
                scale: 10.0,
            },
            project.board_size[0],
            true,
        );
        let at = view.screen(pth);
        // Mirrored in x about the board centre, y unchanged.
        assert!((at.x - (100.0 + (60.0 - pth[0]) as f32 * 10.0)).abs() < 1e-3);
        let back = view.world(at);
        assert!((back[0] - pth[0]).abs() < 1e-4 && (back[1] - pth[1]).abs() < 1e-4);
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let screen_pad = output
            .shapes
            .iter()
            .find_map(|s| text_position(&s.shape, "1"))
            .ok_or("pad number 1 not drawn")?;
        let click = vec![
            egui::Event::PointerMoved(screen_pad),
            egui::Event::PointerButton {
                pos: screen_pad,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: screen_pad,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let (_, none) = canvas_frame(&mut editor, &engine, &ctx, click);
        assert!(none.is_none());
        // R1's pads are drawn (and so numbered) first.
        assert_eq!(editor.selected_instance(), Some(r1));
        assert!((editor.board_pads[0].placed.center()[0] - pth[0]).abs() < 1e-9);
        Ok(())
    }

    /// Routing snaps to a pad's exact outline: just outside the rounded end
    /// of R1's oblong through-hole pad, far from its centre, snaps; inside
    /// its extent's corner but clear of the rounded end, it does not.
    #[test]
    fn route_snap_reaches_the_exact_pad_outline() -> Result<(), Box<dyn std::error::Error>> {
        let engine = shaped_board()?;
        let (r1, _) = ids(&engine);
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        let pth = editor
            .board_pads
            .iter()
            .map(|b| &b.placed)
            .find(|p| p.instance == r1 && p.pad.kind == PadKind::PlatedThrough)
            .ok_or("R1 through-hole pad")?;
        let (c, id) = (pth.center(), pth.pad.id);
        // The oblong is 1.6 × 2.4 mm: its top is 1.2 mm above the centre.
        // At 10 px/mm the 8 px reach is 0.8 mm.
        let snapped = |at: Point, scale: f32| editor.route_snap(at, scale).map(|p| p.pad.id);
        assert_eq!(snapped([c[0], c[1] + 1.25], 10.0), Some(id));
        assert_eq!(snapped([c[0], c[1] + 2.2], 10.0), None);
        // The extent's corner (0.75, 1.15) is 0.26 mm from the rounded end:
        // outside the 0.2 mm reach at 40 px/mm.
        assert_eq!(snapped([c[0] + 0.75, c[1] + 1.15], 40.0), None);
        assert_eq!(snapped([c[0] + 0.75, c[1] + 1.15], 10.0), Some(id));
        assert_eq!(snapped(c, 40.0), Some(id));
        Ok(())
    }

    /// The board checks and airwires the editor caches per revision are
    /// the core's, built once from the same copper.
    #[test]
    fn cached_copper_matches_the_core_checks_and_airwires() -> Result<(), Box<dyn std::error::Error>>
    {
        let engine = shaped_board()?;
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        assert_eq!(editor.airwires, layout::airwires(engine.project()));
        assert_eq!(editor.issues, layout::checks(engine.project()));
        assert_eq!(editor.copper.pads(), layout::pads(engine.project()));
        Ok(())
    }

    /// Zooming keeps the board point under the cursor fixed, from either
    /// side and at any pan.
    #[test]
    fn zoom_pan_keeps_the_board_point_under_the_cursor() {
        let rect = Rect::from_min_size(Pos2::new(40.0, 30.0), egui::vec2(900.0, 700.0));
        let board = [60.0, 40.0];
        for from_bottom in [false, true] {
            for (pan, pointer) in [
                (Vec2::ZERO, Pos2::new(200.0, 500.0)),
                (egui::vec2(-80.0, 35.0), Pos2::new(700.0, 120.0)),
            ] {
                for (before, after) in [(1.0, 1.7), (3.0, 0.4)] {
                    let fixed = board_view(rect, board, before, pan, from_bottom).world(pointer);
                    let pan = zoom_pan(rect, board, before, after, pan, pointer);
                    let view = board_view(rect, board, after, pan, from_bottom);
                    let at = view.world(pointer);
                    assert!(
                        (at[0] - fixed[0]).abs() < 1e-3 && (at[1] - fixed[1]).abs() < 1e-3,
                        "{from_bottom} {before}→{after}: {at:?} vs {fixed:?}"
                    );
                    assert!((view.view.scale / fit_scale(rect, board) - after).abs() < 1e-5);
                }
            }
        }
    }

    /// A pinch over the canvas zooms about the pointer: the pad the
    /// pointer rests on stays under it.
    #[test]
    fn pinch_zooms_the_canvas_about_the_pointer() -> Result<(), Box<dyn std::error::Error>> {
        let engine = shaped_board()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let pad_label = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|s| text_position(&s.shape, "1"))
        };
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let before = pad_label(&output).ok_or("pad number 1 not drawn")?;
        canvas_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::PointerMoved(before), egui::Event::Zoom(1.5)],
        );
        assert!((editor.zoom - 1.5).abs() < 1e-5, "{}", editor.zoom);
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let after = pad_label(&output).ok_or("pad number 1 not drawn after zoom")?;
        assert!((after - before).length() < 0.5, "{before:?} → {after:?}");
        Ok(())
    }

    /// PCB 2D's copper and silkscreen toggles and PCB 3D's layer view are
    /// one state: a change on either side shows on the other, and 3D-only
    /// layers stay 3D's.
    #[test]
    fn pcb_2d_and_3d_share_the_copper_and_silkscreen_toggles() {
        use crate::layer_view::ViewLayer;
        let mut editor = PcbEditor::default();
        editor.layers.copper_bottom = false;
        editor.layers.silk_top = false;
        let view = editor.layer_view();
        assert!(!view.visible(ViewLayer::Copper(pcbcraft_core::CopperLayer::Bottom)));
        assert!(!view.visible(ViewLayer::Silkscreen(Layer::Top)));
        assert!(view.visible(ViewLayer::Copper(pcbcraft_core::CopperLayer::Top)));
        let mut view = view;
        view.set(ViewLayer::Copper(pcbcraft_core::CopperLayer::Bottom), true);
        view.set(ViewLayer::Silkscreen(Layer::Bottom), false);
        view.set(ViewLayer::Mask(Layer::Top), false);
        editor.set_layer_view(view.clone());
        assert!(editor.layers.copper_bottom && !editor.layers.silk_bottom);
        assert!(!editor.layers.silk_top);
        assert_eq!(editor.layer_view(), view);
        // Only the shared toggles reach 2D.
        assert!(editor.layers.courtyard && editor.layers.holes);
    }
}
