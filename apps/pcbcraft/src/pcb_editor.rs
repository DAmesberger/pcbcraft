//! PCB content for Solidcraft's shared shell. Controls follow upstream
//! ui_kit/layout.rs and fields.rs; gesture previews commit on release, following
//! drag_snap.rs. The root owns the native Solidcraft viewport and its camera.
//!
//! Layout draws only what core places ([`layout::placed_pads`],
//! [`layout::placed_graphics`]) and picks through core queries
//! ([`layout::pick_pad`], [`layout::pick_graphic`]); it never decides pad or
//! graphic geometry. The cached board is rebuilt per document revision, not
//! on selection, layer or camera changes.
use crate::kit::{
    self,
    card::{CardResponse, Inspector, IssueSlot, Region, Subject},
};
use crate::refusal::{self, Origin};
use crate::symbol_view::{self, View};
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use pcbcraft_core::{
    Engine, Id, Layer, ObjectRef, Operation, Placement, Point, Project, Query, QueryResult,
    geometry::{Shape as CoreShape, Transform2},
    instances,
    issues::{IssueArea, IssueScope},
    layout::{
        self, BoardCopper, BoardDrawing, LayoutCommand, PlacedGraphic, PlacedHole, PlacedItem,
        PlacedPad, RouteRequest, WorldPad,
    },
    library::{PadKind, PlacedText, TextMetric},
    updates::Change,
};
use solidcraft_ui_egui::{
    fonts,
    theme::{self, Tokens},
};
use std::collections::BTreeSet;

mod board_card;
mod placement_card;
mod route_card;
mod track_card;

/// Scroll and pinch zoom limits, relative to the board fitting the canvas.
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.2..=12.0;

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

/// A card field whose commit can be refused; a refusal shows under it
/// (`refusal::Origin::Field`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Field {
    Position,
    Rotation,
    Side,
    TrackWidth,
    TrackLayer,
}

impl Field {
    const ALL: [Field; 5] = [
        Field::Position,
        Field::Rotation,
        Field::Side,
        Field::TrackWidth,
        Field::TrackLayer,
    ];
    fn id(self) -> egui::Id {
        egui::Id::new(("pcb-field", self))
    }
}

/// Refused commits, by the field that sent them, in shown wording. They
/// are dropped when the subject or the document changes.
#[derive(Default)]
struct FieldErrors(std::collections::BTreeMap<Field, String>);

/// What the slot's Properties card inspects, for its scroll key and its
/// cached issue slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Inspected {
    Board,
    Placement(Id),
    Unplaced(Id),
    Track(Id),
    Review,
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
    from_bottom: bool,
    /// The staged width of the selected track, committed on focus loss.
    track_width: f64,
    /// The field that had keyboard focus last frame, so Esc reverts it.
    editing: Option<Field>,
    /// The staged values last sent, so a refused commit is not resent
    /// every frame while the field still shows it.
    sent: std::collections::BTreeMap<Field, [f64; 2]>,
    errors: FieldErrors,
    /// The Board setup dialog while it is open.
    setup: Option<board_card::BoardSetup>,
    /// Board bounds a Locate asked the next Layout frame to frame.
    frame_request: Option<[Point; 2]>,
    /// Instance whose schematic symbol the app should show next.
    show_in_schematic: Option<Id>,
    /// Every Connections row instead of the first eight.
    connections_all: bool,
    /// The issue slot of the last inspected subject, per document revision.
    issue_slot: Option<((u64, usize, Inspected), Option<IssueSlot>)>,
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
            from_bottom: false,
            track_width: 0.0,
            editing: None,
            sent: Default::default(),
            errors: FieldErrors::default(),
            setup: None,
            frame_request: None,
            show_in_schematic: None,
            connections_all: false,
            issue_slot: None,
        }
    }
}

