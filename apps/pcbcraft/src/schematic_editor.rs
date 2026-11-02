//! Single-sheet content for the shared shell. Control metrics and input ownership
//! follow Solidcraft ui_kit/{layout,fields}.rs and tools.rs; public kit controls,
//! fonts and palette are used directly. Electrical connectivity lives in core.
//!
//! The Properties card follows docs/sidebar-ia.md: one subject (the sheet,
//! an instance, a wire's net or the Place preview), fields that commit live
//! one typed command at a time, a reserved issue slot and a pinned footer.
//! Findings live in Issues; a refused field edit shows under its field.
use crate::kit;
use crate::kit::card::{CardResponse, Inspector, IssueSlot, Region};
use crate::place_palette::{ChooseAction, PaletteAction, PlacePalette};
use crate::refusal::{self, Origin};
use crate::resolve_parts::{self, ResolveAction, ResolveParts};
use crate::symbol_view;
use egui::{Pos2, Rect, Sense, Stroke, Vec2, pos2};
use pcbcraft_core::completeness::{InstanceReport, PurchasingState};
use pcbcraft_core::geometry::{Bounds, Transform2};
use pcbcraft_core::instances::{self, NewInstance};
use pcbcraft_core::issues::{IssueArea, IssueCounts, IssueScope, Severity};
use pcbcraft_core::library::graphics::SHEET_GRID_MM;
use pcbcraft_core::library::{Rotation, TextMetric, Unit};
use pcbcraft_core::schematic::PlacedUnit;
use pcbcraft_core::{
    ComponentRef, DefinitionId, Endpoint, Engine, Id, InstanceCommand, Net, ObjectRef, Operation,
    PhysicalChoice, Point, Project, Query, QueryResult, WireEnd,
    schematic::{self, SchematicCommand, WirePoint, WireTarget},
    updates::{Change, InstanceChange},
};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

mod instance_card;
mod marking_row;
mod net_card;
mod sheet_card;

/// An edit buffer for one field shown on the symbol: its lineage, label,
/// the effective text when loaded and the text being typed.
#[derive(Clone)]
struct FieldBuffer {
    field: DefinitionId,
    revision: pcbcraft_core::FieldRef,
    label: String,
    loaded: String,
    text: String,
}

/// A sheet action on the selected occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrange {
    Rotate,
    Mirror,
}

/// Pick tolerance around drawn symbol content, in pixels.
const HIT_PX: f32 = 6.0;
/// Pick radius of a pin end or junction, and of a wire's path, in pixels.
const PIN_HIT_PX: f32 = 9.0;
const WIRE_HIT_PX: f32 = 6.0;
/// The junction dot's radius in sheet millimetres, and its on-screen bounds
/// in pixels so it stays visible zoomed out and tidy zoomed in.
const DOT_RADIUS_MM: f32 = 0.3;
const DOT_RADIUS_PX: (f32, f32) = (2.0, 10.0);

/// How a pin end is marked on the sheet, by the schematic convention: a
/// wired end needs no mark (a junction dot is drawn where three or more
/// conductors meet); an end still needing a decision shows the Issues
/// to-do ring; a decided no-connect shows a cross.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinMark {
    Wired,
    ToDo,
    NoConnect,
}

/// The pin mark of every terminal, from core's accounting (the same rule
/// that raises the Issues to-do for an unconnected terminal).
fn pin_marks(project: &Project) -> BTreeMap<Endpoint, PinMark> {
    let wired: BTreeSet<Endpoint> = project
        .wires
        .iter()
        .flat_map(|w| [w.from, w.to])
        .filter_map(WireEnd::terminal)
        .collect();
    pcbcraft_core::nets::terminals(project)
        .into_iter()
        .map(|e| {
            let mark = if wired.contains(&e) {
                PinMark::Wired
            } else if pcbcraft_core::nets::accounted_for(project, e) {
                PinMark::NoConnect
            } else {
                PinMark::ToDo
            };
            (e, mark)
        })
        .collect()
}

/// The junction dot radius on screen at `scale` pixels per millimetre.
fn dot_radius(scale: f32) -> f32 {
    (DOT_RADIUS_MM * scale).clamp(DOT_RADIUS_PX.0, DOT_RADIUS_PX.1)
}
/// Half the nominal symbol box a library placement keeps clear, in mm: a
/// 25 × 15 mm box and half a sheet grid of clearance on each side.
const PLACE_HALF: [f64; 2] = [15.0, 10.0];
/// The search pitch of a library placement, in mm (sheet-grid multiples).
const PLACE_PITCH: [f64; 2] = [15.0, 10.0];

/// What the Properties card shows: exactly one subject at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Subject {
    /// A docked update review.
    Review,
    /// The Place tool: the COMPONENT preview.
    Place,
    Instance(Id),
    /// A picked wire and its net.
    Wire(Id),
    /// The Wire tool with nothing picked.
    Wiring,
    /// Nothing selected: the sheet.
    Sheet,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Select,
    Place,
    Wire,
}

pub struct SchematicEditor {
    /// Two-finger scrolling across frames.
    trackpad: crate::navigation::Trackpad,
    tool: Tool,
    component: Option<ComponentRef>,
    /// The selected instance and the occurrence that was picked.
    selected: Option<Id>,
    selected_occurrence: Option<Id>,
    selected_wire: Option<Id>,
    /// Net-name edit buffer for the selected wire's net.
    net_label: Option<(Id, String)>,
    /// Where the Wire tool's wire starts.
    wire_from: Option<WireTarget>,
    draft_reference: String,
    draft_fields: Vec<FieldBuffer>,
    draft_position: Point,
    draft_rotation: Rotation,
    draft_mirror: bool,
    draft_id: Option<Id>,
    pan: Vec2,
    scale: f32,
    /// Dragged occurrence and the pointer's offset from its position.
    drag: Option<(Id, Point)>,
    /// Occurrence being dragged and its previewed position.
    preview: Option<(Id, Point)>,
    report: Option<(u64, Id, InstanceReport)>,
    fit_requested: bool,
    observed_revision: Option<u64>,
    nets_revision: Option<u64>,
    nets: Vec<Net>,
    /// Each wire's net, by wire ID, once per revision.
    wire_nets: BTreeMap<Id, Id>,
    /// Junction dots core derives from connectivity, once per revision.
    dots: Vec<(WireEnd, Point)>,
    /// How each pin end is marked, once per revision.
    pin_marks: BTreeMap<Endpoint, PinMark>,
    /// A refusal handed back to one card field: the field's id and the
    /// words shown under it until the field is edited again.
    refused: Option<(egui::Id, String)>,
    /// A property commit on this instance while its purchasing was in this
    /// state: the next frame says so when the commit left it unresolved.
    purchasing_watch: Option<(Id, PurchasingState)>,
    /// The subject's issue slot at one document (revision and project).
    slot: Option<(u64, usize, Subject, Option<IssueSlot>)>,
    /// Sheet bounds a locate asked the canvas to bring into view.
    frame_request: Option<Bounds>,
    /// The net card lists every member instead of the first eight.
    all_members: bool,
    /// What each occurrence draws, resolved by core once per revision so
    /// selection and hover never re-resolve.
    /// Shared so a frame can hold it while selection mutates the editor.
    placed: Arc<BTreeMap<Id, PlacedUnit>>,
    /// The extent of each resolved occurrence, with a wide text metric, so
    /// painting skips occurrences outside the view without measuring them.
    placed_bounds: BTreeMap<Id, Bounds>,
    /// The canvas size of the last frame, in points.
    view: Vec2,
    /// The Place tool's ghost: the component's first unit as core draws it
    /// in its own frame, resolved once per component and revision.
    ghost: Option<(ComponentRef, u64, PlacedUnit)>,
    update: crate::update_preview::UpdatePreview,
    /// Finds the component the Place tool puts down.
    palette: PlacePalette,
    /// The Resolve parts card.
    resolve: ResolveParts,
    /// New Part… was chosen: the app opens the component's Parts editor.
    new_part: Option<NewPartRequest>,
    /// Library sources' update marks, set by the app each frame.
    updates: Arc<Vec<pcbcraft_core::catalog::UpdateAvailable>>,
    /// An instance's Update… for a revision only a library source holds:
    /// the app copies it in, then opens the review for that instance.
    library_update: Option<(Id, crate::library_sources::FolderPick)>,
}

impl Default for SchematicEditor {
    fn default() -> Self {
        Self {
            trackpad: Default::default(),
            tool: Tool::Select,
            component: None,
            selected: None,
            selected_occurrence: None,
            selected_wire: None,
            net_label: None,
            wire_from: None,
            draft_reference: String::new(),
            draft_fields: Vec::new(),
            draft_position: [0.0; 2],
            draft_rotation: Rotation::R0,
            draft_mirror: false,
            draft_id: None,
            pan: Vec2::ZERO,
            scale: 12.0,
            drag: None,
            preview: None,
            report: None,
            fit_requested: false,
            observed_revision: None,
            nets_revision: None,
            nets: Vec::new(),
            wire_nets: BTreeMap::new(),
            dots: Vec::new(),
            pin_marks: BTreeMap::new(),
            refused: None,
            purchasing_watch: None,
            slot: None,
            frame_request: None,
            all_members: false,
            placed: Arc::default(),
            placed_bounds: BTreeMap::new(),
            view: Vec2::ZERO,
            ghost: None,
            update: Default::default(),
            palette: PlacePalette::default(),
            resolve: ResolveParts::default(),
            updates: Arc::default(),
            library_update: None,
            new_part: None,
        }
    }
}

fn operation(command: SchematicCommand) -> Option<Operation> {
    Some(Operation::Schematic(command))
}

/// The drawn unit of an occurrence.
fn unit_of<'a>(
    project: &'a Project,
    occurrence: &pcbcraft_core::SymbolOccurrence,
) -> Option<&'a Unit> {
    let instance = project.instance(occurrence.instance)?;
    instances::resolve(&project.library, instance)
        .ok()?
        .symbol
        .definition
        .units
        .iter()
        .find(|u| u.id == occurrence.unit)
}

/// The unit placing `component` puts down, drawn by core in its own frame.
/// Pin numbers are hidden: the ghost is an outline only.
fn ghost_unit(project: &Project, component: ComponentRef) -> Option<PlacedUnit> {
    let library = &project.library;
    let published = library.component(component)?;
    let symbol = library.symbol(*published.definition.representations.first()?)?;
    let unit = symbol.definition.units.first()?;
    schematic::definition_unit(
        library,
        schematic::DefinitionUnit {
            symbol: &symbol.definition,
            unit: unit.id,
            component: &published.definition,
            component_name: &published.meta.name,
            physical: None,
        },
    )
    .ok()
}

/// What placing `component` creates: its first representation and unit and
/// its first offered implementation, if any.
fn first_choices(project: &Project, component: ComponentRef) -> Option<(NewInstance, &Unit)> {
    let library = &project.library;
    let published = library.component(component)?;
    let symbol_ref = *published.definition.representations.first()?;
    let symbol = library.symbol(symbol_ref)?;
    let unit = symbol.definition.units.first()?;
    let physical = published.definition.implementations.first().and_then(|b| {
        let binding = library.binding(*b)?;
        let footprint = binding.definition.footprints.first()?;
        Some(PhysicalChoice {
            binding: b.id,
            footprint: footprint.id,
        })
    });
    Some((
        NewInstance {
            component,
            designator: None,
            properties: Vec::new(),
            representation: symbol_ref.id,
            unit: unit.id,
            position: [0.0, 0.0],
            physical,
        },
        unit,
    ))
}

/// A component lineage and the values a new Part of it is rated at.
pub type NewPartRequest = (
    pcbcraft_core::DefinitionId,
    Vec<(pcbcraft_core::FieldRef, pcbcraft_core::fields::FieldValue)>,
);

impl SchematicEditor {
    /// The whole subject slot for the right column: the docked update
    /// review, the Choose part dialog, or the Properties card with its
    /// issue slot and pinned footer (docs/sidebar-ia.md, "Layout").
    pub fn subject_slot(&mut self, ui: &mut egui::Ui, engine: &Engine) -> CardResponse<Operation> {
        self.refresh_connectivity(engine);
        if self.update.is_open() {
            return CardResponse::output(self.update.show(ui, engine));
        }
        if self.dialog_open() {
            return CardResponse::output(self.dialog(ui, engine));
        }
        let inspector = self.inspector_card(engine);
        kit::card::card(ui, &inspector, |ui, region| {
            self.card_region(ui, engine, region)
        })
    }

    /// Whether a docked dialog takes the subject slot: Choose part.
    fn dialog_open(&self) -> bool {
        self.palette.choosing() && !self.update.is_open()
    }

    /// The docked dialog in the subject slot `ui` offers (Choose part);
    /// returns the operation it issues.
    fn dialog(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        match self.palette.choose_dialog(ui, engine)? {
            ChooseAction::Assign { instances, part } => {
                Some(resolve_parts::assign(instances, Some(part)))
            }
            ChooseAction::UpdateAndAssign {
                instances,
                part,
                target,
            } => self.stage_update(
                engine,
                resolve_parts::update_and_assign(instances, part, target),
            ),
            ChooseAction::NewPart(request) => {
                self.resolve.close();
                self.new_part = Some(request);
                None
            }
            ChooseAction::Cancel => None,
        }
    }

    /// The Resolve parts window, while open. Choose… opens the docked
    /// dialog and the window steps aside until the dialog closes, then
    /// shows the updated table; Select on sheet keeps it open.
    fn resolve_window(&mut self, ctx: &egui::Context, engine: &Engine) {
        if self.palette.choosing() {
            return;
        }
        match self.resolve.window(ctx, engine) {
            Some(ResolveAction::Choose { query, instances }) => {
                self.palette.open_assign(query, instances);
            }
            Some(ResolveAction::Select(id)) => {
                self.select(Some(id), None, engine.project());
            }
            Some(ResolveAction::NewPart { component, values }) => {
                self.resolve.close();
                self.new_part = Some((component.id, values));
            }
            Some(ResolveAction::Close) | None => {}
        }
    }

    /// New Part… from Resolve parts or Choose…, once.
    pub fn take_new_part(&mut self) -> Option<NewPartRequest> {
        self.new_part.take()
    }

    /// The Place palette, for the app's library sources (S3): its context,
    /// its events and the other-revision choice.
    pub fn palette_mut(&mut self) -> &mut PlacePalette {
        &mut self.palette
    }

