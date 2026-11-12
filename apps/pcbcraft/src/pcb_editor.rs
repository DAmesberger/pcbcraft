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
    CopperLayer, Engine, Id, Layer, ObjectRef, Operation, Placement, Point, Project, Query,
    QueryResult, Via,
    geometry::{Shape as CoreShape, Transform2},
    instances,
    issues::{IssueArea, IssueScope},
    layout::{
        self, BoardCopper, BoardDrawing, CopperGraph, LayoutCommand, PlacedGraphic, PlacedHole,
        PlacedItem, PlacedPad, WorldPad,
    },
    library::{PadKind, PlacedText, TextMetric},
    updates::Change,
};
use solidcraft_theme::{self as theme, Tokens};
use std::collections::BTreeSet;
use std::sync::Arc;

mod board_card;
pub(crate) mod board_shape;
mod board_shape_ui;
mod commands;
pub(crate) use commands::LAYER_KEYS;
pub(crate) use commands::shape_tool_entries;
pub(crate) use route_tool::ROUTE_KEYS;
pub(crate) use tune_tool::TUNE_KEYS;
mod dimensions;
mod edge_card;
mod fill_card;
mod fill_paint;
#[cfg(test)]
mod fill_tests;
mod fill_tool;
mod group;
pub use group::Group;
mod hold_mark;
#[cfg(test)]
mod hold_tests;
mod impedance_view;
mod layer_focus;
mod menu;
#[cfg(test)]
use layer_focus::OTHER_SHADE;
use layer_focus::{FAR_SHADE, LayerFocus, Look};
#[cfg(test)]
use solidcraft_ui_egui::fonts;
use solidcraft_ui_egui::ui_kit::type_face;
#[cfg(test)]
mod layer_focus_tests;
pub(crate) mod net_classes;
#[cfg(test)]
mod pair_tests;
mod pair_tool;
mod part_place;
mod parts_table;
pub use parts_table::Tab as TableTab;
#[cfg(test)]
mod pick_tests;
mod placement_card;
mod production;
#[cfg(test)]
pub(crate) mod raster_tests;
mod route_card;
#[cfg(test)]
mod route_entry_tests;
#[cfg(test)]
mod route_tests;
pub(crate) mod route_tool;
mod rules_panel;
#[cfg(test)]
mod shove_tests;
mod signal_card;
mod stackup_panel;
mod text_card;
mod track_drag;
#[cfg(test)]
mod track_drag_tests;
mod tune_tool;
pub use rules_panel::{RuleTopic, topic_for as rule_topic_for};
mod track_card;
mod via_card;
#[cfg(test)]
mod via_tests;

/// How close to the canvas edge a located object may sit before the view
/// pans to it, in points.
const REVEAL_MARGIN: f32 = 24.0;

/// Scroll and pinch zoom limits, relative to the board fitting the canvas.
const ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.2..=12.0;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Select,
    Route,
    /// Place a through via on a net.
    Via,
    /// Draw a copper fill's outline.
    Fill,
    /// Meander a track segment to its length target (`tune_tool`).
    Tune,
    /// Dimension a part to a face, edge, hole or board edge, in Layout or
    /// 3D (`dimensions`).
    Dimension,
}

/// Most inner copper layers a stackup has: every layer but the two sides
/// of the largest stackup the rules allow.
const MAX_INNER: usize = pcbcraft_core::MAX_COPPER_LAYERS - 2;