impl PcbEditor {
    pub fn is_3d(&self) -> bool {
        self.view_3d
    }
    /// Switch between Layout and 3D. 3D has no 2D tools: an armed Route
    /// ends, so no 2D tool card shows there; selection is kept.
    pub fn set_3d(&mut self, enabled: bool) {
        self.view_3d = enabled;
        self.route_start = None;
        if enabled {
            self.tool = Tool::Select;
        }
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
    pub fn set_selection(&mut self, instance: Option<Id>, track: Option<Id>, net: Option<Id>) {
        self.net = net;
        self.highlighted = net;
        if instance != self.instance {
            self.connections_all = false;
        }
        self.instance = instance;
        self.track = track;
        self.discard_staged();
    }

    /// Drop uncommitted field edits and their refusals; the next sync
    /// reloads the fields from the document. Called on every tool or
    /// subject change.
    fn discard_staged(&mut self) {
        self.property_key = None;
        self.editing = None;
        self.sent.clear();
        self.errors.0.clear();
    }

    /// The instance whose symbol the Schematic workspace should select
    /// ("Schematic ›" in the placement footer), once.
    pub fn take_show_in_schematic(&mut self) -> Option<Id> {
        self.show_in_schematic.take()
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
        let key = (engine.revision(), self.instance.or(self.track));
        if self.property_key != Some(key) {
            // A new revision or subject: the fields show the document again.
            self.sent.clear();
            self.errors.0.clear();
            if let Some(track) = engine
                .project()
                .tracks
                .iter()
                .find(|t| Some(t.id) == self.track)
            {
                self.track_width = track.width;
            }
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

    /// What the Properties card inspects.
    fn inspected(&self, project: &Project) -> Inspected {
        if self.update.is_open() {
            return Inspected::Review;
        }
        if let Some(id) = self.instance.filter(|id| project.instance(*id).is_some()) {
            return if project.placements.iter().any(|p| p.instance == id) {
                Inspected::Placement(id)
            } else {
                Inspected::Unplaced(id)
            };
        }
        match self
            .track
            .filter(|id| project.tracks.iter().any(|t| t.id == *id))
        {
            Some(id) => Inspected::Track(id),
            None => Inspected::Board,
        }
    }

    pub fn header(&self, engine: &Engine) -> crate::shell::Header {
        use crate::shell::Header;
        let project = engine.project();
        match self.inspected(project) {
            Inspected::Review => Header::new("component", "Update PCB", "Review"),
            Inspected::Placement(id) | Inspected::Unplaced(id) => {
                let designator = project
                    .instance(id)
                    .map_or_else(String::new, |i| i.designator.clone());
                let kind = if matches!(self.inspected(project), Inspected::Placement(_)) {
                    "Placement"
                } else {
                    "Not placed"
                };
                Header::new("component", designator, kind)
            }
            Inspected::Track(id) => {
                let net = project
                    .tracks
                    .iter()
                    .find(|t| t.id == id)
                    .map_or("", |t| project.net_name(t.net));
                Header::new("trace", format!("Track · {net}"), "Track")
            }
            Inspected::Board => Header::new("plane", "Board", "Board"),
        }
    }

    /// What the subject slot shows: the update review, the Board setup
    /// dialog, the Route tool card while Route is armed in Layout, or the
    /// Properties card (docs/sidebar-ia.md, "Layout").
    pub fn subject(&self) -> Subject {
        if self.update.is_open() {
            Subject::Review
        } else if self.setup.is_some() {
            Subject::Dialog
        } else if self.tool == Tool::Route && !self.view_3d {
            Subject::Tool
        } else {
            Subject::Properties
        }
    }

    /// The Properties card's frame: header, scroll key, footer and the
    /// reserved issue slot.
    pub fn card(&mut self, engine: &Engine) -> Inspector {
        let inspected = self.inspected(engine.project());
        Inspector {
            header: self.header(engine),
            subject: egui::Id::new(("pcb-card", inspected)),
            footer: matches!(inspected, Inspected::Placement(_) | Inspected::Track(_)),
            issues: self.issue_slot(engine, inspected),
        }
    }

    /// The whole subject slot for the right column: the docked update
    /// review, the tool card, the Board setup dialog, or the Properties
    /// card with its pinned footer and issue slot. Refusals of this editor's fields and dialog are
    /// claimed here, the frame after they were refused.
    pub fn subject_slot(&mut self, ui: &mut Ui, engine: &Engine) -> CardResponse<Operation> {
        self.sync(engine);
        self.claim_refusals(ui.ctx());
        match self.subject() {
            Subject::Tool => CardResponse::output(self.route_card(ui, engine.project())),
            Subject::Dialog => CardResponse::output(self.board_setup(ui, engine)),
            // The review docks with its own header and pinned footer
            // (slice R1), in place of the Properties card.
            Subject::Review => CardResponse::output(self.update.show(ui, engine)),
            Subject::Properties => {
                let inspector = self.card(engine);
                kit::card::card(ui, &inspector, |ui, region| match region {
                    Region::Body => self.inspector(ui, engine),
                    Region::Footer => self.footer(ui, engine),
                })
            }
        }
    }

    /// Take the refusals handed back for this editor's fields and dialog.
    fn claim_refusals(&mut self, ctx: &egui::Context) {
        for field in Field::ALL {
            if let Some(text) = refusal::claim(ctx, Origin::Field(field.id())) {
                self.errors.0.insert(field, text);
            }
        }
        if let Some(setup) = &mut self.setup {
            setup.claim(ctx);
        }
    }

    /// The issue slot of `inspected`: its Board issues from the one
    /// `project.issues` query, cached per document revision. `None` (an
    /// empty slot) when the query refuses.
    fn issue_slot(&mut self, engine: &Engine, inspected: Inspected) -> Option<IssueSlot> {
        let key = (
            engine.revision(),
            std::sync::Arc::as_ptr(engine.project_arc()) as usize,
            inspected,
        );
        if let Some((cached, slot)) = &self.issue_slot
            && *cached == key
        {
            return slot.clone();
        }
        let slot = issue_slot_of(engine, inspected);
        self.issue_slot = Some((key, slot.clone()));
        slot
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
        match self.inspected(project) {
            Inspected::Placement(id) => {
                let placement = project.placements.iter().find(|p| p.instance == id)?;
                self.placement_section(ui, project, placement)
            }
            Inspected::Unplaced(_) => {
                self.unplaced_section(ui, engine);
                None
            }
            Inspected::Track(id) => {
                let track = project.tracks.iter().find(|t| t.id == id)?;
                self.track_section(ui, project, track)
            }
            Inspected::Board | Inspected::Review => {
                self.board_section(ui, project);
                None
            }
        }
    }

    /// The pinned footer's object actions: Rotate 90°, Flip and Schematic ›
    /// for a placement, Delete track for a track. Never Apply.
    pub fn footer(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let project = engine.project();
        match self.inspected(project) {
            Inspected::Placement(id) => {
                let placement = project.placements.iter().find(|p| p.instance == id)?;
                self.placement_footer(ui, placement)
            }
            Inspected::Track(id) => {
                let track = project.tracks.iter().find(|t| t.id == id)?;
                track_card::track_footer(ui, track)
            }
            _ => None,
        }
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
        if let Some(bounds) = self.frame_request.take() {
            (self.zoom, self.pan) = frame_view(rect, project.board_size, bounds, self.from_bottom);
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
                                layer: self.layer,
                            },
                        }));
                        // The route stays in progress until the engine takes
                        // it: a refusal (a toast) leaves the start, so the
                        // next click tries again; an accepted segment is a
                        // new revision, whose sync ends the route.
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
        // Slice R1: the docked review's canvas preview.
        if let Some(marks) = self.update.marks() {
            crate::update_preview::paint_board(&painter, project, &marks, screen, |id| {
                let placement = project.placements.iter().find(|p| p.instance == id)?;
                body_polygon(project, placement)
            });
        }
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
                "{} · {} copper · grid 0.5 mm · {} airwires{}",
                if self.tool == Tool::Route {
                    "Route"
                } else {
                    "Select / move"
                },
                layer_name(self.layer),
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
        self.pills(ui, rect, project);
        operation
    }