    /// Where a library folder placement goes: the sheet-grid point nearest
    /// the view centre, inside the view, where a nominal symbol box with
    /// clearance clears every drawn symbol. The folder's symbol is not in
    /// the project before the transaction, so its own size is not known.
    /// With no free point in view it is the view centre.
    pub fn place_point(&self) -> pcbcraft_core::geometry::Point {
        let scale = f64::from(self.scale).max(f64::EPSILON);
        let snap = |v: f64| (v / SHEET_GRID_MM).round() * SHEET_GRID_MM;
        let centre = [
            snap(f64::from(-self.pan.x) / scale),
            snap(f64::from(self.pan.y) / scale),
        ];
        // Half the view in mm, less the box, bounds the search.
        let reach = [
            (f64::from(self.view.x) / scale / 2.0 - PLACE_HALF[0]).max(0.0),
            (f64::from(self.view.y) / scale / 2.0 - PLACE_HALF[1]).max(0.0),
        ];
        let free = |at: [f64; 2]| {
            !self.placed_bounds.values().any(|b| {
                at[0] - PLACE_HALF[0] < b.max[0]
                    && at[0] + PLACE_HALF[0] > b.min[0]
                    && at[1] - PLACE_HALF[1] < b.max[1]
                    && at[1] + PLACE_HALF[1] > b.min[1]
            })
        };
        let mut candidates: Vec<[f64; 2]> = (-8i8..=8)
            .flat_map(|row| (-8i8..=8).map(move |column| (row, column)))
            .map(|(row, column)| {
                [
                    f64::from(column) * PLACE_PITCH[0],
                    f64::from(row) * PLACE_PITCH[1],
                ]
            })
            .filter(|d| d[0].abs() <= reach[0] && d[1].abs() <= reach[1])
            .collect();
        // Nearest first; ties right, then down, so equal inputs give one
        // answer.
        candidates.sort_by(|a, b| {
            let distance = |d: &[f64; 2]| d[0].hypot(d[1]);
            distance(a)
                .total_cmp(&distance(b))
                .then(b[1].total_cmp(&a[1]))
                .then(a[0].total_cmp(&b[0]))
        });
        candidates
            .into_iter()
            .map(|d| [snap(centre[0] + d[0]), snap(centre[1] + d[1])])
            .find(|at| free(*at))
            .unwrap_or(centre)
    }

    /// After a library placement: select the new instance and bring it into
    /// view when it is outside.
    pub fn reveal_placed(
        &mut self,
        instance: Id,
        position: pcbcraft_core::geometry::Point,
        project: &Project,
    ) {
        self.select_instance(Some(instance), project);
        let scale = self.scale;
        let at = Vec2::new(position[0] as f32, -position[1] as f32) * scale;
        let half = self.view / 2.0 - Vec2::new(PLACE_HALF[0] as f32, PLACE_HALF[1] as f32) * scale;
        let screen = at + self.pan;
        if screen.x.abs() > half.x.max(0.0) || screen.y.abs() > half.y.max(0.0) {
            self.pan = -at;
        }
    }

    /// Open the update review for `change` (the palette's "Update project
    /// copy" after `library.import`), with a checkbox per instance and
    /// `note` (what the import already did) above the changes.
    pub fn review_update(&mut self, engine: &Engine, change: Change, note: Option<String>) {
        let _ = self.stage_update(engine, change.clone());
        self.update.begin_choosing(engine, change, note);
    }

    /// How the last update review closed, once.
    pub fn take_review_closed(&mut self) -> Option<crate::update_preview::Closed> {
        self.update.take_closed()
    }

    /// The library sources' update marks, for the instance Update… row.
    pub fn set_updates(&mut self, updates: Arc<Vec<pcbcraft_core::catalog::UpdateAvailable>>) {
        self.updates = updates;
    }

    /// An instance's Update… for a revision a library source holds, once.
    pub fn take_library_update(&mut self) -> Option<(Id, crate::library_sources::FolderPick)> {
        self.library_update.take()
    }

    fn stage_update(&mut self, engine: &Engine, change: Change) -> Option<Operation> {
        self.update.begin(engine, change);
        self.tool = Tool::Select;
        self.wire_from = None;
        self.drag = None;
        self.preview = None;
        None
    }

    fn refresh_connectivity(&mut self, engine: &Engine) {
        if self.nets_revision != Some(engine.revision()) {
            self.nets = schematic::nets(engine.project()).to_vec();
            self.wire_nets = pcbcraft_core::nets::wire_nets(engine.project());
            self.dots = schematic::junction_dots(engine.project());
            self.pin_marks = pin_marks(engine.project());
            self.placed = Arc::new(
                engine
                    .project()
                    .occurrences
                    .iter()
                    .filter_map(|o| {
                        Some((o.id, schematic::placed_unit(engine.project(), o.id).ok()?))
                    })
                    .collect(),
            );
            // Generous: a full em per character, so no drawn text sticks
            // out of the box the culling uses.
            let wide = TextMetric { advance: 1.0 };
            self.placed_bounds = self
                .placed
                .iter()
                .filter_map(|(id, p)| Some((*id, p.bounds(wide)?)))
                .collect();
            self.nets_revision = Some(engine.revision());
        }
    }

    pub fn ribbon(&mut self, engine: &Engine) -> crate::shell::Ribbon {
        use crate::shell::{Group, Ribbon, Tab, Tool as Button};
        let reviewing = self.update.is_open();
        let has_parts = !engine.project().library.components.is_empty();
        let arrangeable = !reviewing && self.arranged(engine.project(), Arrange::Rotate).is_some();
        Ribbon {
            tabs: vec![Tab {
                id: "sheet",
                label: "Sheet",
                active: true,
            }],
            groups: vec![
                Group {
                    label: "Select",
                    tools: vec![
                        Button::new("select", "select", "Select")
                            .selected(self.tool == Tool::Select)
                            .tip("Select and move symbols and wires"),
                    ],
                },
                Group {
                    label: "Create",
                    tools: vec![
                        Button::new("place", "component", "Place")
                            .enabled(has_parts && !reviewing)
                            .selected(self.tool == Tool::Place)
                            .tip("Find a component and place it"),
                        Button::new("wire", "line", "Wire")
                            .enabled(!reviewing)
                            .selected(self.tool == Tool::Wire)
                            .tip("Connect two terminals"),
                    ],
                },
                Group {
                    label: "Arrange",
                    tools: vec![
                        Button::new("rotate", "spin", "Rotate")
                            .enabled(arrangeable)
                            .tip("Turn the selected symbol 90° counter-clockwise"),
                        Button::new("mirror", "mirror", "Mirror")
                            .enabled(arrangeable)
                            .tip("Mirror the selected symbol left to right"),
                    ],
                },
                Group {
                    label: "Inspect",
                    tools: vec![
                        Button::new("resolve", "component", "Resolve")
                            .enabled(!reviewing)
                            .selected(self.resolve.is_open())
                            .tip("Resolve parts: choose one manufacturer part per group of instances"),
                    ],
                },
            ],
            notice: None,
        }
    }

    pub fn on_tool(&mut self, tool: &str, engine: &Engine) -> Option<Operation> {
        let next = match tool {
            "select" => Tool::Select,
            "place" => Tool::Place,
            "wire" => Tool::Wire,
            "rotate" => return self.arranged(engine.project(), Arrange::Rotate),
            "mirror" => return self.arranged(engine.project(), Arrange::Mirror),
            "resolve" => {
                if self.resolve.is_open() {
                    self.resolve.close();
                } else {
                    self.select(None, None, engine.project());
                    self.palette.close();
                    self.tool = Tool::Select;
                    self.resolve.open();
                }
                return None;
            }
            _ => return None,
        };
        // A creation tool takes over the panel, so the selection is released.
        if next != Tool::Select {
            self.select(None, None, engine.project());
        }
        if next == Tool::Place {
            // One tool owns the card: Place closes Resolve parts.
            self.resolve.close();
            self.palette.open();
        } else {
            self.palette.close();
        }
        self.tool = next;
        self.wire_from = None;
        None
    }

    /// What the Properties card shows now.
    fn subject(&self, project: &Project) -> Subject {
        if self.update.is_open() {
            return Subject::Review;
        }
        // Choose part docks as a dialog in the subject slot and Resolve
        // parts is a window, so neither is a subject of the card.
        if self.palette.is_open() && !self.palette.choosing() {
            return Subject::Place;
        }
        if let Some(id) = self.selected.filter(|id| project.instance(*id).is_some()) {
            return Subject::Instance(id);
        }
        if let Some(id) = self
            .selected_wire
            .filter(|id| project.wires.iter().any(|w| w.id == *id))
        {
            return Subject::Wire(id);
        }
        match self.tool {
            Tool::Place => Subject::Place,
            Tool::Wire => Subject::Wiring,
            Tool::Select => Subject::Sheet,
        }
    }

    pub fn header(&self, engine: &Engine) -> crate::shell::Header {
        use crate::shell::Header;
        let project = engine.project();
        match self.subject(project) {
            Subject::Review => Header::new("component", "Review change", "Review"),
            Subject::Place => Header::new("component", "Place component", "Library"),
            Subject::Instance(id) => {
                let title = project.instance(id).map_or_else(String::new, |instance| {
                    // "R1 · 10 kΩ": the first value the symbol shows.
                    let value = instances::effective_fields(&project.library, instance)
                        .unwrap_or_default()
                        .into_iter()
                        .find(|f| f.show_on_symbol)
                        .and_then(|f| f.text);
                    match value {
                        Some(value) => format!("{} · {value}", instance.designator),
                        None => instance.designator.clone(),
                    }
                });
                Header::new("component", title, "Instance")
            }
            Subject::Wire(_) => {
                let name = self
                    .selected_net(engine)
                    .and_then(|id| project.net(id))
                    .map_or_else(|| "Wire".to_owned(), |n| n.name.clone());
                Header::new("line", name, "Net")
            }
            Subject::Wiring => Header::new("line", "Wire", "Tool"),
            Subject::Sheet => Header::new("sketch", "Sheet", "Schematic"),
        }
    }

    /// The Properties card of the current subject: header, the subject's
    /// identity (the body scroll resets when it changes), whether it has a
    /// pinned footer and its issue slot.
    fn inspector_card(&mut self, engine: &Engine) -> Inspector {
        let subject = self.subject(engine.project());
        Inspector {
            header: self.header(engine),
            subject: egui::Id::new(("schematic-card", subject)),
            footer: matches!(subject, Subject::Instance(_) | Subject::Wire(_)),
            issues: self.issue_slot(engine, subject),
        }
    }

    /// The issue slot of `subject`: what `project.issues` counts on it in
    /// the Schematic area ("on R1", "on N3", "on this sheet"), once per
    /// document revision. Subjects that are commands have no slot.
    fn issue_slot(&mut self, engine: &Engine, subject: Subject) -> Option<IssueSlot> {
        let document = Arc::as_ptr(engine.project_arc()) as usize;
        if let Some((revision, at, cached, slot)) = &self.slot
            && *revision == engine.revision()
            && *at == document
            && *cached == subject
        {
            return slot.clone();
        }
        let project = engine.project();
        let (scope, label) = match subject {
            Subject::Instance(id) => (
                IssueScope::Selection {
                    objects: vec![ObjectRef::Instance { id }],
                },
                project.instance(id).map(|i| format!("on {}", i.designator)),
            ),
            Subject::Wire(_) => {
                let net = self.selected_net(engine).and_then(|id| project.net(id));
                (
                    IssueScope::Selection {
                        objects: net
                            .map(|n| ObjectRef::Net { id: n.id })
                            .into_iter()
                            .collect(),
                    },
                    net.map(|n| format!("on {}", n.name)),
                )
            }
            Subject::Sheet => (
                IssueScope::Area {
                    area: IssueArea::Schematic,
                },
                Some("on this sheet".to_owned()),
            ),
            _ => (IssueScope::All, None),
        };
        let slot = label.and_then(|label| {
            let Ok(QueryResult::Issues(report)) = engine.query(Query::Issues { scope }) else {
                return None;
            };
            // The slot counts what the Schematic's Issues show for it.
            let mut counts = IssueCounts::default();
            for row in report
                .rows
                .iter()
                .filter(|r| r.area == IssueArea::Schematic)
            {
                let count = match row.severity {
                    Severity::Error => &mut counts.error,
                    Severity::Warning => &mut counts.warning,
                    Severity::ToDo => &mut counts.to_do,
                    Severity::Info => &mut counts.info,
                };
                *count = count.saturating_add(1);
            }
            Some(IssueSlot { counts, label })
        });
        self.slot = Some((engine.revision(), document, subject, slot.clone()));
        slot
    }

    /// Turn or mirror the selected occurrence on the sheet about its origin:
    /// one `symbol.orient_unit`. `None` without a selected occurrence or
    /// while a change is reviewed.
    fn arranged(&self, project: &Project, action: Arrange) -> Option<Operation> {
        if self.update.is_open() || self.tool != Tool::Select {
            return None;
        }
        let o = self
            .selected_occurrence
            .and_then(|o| project.occurrence(o))?;
        let (rotation, mirror) = match action {
            Arrange::Rotate => schematic::rotated_on_sheet(o.rotation, o.mirror),
            Arrange::Mirror => schematic::mirrored_on_sheet(o.rotation, o.mirror),
        };
        operation(SchematicCommand::OrientUnit {
            occurrence: o.id,
            rotation,
            mirror,
        })
    }

    pub fn fit(&mut self) {
        self.fit_requested = true;
    }

    pub fn selected_instance(&self) -> Option<Id> {
        self.selected
    }

    /// The net of the selected wire.
    pub fn selected_net(&self, engine: &Engine) -> Option<Id> {
        let project = engine.project();
        let wire = self.selected_wire.and_then(|id| project.wire(id))?;
        schematic::wire_net(project, wire).map(|n| n.id)
    }

    /// Select an instance from outside the canvas (the Browser).
    pub fn select_instance(&mut self, id: Option<Id>, project: &Project) {
        self.select(id, None, project);
        self.tool = Tool::Select;
    }

    /// Select a net through its first wire.
    pub fn select_net(&mut self, net: Id, project: &Project) {
        let wire = project
            .wires
            .iter()
            .find(|w| schematic::wire_net(project, w).is_some_and(|n| n.id == net));
        self.select(None, None, project);
        self.selected_wire = wire.map(|w| w.id);
        self.tool = Tool::Select;
    }