/// Which board layers Layout draws and picks. Hidden layers do not pick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Layers {
    copper_top: bool,
    copper_bottom: bool,
    /// Inner copper layers 1 to [`MAX_INNER`].
    inner: [bool; MAX_INNER],
    /// Board vias (their rings and drills).
    vias: bool,
    silk_top: bool,
    silk_bottom: bool,
    courtyard: bool,
    /// The assembly drawing; hidden by default, since its outline and
    /// designator repeat the silkscreen's in the same place.
    assembly: bool,
    holes: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Self {
            copper_top: true,
            copper_bottom: true,
            inner: [true; MAX_INNER],
            vias: true,
            silk_top: true,
            silk_bottom: true,
            courtyard: true,
            assembly: false,
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
    /// Whether a copper layer of the stackup shows.
    fn copper_layer(&self, layer: CopperLayer) -> bool {
        match layer {
            CopperLayer::Top => self.copper_top,
            CopperLayer::Bottom => self.copper_bottom,
            CopperLayer::Inner(n) => usize::from(n)
                .checked_sub(1)
                .and_then(|i| self.inner.get(i))
                .copied()
                .unwrap_or(true),
        }
    }
    /// The toggle of a copper layer, for the view popover; `None` for an
    /// inner layer no stackup can have.
    fn copper_layer_mut(&mut self, layer: CopperLayer) -> Option<&mut bool> {
        match layer {
            CopperLayer::Top => Some(&mut self.copper_top),
            CopperLayer::Bottom => Some(&mut self.copper_bottom),
            CopperLayer::Inner(n) => usize::from(n)
                .checked_sub(1)
                .and_then(|i| self.inner.get_mut(i)),
        }
    }
    /// A via shows with the via layer while a copper layer it joins does.
    fn via(&self, via: &Via, stack: &[CopperLayer]) -> bool {
        self.vias
            && stack
                .iter()
                .any(|l| self.copper_layer(*l) && via.on(*l, stack))
    }
    /// A through-hole pad shows while any copper layer does (inner
    /// flags of layers the stackup lacks are cleared by
    /// [`PcbEditor::seen`]); a bare hole with the hole layer.
    fn pad(&self, pad: &PlacedPad) -> bool {
        match pad.pad.kind {
            PadKind::PlatedThrough => {
                self.copper_top || self.copper_bottom || self.inner.iter().any(|v| *v)
            }
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
    TextOffset,
}

impl Field {
    const ALL: [Field; 6] = [
        Field::Position,
        Field::Rotation,
        Field::Side,
        Field::TrackWidth,
        Field::TrackLayer,
        Field::TextOffset,
    ];
    fn id(self) -> egui::Id {
        egui::Id::unique(("pcb-field", self))
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
    Via(Id),
    Fill(Id),
    /// Two or more objects (`group`).
    Group,
    Review,
}

struct DragAirwires {
    key: (u64, Id, Point),
    connections: Vec<(WorldPad, WorldPad)>,
}

pub struct PcbEditor {
    /// Two-finger scrolling across frames.
    trackpad: crate::navigation::Trackpad,
    view_3d: bool,
    tool: Tool,
    instance: Option<Id>,
    /// Further parts a Locate selected with `instance` (both parts of a
    /// clearance issue); cleared by any other selection.
    also: Vec<Id>,
    track: Option<Id>,
    /// The selected board via.
    via: Option<Id>,
    net: Option<Id>,
    highlighted: Option<Id>,
    /// The net the Route tool highlighted, so ending the route turns off a
    /// highlight it made (and leaves one the user turned on).
    route_lit: Option<Id>,
    /// The copper layer Route draws on.
    layer: CopperLayer,
    /// The board's copper layers, top to bottom, per revision.
    stack: Vec<CopperLayer>,
    /// The Board card's Project row: the name last seen and the text being edited.
    project_name: (String, String),
    /// The Route tool's board point under the pointer while it hovers the
    /// canvas: where a layer key's via goes during a route.
    hover_at: Option<Point>,
    width: f64,
    /// The net and class width `width` was last set from, so a new net or
    /// a class edit sets the width and an edit by hand keeps it.
    width_from: Option<(Id, u64)>,
    route_start: Option<Point>,
    /// A layer change pressed (V) during a route: the via's position and
    /// the layer the route reaches it on. The next click routes there,
    /// places the via and routes on on `layer` (`layout.route_layer_change`).
    pending_via: Option<(Point, CopperLayer)>,
    /// The route in progress besides its head, and the routing settings
    /// (`route_tool`).
    route: route_tool::RouteState,
    /// The tracks and vias the previewed route step shoves: drawn at their
    /// new places by the preview, not at their old ones.
    shoved: (Vec<Id>, Vec<Id>),
    /// Whether vias the Via tool and layer changes place are tented:
    /// the tool's own choice, else the rules' `tent vias` ([`Self::via_tented`]).
    via_tented: Option<bool>,
    property_key: Option<(u64, Option<Id>)>,
    position: Point,
    rotation: f64,
    placement_layer: Layer,
    update: crate::update_preview::UpdatePreview,
    zoom: f32,
    pan: Vec2,
    drag: Option<(Placement, Point)>,
    drag_position: Option<Point>,
    /// A track dragged with Select (`track_drag`).
    track_drag: Option<track_drag::TrackDrag>,
    drag_airwires: Option<DragAirwires>,
    show_airwires: bool,
    cached_revision: Option<u64>,
    /// Connected pads with their exact outlines, built once per revision,
    /// for the clearance checks.
    copper: BoardCopper,
    /// The engine's copper graph of the cached revision: the checks'
    /// connectivity, the airwires and the drag preview read it.
    graph: Arc<CopperGraph>,
    airwires: Vec<(WorldPad, WorldPad)>,
    issues: Vec<pcbcraft_core::Finding>,
    board_pads: Vec<BoardPad>,
    graphics: Vec<PlacedGraphic>,
    /// Both ends of every track with its net and layer, per revision, for
    /// snapping layer changes and vias to track ends.
    track_ends: Vec<(Point, Id, CopperLayer)>,
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
    /// The staged width of the selected track, committed on focus loss.
    track_width: f64,
    /// The field that had keyboard focus last frame, so Esc reverts it.
    editing: Option<Field>,
    /// The staged values last sent, so a refused commit is not resent
    /// every frame while the field still shows it.
    sent: std::collections::BTreeMap<Field, [f64; 2]>,
    /// The Designator section's Offset being edited.
    text_offset: Point,
    /// Esc this frame, for the Placement card's sections below its own.
    card_escape: bool,
    errors: FieldErrors,
    /// The Board setup dialog while it is open.
    setup: Option<board_card::BoardSetup>,
    /// Place 3D part…: the part-file chooser in Place mode.
    part_chooser: crate::part_chooser::PartChooser,
    /// The 3D part section of a selected mechanical part.
    part_section: part_place::PartSection,
    /// The revision before a 3D part was placed: the new part is selected
    /// once the engine is past it.
    select_placed: Option<u64>,
    /// The board point at the centre of the Layout view, last drawn.
    view_centre: Option<Point>,
    /// The docked Stackup dialog (`stackup_panel`).
    stackup: Option<stackup_panel::StackupSetup>,
    /// The Design rules panel while it is open.
    rules: Option<rules_panel::RulesPanel>,
    /// The Production files dialog while it is open.
    production: Option<production::ProductionDialog>,
    /// The production folder kept for this project (the app's per-user
    /// settings); a new Production files dialog starts there.
    pub production_folder: Option<std::path::PathBuf>,
    /// A folder the dialog was just pointed at, for the app to keep.
    chosen_production_folder: Option<std::path::PathBuf>,
    /// Board bounds a Locate asked the next Layout frame to frame.
    frame_request: Option<[Point; 2]>,
    /// Bounds the next Layout frame shows whole, zooming out past the
    /// board's fit if need be (Update PCB's staged parts).
    overview_request: Option<[Point; 2]>,
    /// Instance whose schematic symbol the app should show next.
    show_in_schematic: Option<Id>,
    /// Every Connections row instead of the first eight.
    connections_all: bool,
    /// The Net classes card while it is open.
    net_classes: Option<net_classes::NetClassesCard>,
    /// The issue slot of the last inspected subject, per document revision.
    issue_slot: Option<((u64, usize, Inspected), Option<IssueSlot>)>,
    /// The selected copper fill.
    fill: Option<Id>,
    /// The fills' display copper, kept while their copper is unchanged.
    fill_meshes: fill_paint::FillCache,
    /// Fills whose copper is stale, per revision.
    stale_fills: BTreeSet<Id>,
    /// How Layout shows fill copper.
    fill_display: fill_paint::FillDisplay,
    /// How Layout shows the copper layers besides the active one.
    focus: LayerFocus,
    /// Board and footprint keepouts with flattened outlines, per revision.
    keepouts: Vec<layer_focus::KeepoutShape>,
    /// The Fill tool's outline in progress and settings.
    fill_draw: fill_tool::FillDraw,
    /// A handle of the selected fill's outline being dragged.
    fill_drag: Option<fill_tool::HandleDrag>,
    /// The Fill card's staged edit.
    fill_card: Option<fill_card::FillDraft>,
    /// The Board shape mode (`board_shape_ui`).
    shape: board_shape_ui::ShapeMode,
    /// The Placement card's Board edge fields (`edge_card`).
    edge: edge_card::EdgeEdit,
    /// The High speed card while it is open (`signal_card`).
    signals: Option<signal_card::SignalCard>,
    /// The Tune tool's settings and preview (`tune_tool`).
    tune: tune_tool::TuneState,
    /// The right-click menu and what it is about (`menu`).
    menu: crate::context_menu::State<menu::Target>,
    /// A registry command a menu row asked the app to run.
    menu_command: Option<&'static str>,
    /// The selected part dimension, shared by Layout, 3D and the
    /// Properties card (its part is selected with it).
    dimension: Option<Id>,
    /// The Dimension tool and the drawn part dimensions of both views
    /// (`dimensions`).
    dims: dimensions::Dimensions,
    /// Two or more selected objects; empty for a single selection
    /// (`group`).
    group: group::Group,
    /// The group's parts being dragged.
    group_drag: Option<group::GroupDrag>,
    /// Where a selection rectangle was started, on screen.
    box_from: Option<Pos2>,
    /// Which kinds a click, the rectangle and ⌘A may select.
    filter: group::SelectFilter,
    /// The Parts and Vias band under the canvas.
    table: parts_table::PartsTable,
    /// The one cross-highlight slot the table, Layout and 3D share.
    hover: parts_table::Hover,
    /// PCB 3D's last hover ray: the pointer and what it hit.
    hover_3d_at: Option<(Pos2, Option<group::Pick>)>,
}

impl Default for PcbEditor {
    fn default() -> Self {
        Self {
            trackpad: Default::default(),
            view_3d: false,
            tool: Tool::Select,
            instance: None,
            also: Vec::new(),
            track: None,
            via: None,
            net: None,
            highlighted: None,
            route_lit: None,
            layer: CopperLayer::Top,
            hover_at: None,
            project_name: Default::default(),
            stack: vec![CopperLayer::Top, CopperLayer::Bottom],
            width: 0.3,
            width_from: None,
            route_start: None,
            pending_via: None,
            route: Default::default(),
            shoved: Default::default(),
            via_tented: None,
            property_key: None,
            position: [0.0; 2],
            rotation: 0.0,
            placement_layer: Layer::Top,
            update: Default::default(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            drag: None,
            drag_position: None,
            track_drag: None,
            drag_airwires: None,
            show_airwires: true,
            cached_revision: None,
            copper: BoardCopper::default(),
            graph: Arc::default(),
            airwires: vec![],
            issues: vec![],
            board_pads: vec![],
            graphics: vec![],
            track_ends: vec![],
            outlined: BTreeSet::new(),
            labelled: BTreeSet::new(),
            layers: Layers::default(),
            view_layers: Default::default(),
            from_bottom: false,
            track_width: 0.0,
            editing: None,
            sent: Default::default(),
            text_offset: [0.0, 0.0],
            card_escape: false,
            errors: FieldErrors::default(),
            setup: None,
            part_chooser: Default::default(),
            part_section: Default::default(),
            select_placed: None,
            view_centre: None,
            stackup: None,
            rules: None,
            production: None,
            production_folder: None,
            chosen_production_folder: None,
            frame_request: None,
            overview_request: None,
            show_in_schematic: None,
            connections_all: false,
            net_classes: None,
            issue_slot: None,
            fill: None,
            fill_meshes: Default::default(),
            stale_fills: BTreeSet::new(),
            fill_display: Default::default(),
            focus: LayerFocus::All,
            keepouts: Vec::new(),
            fill_draw: Default::default(),
            fill_drag: None,
            fill_card: None,
            shape: Default::default(),
            edge: Default::default(),
            signals: None,
            tune: Default::default(),
            menu: Default::default(),
            menu_command: None,
            dimension: None,
            dims: Default::default(),
            group: Default::default(),
            group_drag: None,
            box_from: None,
            filter: Default::default(),
            table: Default::default(),
            hover: Default::default(),
            hover_3d_at: None,
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
        for (layer, _) in self.shared_layers() {
            let visible = view.visible(layer);
            let l = &mut self.layers;
            let flag = match layer {
                ViewLayer::Copper(copper) => l.copper_layer_mut(copper),
                ViewLayer::Silkscreen(Layer::Top) => Some(&mut l.silk_top),
                ViewLayer::Silkscreen(Layer::Bottom) => Some(&mut l.silk_bottom),
                ViewLayer::Vias => Some(&mut l.vias),
                _ => None,
            };
            if let Some(flag) = flag {
                *flag = visible;
            }
        }
        self.view_layers = view;
    }

    /// The toggles PCB 2D and 3D share: every copper layer, the vias and
    /// both silkscreens.
    fn shared_layers(&self) -> Vec<(crate::layer_view::ViewLayer, bool)> {
        use crate::layer_view::ViewLayer;
        let l = &self.layers;
        let inner = (1..=MAX_INNER).filter_map(|n| u8::try_from(n).ok());
        [CopperLayer::Top, CopperLayer::Bottom]
            .into_iter()
            .chain(inner.map(CopperLayer::Inner))
            .map(|c| (ViewLayer::Copper(c), l.copper_layer(c)))
            .chain([
                (ViewLayer::Vias, l.vias),
                (ViewLayer::Silkscreen(Layer::Top), l.silk_top),
                (ViewLayer::Silkscreen(Layer::Bottom), l.silk_bottom),
            ])
            .collect()
    }

    pub fn is_3d(&self) -> bool {
        self.view_3d
    }
    /// Switch between Layout and 3D. 3D has no 2D tools: an armed Route
    /// ends, so no 2D tool card shows there; selection is kept. The
    /// Dimension tool is both views': it stays on with its picks.
    pub fn set_3d(&mut self, enabled: bool) {
        self.view_3d = enabled;
        self.reset_route();
        if enabled && self.tool != Tool::Dimension {
            self.end_route();
        }
        self.dims.view_switched();
        self.cancel_drag();
    }
    pub fn selected_instance(&self) -> Option<Id> {
        self.instance
    }
    pub fn selected_track(&self) -> Option<Id> {
        self.track
    }
    pub fn selected_via(&self) -> Option<Id> {
        self.via
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
        self.also.clear();
        self.group = Group::default();
        // A dimension is selected with its part (`select_dimension`).
        self.dimension = None;
        self.track = track;
        self.via = None;
        self.fill = None;
        self.fill_drag = None;
        self.discard_staged();
    }

    /// Select a board via and light its net, as a 2D or 3D pick does.
    pub fn select_via(&mut self, via: Id, net: Option<Id>) {
        self.set_selection(None, None, net);
        self.via = Some(via);
    }

    /// Leave the Route tool for Select: the route in progress ends and the
    /// net highlight the route made goes with it.
    fn end_route(&mut self) {
        self.tool = Tool::Select;
        self.reset_route();
        self.route.ended = false;
        self.fill_draw.clear();
        if let Some(net) = self.route_lit.take()
            && self.highlighted == Some(net)
        {
            self.highlighted = None;
        }
    }

    /// The Route tool lights `net`; [`Self::end_route`] turns it off.
    fn route_highlight(&mut self, net: Option<Id>) {
        if self.highlighted != net {
            self.route_lit = net;
        }
        self.highlighted = net;
    }

    /// Whether a Locate asked the next frame to frame something.
    #[cfg(test)]
    pub fn has_frame_request(&self) -> bool {
        self.frame_request.is_some()
    }

    /// Whether `id` is selected, inspected or located with the inspected.
    #[cfg(test)]
    pub fn is_located(&self, id: Id) -> bool {
        self.is_selected(id)
    }

    /// Whether `id` is selected: the inspected part or one located with it.
    fn is_selected(&self, id: Id) -> bool {
        self.instance == Some(id) || self.also.contains(&id) || self.group.parts.contains(&id)
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
        // A route step the engine took continues the route; any other
        // change ends it.
        let revision = engine.revision();
        let continued = self.settle_route(engine.project(), revision)
            || self
                .cached_revision
                .is_some_and(|cached| self.route_revision(cached, revision) == cached);
        self.route
            .fill_stores
            .retain(|(_, after)| *after > revision);
        if self.cached_revision != Some(revision) {
            self.cancel_drag();
            if !continued {
                self.reset_route();
            }
            self.stack = engine.project().stackup.layers();
            if !self.stack.contains(&self.layer) {
                self.layer = CopperLayer::Top;
            }
            if self
                .via
                .is_some_and(|id| !engine.project().vias.iter().any(|v| v.id == id))
            {
                self.via = None;
            }
            self.fill_meshes.update(engine.project());
            self.stale_fills = layout::fill::status(engine.project())
                .into_iter()
                .filter(|s| s.stale)
                .map(|s| s.fill)
                .collect();
            self.fill_drag = None;
            self.fill_draw.clear();
            if self
                .fill
                .is_some_and(|id| engine.project().fill(id).is_none())
            {
                self.fill = None;
            }
            self.copper = BoardCopper::of(engine.project());
            self.graph = engine.copper_graph();
            self.airwires = self.graph.airwires();
            self.issues = self.copper.checks(engine.project(), &self.graph);
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
            self.prune_group(engine.project());
            if self
                .net
                .is_some_and(|id| engine.project().net(id).is_none())
            {
                self.net = None;
                self.highlighted = None;
                self.reset_route();
            }
        }
        let key = (
            engine.revision(),
            self.instance.or(self.track).or(self.via).or(self.fill),
        );
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
                self.text_offset = engine
                    .project()
                    .instance(p.instance)
                    .and_then(|instance| {
                        let text = target_text(engine.project(), p.instance)?;
                        instance.text_placements.get(&text).map(|t| t.offset)
                    })
                    .unwrap_or_default();
            }
            self.property_key = Some(key);
        }
        self.select_added_dimension(engine.project());
        self.sync_shape(engine);
    }

    /// The copper graph of `project` at `revision`: the engine's, which the
    /// editor holds for its cached revision, else built for it.
    pub(super) fn revision_graph(&self, project: &Project, revision: u64) -> Arc<CopperGraph> {
        match self.cached_revision {
            Some(r) if r == revision => Arc::clone(&self.graph),
            _ => Arc::new(CopperGraph::of(project)),
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
        self.keepouts = layer_focus::keepout_shapes(project);
        self.track_ends = project
            .tracks
            .iter()
            .flat_map(|t| [(t.start, t.net, t.layer), (t.end, t.net, t.layer)])
            .collect();
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
        self.group_drag = None;
        self.box_from = None;
        self.track_drag = None;
        self.drag_position = None;
        self.drag_airwires = None;
    }

    /// Recompute only when the snapped position changes. The revision's
    /// copper graph, with only the dragged part's pads joined again at
    /// their moved outlines, detects newly detached routed pads as well as
    /// moving existing airwire endpoints.
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
            connections: self.graph.carried_airwires(placement.instance, delta),
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
        if self.shape.active && !self.view_3d {
            return Ribbon {
                tabs,
                groups: self.shape_groups(),
                notice: None,
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
                        Button::new("via", "circle", "Via")
                            .enabled(!self.view_3d)
                            .selected(self.tool == Tool::Via)
                            .tip("Place a through via on a net; switch to Layout to place"),
                        Button::new("tune", "dimension", "Tune")
                            .enabled(!self.view_3d && !engine.project().tracks.is_empty())
                            .selected(self.tool == Tool::Tune)
                            .tip("Meander a track to its length target; switch to Layout to tune"),
                        Button::new("dimension", "dimension", "Dimension")
                            .selected(self.tool == Tool::Dimension)
                            .tip("Dimension a part to a face, edge, hole or board edge"),
                        Button::new("board-shape", "sketch", "Shape")
                            .enabled(!self.view_3d)
                            .tip("Edit the board outline, cutouts and named edges as a sketch"),
                        Button::new("place-part", "component", "3D part")
                            .selected(self.part_chooser.is_open())
                            .tip("Place a parametric 3D part (nut, spacer, heatsink) from a part file"),
                    ],
                },
                Group {
                    label: "Fills",
                    tools: vec![
                        Button::new("fill", "polygon", "Fill")
                            .enabled(!self.view_3d)
                            .selected(self.tool == Tool::Fill)
                            .tip("Draw a copper fill outline; switch to Layout to draw"),
                        Button::new("refill", "update", "Refill")
                            .enabled(!engine.project().fills.is_empty())
                            .tip("Compute the copper of every fill from the board as it is now"),
                    ],
                },
                Group {
                    label: "Output",
                    tools: vec![
                        Button::new("production", "export", "Production")
                            .selected(self.production.is_some())
                            .tip("Check the board and write Gerber, drill, assembly and drawing files"),
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
        if let Some(operation) = self.shape_tool(tool, engine) {
            return operation;
        }
        match tool {
            "update" => {
                self.begin_update_pcb(engine);
                self.end_route();
                self.cancel_drag();
            }
            "select" => self.end_route(),
            "dimension" => {
                self.end_route();
                self.arm_dimension(true);
            }
            "route" => {
                if self.tool != Tool::Route {
                    self.reset_route();
                    self.route.ended = false;
                }
                self.tool = Tool::Route;
            }
            "via" => {
                self.end_route();
                self.tool = Tool::Via;
            }
            "tune" => {
                self.end_route();
                self.tool = Tool::Tune;
            }
            "fill" => {
                self.end_route();
                self.tool = Tool::Fill;
                if self.fill_draw.net.is_none() {
                    self.fill_draw.net = self.highlighted;
                }
            }
            "production" => self.open_production(),
            "place-part" => self.open_part_chooser(),
            "refill" if !engine.project().fills.is_empty() => {
                return Some(Operation::Fill(layout::fill::FillCommand::Refill {
                    target: layout::fill::RefillTarget::All,
                }));
            }
            _ => {}
        }
        None
    }

    /// What the Properties card inspects.
    fn inspected(&self, project: &Project) -> Inspected {
        if self.update.is_open() {
            return Inspected::Review;
        }
        if !self.group.is_empty() {
            return Inspected::Group;
        }
        if let Some(id) = self.instance.filter(|id| project.instance(*id).is_some()) {
            return if project.placements.iter().any(|p| p.instance == id) {
                Inspected::Placement(id)
            } else {
                Inspected::Unplaced(id)
            };
        }
        if let Some(id) = self
            .via
            .filter(|id| project.vias.iter().any(|v| v.id == *id))
        {
            return Inspected::Via(id);
        }
        if let Some(id) = self.fill.filter(|id| project.fill(*id).is_some()) {
            return Inspected::Fill(id);
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
                // "R13 · 10 kΩ · R0603": designator, shown value, package.
                let designator = project.instance(id).map_or_else(String::new, |i| {
                    let value = instances::effective_fields(&project.library, i)
                        .unwrap_or_default()
                        .into_iter()
                        .find(|f| f.show_on_symbol)
                        .and_then(|f| f.text);
                    let package = instances::resolve(&project.library, i)
                        .ok()
                        .and_then(|r| r.physical)
                        .map(|p| p.package.meta.name.clone())
                        .filter(|n| !n.trim().is_empty());
                    std::iter::once(i.designator.clone())
                        .chain(value)
                        .chain(package)
                        .collect::<Vec<_>>()
                        .join(" · ")
                });
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
            Inspected::Via(id) => {
                let net = project
                    .vias
                    .iter()
                    .find(|v| v.id == id)
                    .map_or("", |v| project.net_name(v.net));
                Header::new("circle", format!("Via · {net}"), "Via")
            }
            Inspected::Fill(id) => {
                let title = project.fill(id).map_or_else(String::new, |f| {
                    let net = f.net.map_or("No net", |n| project.net_name(n));
                    if f.name.is_empty() {
                        format!("Fill · {net} · {}", f.layer.name())
                    } else {
                        format!("{} · {net}", f.name)
                    }
                });
                Header::new("polygon", title, "Fill")
            }
            Inspected::Board => Header::new("plane", "Board", "Board"),
            Inspected::Group => Header::new(
                "select",
                format!("{} objects", self.group.len()),
                "Selection",
            ),
        }
    }

    /// What the subject slot shows: the update review, the Board setup
    /// dialog, the Route tool card while Route is armed in Layout, or the
    /// Properties card (docs/sidebar-ia.md, "Layout").
    pub fn subject(&self) -> Subject {
        if self.update.is_open() {
            Subject::Review
        } else if let Some(subject) = self.production_subject() {
            subject
        } else if let Some(subject) = self.rules_subject() {
            subject
        } else if self.setup.is_some()
            || self.stackup.is_some()
            || self.net_classes.is_some()
            || self.signals.is_some()
        {
            Subject::Dialog
        } else if ((self.shape.active
            || matches!(self.tool, Tool::Route | Tool::Via | Tool::Fill | Tool::Tune))
            && !self.view_3d)
            || self.tool == Tool::Dimension
        {
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
            subject: egui::Id::unique(("pcb-card", inspected)),
            footer: matches!(
                inspected,
                Inspected::Placement(_)
                    | Inspected::Track(_)
                    | Inspected::Via(_)
                    | Inspected::Fill(_)
                    | Inspected::Group
            ),
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
        if let Some(operation) = self.part_chooser_frame(ui.ctx(), engine) {
            return CardResponse::output(Some(operation));
        }
        match self.subject() {
            Subject::Dialog if self.production.is_some() && !self.update.is_open() => {
                self.production_slot(ui, engine)
            }
            Subject::Dialog | Subject::Review if self.rules.is_some() && !self.update.is_open() => {
                self.rules_slot(ui, engine)
            }
            Subject::Tool if self.tool == Tool::Dimension => {
                CardResponse::output(self.dimension_card(ui))
            }
            Subject::Tool if self.shape.active => CardResponse::output(self.shape_card(ui, engine)),
            Subject::Tool if self.tool == Tool::Via => {
                CardResponse::output(self.via_tool_card(ui, engine.project()))
            }
            Subject::Tool if self.tool == Tool::Fill => {
                CardResponse::output(self.fill_tool_card(ui, engine.project()))
            }
            Subject::Tool if self.tool == Tool::Tune => {
                self.tune_card(ui, engine.project(), engine.revision());
                CardResponse::output(None)
            }
            Subject::Tool => CardResponse::output(self.route_card(ui, engine.project())),
            Subject::Dialog if self.stackup.is_some() => {
                CardResponse::output(self.stackup_setup(ui, engine))
            }
            Subject::Dialog if self.signals.is_some() => self.signals_card(ui, engine),
            Subject::Dialog if self.setup.is_none() => self.net_classes_card(ui, engine),
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
        if let Some(stackup) = &mut self.stackup {
            stackup.claim(ctx);
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

    /// Open the Update PCB review and frame the board with the parts it
    /// stages beside it, so the preview shows where they go.
    pub(super) fn begin_update_pcb(&mut self, engine: &Engine) {
        self.update.begin(engine, Change::SyncPcb {});
        // The review stages parts beside the board on the Layout canvas;
        // the 3D view (and its camera) stays as it was for later.
        if self.update.is_open() && self.view_3d {
            self.set_3d(false);
        }
        let board = engine.project().board_size;
        let ghosts = self.update.marks().map(|m| m.ghosts).unwrap_or_default();
        if !ghosts.is_empty() {
            let reach = pcbcraft_core::layout::staging::BOARD_MARGIN_MM;
            let (lo, hi) = ghosts.iter().fold(([0.0_f64, 0.0], board), |(lo, hi), g| {
                let p = g.position;
                (
                    [lo[0].min(p[0] - reach), lo[1].min(p[1] - reach)],
                    [hi[0].max(p[0] + reach), hi[1].max(p[1] + reach)],
                )
            });
            self.overview_request = Some([lo, hi]);
        }
    }

    /// Whether an update review is open.
    #[cfg(test)]
    pub(crate) fn reviewing(&self) -> bool {
        self.update.is_open()
    }

    /// Return the layout view to its fitted scale.
    pub fn fit(&mut self) {
        self.zoom = 1.0;
        self.pan = Vec2::ZERO;
    }

    /// The docked update review, for the app's Esc.
    pub fn review_mut(&mut self) -> &mut crate::update_preview::UpdatePreview {
        &mut self.update
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
                let moved = self.placement_section(ui, project, placement);
                let text = self.text_section(ui, project, id);
                let part = self.part_section(ui, engine, id);
                moved.or(text).or(part)
            }
            Inspected::Unplaced(_) => {
                self.unplaced_section(ui, engine);
                None
            }
            Inspected::Track(id) => {
                let track = project.tracks.iter().find(|t| t.id == id)?;
                self.track_section(ui, project, track)
            }
            Inspected::Via(id) => {
                let via = project.vias.iter().find(|v| v.id == id)?;
                self.via_section(ui, project, via)
            }
            Inspected::Fill(id) => {
                let fill = project.fill(id)?;
                self.fill_section(ui, project, engine.revision(), fill)
            }
            Inspected::Group => self.group_section(ui, project),
            Inspected::Board | Inspected::Review => {
                let renamed = self.board_section(ui, project);
                renamed.or(self.board_shape_section(ui, engine))
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
                self.placement_footer(ui, project, placement)
            }
            Inspected::Track(id) => {
                let track = project.tracks.iter().find(|t| t.id == id)?;
                track_card::track_footer(ui, track)
            }
            Inspected::Via(id) => {
                let via = project.vias.iter().find(|v| v.id == id)?;
                via_card::via_footer(ui, via)
            }
            Inspected::Fill(id) => fill_card::fill_footer(ui, project.fill(id)?),
            Inspected::Group => self.group_footer(ui, project),
            _ => None,
        }
    }

    /// Delete or Backspace with the Select tool: the selected track, via or
    /// fill is deleted by the command its card's Delete button sends, one
    /// undo step (the engine refuses a locked one). `keys::pressed` never
    /// takes a plain key from a focused text field.
    /// Whether a selection is there that the delete key removes.
    pub fn can_delete(&self, project: &Project) -> bool {
        if self.view_3d {
            return false;
        }
        if self.shape.active {
            return self.shape.tool == board_shape::ShapeTool::Select
                && !self.shape.selection.is_empty();
        }
        if self.dimension.is_some() && matches!(self.tool, Tool::Select | Tool::Dimension) {
            return true;
        }
        self.tool == Tool::Select
            && match self.inspected(project) {
                Inspected::Track(_) | Inspected::Via(_) | Inspected::Fill(_) => true,
                Inspected::Group => self.delete_group().is_some(),
                _ => false,
            }
    }

    fn delete_key(&mut self, ui: &Ui, project: &Project) -> Option<Operation> {
        // A selected part dimension first: its part is selected with it.
        if self.dimension.is_some()
            && matches!(self.tool, Tool::Select | Tool::Dimension)
            && crate::keys::pressed(ui.ctx(), "pcb.delete")
        {
            return self.layout_dimension_delete();
        }
        if self.tool != Tool::Select {
            return None;
        }
        let operation = match self.inspected(project) {
            Inspected::Track(id) => Operation::Layout(LayoutCommand::DeleteTrack { id }),
            Inspected::Via(id) => Operation::Layout(LayoutCommand::DeleteVia { id }),
            Inspected::Fill(id) => Operation::Fill(layout::fill::FillCommand::Delete { id }),
            Inspected::Group => self.delete_group()?,
            _ => return None,
        };
        crate::keys::pressed(ui.ctx(), "pcb.delete").then_some(operation)
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
        if self.shape.active {
            return self.shape_canvas(ui, engine);
        }
        let project = engine.project();
        let t = Tokens::get();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        // Named for screen readers and UI automation, which find the board
        // by this name and its bounds.
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Board"));
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        self.layout_dimension_start(ui, engine);
        if response.dragged_by(PointerButton::Middle) {
            self.pan += response.drag_delta();
        }
        // The wheel, or a pinch (and Cmd/Ctrl+wheel), zooms about the
        // hovered board point: it stays under the cursor. Two fingers pan.
        let wheel = self.trackpad.read(ui, false);
        if response.contains_pointer() && wheel.moves() {
            self.pan += wheel.pan;
            let zoom =
                (self.zoom * wheel.scale_factor()).clamp(*ZOOM_RANGE.start(), *ZOOM_RANGE.end());
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
        if let Some(bounds) = self.overview_request.take() {
            (self.zoom, self.pan) = overview(rect, project.board_size, bounds, self.from_bottom);
        }
        // A table row's Locate: bring the object into view at this zoom.
        if let Some(at) = self.table.reveal.take() {
            let seen = board_view(
                rect,
                project.board_size,
                self.zoom,
                self.pan,
                self.from_bottom,
            );
            let shown = seen.screen(at);
            if !rect.shrink(REVEAL_MARGIN).contains(shown) {
                self.pan += rect.center() - shown;
            }
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
        self.view_centre = Some(world(rect.center()));
        let painter = ui.painter().with_clip_rect(rect);
        let metric = symbol_view::metric(&painter);
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.hover_pos()));
        let cursor = pointer.map(world);
        // Strokes and text pick within a few pixels.
        let tolerance = f64::from(4.0 / scale);
        // The Fill tool's keys: Enter may close its outline into a fill.
        let (closed, fill_escaped) = if response.hovered() {
            self.fill_keys(ui)
        } else {
            (None, false)
        };
        let mut operation = closed;
        if self.route_start.is_some() {
            self.route.ended = false;
        }
        if response.hovered() && !fill_escaped {
            let typing = ui.ctx().egui_wants_keyboard_input();
            let end =
                self.tool == Tool::Route && !typing && crate::keys::pressed(ui.ctx(), "route.end");
            if self.tool == Tool::Dimension {
                // The Dimension tool's Esc: the chooser, the picks, then
                // the tool.
                if crate::keys::pressed(ui.ctx(), "pcb.cancel") {
                    self.layout_dimension_cancel();
                }
            } else if end && (self.route_start.is_some() || self.route.ended) {
                // The first Esc ends the route, or follows one that has
                // just finished, and keeps Route; the next leaves it.
                self.finish_route();
                self.route.ended = false;
            } else if end || crate::keys::pressed(ui.ctx(), "pcb.cancel") {
                // With Select and nothing under way, Esc clears the
                // selection; otherwise it ends the tool or gesture first.
                let idle = self.tool == Tool::Select
                    && self.drag.is_none()
                    && self.track_drag.is_none()
                    && self.fill_drag.is_none();
                if idle {
                    self.set_selection(None, None, None);
                }
                self.end_route();
                self.cancel_drag();
                self.discard_staged();
            }
        }
        // The Route keys (`route_tool::ROUTE_KEYS`). A layer change during
        // a route puts a via at the pointer, which the next click routes
        // through.
        self.hover_at = None;
        if self.tool == Tool::Route && response.hovered() {
            let at = cursor.map(|at| self.route_point(project, at, scale));
            self.hover_at = at;
            if let Some(back) = self.route_keys(ui, project, engine.revision(), at) {
                operation = Some(back);
            }
        }
        // The Tune tool: the segment under the pointer and its keys.
        if self.tool == Tool::Tune {
            self.tune_hover(project, cursor.filter(|_| response.hovered()), tolerance);
            if response.hovered() {
                self.tune_keys(ui, project);
            }
        }
        if self.tool == Tool::Route && response.clicked_by(PointerButton::Secondary) {
            self.finish_route();
        }
        kit::tool_cursor(ui, &response, self.tool != Tool::Select);
        let double = self.tool == Tool::Route && response.double_clicked();
        if double {
            self.finish_route();
        }
        // A handle of the selected fill's outline drags the handle.
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.drag_started_by(PointerButton::Primary)
            && let Some(start) = ui.input(|i| i.pointer.press_origin()).or(pointer)
            && let Some((fill, handle)) = self.fill_handle_at(project, view, start)
        {
            self.fill_drag = Some(fill_tool::HandleDrag {
                fill,
                handle,
                to: world(start),
            });
        }
        if let (Some(drag), Some(at)) = (&mut self.fill_drag, cursor) {
            drag.to = snap(at);
        }
        if response.drag_stopped_by(PointerButton::Primary)
            && let Some(drag) = self.fill_drag.take()
            && let Some(fill) = project.fill(drag.fill)
            && let Some(outline) = drag.preview(fill)
        {
            operation = PcbEditor::fill_outline_edit(fill, outline);
        }
        // Double-clicking a vertex of the selected fill removes it.
        if self.tool == Tool::Select
            && response.double_clicked()
            && let Some(at) = pointer
            && let Some((id, fill_tool::Handle::Vertex(i))) = self.fill_handle_at(project, view, at)
            && let Some(fill) = project.fill(id)
            && let Some(outline) = fill_tool::without_vertex(&fill.outline, i)
        {
            operation = PcbEditor::fill_outline_edit(fill, outline);
        }
        if self.tool == Tool::Select
            && self.fill_drag.is_none()
            && !self.update.is_open()
            && response.drag_started_by(PointerButton::Primary)
        {
            let start = ui.input(|i| i.pointer.press_origin()).or(pointer);
            if let Some(start) = start {
                let at = world(start);
                // A track under the press drags with shove (`track_drag`).
                let hit = self.hit_at(project, at, tolerance, metric);
                let part = self.pick_instance(project, at, tolerance, metric);
                let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
                // ⇧ or ⌘, or a press on nothing a drag moves, draws a
                // selection rectangle; a press on a part of the group
                // drags the group's parts.
                let movable = matches!(hit, menu::Hit::Track(..)) || part.is_some();
                if add || !movable {
                    self.box_from = Some(start);
                } else if let Some(id) = part.filter(|id| self.group.parts.contains(id)) {
                    if let Err(why) = self.start_group_drag(project, at) {
                        let name = project.instance(id).map_or("", |i| i.designator.as_str());
                        crate::kit::toast::post(
                            ui.ctx(),
                            kit::severity::Level::Warning,
                            format!("Can't move the selection with {name}: {why}"),
                            Vec::new(),
                        );
                    }
                } else if let menu::Hit::Track(id, _) = hit
                    && let Some(drag) = track_drag::TrackDrag::of(project, id, at)
                {
                    self.select_hit(hit);
                    self.track_drag = Some(drag);
                } else if let Some(id) = self.pick_instance(project, at, tolerance, metric) {
                    self.set_selection(Some(id), None, None);
                    // A locked part, or one its dimensions hold in place,
                    // does not follow the pointer: the engine says why, in
                    // the words it would refuse the move with, before
                    // anything moves.
                    if let Some(error) = layout::move_refusal(project, id, self.shape.evaluator) {
                        crate::kit::toast::post(
                            ui.ctx(),
                            refusal::level(&error),
                            refusal::shown_error(&error),
                            Vec::new(),
                        );
                    } else if let Some(placement) =
                        project.placements.iter().find(|p| p.instance == id)
                    {
                        self.drag = Some((placement.clone(), at));
                    }
                }
            }
        }
        if let (Some(drag), Some(at)) = (&mut self.group_drag, cursor) {
            drag.by = snap([at[0] - drag.grab[0], at[1] - drag.grab[1]]);
        }
        if response.drag_stopped_by(PointerButton::Primary) && self.group_drag.is_some() {
            operation = self.finish_group_drag();
        }
        let dragged_box = self
            .box_from
            .zip(ui.input(|i| i.pointer.latest_pos()))
            .map(|(from, to)| solidcraft_ui_egui::selection::BoxSel::from_drag(from, to));
        if response.drag_stopped_by(PointerButton::Primary)
            && let Some(bx) = &dragged_box
        {
            self.box_from = None;
            let picked = self.box_pick(project, view, bx);
            let add = ui.input(|i| i.modifiers.shift || i.modifiers.command);
            self.finish_box(project, picked, add);
        }
        if let Some(at) = cursor {
            let alone = ui.input(|i| i.modifiers.alt);
            self.plan_track_drag(project, engine.revision(), at, alone);
        }
        if response.drag_stopped_by(PointerButton::Primary) && self.track_drag.is_some() {
            operation = self.finish_track_drag(ui.ctx(), engine.revision());
        }
        if let (Some((placement, start)), Some(at)) = (&self.drag, cursor) {
            let to = snap([
                placement.position[0] + at[0] - start[0],
                placement.position[1] + at[1] - start[1],
            ]);
            // A dimensioned part slides along what holds it: the preview is
            // where the engine's move will land it.
            let landed = self.shape.evaluator.and_then(|ev| {
                layout::dimensions::landing(
                    project,
                    placement.instance,
                    layout::dimensions::PartPose {
                        position: to,
                        rotation_deg: placement.rotation_deg,
                    },
                    ev,
                )
            });
            self.drag_position = Some(match landed {
                Some(Ok(pose)) => pose.position,
                _ => to,
            });
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
        // A right-click with Select opens the menu of what a click there
        // picks, selecting it first unless it is selected.
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.clicked_by(PointerButton::Secondary)
            && let (Some(at), Some(p)) = (cursor, pointer)
        {
            let hit = self.hit_at(project, at, tolerance, metric);
            let target = self.menu_select(hit);
            self.menu.open(p, target);
        }
        if response.clicked_by(PointerButton::Primary)
            && let Some(at) = cursor
        {
            if self.tool == Tool::Via {
                operation = self.place_via(project, at, scale);
            } else if self.tool == Tool::Fill {
                operation = self.fill_click(view, at);
            } else if self.tool == Tool::Tune {
                operation = self.tune_click(ui.ctx(), project, engine.revision());
            } else if self.tool == Tool::Route {
                if !double {
                    operation = self.route_click(project, engine.revision(), at, scale);
                    if let Some(why) = self.route.refused.take() {
                        kit::toast::post(ui.ctx(), kit::severity::Level::Warning, why, Vec::new());
                    }
                }
            } else if self.layout_dimension_click(ui, project, &view, screen(at)) {
                // The Dimension tool's pick, or a click on a drawn
                // dimension: it selects it, a double-click opens its value
                // (shared with 3D).
            } else if ui.input(|i| i.modifiers.shift || i.modifiers.command) {
                // ⇧ or ⌘ adds what is under the pointer, or takes it out.
                let hit = self.hit_at(project, at, tolerance, metric);
                self.toggle_hit(hit, project);
            } else if self.fill_handle_at(project, view, screen(at)).is_none() {
                // A click on a handle of the selected fill keeps it
                // selected, so a double-click there can remove the vertex.
                self.select_at(project, at, tolerance, metric);
            }
        }
        self.refresh_drag_airwires(engine);
        // The route step under the pointer, planned before the board is
        // drawn so the copper it shoves is drawn at its new place only.
        let route_end = match (self.route_start, cursor) {
            (Some(_), Some(at)) => Some(
                self.route_pad(project, at, scale)
                    .map(PlacedPad::center)
                    .unwrap_or_else(|| self.route_destination(project, at, scale)),
            ),
            _ => None,
        };
        if let Some(end) = route_end {
            let _ = self.route_plan(project, engine.revision(), end);
        }
        self.shoved = self
            .track_drag_moved(engine.revision())
            .unwrap_or_else(|| self.route_moved(engine.revision()));
        self.paint_board(&painter, view, project);
        if let Some(bx) = &dragged_box {
            crate::schematic_editor::paint_box(&painter, bx.clone());
        }
        self.paint_fill_handles(&painter, view, project);
        self.paint_fill_draw(&painter, view, cursor);
        hold_mark::paint_marks(&painter, &view, project);
        // Part dimensions, the body traces and the Dimension tool, as in
        // 3D (`dimensions`).
        let hovered_part = match (self.tool, cursor) {
            (Tool::Select, Some(at)) if response.hovered() => {
                self.pick_instance(project, at, tolerance, metric)
            }
            _ => None,
        };
        self.hover_board(hovered_part.map(group::Pick::Part));
        self.paint_cross_highlight(ui, &painter, view, project);
        let shown_pointer = pointer.filter(|_| response.hovered());
        if let Some(dimensioned) =
            self.layout_dimensions(ui, &painter, &view, engine, shown_pointer, hovered_part)
        {
            operation = Some(dimensioned);
        }
        // Slice R1: the docked review's canvas preview.
        if let Some(marks) = self.update.marks() {
            crate::update_preview::paint_board(
                &painter,
                project,
                &marks,
                screen,
                |id| {
                    let placement = project.placements.iter().find(|p| p.instance == id)?;
                    body_polygon(project, placement)
                },
                |placement| body_polygon(project, placement),
            );
        }
        if let Some(end) = route_end {
            self.paint_route_preview(&painter, view, project, end);
        }
        if let Some(at) = cursor {
            self.paint_track_drag(&painter, view, project, at);
        }
        if self.tool == Tool::Tune {
            self.paint_tune_preview(&painter, view, project, engine.revision(), pointer);
        }
        if self.tool == Tool::Via
            && let Some(at) = cursor
            && response.hovered()
        {
            let at = self.via_point(at, scale);
            painter.circle_stroke(
                screen(at),
                (self.class_via_size(project).1 as f32 * scale / 2.0).max(4.0),
                Stroke::new(1.5, t.body_selected),
            );
        }
        let focus = crate::keys::labelled(ui.ctx(), self.focus.label(), "pcb.layer-focus");
        kit::canvas_hint(
            &painter,
            rect,
            t.space_16,
            &format!(
                "{} · {} copper · {focus} · grid 0.5 mm · {} airwires{}",
                match self.tool {
                    Tool::Route => "Route",
                    Tool::Via => "Via",
                    Tool::Fill => "Fill (A arcs, Enter closes)",
                    Tool::Tune => "Tune",
                    Tool::Dimension => "Dimension",
                    Tool::Select if self.dragging_pair_half(project) => {
                        "Drag the pair (⌥: this half alone)"
                    }
                    Tool::Select => "Select / move",
                },
                self.layer.name(),
                self.visible_airwires().len(),
                if self.from_bottom {
                    " · viewed from bottom"
                } else {
                    ""
                }
            ),
        );
        if operation.is_none() {
            operation = self.delete_key(ui, project);
        }
        self.pills(ui, rect, project);
        if let Some(menu) = self.show_menu(ui.ctx(), engine) {
            operation = Some(menu);
        }
        operation
    }

    /// The text of the canvas pill: the route in progress (its net, width,
    /// layer, posture and router mode, each with the key that steps it),
    /// else the highlighted net; `None` without either. A pair route names
    /// the pair and the width both halves are routed at, which W does not
    /// step.
    fn pill_text(&self, ctx: &egui::Context, project: &Project) -> Option<String> {
        match (self.route_start, self.net, self.highlighted) {
            (Some(_), Some(net), _) => {
                let pair = self
                    .route
                    .pair
                    .as_ref()
                    .and_then(|p| project.rules.diff_pair(p.pair))
                    .zip(self.pair_width_gap(project));
                let (name, width) = match pair {
                    Some((pair, (width, _))) => {
                        (format!("pair {}", pair.name), impedance_view::mm(width))
                    }
                    None => (
                        project.net_name(net).to_owned(),
                        crate::keys::labelled(
                            ctx,
                            &format!("{} mm", kit::number_text(self.width)),
                            "route.width-next",
                        ),
                    ),
                };
                Some(format!(
                    "Routing {name} · {width} · {} · {} · {}",
                    self.layer_label(ctx),
                    crate::keys::labelled(ctx, self.route.posture.label(), "route.posture"),
                    crate::keys::labelled(ctx, self.route.mode.label(), "route.router-mode-next"),
                ))
            }
            (None, _, Some(net)) if project.net(net).is_some() => {
                Some(format!("{} highlighted", project.net_name(net)))
            }
            _ => None,
        }
    }

    /// Pills for the persistent modes, top-centre of the canvas: the route
    /// in progress, else the highlighted net. × ends the mode.
    fn pills(&mut self, ui: &mut Ui, rect: Rect, project: &Project) {
        let t = Tokens::get();
        let Some(text) = self.pill_text(ui.ctx(), project) else {
            return;
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
                self.finish_route();
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
        let seen = self.seen();
        // During a route, a pad of the routed net beats a nearer one of
        // another net.
        let routed = self.route_start.and(self.net);
        let other = |p: &PlacedPad| routed.is_some_and(|n| p.net != Some(n));
        self.board_pads
            .iter()
            .filter(|b| {
                let p = &b.placed;
                p.net.is_some() && p.on_copper(self.layer) && seen.pad(p)
            })
            .map(|b| {
                (
                    b,
                    b.outline.distance(at, true),
                    pcbcraft_core::geometry::distance(b.placed.center(), at),
                )
            })
            .filter(|(_, edge, _)| *edge < reach)
            .min_by(|a, b| {
                other(&a.0.placed)
                    .cmp(&other(&b.0.placed))
                    .then(a.1.total_cmp(&b.1))
                    .then(a.2.total_cmp(&b.2))
            })
            .map(|(b, _, _)| &b.placed)
    }

    /// The pad a route step at `at` snaps to: not another net's pad while
    /// copper of the routed net is just as close.
    fn route_pad(&self, project: &Project, at: Point, scale: f32) -> Option<&PlacedPad> {
        self.route_snap(at, scale).filter(|pad| {
            self.route_start.is_none()
                || pad.net == self.net
                || (self.track_end(at, scale, self.net, None).is_none()
                    && self.net_track_point(project, at, scale).is_none())
        })
    }

    /// Where a route click or V at `at` lands: the end of a track of the
    /// routed net within 8 pixels, else the nearest point of such a track
    /// on the routing layer during a route, else the 0.5 mm grid. A layer
    /// change at a track end continues that track through a via.
    fn route_point(&self, project: &Project, at: Point, scale: f32) -> Point {
        self.track_end(at, scale, self.net, None)
            .or_else(|| self.net_track_point(project, at, scale))
            .unwrap_or_else(|| snap(at))
    }

    /// Where a route step to `at` ends: [`Self::route_point`], but only at
    /// the end of a track on the routing layer. A step ending on another
    /// layer's track end would leave its own end joining nothing.
    pub(super) fn route_destination(&self, project: &Project, at: Point, scale: f32) -> Point {
        self.track_end(at, scale, self.net, Some(self.layer))
            .or_else(|| self.net_track_point(project, at, scale))
            .unwrap_or_else(|| snap(at))
    }

    /// The nearest end of a visible track (of `net` and on `on`, if given)
    /// within 8 pixels of `at`.
    fn track_end(
        &self,
        at: Point,
        scale: f32,
        net: Option<Id>,
        on: Option<CopperLayer>,
    ) -> Option<Point> {
        let reach = f64::from(8.0 / scale);
        let seen = self.seen();
        self.track_ends
            .iter()
            .filter(|(end, track_net, layer)| {
                net.is_none_or(|n| n == *track_net)
                    && on.is_none_or(|l| l == *layer)
                    && seen.copper_layer(*layer)
                    && pcbcraft_core::geometry::distance(*end, at) < reach
            })
            .min_by(|a, b| {
                let d = |p: Point| pcbcraft_core::geometry::distance(p, at);
                d(a.0).total_cmp(&d(b.0))
            })
            .map(|(end, _, _)| *end)
    }

    /// Where the Via tool places a via for a click at `at`: a track end
    /// within 8 pixels, else the 0.5 mm grid.
    fn via_point(&self, at: Point, scale: f32) -> Point {
        self.track_end(at, scale, None, None)
            .unwrap_or_else(|| snap(at))
    }

    /// The via the Via tool places for a click at `at`: on the net of the
    /// pad, via or track under the pointer, else the chosen net. `None`
    /// without a net.
    fn place_via(&mut self, project: &Project, at: Point, scale: f32) -> Option<Operation> {
        let layers = self.seen();
        let position = self.via_point(at, scale);
        let under = self
            .pick_pad(at, |p| layers.pad(p))
            .and_then(|p| p.net)
            .or_else(|| {
                project
                    .tracks
                    .iter()
                    .rev()
                    .find(|t| {
                        layers.copper_layer(t.layer)
                            && (layout::track_contains(t, at, 0.0)
                                || t.start == position
                                || t.end == position)
                    })
                    .map(|t| t.net)
            });
        let net = under.or(self.net)?;
        self.net = Some(net);
        self.route_highlight(Some(net));
        Some(Operation::Layout(LayoutCommand::AddVia {
            via: layout::ViaRequest {
                tented: self.via_tented(project),
                ..layout::class_via(project, net, position)
            },
        }))
    }

    /// The topmost visible via whose pad is under `at`.
    fn pick_via<'a>(&self, project: &'a Project, at: Point) -> Option<&'a Via> {
        let seen = self.seen();
        project.vias.iter().rev().find(|via| {
            seen.via(via, &self.stack)
                && pcbcraft_core::geometry::distance(via.position, at) <= via.diameter / 2.0
        })
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
        let hit = self.hit_at(project, at, tolerance, metric);
        self.select_hit(hit);
    }

    /// What a click at `at` picks: a visible pad, via, track, part body,
    /// then fill. A track picks within `tolerance` (a few pixels) of its
    /// copper, so a thin one is easy to hit; the active layer's tracks
    /// come first, then the one whose copper is nearest, topmost on a tie.
    fn hit_at(
        &self,
        project: &Project,
        at: Point,
        tolerance: f64,
        metric: TextMetric,
    ) -> menu::Hit {
        use menu::Hit;
        let layers = self.seen();
        let track = project
            .tracks
            .iter()
            .rev()
            .filter(|track| {
                layers.copper_layer(track.layer) && layout::track_contains(track, at, tolerance)
            })
            .min_by(|a, b| {
                let gap = |t: &pcbcraft_core::Track| {
                    (pcbcraft_core::geometry::segment_distance(at, t.start, t.end) - t.width / 2.0)
                        .max(0.0)
                };
                (a.layer != self.layer)
                    .cmp(&(b.layer != self.layer))
                    .then(gap(a).total_cmp(&gap(b)))
            });
        // The selection filter leaves kinds out: a click passes through
        // them to what lies below.
        let f = self.filter;
        if let Some(pad) = self.pick_pad(at, |p| layers.pad(p)).filter(|_| f.parts) {
            Hit::Pad(pad.instance, pad.net)
        } else if let Some(via) = self.pick_via(project, at).filter(|_| f.vias) {
            Hit::Via(via.id, via.net)
        } else if let Some(track) = track.filter(|_| f.tracks) {
            Hit::Track(track.id, track.net)
        } else if let Some(instance) = self
            .pick_instance(project, at, tolerance, metric)
            .filter(|_| f.parts)
        {
            Hit::Part(instance)
        } else if let Some(fill) = self.pick_fill(project, at, tolerance).filter(|_| f.fills) {
            Hit::Fill(fill.id, fill.net)
        } else {
            Hit::Nothing
        }
    }

    /// Select what a 3D pick names, as a Layout click on the same object
    /// does ([`Self::select_hit`]): a pad selects its part and lights the
    /// pad's net (pads are not selected on their own in either view).
    pub fn select_picked(&mut self, source: Option<&pcbcraft_solidcraft::project::Source>) {
        use menu::Hit;
        let Some(s) = source else {
            return self.select_hit(Hit::Nothing);
        };
        match (s.instance, s.via, s.fill, s.track) {
            (Some(instance), ..) if s.pad.is_some() => self.select_hit(Hit::Pad(instance, s.net)),
            (None, Some(via), ..) => self.select_via(via, s.net),
            (_, _, Some(fill), _) => self.select_fill(fill, s.net),
            (Some(instance), ..) => self.select_hit(Hit::Part(instance)),
            (None, None, None, Some(track)) => self.set_selection(None, Some(track), s.net),
            _ => self.select_hit(Hit::Nothing),
        }
    }

    /// Select what `hit` picked (nothing clears the selection).
    fn select_hit(&mut self, hit: menu::Hit) {
        use menu::Hit;
        match hit {
            Hit::Pad(instance, net) => self.set_selection(Some(instance), None, net),
            Hit::Via(id, net) => self.select_via(id, Some(net)),
            Hit::Track(id, net) => self.set_selection(None, Some(id), Some(net)),
            Hit::Part(instance) => self.set_selection(Some(instance), None, None),
            Hit::Fill(id, net) => self.select_fill(id, net),
            Hit::Nothing => self.set_selection(None, None, None),
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
        let layers = self.seen();
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
        if let Some(drag) = &self.group_drag
            && drag.parts.iter().any(|p| p.instance == instance)
        {
            return drag.by;
        }
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
        if project.board_shape.is_rectangle() {
            let board = [
                [0.0, 0.0],
                [project.board_size[0], 0.0],
                project.board_size,
                [0.0, project.board_size[1]],
            ];
            painter.add(Shape::convex_polygon(
                board.map(screen).to_vec(),
                ink.board,
                Stroke::new(1.5, t.body_edge),
            ));
        } else {
            board_shape_ui::paint_solved(
                painter,
                &view,
                &project.board(),
                &ink,
                Stroke::new(1.5, t.body_edge),
            );
        }
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
        // Far side drawings dimmed underneath; then every copper layer but
        // the active one, farthest first and dimmed by the focus mode; the
        // near assembly drawing, the through-hole pads and the active
        // layer on top at full strength; the other layers' selected and
        // highlighted copper over it; vias, holes, keepouts, then the near
        // silkscreen and courtyard (docs/pcb-realism.md, PCB 2D).
        self.paint_drawings(painter, view, &ink, far, true, |_| true);
        let order = self.paint_order();
        let looks: Vec<(CopperLayer, Look)> = order
            .iter()
            .filter_map(|l| self.look(*l, near).map(|look| (*l, look)))
            .collect();
        for (layer, look) in looks.iter().filter(|(l, _)| *l != self.layer) {
            self.paint_layer(painter, view, project, *layer, *look, false);
        }
        let assembly = |l: BoardDrawing| matches!(l, BoardDrawing::Assembly(_));
        self.paint_drawings(painter, view, &ink, near, false, assembly);
        self.paint_through_pads(painter, view, near);
        if let Some((layer, look)) = looks.iter().find(|(l, _)| *l == self.layer) {
            self.paint_layer(painter, view, project, *layer, *look, false);
        }
        for (layer, look) in looks.iter().filter(|(l, _)| *l != self.layer) {
            self.paint_layer(painter, view, project, *layer, *look, true);
        }
        self.paint_vias(painter, view, project, near);
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
        self.paint_keepouts(painter, view);
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
            let selected = self.is_selected(placement.instance);
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
                    type_face::semibold(t.font_label),
                    color,
                );
            }
        }
        if scale > 7.0 {
            let seen = self.seen();
            for pad in &self.board_pads {
                if pad.placed.pad.number.is_empty() || !seen.pad(&pad.placed) {
                    continue;
                }
                // A number shows only inside its pad: fine-pitch pads (a
                // USB-C receptacle at fit zoom) would stack their numbers
                // over each other and the designator.
                let font = FontId::proportional(t.font_small);
                let galley =
                    painter.layout_no_wrap(pad.placed.pad.number.clone(), font, t.on_accent);
                let bounds = pad.outline.bounds(0.0);
                let width = (bounds.max[0] - bounds.min[0]) as f32 * scale;
                let height = (bounds.max[1] - bounds.min[1]) as f32 * scale;
                if galley.size().x > width || t.font_small * PAD_NUMBER_CAP > height {
                    continue;
                }
                let delta = self.drag_delta(pad.placed.instance);
                let c = pad.placed.center();
                let at = screen([c[0] + delta[0], c[1] + delta[1]]);
                painter.galley(at - galley.size() / 2.0, galley, t.on_accent);
            }
        }
    }

    /// The copper of one layer as `look` has it: its fills, tracks and
    /// surface pads; with `chosen` only its selected and highlighted
    /// copper (drawn again over the active layer).
    fn paint_layer(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        layer: CopperLayer,
        look: Look,
        chosen: bool,
    ) {
        let seen = self.seen();
        if !seen.copper_layer(layer) {
            return;
        }
        if !chosen {
            self.paint_fills(painter, view, project, layer, look);
        }
        self.paint_tracks(painter, view, project, layer, look.shade, chosen);
        let Some(side) = layer.side() else {
            return;
        };
        let ink = BoardInk::get();
        for pad in &self.board_pads {
            let placed = &pad.placed;
            if placed.pad.kind.through_hole() || placed.layer != side || !seen.pad(placed) {
                continue;
            }
            let selected = self.pad_chosen(placed);
            if chosen && !selected {
                continue;
            }
            let shape = view.shape(&pad.outline, self.drag_delta(placed.instance));
            symbol_view::paint_shape(
                painter,
                view.view,
                &shape,
                Some(ink.on_board(copper_color(side, selected), look.shade)),
                None,
            );
        }
    }

    /// Whether a pad is selected or on the highlighted net.
    fn pad_chosen(&self, placed: &PlacedPad) -> bool {
        self.is_selected(placed.instance)
            || (placed.net.is_some() && self.highlighted == placed.net)
    }

    /// The pads through every layer, in the active layer's colour (the
    /// near side's while it is hidden), and bare holes' keep-out lands.
    fn paint_through_pads(&self, painter: &egui::Painter, view: BoardView, near: Layer) {
        let ink = BoardInk::get();
        let colors = crate::palette::board();
        let seen = self.seen();
        let base = if seen.copper_layer(self.layer) {
            colors.copper_layer(self.layer)
        } else {
            colors.copper(near)
        };
        for pad in &self.board_pads {
            let placed = &pad.placed;
            if !placed.pad.kind.through_hole() || !seen.pad(placed) {
                continue;
            }
            let selected = self.pad_chosen(placed);
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
            let color = if selected { colors.selected } else { base };
            symbol_view::paint_shape(
                painter,
                view.view,
                &shape,
                Some(ink.on_board(color, 1.0)),
                None,
            );
        }
    }

    /// The tracks on one copper layer in its colour, `shade` of the way
    /// from the board; the selected track and the highlighted net in the
    /// selection colour (with `chosen`, only those). Each track is its
    /// round-capped copper outline ([`layout::track_shape`]) in an opaque
    /// colour, so joins at any angle are seamless and overlapping copper
    /// of a layer reads as one.
    fn paint_tracks(
        &self,
        painter: &egui::Painter,
        view: BoardView,
        project: &Project,
        layer: CopperLayer,
        shade: f32,
        chosen_only: bool,
    ) {
        if !self.seen().copper_layer(layer) {
            return;
        }
        let colors = crate::palette::board();
        let ink = BoardInk::get();
        let plain = ink.on_board(colors.copper_layer(layer), shade);
        let selected = ink.on_board(colors.selected, shade);
        for track in project
            .tracks
            .iter()
            .filter(|t| t.layer == layer && !self.shoved.0.contains(&t.id))
        {
            let chosen = self.track == Some(track.id)
                || self.group.tracks.contains(&track.id)
                || self.highlighted == Some(track.net);
            if chosen_only && !chosen {
                continue;
            }
            painter.add(Shape::convex_polygon(
                capsule(
                    view.screen(track.start),
                    view.screen(track.end),
                    track.width as f32 * view.view.scale / 2.0,
                ),
                if chosen { selected } else { plain },
                Stroke::NONE,
            ));
        }
    }

    /// Board vias: the pad ring and the drill. A via through the active
    /// layer has its ring in that layer's colour at full strength; any
    /// other in the colour of the first shown layer it joins, dimmed as
    /// that layer is. A tented via's ring shows under a mask tint with an
    /// outline; an open via's ring is solid with a bright finished edge.
    fn paint_vias(&self, painter: &egui::Painter, view: BoardView, project: &Project, near: Layer) {
        let colors = crate::palette::board();
        let ink = BoardInk::get();
        let scale = view.view.scale;
        let seen = self.seen();
        for via in &project.vias {
            if !seen.via(via, &self.stack) || self.shoved.1.contains(&via.id) {
                continue;
            }
            let selected = self.via == Some(via.id)
                || self.group.vias.contains(&via.id)
                || self.highlighted == Some(via.net);
            let joins = |l: CopperLayer| seen.copper_layer(l) && via.on(l, &self.stack);
            let (ring, share) = if selected {
                (colors.selected, 1.0)
            } else if joins(self.layer) {
                (colors.copper_layer(self.layer), 1.0)
            } else {
                let shown = self
                    .stack
                    .iter()
                    .copied()
                    .find(|l| joins(*l))
                    .unwrap_or(CopperLayer::from(near));
                let share = self.look(shown, near).map_or(1.0, |l| l.shade);
                (colors.copper_layer(shown), share)
            };
            let at = view.screen(via.position);
            let outer = (via.diameter as f32 * scale / 2.0).max(2.5);
            let drill = (via.drill as f32 * scale / 2.0).max(1.0).min(outer - 0.5);
            if via.tented {
                painter.circle(
                    at,
                    outer,
                    ink.on_board(ring, TENTED_SHADE * share),
                    Stroke::new(1.0, ink.on_board(ring, share)),
                );
            } else {
                painter.circle(
                    at,
                    outer,
                    ink.on_board(ring, share),
                    Stroke::new(1.0, ink.unplated),
                );
            }
            painter.circle(at, drill, ink.drill, Stroke::new(0.75, ink.plated));
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
            let mut color = if self.is_selected(graphic.instance) {
                selected_ink
            } else {
                match graphic.layer {
                    BoardDrawing::Silkscreen(_) => ink.silkscreen,
                    BoardDrawing::Courtyard(_) => ink.courtyard,
                    BoardDrawing::Assembly(_) => ink.assembly,
                }
            };
            if dim {
                color = ink.on_board(color, FAR_SHADE);
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
    /// The bare board, opaque: copper colours are mixed over it.
    board: Color32,
}

/// A tented via's ring under its mask.
const TENTED_SHADE: f32 = 0.55;

/// A pad number's digit height as a share of its font size: the number
/// shows when the pad is at least this tall on screen.
const PAD_NUMBER_CAP: f32 = 0.7;

/// `top` (premultiplied, maybe translucent) composited over opaque `under`.
fn over(under: Color32, top: Color32) -> Color32 {
    let keep = 1.0 - f32::from(top.a()) / 255.0;
    let mix = |t: u8, u: u8| {
        (f32::from(t) + f32::from(u) * keep)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(
        mix(top.r(), under.r()),
        mix(top.g(), under.g()),
        mix(top.b(), under.b()),
    )
}

/// The opaque colour `share` of the way from `under` to `color`.
fn mixed(under: Color32, color: Color32, share: f32) -> Color32 {
    let color = over(under, color);
    let share = share.clamp(0.0, 1.0);
    let mix = |c: u8, u: u8| {
        (f32::from(u) + (f32::from(c) - f32::from(u)) * share)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(
        mix(color.r(), under.r()),
        mix(color.g(), under.g()),
        mix(color.b(), under.b()),
    )
}

/// The outline of a round-capped stroke from `a` to `b` of `radius`
/// points, in screen space: each cap's chords stay within a quarter point
/// of the true arc. A zero-length stroke is a disc.
fn capsule(a: Pos2, b: Pos2, radius: f32) -> Vec<Pos2> {
    let radius = radius.max(0.5);
    let d = b - a;
    let length = d.length();
    let u = if length > 1e-4 {
        d / length
    } else {
        egui::vec2(1.0, 0.0)
    };
    // Chord error r·(1 − cos(step / 2)) ≤ ¼ point.
    let step = (1.0 - (0.25 / radius).min(1.0)).acos() * 2.0;
    let chords = (std::f32::consts::PI / step.max(1e-3))
        .ceil()
        .clamp(2.0, 64.0) as usize;
    let mut out = Vec::with_capacity(2 * chords + 2);
    for (center, toward) in [(b, u), (a, -u)] {
        let side = egui::vec2(-toward.y, toward.x);
        for k in 0..=chords {
            let angle = std::f32::consts::PI * (k as f32 / chords as f32 - 0.5);
            let (s, c) = angle.sin_cos();
            out.push(center + (toward * c + side * s) * radius);
        }
    }
    out
}

impl BoardInk {
    /// `color`, `share` of the way from the bare board, opaque: copper (or
    /// a far-side drawing) of one layer reads as one colour where its
    /// items overlap, instead of darkening as translucent layers would.
    fn on_board(&self, color: Color32, share: f32) -> Color32 {
        mixed(self.board, color, share)
    }

    fn get() -> Self {
        let t = Tokens::get();
        let dark = theme::is_dark();
        Self {
            board: over(t.viewport_bottom, t.panel.gamma_multiply(0.35)),
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
/// The board sides by their names.
/// The silkscreen designator the Designator section edits.
fn target_text(project: &Project, instance: Id) -> Option<pcbcraft_core::ids::GraphicId> {
    pcbcraft_core::instances::text::target(project, instance, None).ok()
}

fn layer_options() -> [(Layer, String); 2] {
    [(Layer::Top, "Top"), (Layer::Bottom, "Bottom")].map(|(layer, name)| (layer, name.to_owned()))
}
/// The copper layers of `stack` (top to bottom) by their names.
fn copper_options(stack: &[CopperLayer]) -> Vec<(CopperLayer, String)> {
    stack.iter().map(|layer| (*layer, layer.name())).collect()
}
/// Board millimetres to the canvas `rect`: the board fitted with a 50 px
/// margin and centred, times `zoom`, moved by `pan`.
fn board_view(rect: Rect, board: Point, zoom: f32, pan: Vec2, from_bottom: bool) -> BoardView {
    let scale = fit_scale(rect, board) * zoom;
    let centring = egui::vec2(-board[0] as f32, board[1] as f32) * 0.5;
    let origin = fit_area(rect).center() + pan + centring * scale;
    BoardView::new(View { origin, scale }, board[0], from_bottom)
}

/// The part of the canvas `rect` a fitted board is centred in: above the
/// navigation bar's band, so the bar never covers the board or a label at
/// its bottom edge.
fn fit_area(rect: Rect) -> Rect {
    let band = kit::navigation_band().min(rect.height() / 4.0);
    Rect::from_min_max(rect.min, egui::pos2(rect.max.x, rect.max.y - band))
}

fn fit_scale(rect: Rect, board: Point) -> f32 {
    let rect = fit_area(rect);
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
    let centre = fit_area(rect).center();
    let origin = centre + pan + centring * from;
    let moved = pointer - (pointer - origin) * (to / from);
    moved - centre - centring * to
}

fn snap(p: Point) -> Point {
    p.map(|v| (v * 2.0).round() / 2.0)
}
/// The package envelope of a placement on the board: the footprint's
/// [`envelope_outline`](pcbcraft_core::library::FootprintDefinition::envelope_outline)
/// through the placement transform, the frame the Component editor draws.
/// An (instance, track, net) selection, as `PcbEditor::set_selection` takes.
type Selection = (Option<Id>, Option<Id>, Option<Id>);

/// Every part `refs` names, directly or through a pad, unit, property or
/// terminal, in order and with repeats.
fn located_parts(refs: &[ObjectRef]) -> impl Iterator<Item = Id> + '_ {
    refs.iter().filter_map(|o| match o {
        ObjectRef::Instance { id }
        | ObjectRef::Pad { instance: id, .. }
        | ObjectRef::Unit { instance: id, .. }
        | ObjectRef::Property { instance: id, .. } => Some(*id),
        ObjectRef::Terminal { endpoint } => Some(endpoint.instance),
        _ => None,
    })
}

/// The object a finding row locates: the first part (directly, or through
/// a pad or terminal) it names, else its first track, else its first net.
fn locate(objects: &[pcbcraft_core::diagnostics::ObjectRef]) -> Option<Selection> {
    use pcbcraft_core::diagnostics::ObjectRef;
    let instance = located_parts(objects).next();
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
        Inspected::Via(id) => (
            IssueScope::Selection {
                objects: vec![ObjectRef::Via { id }],
            },
            "on this via".to_owned(),
        ),
        // Findings do not name fills yet: the slot stays empty.
        Inspected::Fill(_) => return None,
        // A group's findings are on its objects' own cards.
        Inspected::Group => return None,
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
    Some(IssueSlot {
        counts,
        label,
        whole: inspected == Inspected::Board,
    })
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
                    points.extend(layout::track_outline(track));
                }
            }
            ObjectRef::Net { id } => {
                pads_where(&|p| p.net == Some(id), &mut points);
                for track in project.tracks.iter().filter(|t| t.net == id) {
                    points.extend(layout::track_outline(track));
                }
            }
            ObjectRef::Via { id } => {
                for via in project.vias.iter().filter(|v| v.id == id) {
                    let r = via.diameter / 2.0;
                    let [x, y] = via.position;
                    points.extend([[x - r, y - r], [x + r, y + r]]);
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

/// The furthest a Locate zooms in, as a multiple of the fitted board, so
/// small parts keep their neighbours and readable designators in view.
const LOCATE_MAX_ZOOM: f32 = 4.0;

/// The margin a Locate keeps around the located objects, in millimetres.
const LOCATE_MARGIN_MM: f64 = 2.0;

/// The zoom and pan that frame `bounds` in the canvas `rect`: centred, the
/// bounds plus a margin filling about two thirds of the canvas, never
/// zoomed out past the fitted board nor in past [`LOCATE_MAX_ZOOM`].
/// The zoom and pan that show `bounds` whole, at most at the board's fit
/// and as far out as the zoom range allows, centred.
fn overview(rect: Rect, board: Point, bounds: [Point; 2], from_bottom: bool) -> (f32, Vec2) {
    let fit = fit_scale(rect, board);
    let size = [
        (bounds[1][0] - bounds[0][0]).max(1.0) as f32,
        (bounds[1][1] - bounds[0][1]).max(1.0) as f32,
    ];
    let wanted = (rect.width() / size[0]).min(rect.height() / size[1]) * 0.9;
    let zoom = (wanted / fit).clamp(*ZOOM_RANGE.start(), 1.0);
    let centre = [
        (bounds[0][0] + bounds[1][0]) / 2.0,
        (bounds[0][1] + bounds[1][1]) / 2.0,
    ];
    let unpanned = board_view(rect, board, zoom, Vec2::ZERO, from_bottom);
    (zoom, rect.center() - unpanned.screen(centre))
}

fn frame_view(rect: Rect, board: Point, bounds: [Point; 2], from_bottom: bool) -> (f32, Vec2) {
    let fit = fit_scale(rect, board);
    let size = [
        (bounds[1][0] - bounds[0][0] + 2.0 * LOCATE_MARGIN_MM).max(1.0) as f32,
        (bounds[1][1] - bounds[0][1] + 2.0 * LOCATE_MARGIN_MM).max(1.0) as f32,
    ];
    let wanted = (rect.width() / (1.5 * size[0])).min(rect.height() / (1.5 * size[1]));
    let zoom = (wanted / fit).clamp(1.0, LOCATE_MAX_ZOOM.min(*ZOOM_RANGE.end()));
    let centre = [
        (bounds[0][0] + bounds[1][0]) / 2.0,
        (bounds[0][1] + bounds[1][1]) / 2.0,
    ];
    let unpanned = board_view(rect, board, zoom, Vec2::ZERO, from_bottom);
    (zoom, rect.center() - unpanned.screen(centre))
}

impl crate::issues::Locate for PcbEditor {
    /// Select every part `refs` names (else its track, else its net), the
    /// first one inspected, and frame all of them on the next Layout
    /// frame; the board ref frames the whole board. Selection and camera
    /// only: the tool, document and geometry stay as they are.
    fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool {
        self.sync(engine);
        let project = engine.project();
        // A finding on a via (and no part or track) selects the via and
        // its net and frames it.
        let via = refs.iter().find_map(|o| match o {
            ObjectRef::Via { id } => project.vias.iter().find(|v| v.id == *id),
            _ => None,
        });
        let track = refs.iter().any(|o| matches!(o, ObjectRef::Track { .. }));
        if let Some(via) = via
            && located_parts(refs).next().is_none()
            && !track
        {
            self.select_via(via.id, Some(via.net));
            self.frame_request = bounds_of(self, project, refs);
            return true;
        }
        let Some((instance, track, net)) = locate(refs) else {
            if refs.contains(&ObjectRef::Board) {
                self.set_selection(None, None, None);
                self.frame_request = Some([[0.0, 0.0], project.board_size]);
                return true;
            }
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
        // A finding on several tracks and vias (a dangling stub) selects
        // them all, so one Delete removes them in one undo step.
        let mut copper = Group::default();
        for o in refs {
            match o {
                ObjectRef::Track { id } if project.tracks.iter().any(|t| t.id == *id) => {
                    copper.tracks.insert(*id);
                }
                ObjectRef::Via { id } if project.vias.iter().any(|v| v.id == *id) => {
                    copper.vias.insert(*id);
                }
                _ => {}
            }
        }
        if instance.is_none() && copper.len() > 1 {
            self.set_group(copper, project);
            self.net = net;
            self.highlighted = net;
        }
        if instance.is_some() {
            let mut seen: BTreeSet<Id> = instance.into_iter().collect();
            self.also = located_parts(refs)
                .filter(|id| project.instance(*id).is_some() && seen.insert(*id))
                .collect();
        }
        self.frame_request = bounds_of(self, project, refs);
        true
    }
}

/// A part's outline on the board: the one align and distribute line up.
fn body_polygon(project: &Project, placement: &Placement) -> Option<[Point; 4]> {
    layout::part_envelope(project, placement)
}

#[cfg(test)]
mod tests {
    use super::board_card::fabrication_edit;
    use super::*;
    use pcbcraft_core::layout::RouteRequest;
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
                    layer: Layer::Top.into(),
                },
            }),
        )?;
        Ok(engine)
    }

    /// A route's net highlight is temporary: it ends with the route (Select,
    /// Update PCB), while a highlight the user turned on stays.
    #[test]
    fn a_routes_net_highlight_ends_with_the_route() -> pcbcraft_core::Result<()> {
        let engine = Engine::demo()?;
        let mut editor = PcbEditor::default();
        editor.on_tool("route", &engine);
        editor.route_highlight(Some(26));
        assert_eq!(editor.highlighted_net(), Some(26));
        editor.on_tool("select", &engine);
        assert_eq!(editor.highlighted_net(), None, "the route's highlight ends");
        editor.set_selection(None, None, Some(7));
        editor.on_tool("route", &engine);
        editor.route_highlight(Some(7));
        editor.on_tool("update", &engine);
        assert_eq!(
            editor.highlighted_net(),
            Some(7),
            "the user's highlight stays"
        );
        Ok(())
    }

    /// Demo run item 7: Update PCB's review shows in Layout, where the
    /// staged parts are drawn beside the board, even when asked from 3D.
    #[test]
    fn update_pcb_reviews_in_layout() -> pcbcraft_core::Result<()> {
        let mut engine = Engine::demo()?;
        let first = engine.project().instances[0].clone();
        engine.apply(
            engine.revision(),
            Operation::Instance(pcbcraft_core::InstanceCommand::Place(
                pcbcraft_core::instances::NewInstance {
                    sheet: None,
                    component: first.component,
                    designator: Some("R9".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: crate::schematic_editor::pt([30.0, 0.0]),
                    physical: first.physical,
                },
            )),
        )?;
        let mut editor = PcbEditor::default();
        editor.set_3d(true);
        editor.on_tool("update", &engine);
        assert!(editor.reviewing());
        assert!(!editor.is_3d(), "the review opened in 3D");
        Ok(())
    }

    #[test]
    fn drag_reconnects_airwires_live_and_cancel_preserves_document_and_history()
    -> pcbcraft_core::Result<()> {
        let mut engine = routed_board()?;
        let (_, r2) = ids(&engine);
        // A locked track stays when its part moves, so the drag opens an
        // airwire (an unlocked one is carried: see below).
        let track = engine.project().tracks[0].id;
        let lock = |locked| Operation::Layout(LayoutCommand::SetTrackLock { id: track, locked });
        engine.apply(engine.revision(), lock(true))?;
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

        // Unlocked, the track follows the dragged part: no airwire opens,
        // in the preview as on the moved board.
        engine.apply(engine.revision(), lock(false))?;
        editor.sync(&engine);
        editor.drag = Some((placement.clone(), placement.position));
        editor.drag_position = Some([22.0, 15.0]);
        editor.refresh_drag_airwires(&engine);
        assert!(editor.visible_airwires().is_empty());
        editor.cancel_drag();
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Move {
                instance: r2,
                position: [22.0, 15.0],
                rotation_deg: placement.rotation_deg,
                layer: placement.layer,
            }),
        )?;
        editor.sync(&engine);
        assert!(editor.visible_airwires().is_empty());
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
                let action = refusal::take_action(ui.ctx());
                ui.ctx().data_mut(|d| {
                    d.insert_temp(egui::Id::unique("test-mark"), mark);
                    d.insert_temp(egui::Id::unique("test-action"), action);
                });
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
            Some(Origin::Dialog(egui::Id::unique("pcb-board-setup")))
        );
        engine.apply(engine.revision(), operation)?;
        assert_eq!(engine.project().fabrication, expected);
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(editor.subject(), Subject::Properties);
        let model = pcbcraft_solidcraft::project::build(engine.project())?;
        let blue = pcbcraft_solidcraft::material::material(
            pcbcraft_solidcraft::material::BodyKind::MaskTop,
            &expected,
            &pcbcraft_solidcraft::stack::Stack::of(
                &engine.project().stackup,
                engine.project().fabrication.pad_finish,
            ),
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

    /// Stackup… opens the docked Stackup dialog; a preset and Add layer
    /// pair edit the draft, and OK sends one `layout.edit_stackup` that the
    /// engine applies as one undo step. An odd draft cannot be sent: the
    /// core's rule says why. A removed layer that carries copper offers
    /// where its copper goes, and the edit moves it there.
    #[test]
    fn the_stackup_dialog_edits_layers_as_one_command() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            painted(&output)
                .texts
                .iter()
                .any(|t| t == "2 layers · 1.67 mm")
        );
        assert!(click(&mut editor, &engine, &ctx, "Stackup…")?.is_none());
        assert_eq!(editor.subject(), Subject::Dialog);
        assert!(click(&mut editor, &engine, &ctx, "2 layers")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "6 layers")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "Add pair")?.is_none());
        // One more foil makes an odd draft: OK is refused by the rule.
        assert!(click(&mut editor, &engine, &ctx, "Add core")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "OK")?.is_none());
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            painted(&output)
                .texts
                .iter()
                .any(|t| t.contains("even number of copper layers")),
            "the rule shows"
        );
        assert!(click(&mut editor, &engine, &ctx, "Add prepreg")?.is_none());
        let operation = click(&mut editor, &engine, &ctx, "OK")?.ok_or("OK did nothing")?;
        let Operation::Layout(LayoutCommand::EditStackup(edit)) = &operation else {
            return Err("OK issued another command".into());
        };
        assert_eq!(edit.stackup.copper.len(), 10);
        edit.stackup.check()?;
        engine.apply(engine.revision(), operation)?;
        frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(editor.subject(), Subject::Properties);
        assert_eq!(editor.stack.len(), 10);
        // Copper on Inner 7, then shrink to four layers moving it to Inner 2.
        let net = engine.project().nets[0].id;
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::Route {
                request: RouteRequest {
                    net,
                    start: [20.0, 20.0],
                    end: [30.0, 20.0],
                    width: 0.3,
                    layer: CopperLayer::Inner(7),
                },
            }),
        )?;
        assert!(click(&mut editor, &engine, &ctx, "Stackup…")?.is_none());
        // The ten-layer build is no preset any more.
        assert!(click(&mut editor, &engine, &ctx, "Custom")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "4 layers")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "Not moved")?.is_none());
        assert!(click(&mut editor, &engine, &ctx, "Move to Inner 2")?.is_none());
        let operation = click(&mut editor, &engine, &ctx, "OK")?.ok_or("OK did nothing")?;
        let before = engine.project().clone();
        engine.apply(engine.revision(), operation)?;
        assert_eq!(engine.project().stackup.copper.len(), 4);
        assert_eq!(engine.project().tracks[0].layer, CopperLayer::Inner(2));
        engine.undo()?;
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    /// The layer keys set the active layer outside a route, and a key past
    /// the stackup is unavailable with a hint.
    #[test]
    fn layer_keys_set_the_active_layer_within_the_stackup() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut engine = Engine::demo()?;
        engine.apply(
            engine.revision(),
            Operation::Layout(LayoutCommand::SetStackup(pcbcraft_core::Stackup::preset(
                12,
            )?)),
        )?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        frame(&mut editor, &engine, &ctx, vec![]);
        editor.select_layer(9);
        assert_eq!(editor.layer, CopperLayer::Inner(9));
        assert!(editor.pending_via.is_none(), "no route: no via");
        assert_eq!(editor.layer_key_id(), Some("pcb.layer-10"));
        assert_eq!(editor.layer_label(&ctx), "Inner 9 (0)");
        editor.select_layer(11);
        assert_eq!(editor.layer, CopperLayer::Bottom);
        assert_eq!(editor.layer_key_id(), None);
        assert!(editor.layer_at(12).is_err());
        editor.select_layer(12);
        assert_eq!(
            editor.layer,
            CopperLayer::Bottom,
            "past the stackup: nothing"
        );
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
                    sheet: None,
                    component: first.component,
                    designator: Some("R3".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: crate::schematic_editor::pt([30.0, 0.0]),
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
                    sheet: None,
                    component: first.component,
                    designator: Some("R3".into()),
                    properties: vec![],
                    representation: first.representation,
                    unit: pcbcraft_core::UnitId(1),
                    position: crate::schematic_editor::pt([30.0, 0.0]),
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
        // Designator · shown value · package, as the schematic header reads.
        assert_eq!(
            (header.title.as_str(), header.kind),
            ("R3 · Resistor package", "Not placed")
        );
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
                    shape: crate::schematic_editor::shape_nm(shape),
                    width_nm: crate::schematic_editor::nm(0.15),
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
                    crate::schematic_editor::pt([0.0, 3.2]),
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
            ("top copper", ink.on_board(board.copper_top, 1.0)),
            // Seen from the top, bottom copper is the dimmed far side.
            (
                "bottom copper",
                ink.on_board(board.copper_bottom, FAR_SHADE),
            ),
            ("drill", ink.drill),
            ("silkscreen", ink.silkscreen),
            ("courtyard", ink.courtyard),
        ] {
            assert!(all.colors.contains(&color), "{what} not painted");
        }
        // Courtyard and silkscreen are drawn above the copper (spec order).
        let last = |c: Color32| all.colors.iter().rposition(|x| *x == c);
        let first = |c: Color32| all.colors.iter().position(|x| *x == c);
        assert!(first(ink.courtyard) > last(ink.on_board(board.copper_top, 1.0)));
        assert!(first(ink.silkscreen) > last(ink.on_board(board.copper_top, 1.0)));
        // R2 is on the bottom: its silkscreen is the dimmed far side.
        assert!(
            all.colors
                .contains(&ink.on_board(ink.silkscreen, FAR_SHADE))
        );
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
            ("bottom copper", ink.on_board(board.copper_bottom, 1.0)),
            (
                "dimmed bottom copper",
                ink.on_board(board.copper_bottom, FAR_SHADE),
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

    /// VM check #7: a 3D pick selects as a Layout click on the same object.
    /// A pad picks its part and lights the pad's net in both views (pads
    /// are not selected on their own); a part's body picks the part.
    #[test]
    fn a_3d_pick_selects_as_a_layout_click() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let model = pcbcraft_solidcraft::project::build(engine.project())?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut layout = PcbEditor::default();
        canvas_frame(&mut layout, &engine, &ctx, vec![]);
        let state = |e: &PcbEditor| {
            (
                e.selected_instance(),
                e.selected_track(),
                e.selected_via(),
                e.net,
                e.highlighted_net(),
            )
        };
        let mut picked = PcbEditor::default();
        let mut pads = 0;
        for source in model.sources.iter().filter(|s| s.pad.is_some()) {
            let pad = layout
                .board_pads
                .iter()
                .map(|b| &b.placed)
                .find(|p| Some(p.instance) == source.instance && Some(p.pad.id) == source.pad)
                .ok_or("the pad in Layout")?;
            layout.select_at(engine.project(), pad.center(), 0.0, TextMetric::default());
            picked.select_picked(Some(source));
            assert_eq!(state(&picked), state(&layout), "{}", source.label);
            assert_eq!(
                picked.selected_instance(),
                source.instance,
                "{}",
                source.label
            );
            pads += 1;
        }
        assert!(pads > 0);
        assert!(
            model
                .sources
                .iter()
                .any(|s| s.pad.is_some() && s.net.is_some()),
            "a pad with a net was checked"
        );
        let body = model
            .sources
            .iter()
            .find(|s| s.pad.is_none() && s.instance.is_some())
            .ok_or("a part body")?;
        picked.select_picked(Some(body));
        assert_eq!(
            state(&picked),
            (body.instance, None, None, None, None),
            "{}",
            body.label
        );
        picked.select_picked(None);
        assert_eq!(state(&picked), (None, None, None, None, None));
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

    /// Run 3, finding 11: a pad number shows only where it fits inside its
    /// pad, so fine-pitch numbers ("A1B12" on a USB-C receptacle) don't
    /// stack over each other and the designator at fit zoom.
    #[test]
    fn a_pad_number_shows_only_inside_its_pad() -> Result<(), Box<dyn std::error::Error>> {
        let mut file = shaped_board()?.to_file();
        for footprint in &mut file.project.library.footprints {
            if let Some(pad) = footprint
                .definition
                .pads
                .iter_mut()
                .find(|p| p.kind == PadKind::PlatedThrough)
            {
                pad.number = "A1B12".into();
            }
        }
        let engine = Engine::from_file(file)?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let texts = painted(&output).texts;
        // The 1.6 mm pad is too narrow for five characters at fit zoom;
        // the custom pad's "2" fits.
        assert!(!texts.iter().any(|t| t == "A1B12"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "2"), "{texts:?}");
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

    /// The fitted board keeps its margin clear of the navigation bar at
    /// the canvas's bottom (the fit and layers buttons and the gap under
    /// them), so a label under the board's bottom edge is not drawn under
    /// the buttons.
    #[test]
    fn the_fitted_board_leaves_room_for_the_navigation_bar() {
        let t = Tokens::get();
        let rect = Rect::from_min_size(Pos2::new(40.0, 30.0), egui::vec2(900.0, 500.0));
        let bar_top = rect.bottom() - (t.control_height + 2.0 * t.space_4 + t.space_16);
        let board = [40.0, 30.0];
        for from_bottom in [false, true] {
            let view = board_view(rect, board, 1.0, Vec2::ZERO, from_bottom);
            let low = [[0.0, 0.0], [board[0], 0.0], [0.0, board[1]], board]
                .into_iter()
                .map(|p| view.screen(p).y)
                .fold(f32::MIN, f32::max);
            let high = [[0.0, 0.0], [board[0], 0.0], [0.0, board[1]], board]
                .into_iter()
                .map(|p| view.screen(p).y)
                .fold(f32::MAX, f32::min);
            assert!(
                bar_top - low >= high - rect.top() - 1e-3,
                "{from_bottom}: board {high}..{low}, bar at {bar_top}"
            );
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

    /// Two fingers pan the board with the fingers and never zoom; a mouse
    /// wheel notch zooms.
    #[test]
    fn two_fingers_pan_the_canvas_and_the_wheel_zooms() -> Result<(), Box<dyn std::error::Error>> {
        use crate::navigation::tests::{notch, swipe};
        let engine = shaped_board()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        canvas_frame(&mut editor, &engine, &ctx, vec![]);
        let (zoom, pan) = (editor.zoom, editor.pan);
        let mut events = vec![egui::Event::PointerMoved(Pos2::new(300.0, 300.0))];
        events.extend(swipe(egui::vec2(4.0, -6.0), egui::Modifiers::NONE, 3));
        canvas_frame(&mut editor, &engine, &ctx, events);
        assert_eq!(editor.zoom, zoom, "two fingers must not zoom");
        assert_eq!(editor.pan - pan, egui::vec2(12.0, -18.0));
        for _ in 0..20 {
            canvas_frame(&mut editor, &engine, &ctx, vec![]);
        }
        assert_eq!(editor.zoom, zoom, "nor their smoothing tail");
        canvas_frame(&mut editor, &engine, &ctx, vec![notch(1.0)]);
        for _ in 0..20 {
            canvas_frame(&mut editor, &engine, &ctx, vec![]);
        }
        assert!(editor.zoom > zoom, "{zoom} → {}", editor.zoom);
        Ok(())
    }

    /// The refusal origin the last `frame` marked for its operation.
    fn marked(ctx: &egui::Context) -> Option<Origin> {
        ctx.data(|d| d.get_temp::<Option<Origin>>(egui::Id::unique("test-mark")))
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

    /// A Position outside the working area is refused by the engine; the
    /// refusal comes back to the field, which keeps the typed value marked
    /// with the engine's bounds under it, and the document is unchanged.
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
        editor.position = [5000.0, 9.0];
        let operation = frame(&mut editor, &engine, &ctx, vec![])
            .1
            .ok_or("the field did not commit")?;
        let origin = marked(&ctx).ok_or("no origin marked")?;
        let error = engine
            .apply(engine.revision(), operation)
            .err()
            .ok_or("the engine took a part outside the working area")?;
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
                .any(|t| t == "R1 must stay within -1000 to 2000 mm on each axis"),
            "{shown:?}"
        );
        assert_eq!(editor.position, [5000.0, 9.0], "the typed value stays");
        assert!(refusal::unclaimed(&ctx).is_empty(), "the field claimed it");
        // Another subject drops the edit and its refusal.
        editor.set_selection(None, None, None);
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            !texts(&output)
                .iter()
                .any(|t| t.contains("must stay within"))
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
        // A refusal toast names the action and the part (round 4 Q2).
        let action = || {
            ctx.data(|d| d.get_temp::<Option<String>>(egui::Id::unique("test-action")))
                .flatten()
        };
        assert_eq!(action().as_deref(), Some("rotate R1"));
        let Some(Operation::Layout(LayoutCommand::Move { layer, .. })) =
            click(&mut editor, &engine, &ctx, "Flip")?
        else {
            return Err("Flip issued no move".into());
        };
        assert_eq!(layer, opposite(before.layer));
        assert_eq!(action().as_deref(), Some("flip R1"));
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
        assert_eq!((layer, width), (Some(Layer::Bottom.into()), None));
        let Some(Operation::Layout(LayoutCommand::DeleteTrack { id })) =
            click(&mut editor, &engine, &ctx, "Delete track")?
        else {
            return Err("Delete track did nothing".into());
        };
        assert_eq!(id, track.id);
        Ok(())
    }

    /// Precision run finding 2: Delete and Backspace delete the selected
    /// track with the Delete track button's command, as one undo step;
    /// with Route armed, Backspace is Route's own key.
    #[test]
    fn delete_and_backspace_delete_the_selected_track() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = routed_board()?;
        let track = engine.project().tracks[0].clone();
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let press = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for key in [egui::Key::Delete, egui::Key::Backspace] {
            let mut editor = PcbEditor::default();
            editor.set_selection(None, Some(track.id), Some(track.net));
            canvas_frame(&mut editor, &engine, &ctx, vec![]);
            let (_, operation) = canvas_frame(&mut editor, &engine, &ctx, vec![press(key)]);
            assert!(
                matches!(
                    operation,
                    Some(Operation::Layout(LayoutCommand::DeleteTrack { id })) if id == track.id
                ),
                "{key:?}"
            );
            // Nothing selected: the key does nothing.
            editor.set_selection(None, None, None);
            assert!(
                canvas_frame(&mut editor, &engine, &ctx, vec![press(key)])
                    .1
                    .is_none()
            );
        }
        let mut editor = PcbEditor::default();
        editor.set_selection(None, Some(track.id), Some(track.net));
        editor.tool = Tool::Route;
        let (_, operation) = canvas_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Backspace)],
        );
        assert!(
            !matches!(
                operation,
                Some(Operation::Layout(LayoutCommand::DeleteTrack { .. }))
            ),
            "Route keeps Backspace"
        );
        let before = engine.revision();
        engine.apply(
            before,
            Operation::Layout(LayoutCommand::DeleteTrack { id: track.id }),
        )?;
        assert!(engine.project().tracks.iter().all(|t| t.id != track.id));
        engine.undo()?;
        assert!(engine.project().tracks.iter().any(|t| t.id == track.id));
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
        // The settings rows show the keys that step them (W and M are
        // past the first eight rows of the Keys list).
        for key in ["W", "M", "D"] {
            assert!(
                shown.iter().any(|t| t == key),
                "{key} chip missing: {shown:?}"
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

    /// An armed tool shows a crosshair over the canvas; Select keeps the
    /// default pointer.
    #[test]
    fn an_armed_tool_shows_a_crosshair() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = PcbEditor::default();
        let hover = || vec![egui::Event::PointerMoved(Pos2::new(450.0, 350.0))];
        let cursor = |editor: &mut PcbEditor| {
            canvas_frame(editor, &engine, &ctx, hover());
            canvas_frame(editor, &engine, &ctx, hover())
                .0
                .platform_output
                .cursor_icon
        };
        assert_eq!(cursor(&mut editor), egui::CursorIcon::Default);
        for tool in ["route", "via", "fill"] {
            editor.on_tool(tool, &engine);
            assert_eq!(cursor(&mut editor), egui::CursorIcon::Crosshair, "{tool}");
        }
        editor.on_tool("select", &engine);
        assert_eq!(cursor(&mut editor), egui::CursorIcon::Default);
        Ok(())
    }

    /// A route segment the engine refuses leaves the route in progress
    /// (its start and the pill); a document change the route did not make
    /// ends it.
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
        // Ignore: the plain step, which the engine refuses (Shove and
        // Walkaround stop at the board edge instead).
        editor.route.mode = pcbcraft_core::layout::router::RouteMode::Ignore;
        let (output, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        // Each setting shows the key that steps it, from the keymap.
        let key = |text: &str, id: &str| crate::keys::labelled(&ctx, text, id);
        let pill = format!(
            "Routing {} · {} · {} · {} · {}",
            engine.project().net_name(net),
            key("0.3 mm", "route.width-next"),
            key("Top", "pcb.layer-1"),
            key("45°", "route.posture"),
            key("Ignore", "route.router-mode-next"),
        );
        assert!(
            pill.contains("0.3 mm (W)") && pill.contains("Top (1)"),
            "{pill}"
        );
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
        // A change the route did not send ends the route.
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
        // Night round 3 (R3-2): the Board slot counts the whole board, so
        // its row lists the board's issues, highlighted net or not.
        assert!(board.whole);
        // The demo's net between R1 and R2 is unrouted: a To do on R1.
        editor.set_selection(Some(r1), None, None);
        let slot = editor.card(&engine).issues.ok_or("no placement slot")?;
        assert_eq!(slot.label, "on R1");
        assert!(!slot.whole);
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