    /// Pills for the persistent modes, top-centre of the canvas: the route
    /// in progress, else the highlighted net. × ends the mode.
    fn pills(&mut self, ui: &mut Ui, rect: Rect, project: &Project) {
        let t = Tokens::get();
        let text = match (self.route_start, self.net, self.highlighted) {
            (Some(_), Some(net), _) => format!(
                "Routing {} · {} mm",
                project.net_name(net),
                kit::number_text(self.width)
            ),
            (None, _, Some(net)) if project.net(net).is_some() => {
                format!("{} highlighted", project.net_name(net))
            }
            _ => return,
        };
        // The pill hugs its text: its frame fills the rect it is given.
        let text_width = ui
            .painter()
            .layout_no_wrap(text.clone(), FontId::proportional(t.font_label), t.text)
            .size()
            .x;
        let width = (text_width + 3.0 * t.control_height).min(rect.width());
        let area = Rect::from_min_size(
            rect.center_top() + egui::vec2(-width / 2.0, t.space_8),
            egui::vec2(width, t.control_height + t.space_8),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area));
        let pill = kit::pill(&mut child, None, &text);
        if pill.closed {
            if self.route_start.is_some() {
                self.route_start = None;
            } else {
                self.highlighted = None;
            }
        }
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
            layers.copper(track.layer)
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
                    shade(copper_color(track.layer, selected)),
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

/// The Board issues on `inspected` and the words after their counts ("on
/// R13"), from `project.issues`. The board card counts the whole board.
fn issue_slot_of(engine: &Engine, inspected: Inspected) -> Option<IssueSlot> {
    let project = engine.project();
    let designator = |id: Id| {
        project
            .instance(id)
            .map_or_else(String::new, |i| i.designator.clone())
    };
    let (scope, label) = match inspected {
        Inspected::Review => return None,
        Inspected::Board => (
            IssueScope::Area {
                area: IssueArea::Board,
            },
            "on this board".to_owned(),
        ),
        Inspected::Placement(id) | Inspected::Unplaced(id) => (
            IssueScope::Selection {
                objects: vec![ObjectRef::Instance { id }],
            },
            format!("on {}", designator(id)),
        ),
        Inspected::Track(id) => (
            IssueScope::Selection {
                objects: vec![ObjectRef::Track { id }],
            },
            "on this track".to_owned(),
        ),
    };
    let Ok(QueryResult::Issues(report)) = engine.query(Query::Issues { scope }) else {
        return None;
    };
    let mut counts = pcbcraft_core::issues::IssueCounts::default();
    for row in report.rows.iter().filter(|r| r.area == IssueArea::Board) {
        let slot = match row.severity {
            pcbcraft_core::issues::Severity::Error => &mut counts.error,
            pcbcraft_core::issues::Severity::Warning => &mut counts.warning,
            pcbcraft_core::issues::Severity::ToDo => &mut counts.to_do,
            pcbcraft_core::issues::Severity::Info => &mut counts.info,
        };
        *slot = slot.saturating_add(1);
    }
    Some(IssueSlot { counts, label })
}

/// Whether the widget with keyboard focus lies inside `rect` (one card
/// row), so the row's edit is still in progress.
fn focus_within(ui: &Ui, rect: Rect) -> bool {
    ui.memory(|m| m.focused())
        .and_then(|id| ui.ctx().read_response(id))
        .is_some_and(|r| rect.expand(1.0).contains_rect(r.rect))
}

/// The board-space bounds of the objects `refs` names, for framing: a
/// part's package envelope, a pad's outline, a track and every pad and
/// track of a net. `None` when none is on the board.
fn bounds_of(editor: &PcbEditor, project: &Project, refs: &[ObjectRef]) -> Option<[Point; 2]> {
    let mut points: Vec<Point> = Vec::new();
    let pads_where = |keep: &dyn Fn(&PlacedPad) -> bool, points: &mut Vec<Point>| {
        for pad in editor.board_pads.iter().filter(|b| keep(&b.placed)) {
            points.extend(pad.outline.flatten(0.05));
        }
    };
    for object in refs {
        match *object {
            ObjectRef::Instance { id }
            | ObjectRef::Unit { instance: id, .. }
            | ObjectRef::Property { instance: id, .. } => {
                if let Some(placement) = project.placements.iter().find(|p| p.instance == id) {
                    match body_polygon(project, placement) {
                        Some(polygon) => points.extend(polygon),
                        None => points.push(placement.position),
                    }
                }
            }
            ObjectRef::Terminal { endpoint } => {
                pads_where(&|p| p.terminal == Some(endpoint), &mut points)
            }
            ObjectRef::Pad { instance, pad } => {
                pads_where(&|p| p.instance == instance && p.pad.id == pad, &mut points)
            }
            ObjectRef::Track { id } => {
                for track in project.tracks.iter().filter(|t| t.id == id) {
                    points.extend(layout::track_polygon(track));
                }
            }
            ObjectRef::Net { id } => {
                pads_where(&|p| p.net == Some(id), &mut points);
                for track in project.tracks.iter().filter(|t| t.net == id) {
                    points.extend(layout::track_polygon(track));
                }
            }
            _ => {}
        }
    }
    let first = *points.first()?;
    Some(points.iter().fold([first, first], |[lo, hi], p| {
        [
            [lo[0].min(p[0]), lo[1].min(p[1])],
            [hi[0].max(p[0]), hi[1].max(p[1])],
        ]
    }))
}

/// The zoom and pan that frame `bounds` in the canvas `rect`: centred, at
/// about a third of the canvas, never zoomed out past the fitted board.
fn frame_view(rect: Rect, board: Point, bounds: [Point; 2], from_bottom: bool) -> (f32, Vec2) {
    let fit = fit_scale(rect, board);
    let size = [
        (bounds[1][0] - bounds[0][0]).max(1.0) as f32,
        (bounds[1][1] - bounds[0][1]).max(1.0) as f32,
    ];
    let wanted = (rect.width() / (3.0 * size[0])).min(rect.height() / (3.0 * size[1]));
    let zoom = (wanted / fit).clamp(1.0, *ZOOM_RANGE.end());
    let centre = [
        (bounds[0][0] + bounds[1][0]) / 2.0,
        (bounds[0][1] + bounds[1][1]) / 2.0,
    ];
    let unpanned = board_view(rect, board, zoom, Vec2::ZERO, from_bottom);
    (zoom, rect.center() - unpanned.screen(centre))
}

impl crate::issues::Locate for PcbEditor {
    /// Select the first board object of `refs` (a part, else a track, else
    /// a net) and frame all of them on the next Layout frame. Selection
    /// and camera only: the tool, document and geometry stay as they are.
    fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool {
        self.sync(engine);
        let project = engine.project();
        let Some((instance, track, net)) = locate(refs) else {
            return false;
        };
        let exists = instance.is_some_and(|id| project.instance(id).is_some())
            || track.is_some_and(|id| project.tracks.iter().any(|t| t.id == id))
            || (instance.is_none()
                && track.is_none()
                && net.is_some_and(|id| project.net(id).is_some()));
        if !exists {
            return false;
        }
        self.set_selection(instance, track, net);
        self.frame_request = bounds_of(self, project, refs);
        true
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
    use super::board_card::fabrication_edit;
    use super::*;
    use pcbcraft_core::{Fabrication, MaskColour};

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
                command = editor.subject_slot(ui, engine).output;
                // The app reads the origin in the same frame (`App::apply`).
                let mark = refusal::take_mark(ui.ctx());
                ui.ctx()
                    .data_mut(|d| d.insert_temp(egui::Id::new("test-mark"), mark));
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

    /// Board setup… opens the docked dialog; choosing a mask colour and OK
    /// issue `layout.set_fabrication` with the other attributes kept; the
    /// engine applies it as one undo step and the rebuilt 3D layers take
    /// the look. The dialog closes once the document has moved on.
    #[test]
    fn board_setup_dialog_issues_set_fabrication() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        assert!(fabrication_edit(Fabrication::default(), Fabrication::default()).is_none());
        // The Board card shows the finish in one line, not the form.
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            painted(&output)
                .texts
                .iter()
                .any(|t| t == "Green · ENIG · White silk")
        );
        assert!(!painted(&output).texts.iter().any(|t| t == "Soldermask"));
        assert!(click(&mut editor, &engine, &ctx, "Setup…")?.is_none());
        assert_eq!(editor.subject(), Subject::Dialog);
        // Open the Soldermask dropdown, choose Blue: nothing is sent yet.
        assert!(click(&mut editor, &engine, &ctx, "Green")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "Blue")?.is_none());
        let operation = click(&mut editor, &engine, &ctx, "OK")?.ok_or("OK did nothing")?;
        let expected = Fabrication {
            mask_colour: MaskColour::Blue,
            ..Fabrication::default()
        };
        let Operation::Layout(LayoutCommand::SetFabrication(issued)) = &operation else {
            return Err("the row issued another command".into());
        };
        assert_eq!(*issued, expected);
        assert_eq!(
            marked(&ctx),
            Some(Origin::Dialog(egui::Id::new("pcb-board-setup")))
        );
        engine.apply(engine.revision(), operation)?;
        assert_eq!(engine.project().fabrication, expected);
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(editor.subject(), Subject::Properties);
        let model = pcbcraft_solidcraft::project::build(engine.project())?;
        let blue = pcbcraft_solidcraft::material::material(
            pcbcraft_solidcraft::material::BodyKind::MaskTop,
            &expected,
        )
        .surface()?;
        assert!(model.geometry.iter().any(|g| g.kind
            == pcbcraft_solidcraft::material::BodyKind::MaskTop
            && g.surface == blue));
        // The summary follows the document, and undo restores Green.
        let (output, none) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(none.is_none());
        assert!(
            painted(&output)
                .texts
                .iter()
                .any(|t| t == "Blue · ENIG · White silk")
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
        assert!(has("Not on the board yet"));
        // Not the Board body under a placement header.
        assert!(!has("Finish"));
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

    /// The refusal origin the last `frame` marked for its operation.
    fn marked(ctx: &egui::Context) -> Option<Origin> {
        ctx.data(|d| d.get_temp::<Option<Origin>>(egui::Id::new("test-mark")))
            .flatten()
    }

    fn texts(output: &egui::FullOutput) -> Vec<String> {
        painted(output).texts
    }

    fn placement_of(engine: &Engine, id: Id) -> Result<Placement, Box<dyn std::error::Error>> {
        Ok(engine
            .project()
            .placements
            .iter()
            .find(|p| p.instance == id)
            .cloned()
            .ok_or("fixture placement missing")?)
    }

    /// A Position edit commits live once the field is left: one Move with
    /// both coordinates, sent once, marked as the Position field's. A
    /// locked placement commits nothing.
    #[test]
    fn a_left_position_field_commits_one_move_and_lock_stops_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let before = placement_of(&engine, r1)?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(r1), None, None);
        let (_, none) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(none.is_none(), "an unedited card sends nothing");
        assert_eq!(editor.position, before.position);
        // What typing 12 and 9.5 into Position leaves once focus moves on.
        editor.position = [12.0, 9.5];
        let (_, operation) = frame(&mut editor, &engine, &ctx, vec![]);
        let operation = operation.ok_or("the left field did not commit")?;
        let Operation::Layout(LayoutCommand::Move {
            instance,
            position,
            rotation_deg,
            layer,
        }) = &operation
        else {
            return Err("Position issued another command".into());
        };
        assert_eq!(
            (*instance, *position, *rotation_deg, *layer),
            (r1, [12.0, 9.5], before.rotation_deg, before.layer)
        );
        assert_eq!(marked(&ctx), Some(Origin::Field(Field::Position.id())));
        // Not resent while the document has not caught up.
        assert!(frame(&mut editor, &engine, &ctx, vec![]).1.is_none());
        engine.apply(engine.revision(), operation)?;
        assert_eq!(placement_of(&engine, r1)?.position, [12.0, 9.5]);
        assert!(frame(&mut editor, &engine, &ctx, vec![]).1.is_none());
        // A rotation edit is its own step, keeping the position.
        editor.rotation = 90.0;
        let Some(Operation::Layout(LayoutCommand::Move {
            position,
            rotation_deg,
            ..
        })) = frame(&mut editor, &engine, &ctx, vec![]).1
        else {
            return Err("Rotation did not commit a move".into());
        };
        assert_eq!((position, rotation_deg), ([12.0, 9.5], 90.0));
        // Locked: the fields are disabled and nothing commits.
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::SetPlacementLock {
                instance: r1,
                locked: true,
            }),
        )?;
        frame(&mut editor, &engine, &ctx, vec![]);
        editor.position = [1.0, 1.0];
        assert!(frame(&mut editor, &engine, &ctx, vec![]).1.is_none());
        assert!(click(&mut editor, &engine, &ctx, "Rotate 90°")?.is_none());
        Ok(())
    }

    /// An off-board Position is refused by the engine; the refusal comes
    /// back to the field, which keeps the typed value marked with the
    /// board bounds under it, and the document is unchanged.
    #[test]
    fn a_refused_position_shows_under_the_field_and_is_not_resent()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let before = engine.project().clone();
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(r1), None, None);
        frame(&mut editor, &engine, &ctx, vec![]);
        editor.position = [500.0, 9.0];
        let operation = frame(&mut editor, &engine, &ctx, vec![])
            .1
            .ok_or("the field did not commit")?;
        let origin = marked(&ctx).ok_or("no origin marked")?;
        let error = engine
            .apply(engine.revision(), operation)
            .err()
            .ok_or("the engine took an off-board placement")?;
        assert_eq!(engine.project(), &before);
        // The app hands it back during the frame that applied it.
        let mut refused = Some(refusal::Refused { origin, error });
        ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(refused) = refused.take() {
                refusal::hand_back(ui.ctx(), refused, false);
            }
        })
        .textures_delta
        .clear();
        let (output, again) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(again.is_none(), "a refused value is not resent");
        let shown = texts(&output);
        assert!(
            shown
                .iter()
                .any(|t| t == "Must stay inside the board (60 × 40 mm)"),
            "{shown:?}"
        );
        assert_eq!(editor.position, [500.0, 9.0], "the typed value stays");
        assert!(refusal::unclaimed(&ctx).is_empty(), "the field claimed it");
        // Another subject drops the edit and its refusal.
        editor.set_selection(None, None, None);
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            !texts(&output)
                .iter()
                .any(|t| t.starts_with("Must stay inside"))
        );
        Ok(())
    }

    /// The placement footer: Rotate 90° and Flip issue one Move each, and
    /// Schematic › asks the app to show the symbol.
    #[test]
    fn the_placement_footer_rotates_flips_and_opens_the_schematic()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let before = placement_of(&engine, r1)?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(r1), None, None);
        assert!(editor.card(&engine).footer);
        let Some(Operation::Layout(LayoutCommand::Move { rotation_deg, .. })) =
            click(&mut editor, &engine, &ctx, "Rotate 90°")?
        else {
            return Err("Rotate 90° issued no move".into());
        };
        assert_eq!(rotation_deg, (before.rotation_deg + 90.0).rem_euclid(360.0));
        let Some(Operation::Layout(LayoutCommand::Move { layer, .. })) =
            click(&mut editor, &engine, &ctx, "Flip")?
        else {
            return Err("Flip issued no move".into());
        };
        assert_eq!(layer, opposite(before.layer));
        assert!(click(&mut editor, &engine, &ctx, "Schematic ›")?.is_none());
        assert_eq!(editor.take_show_in_schematic(), Some(r1));
        assert_eq!(editor.take_show_in_schematic(), None);
        Ok(())
    }

    /// Connections lists every pad with its net and routing state from the
    /// core's unrouted findings; routing the net makes both pads routed.
    #[test]
    fn connections_follow_the_core_unrouted_findings() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        let rows = editor.connections(r1);
        assert_eq!(rows.len(), 2);
        let pad2 = rows
            .iter()
            .find(|r| r.pad == "Pad 2")
            .ok_or("R1 pad 2 missing")?;
        assert!(pad2.net.is_some());
        assert!(pad2.unrouted);
        assert_eq!(pad2.state, "unrouted");
        let routed = routed_board()?;
        let mut editor = PcbEditor::default();
        editor.sync(&routed);
        let pad2 = editor
            .connections(r1)
            .into_iter()
            .find(|r| r.pad == "Pad 2")
            .ok_or("R1 pad 2 missing")?;
        assert_eq!((pad2.state, pad2.unrouted), ("routed", false));
        Ok(())
    }

    /// The Track card: header "Track · net", the ends it meets, and live
    /// Width and Layer edits through `layout.track_set`; Delete track sits
    /// in the footer.
    #[test]
    fn the_track_card_edits_width_and_layer_live() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = routed_board()?;
        let track = engine.project().tracks[0].clone();
        let net = engine.project().net_name(track.net).to_owned();
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(None, Some(track.id), Some(track.net));
        let header = editor.header(&engine);
        assert_eq!(
            (header.title.as_str(), header.kind),
            (format!("Track · {net}").as_str(), "Track")
        );
        let (output, none) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(none.is_none());
        let shown = texts(&output);
        assert!(
            shown.iter().any(|t| t == "R1 pad 2 → R2 pad 1"),
            "{shown:?}"
        );
        assert!(shown.iter().any(|t| t == "5.6 mm"), "{shown:?}");
        editor.track_width = 0.5;
        let operation = frame(&mut editor, &engine, &ctx, vec![])
            .1
            .ok_or("Width did not commit")?;
        let Operation::Layout(LayoutCommand::SetTrack { id, width, layer }) = &operation else {
            return Err("Width issued another command".into());
        };
        assert_eq!((*id, *width, *layer), (track.id, Some(0.5), None));
        assert_eq!(marked(&ctx), Some(Origin::Field(Field::TrackWidth.id())));
        engine.apply(engine.revision(), operation)?;
        assert_eq!(engine.project().tracks[0].width, 0.5);
        assert_eq!(engine.project().tracks[0].id, track.id);
        // Layer: open the dropdown, choose Bottom.
        assert!(click(&mut editor, &engine, &ctx, "Top")?.is_none());
        let Some(Operation::Layout(LayoutCommand::SetTrack { layer, width, .. })) =
            click(&mut editor, &engine, &ctx, "Bottom")?
        else {
            return Err("Layer issued no track edit".into());
        };
        assert_eq!((layer, width), (Some(Layer::Bottom), None));
        let Some(Operation::Layout(LayoutCommand::DeleteTrack { id })) =
            click(&mut editor, &engine, &ctx, "Delete track")?
        else {
            return Err("Delete track did nothing".into());
        };
        assert_eq!(id, track.id);
        Ok(())
    }

    /// Route armed in Layout replaces Properties with the Route tool card;
    /// Done returns to the Properties card of the same selection. 3D shows
    /// no 2D tool card, and the route stays in progress after a refusal.
    #[test]
    fn route_shows_a_tool_card_only_in_layout() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.set_selection(Some(r1), None, None);
        editor.on_tool("route", &engine);
        assert_eq!(editor.subject(), Subject::Tool);
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let shown = texts(&output);
        for label in [
            "Route",
            "Done",
            "Width",
            "Layer",
            "Net",
            "From the first pad",
        ] {
            assert!(
                shown.iter().any(|t| t == label),
                "{label} missing: {shown:?}"
            );
        }
        // Not a placement section under the tool.
        assert!(!shown.iter().any(|t| t == "PLACEMENT"));
        assert!(click(&mut editor, &engine, &ctx, "Done")?.is_none());
        assert_eq!(editor.subject(), Subject::Properties);
        assert_eq!(editor.selected_instance(), Some(r1));
        editor.on_tool("route", &engine);
        editor.set_3d(true);
        assert_eq!(editor.subject(), Subject::Properties);
        assert!(!editor.has_view_menu());
        editor.set_3d(false);
        assert!(editor.has_view_menu());
        Ok(())
    }

    /// A route segment the engine refuses leaves the route in progress
    /// (its start and the pill); an accepted one ends it.
    #[test]
    fn a_refused_route_segment_keeps_the_route_in_progress()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        editor.sync(&engine);
        // The net of R1 pad 2 and R2 pad 1, as `routed_board` routes it.
        let net = engine
            .project()
            .net_of(pcbcraft_core::Endpoint {
                instance: ids(&engine).0,
                terminal: pcbcraft_core::TerminalId(2),
            })
            .map(|n| n.id)
            .ok_or("demo net")?;
        editor.on_tool("route", &engine);
        editor.net = Some(net);
        editor.route_start = Some([9.2, 8.0]);
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let pill = format!("Routing {} · 0.3 mm", engine.project().net_name(net));
        assert!(texts(&output).contains(&pill));
        // A click far outside the board is refused by the engine.
        let operation = {
            let click = |p: Pos2| {
                vec![
                    egui::Event::PointerMoved(p),
                    egui::Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: p,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            canvas_frame(&mut editor, &engine, &ctx, click(Pos2::new(5.0, 5.0)))
                .1
                .ok_or("no route requested")?
        };
        assert!(engine.apply(engine.revision(), operation).is_err());
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(editor.route_start, Some([9.2, 8.0]));
        // An accepted segment is a new revision: the route ends.
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
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(editor.route_start, None);
        Ok(())
    }

    /// Locate selects the first part a finding names and frames it on the
    /// next Layout frame, without touching the document or the cached
    /// board; refs with nothing on the board locate nothing.
    #[test]
    fn locate_selects_and_frames_the_named_part() -> Result<(), Box<dyn std::error::Error>> {
        use crate::issues::Locate;
        let engine = shaped_board()?;
        let (_, r2) = ids(&engine);
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let cached = editor.board_pads.as_ptr();
        let revision = engine.revision();
        let pad = editor
            .board_pads
            .iter()
            .find(|b| b.placed.instance == r2)
            .map(|b| b.placed.pad.id)
            .ok_or("R2 pad")?;
        assert!(editor.locate(
            &engine,
            &[
                ObjectRef::Net { id: 999 },
                ObjectRef::Pad { instance: r2, pad }
            ]
        ));
        assert_eq!(editor.selected_instance(), Some(r2));
        let bounds = editor.frame_request.ok_or("no framing asked")?;
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
        assert!(editor.frame_request.is_none());
        assert!(editor.zoom > 1.0);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 700.0));
        let view = board_view(
            rect,
            engine.project().board_size,
            editor.zoom,
            editor.pan,
            false,
        );
        let centre = [
            (bounds[0][0] + bounds[1][0]) / 2.0,
            (bounds[0][1] + bounds[1][1]) / 2.0,
        ];
        assert!((view.screen(centre) - rect.center()).length() < 0.5);
        assert_eq!(engine.revision(), revision);
        assert_eq!(editor.board_pads.as_ptr(), cached);
        assert!(!editor.locate(&engine, &[ObjectRef::Instance { id: 999 }]));
        assert!(!editor.locate(&engine, &[]));
        Ok(())
    }

    /// The card's issue slot counts the inspected object's Board issues
    /// from `project.issues`; the Board card counts the whole board.
    #[test]
    fn the_issue_slot_counts_the_objects_board_issues() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let (r1, _) = ids(&engine);
        let mut editor = PcbEditor::default();
        let board = editor.card(&engine).issues.ok_or("no board slot")?;
        let Ok(QueryResult::Issues(report)) = engine.query(Query::Issues {
            scope: IssueScope::Area {
                area: IssueArea::Board,
            },
        }) else {
            return Err("issues query refused".into());
        };
        assert_eq!(board.counts, report.counts);
        assert_eq!(board.label, "on this board");
        // The demo's net between R1 and R2 is unrouted: a To do on R1.
        editor.set_selection(Some(r1), None, None);
        let slot = editor.card(&engine).issues.ok_or("no placement slot")?;
        assert_eq!(slot.label, "on R1");
        assert!(slot.counts.to_do >= 1, "{:?}", slot.counts);
        assert!(slot.counts.to_do <= board.counts.to_do);
        Ok(())
    }

    /// The view popover holds the layer and overlay toggles in Layout.
    #[test]
    fn the_view_popover_toggles_layers_without_document_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let run = |editor: &mut PcbEditor, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 600.0))),
                    events,
                    ..Default::default()
                },
                |ui| editor.view_menu(ui),
            );
            output.textures_delta.clear();
            output
        };
        let output = run(&mut editor, vec![]);
        let at = output
            .shapes
            .iter()
            .find_map(|s| text_position(&s.shape, "Courtyard"))
            .ok_or("Courtyard toggle missing")?;
        for label in ["Top copper", "Show airwires", "View from bottom"] {
            assert!(texts(&output).iter().any(|t| t == label), "{label}");
        }
        run(
            &mut editor,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        run(
            &mut editor,
            vec![egui::Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!editor.layers.courtyard);
        Ok(())
    }
}