    /// Select an instance, through one of its occurrences when given.
    fn select(&mut self, id: Option<Id>, occurrence: Option<Id>, project: &Project) {
        if self.selected != id {
            self.refused = None;
            self.all_members = false;
        }
        self.selected = id;
        self.selected_occurrence = occurrence
            .or_else(|| id.and_then(|id| project.occurrences_of(id).next().map(|o| o.id)));
        self.selected_wire = None;
        if self.draft_id != id {
            if let Some(instance) = id.and_then(|id| project.instance(id)) {
                self.draft_reference = instance.designator.clone();
                self.draft_fields = shown_fields(project, instance);
            }
            self.draft_id = id;
        }
        if let Some(o) = self.selected_occurrence.and_then(|o| project.occurrence(o)) {
            self.draft_position = o.position;
            self.draft_rotation = o.rotation;
            self.draft_mirror = o.mirror;
        }
    }

    /// Unit states of the selected instance, read from `component.inspect`.
    fn report(&mut self, engine: &Engine) -> Option<&InstanceReport> {
        let instance = self.selected?;
        let fresh = self
            .report
            .as_ref()
            .is_some_and(|(revision, id, _)| *revision == engine.revision() && *id == instance);
        if !fresh {
            self.report = match engine.query(Query::InspectInstance { instance }) {
                Ok(QueryResult::Instance(report)) => Some((engine.revision(), instance, *report)),
                _ => None,
            };
        }
        self.report.as_ref().map(|(_, _, report)| report)
    }

    /// The Properties card's regions in one body, for tests that drive
    /// the card without the column: the body, then the issue slot and the
    /// footer actions at its end.
    #[cfg(test)]
    pub fn inspector(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        let card = self.inspector_card(engine);
        let mut output = self.card_region(ui, engine, Region::Body);
        let slot = card.issues.unwrap_or_default();
        kit::issue_line(ui, &slot.counts, &slot.label);
        if card.footer {
            kit::rule(ui);
            kit::actions(ui, |ui| {
                output = output
                    .take()
                    .or(self.card_region(ui, engine, Region::Footer));
            });
        }
        output
    }

    /// One region of the Properties card: the scrolling body or the pinned
    /// footer (object actions; never Apply).
    fn card_region(
        &mut self,
        ui: &mut egui::Ui,
        engine: &Engine,
        region: Region,
    ) -> Option<Operation> {
        match region {
            Region::Body => self.body(ui, engine),
            Region::Footer => self.footer(ui, engine),
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        self.refresh_connectivity(engine);
        if let Some(operation) = self.update.show(ui, engine) {
            return Some(operation);
        }
        if self.update.is_open() {
            return None;
        }
        let project = engine.project();
        if self.observed_revision != Some(engine.revision()) {
            self.draft_id = None;
            if self.selected.is_some() {
                let occurrence = self
                    .selected_occurrence
                    .filter(|o| project.occurrence(*o).is_some());
                self.select(self.selected, occurrence, project);
            }
            self.observed_revision = Some(engine.revision());
        }
        self.say_purchasing(ui.ctx(), project);
        if self
            .component
            .is_none_or(|c| project.library.component(c).is_none())
        {
            self.component = project
                .library
                .components
                .last()
                .map(|c| ComponentRef::new(c.id, c.revision));
        }
        if self
            .selected
            .is_some_and(|id| project.instance(id).is_none())
        {
            self.select(None, None, project);
        }
        match self.subject(project) {
            Subject::Review => None,
            Subject::Place => {
                if self.palette.is_open() {
                    let _ = self.palette.preview(ui, engine);
                } else {
                    self.place_section(ui, engine);
                }
                None
            }
            Subject::Instance(id) => {
                let instance = project.instance(id)?;
                self.instance_sections(ui, engine, instance)
            }
            Subject::Wire(id) => {
                let wire = project.wires.iter().find(|w| w.id == id)?;
                self.net_section(ui, engine, wire)
            }
            Subject::Wiring => {
                self.wiring_section(ui, project);
                None
            }
            Subject::Sheet => {
                self.sheet_section(ui, project);
                None
            }
        }
    }

    /// The pinned footer's object actions: Remove… for an instance,
    /// Disconnect wire… for a wire. Both open the update review.
    fn footer(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        let change = match self.subject(engine.project()) {
            Subject::Instance(instance) => ui_kit::button(ui, "Remove…", Button::Normal)
                .on_hover_text(
                    "Review what removing it changes on the schematic, the PCB and its tracks",
                )
                .clicked()
                .then_some(Change::Instance {
                    edit: InstanceChange::Remove { instance },
                }),
            Subject::Wire(wire) => ui_kit::button(ui, "Disconnect wire…", Button::Normal)
                .on_hover_text("Review what disconnecting it changes before it applies")
                .clicked()
                .then_some(Change::Schematic {
                    command: SchematicCommand::Disconnect { wire },
                }),
            _ => None,
        }?;
        self.stage_update(engine, change)
    }

    /// After a property commit: a To do toast when it left the instance's
    /// purchasing unresolved, with Undo.
    fn say_purchasing(&mut self, ctx: &egui::Context, project: &Project) {
        let Some((id, before)) = self.purchasing_watch.take() else {
            return;
        };
        if before != PurchasingState::Resolved {
            return;
        }
        if let Some(instance) = project.instance(id)
            && pcbcraft_core::completeness::purchasing(project, instance).state
                == PurchasingState::Unresolved
        {
            crate::kit::toast::post(
                ctx,
                crate::kit::severity::Level::ToDo,
                format!("Purchasing unresolved for {}", instance.designator),
                vec![crate::kit::toast::ToastAction::Undo],
            );
        }
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui, engine: &Engine) -> Option<Operation> {
        self.refresh_connectivity(engine);
        self.resolve_window(ui.ctx(), engine);
        let project = engine.project();
        let t = Tokens::get();
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        self.view = rect.size();
        if self.fit_requested {
            let mut bounds = Rect::NOTHING;
            for placed in self.placed.values() {
                if let Some(extent) = placed.bounds(TextMetric::default()) {
                    // Sheet extent with y flipped to screen orientation.
                    bounds = bounds.union(Rect::from_min_max(
                        pos2(extent.min[0] as f32, -extent.max[1] as f32),
                        pos2(extent.max[0] as f32, -extent.min[1] as f32),
                    ));
                }
            }
            if bounds.is_finite() {
                bounds = bounds.expand(5.0);
                self.scale = (rect.width() / bounds.width())
                    .min(rect.height() / bounds.height())
                    .clamp(2.0, 60.0);
                self.pan = -bounds.center().to_vec2() * self.scale;
            } else {
                self.pan = Vec2::ZERO;
                self.scale = 12.0;
            }
            self.fit_requested = false;
        }
        if let Some(bounds) = self.frame_request.take() {
            self.frame_bounds(bounds, rect);
        }
        let mut output = None;
        // The wheel and a pinch zoom about the pointer; two fingers pan.
        let wheel = self.trackpad.read(ui, false);
        if response.contains_pointer() && wheel.moves() {
            self.pan += wheel.pan;
            let old_scale = self.scale;
            self.scale = (self.scale * wheel.scale_factor()).clamp(2.0, 60.0);
            if let Some(pointer) = response.hover_pos() {
                let relative = pointer - rect.center();
                self.pan = relative - (relative - self.pan) * (self.scale / old_scale);
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            self.pan += ui.input(|i| i.pointer.delta());
        }
        if !self.palette.is_open()
            && !self.update.is_open()
            && !project.library.components.is_empty()
            && !ui.ctx().egui_wants_keyboard_input()
            && !ui.ctx().any_popup_open()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::P))
        {
            self.select(None, None, project);
            self.tool = Tool::Place;
            self.wire_from = None;
            self.palette.open();
        }
        // Cmd/Ctrl+R and Shift+X as in the Component editor's symbol canvas.
        if !self.palette.is_open() && !ui.ctx().egui_wants_keyboard_input() {
            for (modifiers, key, action) in [
                (egui::Modifiers::COMMAND, egui::Key::R, Arrange::Rotate),
                (egui::Modifiers::SHIFT, egui::Key::X, Arrange::Mirror),
            ] {
                if let Some(op) = self.arranged(project, action)
                    && ui.input_mut(|i| i.consume_key(modifiers, key))
                {
                    output = Some(op);
                }
            }
        }
        // The palette owns Esc, arrows and Enter while it is open.
        let placeable = |c: ComponentRef| first_choices(project, c).is_some();
        match self.palette.show(ui, rect, engine, &placeable) {
            Some(PaletteAction::Place {
                component,
                keep_open,
            }) => {
                if let Some((request, unit)) = first_choices(project, component) {
                    output = Some(Operation::Instance(InstanceCommand::Place(NewInstance {
                        position: self.drop_point(
                            project,
                            unit,
                            rect,
                            engine.drafts().grid.symbol_mm,
                        ),
                        ..request
                    })));
                }
                self.component = Some(component);
                if !keep_open {
                    self.tool = Tool::Select;
                }
            }
            Some(PaletteAction::Pick(component)) => {
                self.component = Some(component);
                self.tool = Tool::Place;
            }
            Some(PaletteAction::Assign { instances, part }) => {
                output = Some(resolve_parts::assign(instances, Some(part)));
            }
            Some(PaletteAction::UpdateAndAssign {
                instances,
                part,
                target,
            }) => {
                output = self.stage_update(
                    engine,
                    resolve_parts::update_and_assign(instances, part, target),
                );
            }
            Some(PaletteAction::Close) => self.tool = Tool::Select,
            None => {}
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.wire_from = None;
            self.tool = Tool::Select;
            self.drag = None;
            self.preview = None;
        }
        let origin = rect.center() + self.pan;
        let scale = self.scale;
        let view = symbol_view::View { origin, scale };
        let screen = |p: Point| view.screen(p);
        // Occurrence origins snap to the symbol grid, so on-grid pins land
        // on it too.
        let grid = engine.drafts().grid.symbol_mm;
        let world = |p: Pos2| snap(view.world(p), grid);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0, t.viewport_bottom);
        let step = 5.0 * scale;
        let start = pos2(
            rect.left() + (origin.x - rect.left()).rem_euclid(step),
            rect.top() + (origin.y - rect.top()).rem_euclid(step),
        );
        let mut x = start.x;
        while x < rect.right() {
            let mut y = start.y;
            while y < rect.bottom() {
                painter.circle_filled(pos2(x, y), 0.8, t.grid);
                y += step;
            }
            x += step;
        }

        let mut pin_hits = Vec::new();
        let mut body_hits = Vec::new();
        let removing = self.update.marks().map(|m| m.removed).unwrap_or_default();
        let mut hit_wire = None;
        // The net being wired from or the selected wire's net.
        let selected_net: Option<Id> = match self.wire_from {
            Some(WireTarget::Terminal(e)) => self
                .nets
                .iter()
                .find(|n| n.terminals.contains(&e))
                .map(|n| n.id),
            Some(WireTarget::Junction(j)) => project
                .wires_at(WireEnd::Junction(j))
                .find_map(|w| self.wire_nets.get(&w.id).copied()),
            Some(WireTarget::OnWire(p)) => self.wire_nets.get(&p.wire).copied(),
            None => self
                .selected_wire
                .and_then(|id| self.wire_nets.get(&id).copied()),
        };
        let wire_active = |id: Id| {
            self.selected_wire == Some(id)
                || selected_net.is_some_and(|n| self.wire_nets.get(&id) == Some(&n))
        };
        // Each drawn wire's screen path, for picking a wire or a point on it.
        let mut wire_paths: Vec<(Id, [Pos2; 3])> = Vec::with_capacity(project.wires.len());
        for wire in &project.wires {
            if let Some((a, b)) = schematic::wire_ends(project, wire) {
                let a = self.preview_end(project, wire.from, a);
                let b = self.preview_end(project, wire.to, b);
                let points = schematic::wire_route(a, b).map(screen);
                let active = wire_active(wire.id);
                painter.add(egui::Shape::line(
                    points.to_vec(),
                    Stroke::new(
                        if active { 2.5 } else { 1.5 },
                        if active { t.accent } else { t.form_success },
                    ),
                ));
                if response.clicked()
                    && self.tool == Tool::Select
                    && let Some(p) = response.interact_pointer_pos()
                    && points
                        .windows(2)
                        .any(|s| segment_distance(p, s[0], s[1]) < WIRE_HIT_PX)
                {
                    hit_wire = Some(wire.id);
                }
                wire_paths.push((wire.id, points));
            }
        }
        // Solid dots where three or more conductors meet, derived by core.
        let radius = dot_radius(scale);
        let mut junction_hits = Vec::new();
        for (end, at) in &self.dots {
            let p = screen(self.preview_end(project, *end, *at));
            let active = project.wires_at(*end).any(|w| wire_active(w.id));
            painter.circle_filled(p, radius, if active { t.accent } else { t.form_success });
        }
        for junction in &project.junctions {
            junction_hits.push((WireEnd::junction(junction.id), screen(junction.position)));
        }
        if self.tool == Tool::Place
            && let Some(component) = self.component
            && self
                .ghost
                .as_ref()
                .is_none_or(|(c, revision, _)| *c != component || *revision != engine.revision())
        {
            self.ghost = ghost_unit(project, component).map(|u| (component, engine.revision(), u));
        }
        let m = symbol_view::metric(&painter);
        let tolerance = f64::from(HIT_PX / scale);
        let placed_units = Arc::clone(&self.placed);
        // The sheet area in view, with a margin for pin markers and strokes;
        // occurrences wholly outside it are neither drawn nor picked.
        let visible = Bounds::point(view.world(rect.left_bottom()))
            .including(view.world(rect.right_top()))
            .inflated(f64::from(12.0 / scale.max(f32::EPSILON)));
        for occurrence in &project.occurrences {
            let Some(placed) = placed_units.get(&occurrence.id) else {
                continue;
            };
            // A dragged occurrence is drawn and picked at its preview.
            let delta = self
                .preview
                .filter(|(id, _)| *id == occurrence.id)
                .map_or([0.0, 0.0], |(_, p)| {
                    [p[0] - occurrence.position[0], p[1] - occurrence.position[1]]
                });
            if let Some(b) = self.placed_bounds.get(&occurrence.id) {
                let moved = Bounds {
                    min: [b.min[0] + delta[0], b.min[1] + delta[1]],
                    max: [b.max[0] + delta[0], b.max[1] + delta[1]],
                };
                if !moved.intersects(&visible) {
                    continue;
                }
            }
            let view = symbol_view::View {
                origin: screen(delta),
                scale,
            };
            // Picking uses the cached resolved drawing, so what is drawn
            // is what is picked and nothing re-resolves per frame.
            body_hits.push((occurrence.instance, occurrence.id, placed, delta));
            // A review that removes the instance previews the result: the
            // symbol is dimmed, not drawn as selected.
            let removed = removing.contains(&occurrence.instance);
            let selected = !removed && self.selected == Some(occurrence.instance);
            let dim = |c: egui::Color32| if removed { c.gamma_multiply(0.3) } else { c };
            symbol_view::paint(
                &painter,
                view,
                placed,
                symbol_view::Ink {
                    stroke: dim(if selected { t.accent } else { t.text }),
                    body: t.panel,
                    text: dim(if selected { t.accent } else { t.text }),
                    pin: dim(t.form_success),
                    pin_text: dim(t.text_dim),
                },
            );
            for pin in &placed.pins {
                let endpoint = Endpoint {
                    instance: occurrence.instance,
                    terminal: pin.terminal,
                };
                let p = view.screen(pin.end);
                if self.wire_from == Some(WireTarget::Terminal(endpoint)) {
                    painter.circle_stroke(p, 5.0, Stroke::new(1.5, t.accent));
                } else {
                    paint_pin_mark(
                        &painter,
                        p,
                        self.pin_marks
                            .get(&endpoint)
                            .copied()
                            .unwrap_or(PinMark::ToDo),
                        dim,
                    );
                }
                pin_hits.push((endpoint, p));
            }
        }
        // The topmost occurrence whose drawing core hits under `p`.
        let body_at = |p: Pos2| {
            body_hits
                .iter()
                .rev()
                .find(|(_, _, placed, delta)| {
                    let [x, y] = view.world(p);
                    placed
                        .hit([x - delta[0], y - delta[1]], tolerance, m)
                        .is_some()
                })
                .map(|(id, occurrence, _, _)| (*id, *occurrence))
        };
        let wiring = self.tool == Tool::Wire && !self.palette.is_open();
        // Where a Wire tool click at `pointer` lands: a pin end, then a
        // junction, then a point on a wire's path (snapped to the grid when
        // that stays on the wire), with its screen position.
        let target_at = |pointer: Pos2| -> Option<(WireTarget, Pos2)> {
            let nearest = |hits: &[(WireTarget, Pos2)]| {
                hits.iter()
                    .filter(|(_, p)| p.distance(pointer) < PIN_HIT_PX)
                    .min_by(|a, b| a.1.distance(pointer).total_cmp(&b.1.distance(pointer)))
                    .copied()
            };
            let pins: Vec<(WireTarget, Pos2)> = pin_hits
                .iter()
                .map(|(e, p)| (WireTarget::Terminal(*e), *p))
                .collect();
            let junctions: Vec<(WireTarget, Pos2)> = junction_hits
                .iter()
                .map(|(e, p)| (WireTarget::from(*e), *p))
                .collect();
            nearest(&pins).or_else(|| nearest(&junctions)).or_else(|| {
                let (wire, _) = wire_paths
                    .iter()
                    .map(|(id, path)| {
                        let d = path
                            .windows(2)
                            .map(|s| segment_distance(pointer, s[0], s[1]))
                            .fold(f32::INFINITY, f32::min);
                        (*id, d)
                    })
                    .filter(|(_, d)| *d < WIRE_HIT_PX)
                    .min_by(|a, b| a.1.total_cmp(&b.1))?;
                let here = view.world(pointer);
                let on = schematic::nearest_on_wire(project, wire, here)?;
                let snapped = snap(on, grid);
                let at = schematic::nearest_on_wire(project, wire, snapped)
                    .filter(|p| {
                        (p[0] - snapped[0]).hypot(p[1] - snapped[1])
                            <= schematic::JUNCTION_TOLERANCE_MM
                    })
                    .map_or(on, |_| snapped);
                Some((WireTarget::OnWire(WirePoint { wire, at }), screen(at)))
            })
        };
        if let Some(pointer) = response.hover_pos() {
            let hit = pin_hits
                .iter()
                .filter(|(_, p)| p.distance(pointer) < PIN_HIT_PX)
                .min_by(|a, b| a.1.distance(pointer).total_cmp(&b.1.distance(pointer)));
            let landing = if wiring { target_at(pointer) } else { None };
            if let Some((_, p)) = landing {
                painter.circle_stroke(p, 7.0, Stroke::new(1.0, t.accent));
            } else if let Some((_, p)) = hit {
                painter.circle_stroke(*p, 7.0, Stroke::new(1.0, t.accent));
            }
            if self.tool == Tool::Place
                && !self.palette.is_open()
                && let Some((_, _, unit)) = self
                    .ghost
                    .as_ref()
                    .filter(|(c, _, _)| Some(*c) == self.component)
            {
                // The ghost outlines the shapes and pin stubs core resolves
                // for the unit, at the snapped pointer.
                let ghost = symbol_view::View {
                    origin: screen(world(pointer)),
                    scale,
                };
                let stroke = Stroke::new(1.5, t.accent);
                for shape in &unit.shapes {
                    symbol_view::paint_shape(&painter, ghost, &shape.shape, None, Some(stroke));
                }
                for pin in &unit.pins {
                    painter
                        .line_segment([ghost.screen(pin.end), ghost.screen(pin.body_end)], stroke);
                }
            }
            if let Some(from) = self.wire_from.and_then(|e| target_position(project, e)) {
                // The route core will draw: along x first, then y.
                let a = screen(from);
                let b = landing.map_or(pointer, |(_, p)| p);
                painter.add(egui::Shape::line(
                    vec![a, pos2(b.x, a.y), b],
                    Stroke::new(1.5, t.accent),
                ));
            }
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let pin = pin_hits
                .iter()
                .filter(|(_, p)| p.distance(pointer) < PIN_HIT_PX)
                .min_by(|a, b| a.1.distance(pointer).total_cmp(&b.1.distance(pointer)))
                .map(|(e, _)| *e);
            match self.tool {
                // A click beside the open assign palette dismisses it.
                Tool::Select | Tool::Wire if self.palette.is_open() => self.palette.close(),
                // A click beside the open palette dismisses it.
                Tool::Place if self.palette.is_open() => {
                    self.palette.close();
                    self.tool = Tool::Select;
                }
                Tool::Place => {
                    if let Some((request, _)) =
                        self.component.and_then(|c| first_choices(project, c))
                    {
                        output = Some(Operation::Instance(InstanceCommand::Place(NewInstance {
                            position: world(pointer),
                            ..request
                        })));
                    }
                }
                Tool::Wire => {
                    if let Some((to, _)) = target_at(pointer) {
                        if let Some(from) = self.wire_from.take() {
                            if from != to {
                                output = self.stage_update(
                                    engine,
                                    Change::Schematic {
                                        command: SchematicCommand::Connect { from, to },
                                    },
                                );
                            }
                        } else {
                            self.wire_from = Some(to);
                        }
                    }
                }
                Tool::Select => {
                    let body = body_at(pointer);
                    let id = pin.map(|p| p.instance).or(body.map(|b| b.0));
                    if id.is_some() {
                        self.select(id, body.map(|b| b.1), project);
                    } else {
                        self.select(None, None, project);
                        self.selected_wire = hit_wire;
                    }
                }
            }
        }
        if self.tool == Tool::Select
            && !self.update.is_open()
            && response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pointer) = ui.input(|i| i.pointer.press_origin())
            && let Some((id, occurrence_id)) = body_at(pointer)
            && let Some(occurrence) = project.occurrence(occurrence_id)
        {
            let p = view.world(pointer);
            self.drag = Some((
                occurrence_id,
                [p[0] - occurrence.position[0], p[1] - occurrence.position[1]],
            ));
            self.select(Some(id), Some(occurrence_id), project);
        }
        if let Some((id, offset)) = self.drag
            && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        {
            let p = view.world(pointer);
            self.preview = Some((id, snap([p[0] - offset[0], p[1] - offset[1]], grid)));
        }
        if response.drag_stopped_by(egui::PointerButton::Primary)
            && let Some((occurrence, position)) = self.preview.take()
        {
            self.drag = None;
            self.draft_position = position;
            output = operation(SchematicCommand::MoveUnit {
                occurrence,
                position,
            });
        }
        if let Some(hint) = canvas_hint(self, project) {
            paint_canvas_hint(&painter, rect, &hint);
        }
        output
    }

    /// Where the palette's Enter places `unit`: the free 5 mm grid point
    /// nearest the view centre whose symbol bounds (body and pin ends, plus
    /// a margin) clear every drawn symbol and lie inside the visible canvas
    /// `rect`, which already excludes the Browser and properties cards.
    /// Falls back to the view centre when the view has no free spot.
    fn drop_point(&self, project: &Project, unit: &Unit, rect: Rect, grid: f64) -> Point {
        const MARGIN: f32 = 2.5;
        // Candidates step by the whole number of symbol-grid pitches nearest
        // the 5 mm sheet grid, so every drop point is on the symbol grid.
        let pitch = grid.max(f64::EPSILON);
        let step = pitch * (SHEET_GRID_MM / pitch).round().max(1.0);
        let snap = |v: f64| (v / pitch).round() * pitch;
        let scale = f64::from(self.scale);
        let centre = [
            snap(f64::from(-self.pan.x) / scale),
            snap(f64::from(self.pan.y) / scale),
        ];
        // The visible world, y up.
        let half = [
            f64::from(rect.width()) / scale / 2.0,
            f64::from(rect.height()) / scale / 2.0,
        ];
        let view_centre = [
            f64::from(-self.pan.x) / scale,
            f64::from(self.pan.y) / scale,
        ];
        let visible = Rect::from_min_max(
            pos2(
                (view_centre[0] - half[0]) as f32,
                (view_centre[1] - half[1]) as f32,
            ),
            pos2(
                (view_centre[0] + half[0]) as f32,
                (view_centre[1] + half[1]) as f32,
            ),
        );
        let taken: Vec<Rect> = self
            .placed
            .values()
            .filter_map(|p| p.bounds(TextMetric::default()).map(sheet_rect))
            .chain(
                // Occurrences placed this revision are not resolved yet.
                project
                    .occurrences
                    .iter()
                    .filter(|o| !self.placed.contains_key(&o.id))
                    .filter_map(|o| {
                        Some(symbol_bounds(
                            unit_of(project, o)?,
                            schematic::occurrence_transform(o),
                        ))
                    }),
            )
            .collect();
        let steps = |extent: f64| ((extent / step).ceil() as i32).clamp(0, 64);
        let (nx, ny) = (steps(half[0]), steps(half[1]));
        let mut candidates: Vec<(f64, i32, i32)> = (-ny..=ny)
            .flat_map(|j| (-nx..=nx).map(move |i| (i, j)))
            .map(|(i, j)| {
                let d = f64::from(i * i + j * j);
                (d, j, i)
            })
            .collect();
        // Nearest first; ties go down the sheet, then to the right.
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        candidates
            .into_iter()
            .map(|(_, j, i)| {
                [
                    centre[0] + f64::from(i) * step,
                    centre[1] + f64::from(j) * step,
                ]
            })
            .find(|p| {
                let bounds = symbol_bounds(unit, Transform2::translation(*p)).expand(MARGIN);
                visible.contains_rect(bounds) && taken.iter().all(|r| !r.intersects(bounds))
            })
            .unwrap_or(centre)
    }

    /// Where a wire end is drawn while a dragged occurrence moves its pin.
    /// Junctions stay where they are.
    fn preview_end(&self, project: &Project, end: WireEnd, original: Point) -> Point {
        match end {
            WireEnd::Terminal(endpoint) => self.preview_endpoint(project, endpoint, original),
            WireEnd::Junction(_) => original,
        }
    }

    /// Where a wire end is drawn while its unit's occurrence is dragged.
    fn preview_endpoint(&self, project: &Project, endpoint: Endpoint, original: Point) -> Point {
        if let Some((id, position)) = self.preview
            && let Some(occurrence) = project.occurrence(id)
            && occurrence.instance == endpoint.instance
            && let Some(instance) = project.instance(endpoint.instance)
            && let Ok(resolved) = instances::resolve(&project.library, instance)
            && instances::owning_unit(&resolved.symbol.definition, endpoint.terminal)
                == Some(occurrence.unit)
        {
            return [
                original[0] + position[0] - occurrence.position[0],
                original[1] + position[1] - occurrence.position[1],
            ];
        }
        original
    }
}

/// Fields the pinned component shows on the symbol, with their effective
/// text as formatted in core.
fn shown_fields(
    project: &Project,
    instance: &pcbcraft_core::ComponentInstance,
) -> Vec<FieldBuffer> {
    instances::effective_fields(&project.library, instance)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.show_on_symbol)
        .map(|f| {
            let text = f.text.unwrap_or_default();
            FieldBuffer {
                field: f.field.id,
                revision: f.field,
                label: f.label,
                loaded: text.clone(),
                text,
            }
        })
        .collect()
}

/// Snap a sheet point to a grid of pitch `step` through the origin.
fn snap(p: Point, step: f64) -> Point {
    if !(step.is_finite() && step > 0.0) {
        return p;
    }
    p.map(|v| (v / step).round() * step)
}

/// A core box in sheet millimetres as an egui rectangle (y still up).
fn sheet_rect(b: Bounds) -> Rect {
    Rect::from_min_max(
        pos2(b.min[0] as f32, b.min[1] as f32),
        pos2(b.max[0] as f32, b.max[1] as f32),
    )
}

/// A unit's extent in sheet millimetres (y up) when placed through `t`,
/// from the core's unit bounds (graphics, approximate text and pin stubs),
/// or a point when it draws nothing.
fn symbol_bounds(unit: &Unit, t: Transform2) -> Rect {
    let at = t.translate;
    let local = unit
        .bounds(TextMetric::default())
        .unwrap_or(Bounds::point([0.0, 0.0]));
    let placed = Bounds::of(local.corners().map(|c| t.apply(c))).unwrap_or(Bounds::point(at));
    sheet_rect(placed)
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let delta = b - a;
    let length = delta.length_sq();
    if length <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p - a).dot(delta) / length).clamp(0.0, 1.0);
    p.distance(a + delta * t)
}

/// The one-line canvas hint for the current tool and selection, if any.
/// The card never repeats it.
fn canvas_hint(editor: &SchematicEditor, project: &Project) -> Option<String> {
    if editor.update.is_open() || editor.palette.is_open() || editor.resolve.is_open() {
        return None;
    }
    Some(match editor.tool {
        Tool::Wire if editor.wire_from.is_some() => {
            "Click a terminal or wire to end · Esc cancels".to_owned()
        }
        Tool::Wire => "Click a terminal or wire to start a wire".to_owned(),
        Tool::Place => match editor.component.and_then(|c| project.library.component(c)) {
            Some(component) => format!("Click to place {} · Esc ends", component.meta.name),
            None => "Publish a component to place it".to_owned(),
        },
        Tool::Select if editor.selected.is_some() => {
            "Drag to move · Cmd+R rotates · Shift+X mirrors".to_owned()
        }
        Tool::Select => "Click to select · drag to move · P places".to_owned(),
    })
}

/// Paint `text` as one dim line at the canvas's bottom-left corner, cut
/// short of the centred canvas buttons.
fn paint_canvas_hint(painter: &egui::Painter, rect: Rect, text: &str) {
    crate::kit::canvas_hint(painter, rect, Tokens::get().space_12, text);
}

impl crate::issues::Locate for SchematicEditor {
    /// Select what `refs` name on the sheet and bring it into view: the
    /// first instance (directly, or through a pad, unit, property, terminal
    /// or symbol occurrence), else the first net. The view frames every
    /// named symbol, terminal and net pin. Selection and view only; the
    /// document is never touched.
    fn locate(&mut self, engine: &Engine, refs: &[ObjectRef]) -> bool {
        self.refresh_connectivity(engine);
        let project = engine.project();
        let mut first: Option<(Id, Option<Id>)> = None;
        let mut net = None;
        let mut occurrences: Vec<Id> = Vec::new();
        let mut points: Vec<Point> = Vec::new();
        for object in refs {
            let (instance, unit) = match *object {
                ObjectRef::Instance { id }
                | ObjectRef::Pad { instance: id, .. }
                | ObjectRef::Property { instance: id, .. } => (Some(id), None),
                ObjectRef::Unit { instance, unit } => (Some(instance), Some(unit)),
                ObjectRef::Terminal { endpoint } => {
                    points.extend(schematic::pin_position(project, endpoint));
                    (Some(endpoint.instance), None)
                }
                ObjectRef::Occurrence { id } => {
                    if let Some(o) = project.occurrence(id) {
                        first = first.or(Some((o.instance, Some(o.id))));
                        occurrences.push(o.id);
                    }
                    (None, None)
                }
                ObjectRef::Net { id } => {
                    net = net.or(project.net(id).map(|n| n.id));
                    (None, None)
                }
                _ => (None, None),
            };
            if let Some(id) = instance.filter(|id| project.instance(*id).is_some()) {
                let of: Vec<Id> = project
                    .occurrences_of(id)
                    .filter(|o| unit.is_none_or(|u| o.unit == u))
                    .map(|o| o.id)
                    .collect();
                first = first.or(Some((id, of.first().copied())));
                occurrences.extend(of);
            }
        }
        if let Some(n) = net.and_then(|id| project.net(id)) {
            points.extend(
                n.terminals
                    .iter()
                    .filter_map(|e| schematic::pin_position(project, *e)),
            );
        }
        if first.is_none() && net.is_none() {
            return false;
        }
        let bounds = occurrences
            .iter()
            .filter_map(|o| self.placed_bounds.get(o).copied())
            .chain(points.into_iter().map(Bounds::point))
            .reduce(Bounds::union);
        self.palette.close();
        self.resolve.close();
        self.wire_from = None;
        match (first, net) {
            (Some((id, occurrence)), _) => {
                self.select(Some(id), occurrence, project);
                self.tool = Tool::Select;
            }
            (None, Some(net)) => self.select_net(net, project),
            (None, None) => {}
        }
        self.frame_request = bounds;
        true
    }
}

impl SchematicEditor {
    /// Bring a located object's sheet `bounds` into view: centred, and
    /// zoomed out only as far as it needs to fit.
    fn frame_bounds(&mut self, bounds: Bounds, rect: Rect) {
        const MARGIN_MM: f64 = 5.0;
        let b = bounds.inflated(MARGIN_MM);
        let width = (b.max[0] - b.min[0]).max(f64::EPSILON) as f32;
        let height = (b.max[1] - b.min[1]).max(f64::EPSILON) as f32;
        let fit = (rect.width() / width).min(rect.height() / height);
        if fit < self.scale {
            self.scale = fit.clamp(2.0, 60.0);
        }
        let [x, y] = b.center();
        // Sheet y is up, screen y down.
        self.pan = -Vec2::new(x as f32, -(y as f32)) * self.scale;
    }

    /// The Wire tool's card: where the wire starts, and one hint line.
    fn wiring_section(&mut self, ui: &mut egui::Ui, project: &Project) {
        kit::section(ui, "wiring", "Wire", true, None, |ui| {
            let start = self
                .wire_from
                .map_or_else(|| "Not started".to_owned(), |e| target_label(project, e));
            kit::value_row(ui, "Start", &start);
            kit::hint(
                ui,
                "Crossing wires are not joined",
                "A wire joins where its end lands on a pin or another wire; a dot marks the junction.",
            );
        });
    }
}

/// What a live card field did this frame (docs/sidebar-ia.md, principle
/// 13): nothing, a commit of its text, or Esc back to the stored value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Live {
    Idle,
    Commit,
    Revert,
}

/// One live text field after its row: Enter or focus loss commits text
/// that `changed`, Esc reverts it, and `invalid` text is marked under the
/// field and never commits. A refusal handed back for the field (its
/// operation was marked with [`Origin::Field`]) shows under it until the
/// field is edited again.
fn live(
    ui: &mut egui::Ui,
    response: &egui::Response,
    refused: &mut Option<(egui::Id, String)>,
    changed: bool,
    invalid: Option<&str>,
) -> Live {
    if let Some(message) = refusal::claim(ui.ctx(), Origin::Field(response.id)) {
        *refused = Some((response.id, message));
    }
    let own = |r: &Option<(egui::Id, String)>| r.as_ref().is_some_and(|(id, _)| *id == response.id);
    let state = if left_focus(ui, response) {
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            Live::Revert
        } else if changed && invalid.is_none() {
            Live::Commit
        } else {
            Live::Idle
        }
    } else {
        Live::Idle
    };
    if (response.changed() || state != Live::Idle) && own(refused) {
        *refused = None;
    }
    if let Some(message) = invalid.filter(|_| state != Live::Revert) {
        kit::field_error(ui, response, message, true);
    } else if let Some((_, message)) = refused.as_ref().filter(|_| own(refused)) {
        kit::field_error(ui, response, message, true);
    }
    state
}

/// Whether the field gave up keyboard focus this frame. egui reports a
/// focus surrendered by Enter or Esc on that frame and again on the next,
/// so the repeat is ignored: one edit commits once.
fn left_focus(ui: &egui::Ui, response: &egui::Response) -> bool {
    let key = response.id.with("live-left");
    let frame = ui.ctx().cumulative_frame_nr();
    let last = ui.data(|d| d.get_temp::<u64>(key));
    if !response.lost_focus() {
        return false;
    }
    ui.data_mut(|d| d.insert_temp(key, frame));
    last.is_none_or(|last| last.saturating_add(1) != frame)
}

/// Whether keyboard focus left the controls inside `rect` this frame, and
/// whether it left by Esc: `Some(true)` reverts, `Some(false)` commits.
/// Moving between controls of the row keeps it, so a pair commits together.
fn focus_left(ui: &egui::Ui, key: egui::Id, rect: Rect) -> Option<bool> {
    let ctx = ui.ctx();
    let inside = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
        .is_some_and(|r| rect.contains(r.rect.center()));
    let was = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);
    ui.data_mut(|d| d.insert_temp(key, inside));
    (was && !inside).then(|| ui.input(|i| i.key_pressed(egui::Key::Escape)))
}

/// Paint a pin end's mark: nothing for a wired end, the Issues to-do ring
/// for one still needing a decision, a cross for a decided no-connect.
fn paint_pin_mark(
    painter: &egui::Painter,
    p: Pos2,
    mark: PinMark,
    dim: impl Fn(egui::Color32) -> egui::Color32,
) {
    let t = Tokens::get();
    match mark {
        PinMark::Wired => {}
        PinMark::ToDo => {
            let ink = dim(kit::severity::ink(kit::severity::Level::ToDo));
            painter.circle_stroke(p, 3.5, Stroke::new(1.5, ink));
        }
        PinMark::NoConnect => {
            let ink = dim(t.text_dim);
            let h = 3.0;
            let stroke = Stroke::new(1.5, ink);
            painter.line_segment([p + egui::vec2(-h, -h), p + egui::vec2(h, h)], stroke);
            painter.line_segment([p + egui::vec2(-h, h), p + egui::vec2(h, -h)], stroke);
        }
    }
}

/// Where a Wire tool target sits on the sheet.
fn target_position(project: &Project, target: WireTarget) -> Option<Point> {
    match target {
        WireTarget::Terminal(e) => schematic::pin_position(project, e),
        WireTarget::Junction(j) => project.junction(j.junction).map(|j| j.position),
        WireTarget::OnWire(p) => Some(p.at),
    }
}

/// A wire end as people name it: a terminal label or "Junction".
fn end_label(project: &Project, end: WireEnd) -> String {
    match end {
        WireEnd::Terminal(e) => endpoint_label(project, e),
        WireEnd::Junction(_) => "Junction".to_owned(),
    }
}

/// A Wire tool target as people name it.
fn target_label(project: &Project, target: WireTarget) -> String {
    match target {
        WireTarget::Terminal(e) => endpoint_label(project, e),
        WireTarget::Junction(_) => "Junction".to_owned(),
        WireTarget::OnWire(p) => {
            let net = project
                .wire(p.wire)
                .and_then(|w| schematic::wire_net(project, w));
            net.map_or_else(|| "Wire".to_owned(), |n| format!("Wire on {}", n.name))
        }
    }
}

/// A terminal as people name it: the instance's designator and the
/// terminal's name ("R1 pin 2", "U3 pin VOUT").
fn endpoint_label(project: &Project, endpoint: Endpoint) -> String {
    let Some(instance) = project.instance(endpoint.instance) else {
        return format!("pin {}", endpoint.terminal);
    };
    let name = instances::resolve(&project.library, instance)
        .ok()
        .and_then(|r| {
            r.component
                .definition
                .terminal(endpoint.terminal)
                .map(|t| t.name.clone())
        })
        .unwrap_or_else(|| endpoint.terminal.to_string());
    format!("{} pin {name}", instance.designator)
}

#[cfg(test)]
mod junction_tests;
#[cfg(test)]
mod part_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit;
    use egui::vec2;
    use pcbcraft_core::fields::PropertyInput;
    use pcbcraft_core::functions::{FunctionEdit, TerminalFunctions};
    use pcbcraft_core::updates::InstanceChange;
    use solidcraft_ui_egui::fonts;

    #[test]
    fn placed_instance_removal_opens_review_without_mutating_the_board() -> pcbcraft_core::Result<()>
    {
        let engine = Engine::demo()?;
        let before = engine.project().clone();
        let r1 = engine.project().instances[0].id;
        let occurrence = engine.project().occurrences[0].id;
        let mut editor = SchematicEditor {
            wire_from: Some(WireTarget::Terminal(Endpoint {
                instance: r1,
                terminal: pcbcraft_core::TerminalId(2),
            })),
            drag: Some((occurrence, [0.0, 0.0])),
            preview: Some((occurrence, [10.0, 10.0])),
            ..Default::default()
        };
        assert!(
            editor
                .stage_update(
                    &engine,
                    Change::Instance {
                        edit: InstanceChange::Remove { instance: r1 }
                    }
                )
                .is_none()
        );
        assert!(editor.update.is_open());
        assert!(editor.wire_from.is_none());
        assert!(editor.drag.is_none());
        assert!(editor.preview.is_none());
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    fn frame(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Option<Operation>) {
        let mut command = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 1400.0))),
                events,
                ..Default::default()
            },
            |ui| command = kit::form(ui, |ui| editor.inspector(ui, engine)),
        );
        // Tests have no renderer to apply texture deltas.
        output.textures_delta.clear();
        (output, command)
    }

    /// Shown field values the instance's drawn symbol carries (the demo
    /// symbol draws no Field text), as core resolved them for the canvas.
    fn symbol_values(editor: &SchematicEditor, engine: &Engine, instance: Id) -> Vec<String> {
        engine
            .project()
            .occurrences_of(instance)
            .filter_map(|o| editor.placed.get(&o.id))
            .flat_map(|p| p.fields.iter().map(|t| t.text.clone()))
            .collect()
    }

    fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                text.pos + text.galley.size() * 0.5,
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }

    fn shown(output: &egui::FullOutput) -> Vec<(String, Pos2)> {
        let mut out = Vec::new();
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    /// Click a button and return the operation the inspector emits.
    fn click(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
    ) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let (output, _) = frame(editor, engine, ctx, vec![]);
        let point = shown(&output)
            .into_iter()
            .find(|(text, _)| text == label)
            .map(|(_, p)| p)
            .ok_or_else(|| {
                let all: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
                format!("missing button {label} in {all:?}")
            })?;
        let press = |pressed| egui::Event::PointerButton {
            pos: point,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            editor,
            engine,
            ctx,
            vec![egui::Event::PointerMoved(point), press(true)],
        );
        Ok(frame(editor, engine, ctx, vec![press(false)]).1)
    }

    /// The demo plus an unwired R3, whose two terminals are single-terminal
    /// nets.
    fn with_unwired() -> Result<(Engine, Id), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let first = engine.project().instances[0].clone();
        engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(
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

    /// B-e: the Sheet's Nets count leaves out single-terminal nets.
    #[test]
    fn the_sheet_counts_only_nets_joining_two_or_more_terminals()
    -> Result<(), Box<dyn std::error::Error>> {
        let (engine, _) = with_unwired()?;
        let nets = &engine.project().nets;
        let connected = nets.iter().filter(|n| n.terminals.len() >= 2).count();
        assert!(nets.len() > connected, "fixture has no single-terminal net");
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let shown = shown(&output);
        let label = shown
            .iter()
            .find(|(t, _)| t == "Nets")
            .ok_or("no Nets row")?
            .1;
        // The value is the text on the Nets row's line.
        let value = shown
            .iter()
            .find(|(t, p)| t != "Nets" && (p.y - label.y).abs() < 2.0 && p.x > label.x)
            .ok_or("no Nets value")?;
        assert_eq!(value.0, connected.to_string());
        Ok(())
    }

    fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Click the field showing `shown` and replace its text with `text`
    /// (nothing is sent while typing); returns the frame's output.
    fn type_into(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        shown: &str,
        text: &str,
    ) -> Result<(egui::FullOutput, Option<Operation>), Box<dyn std::error::Error>> {
        assert!(click(editor, engine, ctx, shown)?.is_none());
        let mut events = vec![press(egui::Key::A, egui::Modifiers::COMMAND)];
        if text.is_empty() {
            events.push(press(egui::Key::Backspace, egui::Modifiers::NONE));
        } else {
            events.push(egui::Event::Text(text.into()));
        }
        Ok(frame(editor, engine, ctx, events))
    }

    /// Principle 13: a property commits live on Enter as one
    /// `instance.set_properties` edit; text the core parser rejects is
    /// marked under its field and never sent; Esc reverts; an empty box
    /// returns the field to its default. There is no Apply.
    #[test]
    fn typed_fields_commit_live_through_set_properties_with_core_diagnostics()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let r1 = engine.project().instances[0].id;
        let mut editor = SchematicEditor::default();
        editor.select(Some(r1), None, engine.project());
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(labels.iter().any(|t| t == "Resistance"), "{labels:?}");
        assert!(labels.iter().any(|t| t == "10 kΩ"), "{labels:?}");
        for gone in ["Apply", "Revert", "CHECKS"] {
            assert!(!labels.iter().any(|t| t == gone), "{gone} in {labels:?}");
        }
        assert_eq!(symbol_values(&editor, &engine, r1), vec!["10 kΩ"]);

        // Text of another kind is marked under the field with the core
        // diagnostic; neither typing nor Enter sends it.
        let (output, sent) = type_into(&mut editor, &engine, &ctx, "10 kΩ", "10 µF")?;
        assert!(sent.is_none());
        assert!(
            shown(&output)
                .iter()
                .any(|(t, _)| t.contains("µF") && t.contains("not a unit")),
            "no inline diagnostic"
        );
        let revision = engine.revision();
        let (output, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(sent.is_none(), "invalid text never commits: {sent:?}");
        assert_eq!(engine.revision(), revision);
        assert!(
            shown(&output).iter().any(|(t, _)| t.contains("not a unit")),
            "still marked after Enter"
        );
        // Esc puts the stored value back.
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(sent.is_none());
        let (_, sent) = type_into(&mut editor, &engine, &ctx, "10 µF", "1k")?;
        assert!(sent.is_none());
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(sent.is_none(), "Esc sends nothing");
        assert_eq!(editor.draft_fields[0].text, "10 kΩ");

        type_into(&mut editor, &engine, &ctx, "10 kΩ", "4.7k")?;
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        let operation = sent.ok_or("Enter sent nothing")?;
        let Operation::Instance(InstanceCommand::SetProperties { instance, edits }) = &operation
        else {
            return Err("properties bypassed instance.set_properties".into());
        };
        assert_eq!(*instance, r1);
        assert_eq!(
            edits,
            &[pcbcraft_core::fields::PropertyEdit {
                field: editor.draft_fields[0].field,
                input: PropertyInput::Text("4.7k".into()),
            }],
            "one field, parsed in core"
        );
        let before = engine.project().clone();
        engine.apply(engine.revision(), operation)?;
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(shown(&output).iter().any(|(t, _)| t == "4.7 kΩ"));
        assert_eq!(symbol_values(&editor, &engine, r1), vec!["4.7 kΩ"]);
        assert_eq!(editor.draft_fields[0].text, "4.7 kΩ");
        engine.undo()?;
        assert_eq!(engine.project(), &before, "one commit is one undo step");
        engine.redo()?;
        frame(&mut editor, &engine, &ctx, vec![]);
        // Clearing the box returns the field to its default.
        type_into(&mut editor, &engine, &ctx, "4.7 kΩ", "")?;
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        engine.apply(engine.revision(), sent.ok_or("clearing sent nothing")?)?;
        assert!(
            engine
                .project()
                .instance(r1)
                .is_some_and(|i| i.properties.is_empty())
        );
        Ok(())
    }

    /// The designator commits on Enter; a refusal (a designator in use)
    /// comes back to the field and shows under it, the typed text is kept
    /// and the document is unchanged; editing the field clears it.
    #[test]
    fn a_refused_designator_shows_under_its_field() -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let r1 = engine.project().instances[0].id;
        let mut editor = SchematicEditor::default();
        editor.select(Some(r1), None, engine.project());
        type_into(&mut editor, &engine, &ctx, "R1", "R2")?;
        // As the app does: the operation and the origin marked with it.
        let (mut sent, mut origin) = (None, None);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 1400.0))),
                events: vec![press(egui::Key::Enter, egui::Modifiers::NONE)],
                ..Default::default()
            },
            |ui| {
                sent = kit::form(ui, |ui| editor.inspector(ui, &engine));
                origin = crate::refusal::take_mark(ui.ctx());
            },
        );
        output.textures_delta.clear();
        let operation = sent.ok_or("Enter sent nothing")?;
        assert!(matches!(
            &operation,
            Operation::Instance(InstanceCommand::SetDesignator { designator, .. }) if designator == "R2"
        ));
        let origin = origin.ok_or("the field marked no origin")?;
        assert!(matches!(origin, Origin::Field(_)), "{origin:?}");
        let before = engine.project().clone();
        let error = engine
            .apply(engine.revision(), operation)
            .err()
            .ok_or("a designator in use was accepted")?;
        assert_eq!(engine.project(), &before);
        let message = crate::refusal::shown_error(&error);
        // The app hands it back during the frame that applied it.
        let mut error = Some(error);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(error) = error.take() {
                crate::refusal::hand_back(
                    ui.ctx(),
                    crate::refusal::Refused { origin, error },
                    false,
                );
            }
        });
        output.textures_delta.clear();
        let (output, again) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            again.is_none(),
            "a refused edit is not sent again: {again:?}"
        );
        let texts: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(texts.contains(&message), "{message} in {texts:?}");
        assert_eq!(editor.draft_reference, "R2", "the typed text is kept");
        // Unclaimed refusals would toast; this one was claimed.
        assert!(crate::refusal::unclaimed(&ctx).is_empty());
        let (output, _) = type_into(&mut editor, &engine, &ctx, "R2", "R9")?;
        assert!(
            !shown(&output).iter().any(|(t, _)| *t == message),
            "editing clears the refusal"
        );
        Ok(())
    }

    /// The position pair commits together: moving from X to Y sends
    /// nothing, leaving the pair sends one `symbol.move_unit` with both.
    #[test]
    fn the_position_pair_commits_together_when_focus_leaves_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let r1 = engine.project().instances[0].id;
        let occurrence = engine.project().occurrences_of(r1).next().ok_or("R1")?.id;
        let mut editor = SchematicEditor::default();
        editor.select(Some(r1), None, engine.project());
        let (_, sent) = type_into(&mut editor, &engine, &ctx, "-12 mm", "-10")?;
        assert!(sent.is_none());
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Tab, egui::Modifiers::NONE)],
        );
        assert!(sent.is_none(), "X to Y stays in the pair: {sent:?}");
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![
                press(egui::Key::A, egui::Modifiers::COMMAND),
                egui::Event::Text("5".into()),
            ],
        );
        assert!(sent.is_none(), "typing sends nothing: {sent:?}");
        let (_, sent) = frame(
            &mut editor,
            &engine,
            &ctx,
            vec![press(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        let mut sent: Vec<Operation> = sent.into_iter().collect();
        sent.extend(frame(&mut editor, &engine, &ctx, vec![]).1);
        let [
            Operation::Schematic(SchematicCommand::MoveUnit {
                occurrence: o,
                position,
            }),
        ] = &sent[..]
        else {
            return Err(format!("expected one move, got {sent:?}").into());
        };
        assert_eq!(*o, occurrence);
        assert_eq!(*position, [-10.0, 5.0]);
        Ok(())
    }

    #[test]
    fn placing_uses_the_components_first_representation_and_implementation()
    -> pcbcraft_core::Result<()> {
        let mut engine = Engine::demo()?;
        let component = engine.project().instances[0].component;
        let (request, unit) = first_choices(engine.project(), component)
            .ok_or_else(|| pcbcraft_core::Error::NotFound("choices".into()))?;
        assert_eq!(unit.id, pcbcraft_core::UnitId(1));
        assert!(request.physical.is_some());
        engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                position: [30.0, 0.0],
                ..request
            })),
        )?;
        // The demo holds R1 and R2; the core numbers the next resistor.
        let placed = engine
            .project()
            .instance_by_designator("R3")
            .ok_or_else(|| pcbcraft_core::Error::NotFound("R3".into()))?;
        assert_eq!(engine.project().occurrences_of(placed.id).count(), 1);
        Ok(())
    }

    /// Publish r2 of the demo resistor whose terminal 1 can be `passive`
    /// (the design default) or `sense-in`, through public library commands,
    /// and place it as U1. Returns the instance and the `sense-in` ID.
    fn multi_function_instance(
        engine: &mut Engine,
    ) -> Result<(Id, pcbcraft_core::FunctionId), Box<dyn std::error::Error>> {
        use pcbcraft_core::functions::{Direction, Drive, Function, SignalKind};
        use pcbcraft_core::library::{Definition, DraftEdit, TerminalClass};
        use pcbcraft_core::{FunctionId, LibraryCommand, Outcome};
        let r1 = engine.project().instances[0].component;
        let id = r1.id;
        if engine.drafts().get(id).is_some() {
            engine.apply(
                engine.revision(),
                Operation::Library(LibraryCommand::DiscardDraft { id }),
            )?;
        }
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::DraftFrom {
                revision: r1.erased(),
            }),
        )?;
        let draft = engine.drafts().get(id).cloned().ok_or("no draft")?;
        let mut definition = draft.definition;
        let Definition::Component(component) = &mut definition else {
            return Err("not a component draft".into());
        };
        // New function IDs come from the revision's local ID counter.
        let first = component.next_local;
        component.next_local = first + 2;
        let terminal = component
            .terminals
            .iter_mut()
            .find(|t| t.id == pcbcraft_core::TerminalId(1))
            .ok_or("no terminal 1")?;
        terminal.class = TerminalClass::Electrical(TerminalFunctions {
            functions: vec![
                Function {
                    id: FunctionId(first),
                    label: "passive".into(),
                    direction: Direction::Passive,
                    signal: SignalKind::Unspecified,
                    drive: Drive::None,
                    resource_role: None,
                    mux: None,
                    source: None,
                },
                Function {
                    id: FunctionId(first + 1),
                    label: "sense-in".into(),
                    direction: Direction::Input,
                    signal: SignalKind::Analog,
                    drive: Drive::None,
                    resource_role: None,
                    mux: None,
                    source: None,
                },
            ],
            design_default: Some(FunctionId(first)),
            gpio: None,
            options: Vec::new(),
        });
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::Edit {
                edits: vec![DraftEdit {
                    id,
                    meta: draft.meta,
                    definition,
                }],
            }),
        )?;
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::Publish { drafts: vec![id] }),
        )?;
        let r2 = ComponentRef::new(id, pcbcraft_core::Revision(2));
        let (request, _) = first_choices(engine.project(), r2).ok_or("no r2 choices")?;
        match engine.apply(
            engine.revision(),
            Operation::Instance(InstanceCommand::Place(NewInstance {
                designator: Some("U1".into()),
                position: [0.0, 20.0],
                ..request
            })),
        )? {
            Outcome::InstancePlaced { instance, .. } => Ok((instance, FunctionId(first + 1))),
            other => Err(format!("unexpected outcome {other:?}").into()),
        }
    }

    #[test]
    fn choosing_a_pin_function_sends_one_select_functions_that_undo_restores()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let (u1, sense_in) = multi_function_instance(&mut engine)?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.select(Some(u1), None, engine.project());
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(labels.iter().any(|t| t == "PIN FUNCTIONS"), "{labels:?}");
        // Folded by default; its header opens it.
        assert!(
            !labels.iter().any(|t| t == "Default (passive)"),
            "{labels:?}"
        );
        assert!(click(&mut editor, &engine, &ctx, "PIN FUNCTIONS")?.is_none());
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        // Terminal 2 stays fixed and is listed read-only.
        assert!(labels.iter().any(|t| t == "passive · fixed"), "{labels:?}");

        // Open the dropdown, then pick the alternative function.
        assert!(click(&mut editor, &engine, &ctx, "Default (passive)")?.is_none());
        let mut operations = Vec::new();
        operations.extend(click(&mut editor, &engine, &ctx, "sense-in")?);
        operations.extend(frame(&mut editor, &engine, &ctx, vec![]).1);
        assert_eq!(operations.len(), 1, "{operations:?}");
        let operation = operations.remove(0);
        let Operation::Instance(InstanceCommand::SelectFunctions { instance, edits }) = &operation
        else {
            return Err(format!("expected instance.select_functions, got {operation:?}").into());
        };
        assert_eq!(*instance, u1);
        assert_eq!(
            edits,
            &[FunctionEdit {
                terminal: pcbcraft_core::TerminalId(1),
                function: Some(sense_in),
            }]
        );

        let before = engine.project().clone();
        engine.apply(engine.revision(), operation)?;
        let selected = |engine: &Engine| {
            engine
                .project()
                .instance(u1)
                .and_then(|i| i.functions.get(&pcbcraft_core::TerminalId(1)).copied())
        };
        assert_eq!(selected(&engine), Some(sense_in));
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(shown(&output).iter().any(|(t, _)| t == "sense-in"));
        engine.undo()?;
        assert_eq!(selected(&engine), None);
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    /// One frame of the whole Schematic workspace: canvas (with the palette
    /// over it) and the properties panel, as the app lays them out.
    fn workspace_frame(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> (egui::FullOutput, Vec<Operation>) {
        let mut operations = Vec::new();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                events,
                ..Default::default()
            },
            |ui| {
                let canvas = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 800.0));
                let panel = Rect::from_min_size(pos2(820.0, 0.0), vec2(360.0, 800.0));
                ui.scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| {
                    operations.extend(editor.canvas(ui, engine));
                });
                ui.scope_builder(egui::UiBuilder::new().max_rect(panel), |ui| {
                    operations.extend(kit::form(ui, |ui| editor.inspector(ui, engine)));
                });
            },
        );
        output.textures_delta.clear();
        (output, operations)
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn parts_engine() -> Result<Engine, Box<dyn std::error::Error>> {
        Ok(Engine::from_file(pcbcraft_core::format::EditorFile {
            version: pcbcraft_core::format::FILE_VERSION,
            project: pcbcraft_core::catalog::synthetic::project(24)?,
            drafts: Default::default(),
        })?)
    }

    /// Type a query, move with the arrows, Enter places the highlighted
    /// revision through `component.place`; undo removes it again. The
    /// properties panel previews the highlighted row.
    /// A library folder placement lands where a nominal symbol box clears
    /// every drawn symbol, on the sheet grid.
    #[test]
    fn a_folder_placement_point_clears_the_drawn_symbols() -> Result<(), Box<dyn std::error::Error>>
    {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        assert!(!editor.placed_bounds.is_empty());
        let at = editor.place_point();
        for b in editor.placed_bounds.values() {
            let apart = at[0] + PLACE_HALF[0] <= b.min[0]
                || at[0] - PLACE_HALF[0] >= b.max[0]
                || at[1] + PLACE_HALF[1] <= b.min[1]
                || at[1] - PLACE_HALF[1] >= b.max[1];
            assert!(apart, "{at:?} overlaps {b:?}");
        }
        // Inside the view: on screen, the box's centre is within the canvas.
        let screen = Vec2::new(at[0] as f32, -at[1] as f32) * editor.scale + editor.pan;
        assert!(
            screen.x.abs() <= editor.view.x / 2.0 && screen.y.abs() <= editor.view.y / 2.0,
            "{at:?} is off screen ({screen:?} in {:?})",
            editor.view
        );
        for v in at {
            assert!(((v / SHEET_GRID_MM).round() * SHEET_GRID_MM - v).abs() < 1e-9);
        }
        Ok(())
    }

    #[test]
    fn the_place_palette_finds_moves_and_places_through_component_place()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        assert!(editor.on_tool("place", &engine).is_none());
        assert!(editor.palette.is_open());
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let (output, _) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text("res>=10k pkg:0603".into())],
        );
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        // Ranked by name: 10 kΩ, 1 MΩ, 22 kΩ, each with its key values,
        // revision and four separate badges.
        for name in ["EX-R0603-10K0-F", "EX-R0603-1M00-J", "EX-R0603-22K0-D"] {
            assert!(labels.iter().any(|t| t == name), "{name} in {labels:?}");
        }
        assert!(
            labels
                .iter()
                .any(|t| t == "10 kΩ · ±1% · 0603 · project r1")
        );
        assert!(labels.iter().any(|t| t == "—MPN"), "{labels:?}");
        assert!(
            labels.iter().any(|t| t == "Resistance ≥ 10 kΩ"),
            "{labels:?}"
        );
        assert!(
            labels.iter().any(|t| t == "IN THIS PROJECT  3"),
            "{labels:?}"
        );
        // The COMPONENT preview follows the highlighted (first) row.
        assert_eq!(editor.header(&engine).title, "Place component");
        assert!(
            labels.iter().any(|t| t == "EX-R0603-10K0-F · r1 (latest)"),
            "{labels:?}"
        );

        let (output, _) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::ArrowDown, egui::Modifiers::NONE)],
        );
        let second = editor.palette.highlighted().ok_or("no highlight")?;
        let name = engine
            .project()
            .library
            .component(second)
            .map(|c| c.meta.name.clone());
        assert_eq!(name.as_deref(), Some("EX-R0603-1M00-J"));
        let preview: Vec<String> = shown(&output)
            .into_iter()
            .filter(|(_, p)| p.x > 820.0)
            .map(|(t, _)| t)
            .collect();
        assert!(
            preview.iter().any(|t| t.starts_with("1 MΩ")),
            "preview {preview:?}"
        );

        let (_, operations) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        let [operation] = &operations[..] else {
            return Err(format!("expected one operation, got {operations:?}").into());
        };
        let Operation::Instance(InstanceCommand::Place(request)) = operation else {
            return Err(format!("expected component.place, got {operation:?}").into());
        };
        assert_eq!(request.component, second);
        assert!(request.physical.is_some());
        assert_eq!(request.designator, None, "the core numbers it");
        assert!(!editor.palette.is_open());
        assert_eq!(editor.tool, Tool::Select);

        let before = engine.project().clone();
        engine.apply(engine.revision(), operation.clone())?;
        let placed = engine.project().instances.last().ok_or("not placed")?;
        assert_eq!(placed.component, second);
        assert_eq!(placed.designator, "R1", "a resistor takes its prefix");
        engine.undo()?;
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    #[test]
    fn shift_enter_keeps_the_palette_and_esc_closes_it_without_placing()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.on_tool("place", &engine);
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text("hdr".into())],
        );
        for _ in 0..2 {
            let (_, operations) = workspace_frame(
                &mut editor,
                &engine,
                &ctx,
                vec![key(egui::Key::Enter, egui::Modifiers::SHIFT)],
            );
            assert_eq!(operations.len(), 1, "{operations:?}");
            assert!(editor.palette.is_open());
            for operation in operations {
                engine.apply(engine.revision(), operation)?;
            }
        }
        // Both placed the best "hdr" match, each at its own free spot.
        for instance in &engine.project().instances {
            let name = engine
                .project()
                .library
                .component(instance.component)
                .map(|c| c.meta.name.as_str());
            assert_eq!(name, Some("EX-HDR-1X2-001"));
        }
        let [a, b] = &engine.project().occurrences[..] else {
            return Err("expected two occurrences".into());
        };
        assert_ne!(a.position, b.position);
        // Their symbols, pin ends included, keep a gap: no pin lands on
        // the other part's pin.
        let bounds = |o: &pcbcraft_core::SymbolOccurrence| {
            unit_of(engine.project(), o)
                .map(|unit| symbol_bounds(unit, schematic::occurrence_transform(o)))
        };
        let (Some(first), Some(second)) = (bounds(a), bounds(b)) else {
            return Err("no symbol bounds".into());
        };
        assert!(
            !first.expand(1.0).intersects(second),
            "{first:?} touches {second:?}"
        );
        // A typing error shows inline and Enter does nothing.
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text(" V>=10k".into())],
        );
        let (output, operations) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert!(operations.is_empty());
        assert!(
            shown(&output)
                .iter()
                .any(|(t, _)| t == "V>=10k: 10 k has no unit of voltage"),
            "no inline parse error"
        );
        let revision = engine.revision();
        let (_, operations) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(operations.is_empty());
        assert!(!editor.palette.is_open());
        assert_eq!(editor.tool, Tool::Select);
        assert_eq!(engine.revision(), revision);
        Ok(())
    }

    /// Hover, press and release at `at` over three frames; returns the
    /// operations of all three.
    fn click_at(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        at: Pos2,
    ) -> (Vec<String>, Vec<Operation>) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut operations = Vec::new();
        let mut labels = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
        ] {
            let (output, ops) = workspace_frame(editor, engine, ctx, events);
            operations.extend(ops);
            labels = shown(&output).into_iter().map(|(t, _)| t).collect();
        }
        (labels, operations)
    }

    /// Two fingers pan the sheet and never zoom; a pinch zooms about the
    /// pointer, which keeps its sheet point.
    #[test]
    fn two_fingers_pan_the_sheet_and_a_pinch_zooms_about_the_pointer()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::navigation::tests::swipe;
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        let at = pos2(300.0, 260.0);
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::PointerMoved(at)],
        );
        let (scale, pan) = (editor.scale, editor.pan);
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            swipe(vec2(-5.0, 8.0), egui::Modifiers::NONE, 2),
        );
        assert_eq!(editor.scale, scale, "two fingers must not zoom");
        assert_eq!(editor.pan - pan, vec2(-10.0, 16.0));
        // The sheet point under the pointer, relative to the canvas centre.
        let centre = pos2(400.0, 400.0);
        let sheet = |e: &SchematicEditor| (at - centre - e.pan) / e.scale;
        let before = sheet(&editor);
        workspace_frame(&mut editor, &engine, &ctx, vec![egui::Event::Zoom(1.5)]);
        assert!(
            (editor.scale - scale * 1.5).abs() < 1e-4,
            "{}",
            editor.scale
        );
        assert!((sheet(&editor) - before).length() < 1e-4);
        Ok(())
    }

    /// Clicking a row picks it: the palette closes, the Place tool is armed
    /// with that revision, and one click on the sheet places it once.
    #[test]
    fn clicking_a_row_arms_place_and_the_sheet_click_places_it_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.on_tool("place", &engine);
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::Text("EX-C0402".into())],
        );
        let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
        // The last row label on the canvas side (the preview's copy of the
        // name is in the panel, right of x = 800).
        let (_, at) = shown(&output)
            .into_iter()
            .rfind(|(t, p)| t.starts_with("EX-C0402") && p.x < 800.0)
            .ok_or("no row")?;
        let row = at + vec2(4.0, 4.0);
        let (_, operations) = click_at(&mut editor, &engine, &ctx, row);
        assert!(
            operations.is_empty(),
            "a pick places nothing: {operations:?}"
        );
        assert!(!editor.palette.is_open());
        assert_eq!(editor.tool, Tool::Place);
        let picked = editor.component.ok_or("nothing armed")?;
        let name = engine
            .project()
            .library
            .component(picked)
            .map(|c| c.meta.name.clone())
            .ok_or("unknown component")?;
        assert!(name.starts_with("EX-C0402"), "{name}");
        // Release the pointer elsewhere, then click an empty spot.
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let (_, operations) = click_at(&mut editor, &engine, &ctx, pos2(120.0, 680.0));
        let [Operation::Instance(InstanceCommand::Place(request))] = &operations[..] else {
            return Err(format!("expected one component.place, got {operations:?}").into());
        };
        assert_eq!(request.component, picked);
        assert_eq!(editor.tool, Tool::Place);
        let [operation] = &operations[..] else {
            return Err("one operation".into());
        };
        let mut engine = engine;
        match engine.apply(engine.revision(), operation.clone())? {
            pcbcraft_core::Outcome::InstancePlaced { designator, .. } => {
                assert_eq!(designator, "C1")
            }
            other => return Err(format!("unexpected outcome {other:?}").into()),
        }
        Ok(())
    }

    /// Press at `from`, move to `to` and release over five frames; returns
    /// every operation.
    fn drag_between(
        editor: &mut SchematicEditor,
        engine: &Engine,
        ctx: &egui::Context,
        from: Pos2,
        to: Pos2,
    ) -> Vec<Operation> {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut operations = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(from + (to - from) * 0.5)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ] {
            operations.extend(workspace_frame(editor, engine, ctx, events).1);
        }
        operations
    }

    /// The sheet draws what core resolves for each occurrence (designator,
    /// name, pin names and numbers, the shown value) and picks through the
    /// core hit test: the body and its text select, empty sheet does not.
    #[test]
    fn the_sheet_draws_and_picks_symbols_through_core_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        let r1 = engine.project().instances[0].id;
        let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let canvas: Vec<String> = shown(&output)
            .into_iter()
            .filter(|(_, p)| p.x < 800.0)
            .map(|(t, _)| t)
            .collect();
        for text in ["R1", "R2", "Resistor", "10 kΩ", "1", "2"] {
            assert!(canvas.iter().any(|t| t == text), "{text} in {canvas:?}");
        }
        // 12 px/mm about the canvas centre (400, 400): R1 sits at (−12, 0).
        let r1_body = pos2(400.0 - 12.0 * 12.0, 400.0);
        click_at(&mut editor, &engine, &ctx, r1_body);
        assert_eq!(editor.selected_instance(), Some(r1));
        click_at(&mut editor, &engine, &ctx, pos2(600.0, 650.0));
        assert_eq!(editor.selected_instance(), None);
        // The designator above the body (bottom at 1.5 mm) picks it too.
        click_at(&mut editor, &engine, &ctx, r1_body + vec2(0.0, -2.2 * 12.0));
        assert_eq!(editor.selected_instance(), Some(r1));
        // What is drawn is what is picked: the end of the resolved name
        // (wider than the body) and the value stacked below the body.
        let occurrence = engine.project().occurrences_of(r1).next().ok_or("R1")?.id;
        let placed = schematic::placed_unit(engine.project(), occurrence)?;
        let m = TextMetric::default();
        let name = placed
            .texts
            .iter()
            .find(|t| t.text == "Resistor")
            .ok_or("name")?
            .bounds(m);
        let value = placed.fields.first().ok_or("stacked value")?.bounds(m);
        let sheet = |p: Point| pos2(400.0 + p[0] as f32 * 12.0, 400.0 - p[1] as f32 * 12.0);
        for p in [
            [name.min[0] + 0.1, (name.min[1] + name.max[1]) / 2.0],
            [name.max[0] - 0.1, (name.min[1] + name.max[1]) / 2.0],
            [-12.0, (value.min[1] + value.max[1]) / 2.0],
        ] {
            click_at(&mut editor, &engine, &ctx, pos2(600.0, 650.0));
            assert_eq!(editor.selected_instance(), None);
            click_at(&mut editor, &engine, &ctx, sheet(p));
            assert_eq!(editor.selected_instance(), Some(r1), "click at {p:?}");
        }
        Ok(())
    }

    /// Dragging an occurrence moves its origin onto the symbol grid, so its
    /// on-grid pins stay on the grid; the move is one symbol.move_unit.
    #[test]
    fn dragged_occurrences_snap_to_the_symbol_grid() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let from = pos2(400.0 - 12.0 * 12.0, 400.0);
        // +1.1 mm right, +0.6 mm down: nearest 2.5 mm point is (−10, 0).
        let operations = drag_between(&mut editor, &engine, &ctx, from, from + vec2(13.0, 7.0));
        let [Operation::Schematic(SchematicCommand::MoveUnit { position, .. })] = &operations[..]
        else {
            return Err(format!("expected one move, got {operations:?}").into());
        };
        assert_eq!(*position, [-10.0, 0.0]);
        let grid = engine.drafts().grid.symbol_mm;
        assert!(
            position
                .iter()
                .all(|v| ((v / grid).round() * grid - v).abs() < 1e-9)
        );
        Ok(())
    }

    /// Cmd/Ctrl+R turns and Shift+X mirrors the selected occurrence, each one
    /// `symbol.orient_unit` that undo reverts; the Arrange buttons send the
    /// same operation, the properties show the stored orientation, and the
    /// sheet draws and picks the turned pins where core places them.
    #[test]
    fn rotate_and_mirror_orient_the_selected_occurrence_in_one_step()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        let r1 = engine.project().instances[0].id;
        let occurrence = engine.project().occurrences_of(r1).next().ok_or("R1")?.id;
        let before = engine.project().clone();
        let arrange = |editor: &mut SchematicEditor, engine: &Engine| {
            let ribbon = editor.ribbon(engine);
            ribbon
                .groups
                .iter()
                .find(|g| g.label == "Arrange")
                .map(|g| {
                    g.tools
                        .iter()
                        .map(|t| (t.label, t.enabled))
                        .collect::<Vec<_>>()
                })
        };
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        assert_eq!(
            arrange(&mut editor, &engine),
            Some(vec![("Rotate", false), ("Mirror", false)])
        );
        // Nothing selected: the keys are left for others.
        let (_, ops) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::R, egui::Modifiers::COMMAND)],
        );
        assert!(ops.is_empty());
        let r1_body = pos2(400.0 - 12.0 * 12.0, 400.0);
        let (labels, _) = click_at(&mut editor, &engine, &ctx, r1_body);
        assert_eq!(editor.selected_instance(), Some(r1));
        for row in ["Rotation", "0°", "Mirror"] {
            assert!(labels.iter().any(|l| l == row), "{row} in {labels:?}");
        }
        assert_eq!(
            arrange(&mut editor, &engine),
            Some(vec![("Rotate", true), ("Mirror", true)])
        );
        let (_, ops) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::R, egui::Modifiers::COMMAND)],
        );
        let expected = Operation::Schematic(SchematicCommand::OrientUnit {
            occurrence,
            rotation: Rotation::R90,
            mirror: false,
        });
        assert_eq!(
            serde_json::to_value(&ops)?,
            serde_json::to_value([&expected])?
        );
        assert_eq!(
            serde_json::to_value(editor.on_tool("rotate", &engine))?,
            serde_json::to_value(Some(&expected))?
        );
        engine.apply(engine.revision(), expected)?;
        // R1 at (−12, 0) turned: pin 1 at (−12, −5), 5 mm below on screen.
        let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(labels.iter().any(|l| l == "90°"), "{labels:?}");
        let pin_one = pos2(400.0 - 12.0 * 12.0, 400.0 + 4.5 * 12.0);
        let (_, ops) = workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![key(egui::Key::X, egui::Modifiers::SHIFT)],
        );
        let [
            op @ Operation::Schematic(SchematicCommand::OrientUnit {
                rotation, mirror, ..
            }),
        ] = &ops[..]
        else {
            return Err(format!("expected one orient, got {ops:?}").into());
        };
        assert_eq!((*rotation, *mirror), (Rotation::R90, true));
        engine.apply(engine.revision(), op.clone())?;
        let placed = schematic::placed_unit(engine.project(), occurrence)?;
        let end = placed
            .pins
            .iter()
            .find(|p| p.terminal == pcbcraft_core::TerminalId(1))
            .ok_or("pin 1")?
            .end;
        assert_eq!(end, [-12.0, -5.0]);
        // The wire tool picks the turned pin where it is drawn.
        editor.on_tool("wire", &engine);
        click_at(&mut editor, &engine, &ctx, pin_one);
        assert_eq!(
            editor.wire_from,
            Some(WireTarget::Terminal(Endpoint {
                instance: r1,
                terminal: pcbcraft_core::TerminalId(1)
            }))
        );
        engine.undo()?;
        engine.undo()?;
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    /// A row with no symbol to place keeps the palette open with a notice;
    /// Enter and a click choose nothing.
    #[test]
    fn a_row_without_a_symbol_keeps_the_palette_open_with_a_notice()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = parts_engine()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut palette = PlacePalette::default();
        palette.open();
        let run = |palette: &mut PlacePalette, events: Vec<egui::Event>| {
            let mut action = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 800.0))),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let area = ui.max_rect();
                    action = palette.show(ui, area, &engine, &|_| false);
                },
            );
            output.textures_delta.clear();
            (output, action)
        };
        run(&mut palette, vec![]);
        run(&mut palette, vec![egui::Event::Text("hdr".into())]);
        let (output, action) = run(
            &mut palette,
            vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
        );
        assert_eq!(action, None);
        assert!(palette.is_open());
        let (output, _) = if shown(&output)
            .iter()
            .any(|(t, _)| t.ends_with("has no symbol to place."))
        {
            (output, None)
        } else {
            run(&mut palette, vec![])
        };
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        assert!(
            labels
                .iter()
                .any(|t| t == "EX-HDR-1X2-001 has no symbol to place."),
            "{labels:?}"
        );
        // Nothing can be placed, so the footer drops the Enter hints.
        assert!(
            labels.iter().any(|t| t == "↑↓ move   Esc close"),
            "{labels:?}"
        );
        Ok(())
    }

    /// One card, one subject: the sheet, an instance and a wire each name
    /// their own subject; an instance and a wire have a footer (Remove…,
    /// Disconnect wire…), the sheet none; the issue slot counts what
    /// `project.issues` reports on the subject in the Schematic area.
    #[test]
    fn each_subject_has_its_own_card_footer_and_issue_slot()
    -> Result<(), Box<dyn std::error::Error>> {
        use pcbcraft_core::issues::{IssueArea, IssueScope};
        let (engine, r3) = with_unwired()?;
        let mut editor = SchematicEditor::default();
        let sheet = editor.inspector_card(&engine);
        assert_eq!(sheet.header.title, "Sheet");
        assert!(!sheet.footer);
        let QueryResult::Issues(report) = engine.query(Query::Issues {
            scope: IssueScope::Area {
                area: IssueArea::Schematic,
            },
        })?
        else {
            return Err("not an issue report".into());
        };
        let slot = sheet.issues.ok_or("the sheet has a slot")?;
        assert_eq!(slot.counts, report.counts);
        assert_eq!(slot.label, "on this sheet");
        assert!(!slot.is_empty(), "the unwired R3 has issues");

        editor.select_instance(Some(r3), engine.project());
        let card = editor.inspector_card(&engine);
        assert_eq!(card.header.kind, "Instance");
        assert!(card.header.title.starts_with("R3"), "{}", card.header.title);
        assert!(card.footer);
        assert_ne!(card.subject, sheet.subject);
        let slot = card.issues.ok_or("an instance has a slot")?;
        assert_eq!(slot.label, "on R3");
        let on_r3 = report
            .rows
            .iter()
            .filter(|r| {
                r.objects().any(|o| match o {
                    ObjectRef::Instance { id }
                    | ObjectRef::Pad { instance: id, .. }
                    | ObjectRef::Unit { instance: id, .. }
                    | ObjectRef::Property { instance: id, .. } => *id == r3,
                    ObjectRef::Terminal { endpoint } => endpoint.instance == r3,
                    _ => false,
                })
            })
            .count();
        let c = slot.counts;
        assert_eq!(c.error + c.warning + c.to_do + c.info, on_r3);
        assert!(on_r3 > 0);

        let wire = engine.project().wires[0].id;
        let net = engine
            .project()
            .net_of(
                engine.project().wires[0]
                    .from
                    .terminal()
                    .ok_or("terminal end")?,
            )
            .ok_or("net")?;
        editor.select(None, None, engine.project());
        editor.selected_wire = Some(wire);
        let card = editor.inspector_card(&engine);
        assert_eq!(card.header.kind, "Net");
        assert_eq!(card.header.title, net.name);
        assert!(card.footer);
        assert_eq!(
            card.issues.ok_or("a net has a slot")?.label,
            format!("on {}", net.name)
        );

        editor.on_tool("place", &engine);
        let card = editor.inspector_card(&engine);
        assert_eq!(card.header.title, "Place component");
        assert!(!card.footer);
        assert!(card.issues.is_none(), "a command has no slot");
        Ok(())
    }

    /// The footer's Remove… and Disconnect wire… open the update review;
    /// nothing changes until it is applied.
    #[test]
    fn footer_actions_open_the_review() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let before = engine.project().clone();
        let mut editor = SchematicEditor::default();
        editor.select_instance(Some(engine.project().instances[0].id), engine.project());
        assert!(click(&mut editor, &engine, &ctx, "Remove…")?.is_none());
        assert!(editor.update.is_open());
        assert_eq!(editor.header(&engine).title, "Review change");
        let mut editor = SchematicEditor {
            selected_wire: Some(engine.project().wires[0].id),
            ..Default::default()
        };
        assert!(click(&mut editor, &engine, &ctx, "Disconnect wire…")?.is_none());
        assert!(editor.update.is_open());
        assert_eq!(engine.project(), &before);
        Ok(())
    }

    /// The Net card names its members by designator and terminal; a member
    /// selects its instance. The WIRE section names both ends.
    #[test]
    fn the_net_card_lists_members_that_select_their_instance()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let wire = engine.project().wires[0].clone();
        let mut editor = SchematicEditor {
            selected_wire: Some(wire.id),
            ..Default::default()
        };
        let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
        let labels: Vec<String> = shown(&output).into_iter().map(|(t, _)| t).collect();
        let from = end_label(engine.project(), wire.from);
        let to = end_label(engine.project(), wire.to);
        for text in [
            "NET",
            "Members",
            "WIRE",
            "From",
            "To",
            from.as_str(),
            to.as_str(),
        ] {
            assert!(labels.iter().any(|t| t == text), "{text} in {labels:?}");
        }
        assert!(!from.contains('?'), "{from}");
        // The member row (the second copy of the label; the first is From).
        let rows: Vec<Pos2> = shown(&output)
            .into_iter()
            .filter(|(t, _)| *t == to)
            .map(|(_, p)| p)
            .collect();
        let at = *rows.first().ok_or("no member row")?;
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::PointerMoved(at), press(true)],
        );
        frame(&mut editor, &engine, &ctx, vec![press(false)]);
        assert_eq!(
            editor.selected_instance(),
            wire.to.terminal().map(|e| e.instance)
        );
        Ok(())
    }

    /// The sheet card counts unresolved parts and opens Resolve parts; the
    /// Checks section and the ribbon's Check are gone (findings are in
    /// Issues); help is the canvas hint, not card text.
    #[test]
    fn the_sheet_card_counts_unresolved_parts_and_has_no_checks()
    -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let texts = shown(&output);
        let panel: Vec<&str> = texts
            .iter()
            .filter(|(_, p)| p.x > 820.0)
            .map(|(t, _)| t.as_str())
            .collect();
        let unresolved = sheet_card::unresolved_parts(engine.project());
        assert_eq!(
            unresolved,
            engine.project().instances.len(),
            "demo has no Parts"
        );
        assert!(panel.contains(&"Parts"), "{panel:?}");
        let count = format!("{unresolved} unresolved");
        assert!(panel.contains(&count.as_str()), "{panel:?}");
        assert!(!panel.contains(&"CHECKS"), "{panel:?}");
        assert!(
            texts
                .iter()
                .any(|(t, p)| p.x < 800.0 && t == "Click to select · drag to move · P places"),
            "no canvas hint"
        );
        let ribbon = editor.ribbon(&engine);
        assert!(
            ribbon
                .groups
                .iter()
                .flat_map(|g| &g.tools)
                .all(|t| t.id != "check"),
            "Check moved to Issues"
        );
        assert!(click(&mut editor, &engine, &ctx, "Resolve parts…")?.is_none());
        assert!(editor.resolve.is_open());
        Ok(())
    }

    /// A located instance is brought into view: the canvas centres it.
    #[test]
    fn a_located_instance_is_framed_on_the_canvas() -> Result<(), Box<dyn std::error::Error>> {
        use crate::issues::Locate;
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let r2 = engine
            .project()
            .instance_by_designator("R2")
            .ok_or("R2")?
            .id;
        let occurrence = engine.project().occurrences_of(r2).next().ok_or("R2")?.id;
        let bounds = *editor.placed_bounds.get(&occurrence).ok_or("bounds")?;
        assert!(editor.locate(&engine, &[ObjectRef::Instance { id: r2 }]));
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let [x, y] = bounds.center();
        let screen = Vec2::new(x as f32, -(y as f32)) * editor.scale + editor.pan;
        assert!(screen.length() < 1.0, "R2 is {screen:?} from the centre");
        assert_eq!(editor.selected_instance(), Some(r2));
        Ok(())
    }

    /// With the palette closed, the Place tool's card previews the armed
    /// COMPONENT and offers Find component…, which reopens the palette; the
    /// canvas hint names what a click places.
    #[test]
    fn the_armed_place_card_previews_the_component() -> Result<(), Box<dyn std::error::Error>> {
        let engine = Engine::demo()?;
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut editor = SchematicEditor::default();
        editor.on_tool("place", &engine);
        editor.palette.close();
        // The card arms the latest component on its first frame.
        workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
        let texts = shown(&output);
        let panel: Vec<&str> = texts
            .iter()
            .filter(|(_, p)| p.x > 820.0)
            .map(|(t, _)| t.as_str())
            .collect();
        for text in ["COMPONENT", "Resistor · r1 (latest)", "Find component…"] {
            assert!(panel.contains(&text), "{text} in {panel:?}");
        }
        assert!(
            texts
                .iter()
                .any(|(t, p)| p.x < 800.0 && t == "Click to place Resistor · Esc ends"),
            "no canvas hint"
        );
        assert_eq!(editor.header(&engine).title, "Place component");
        let at = texts
            .iter()
            .find(|(t, _)| t == "Find component…")
            .map(|(_, p)| *p)
            .ok_or("no button")?;
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        workspace_frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::PointerMoved(at), press(true)],
        );
        workspace_frame(&mut editor, &engine, &ctx, vec![press(false)]);
        assert!(editor.palette.is_open());
        Ok(())
    }
}
