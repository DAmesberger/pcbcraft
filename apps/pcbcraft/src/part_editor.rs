//! Component authoring content for the shared shell. Form spacing, controls and
//! typography follow Solidcraft ui_kit/layout.rs and fields.rs at the pinned revision.
//!
//! The editor authors a working set of five drafts (component, symbol,
//! package, footprint, binding). Widgets edit a working copy; a finished
//! gesture commits it as one `library.edit`, so every finished edit is one
//! shared undo step. Publishing sends one `library.publish` of the five.
mod arrange;
mod drawing;
#[cfg(test)]
mod drawing_tests;
mod footprint_canvas;
mod footprint_geometry;
mod footprint_props;
#[cfg(test)]
mod footprint_tests;
mod part_props;
mod snap;
mod symbol_canvas;
mod symbol_props;
mod working_set;

use crate::kit::{self, Tone};
use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Ui};
pub use part_props::{meaning_label, spec_text};
use pcbcraft_core::completeness::CheckTarget;
use pcbcraft_core::fields::{self, FieldDefinition, FieldSelection, FieldValue, MAX_FIELDS};
use pcbcraft_core::functions::{
    Direction, Drive, Function, MAX_FUNCTIONS, SignalKind, TerminalFunctions,
};
use pcbcraft_core::geometry::{Bounds, Shape};
use pcbcraft_core::library::graphics::{
    self, Fill, HAlign, Justify, PIN_TEXT_HEIGHT_MM, SYMBOL_MODULE_MM, SYMBOL_STROKE_MM,
    TextContent, VAlign,
};
use pcbcraft_core::library::{
    ComponentDefinition, Drafts, FieldView, Item, Library, PackageBody, PackageTerminal, Pad, Pin,
    PinDirection, Published, Terminal, TerminalClass, TerminalMap, Unit, designator_prefix_valid,
};
use pcbcraft_core::{
    ComponentRef, DefinitionId, Engine, FieldRef, FunctionId, GraphicId, Id, LibraryCommand,
    Operation, PackageTerminalId, PadId, Point, Query, QueryResult, Stage, TerminalId, UnitId,
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};
use std::sync::Arc;
pub use working_set::{Template, WorkingSet};

/// A terminal's class as the inspector offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClassChoice {
    Electrical,
    DeviceNc,
    Reserved,
}

impl ClassChoice {
    fn of(terminal: &Terminal) -> Self {
        match terminal.class {
            TerminalClass::Electrical(_) => Self::Electrical,
            TerminalClass::DeviceNc => Self::DeviceNc,
            TerminalClass::Reserved => Self::Reserved,
        }
    }
}

/// One selected object of the Symbol view: a pin by its terminal or a
/// graphic by its stable ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Selected {
    Pin(TerminalId),
    Graphic(GraphicId),
}

const CLASS_CHOICES: [(ClassChoice, &str); 3] = [
    (ClassChoice::Electrical, "Electrical"),
    (ClassChoice::DeviceNc, "Device NC"),
    (ClassChoice::Reserved, "Reserved"),
];

const DIRECTIONS: [Direction; 6] = [
    Direction::Passive,
    Direction::Input,
    Direction::Output,
    Direction::Bidirectional,
    Direction::PowerInput,
    Direction::PowerOutput,
];

const SIGNALS: [(SignalKind, &str); 4] = [
    (SignalKind::Unspecified, "Unspecified"),
    (SignalKind::Digital, "Digital"),
    (SignalKind::Analog, "Analog"),
    (SignalKind::Power, "Power"),
];

const DRIVES: [(Drive, &str); 4] = [
    (Drive::None, "None"),
    (Drive::PushPull, "Push-pull"),
    (Drive::OpenDrain, "Open drain"),
    (Drive::OpenSource, "Open source"),
];

/// Capitalise the first letter of a core label for a dropdown.
fn sentence(label: &str) -> String {
    let mut chars = label.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Short text for a terminal's class.
fn pin_summary(terminal: &Terminal) -> String {
    match &terminal.class {
        TerminalClass::Electrical(functions) => match functions.functions.as_slice() {
            [only] => only.label.clone(),
            many => format!("{} functions", many.len()),
        },
        TerminalClass::DeviceNc => "device NC".into(),
        TerminalClass::Reserved => "reserved".into(),
    }
}

/// Take the next local ID of a component, with checked arithmetic.
fn allocate_local(component: &mut ComponentDefinition) -> Option<u32> {
    let id = component.next_local;
    component.next_local = id.checked_add(1)?;
    Some(id)
}

/// Change a terminal's class. Becoming electrical gives it one passive
/// function with a freshly allocated ID; staying electrical keeps its
/// functions.
fn set_class(
    component: &mut ComponentDefinition,
    id: TerminalId,
    choice: ClassChoice,
) -> Result<(), &'static str> {
    let current = component
        .terminal(id)
        .map(ClassChoice::of)
        .ok_or("No such terminal")?;
    if current == choice {
        return Ok(());
    }
    let class = match choice {
        ClassChoice::DeviceNc => TerminalClass::DeviceNc,
        ClassChoice::Reserved => TerminalClass::Reserved,
        ClassChoice::Electrical => {
            let function = allocate_local(component).ok_or("Function identity exhausted")?;
            TerminalClass::Electrical(TerminalFunctions::fixed(
                FunctionId(function),
                Direction::Passive,
            ))
        }
    };
    let terminal = component
        .terminals
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or("No such terminal")?;
    terminal.class = class;
    Ok(())
}

/// Add a passive function with a fresh ID and a label unique on the
/// terminal. Validation of the result stays in the core.
fn add_function(
    component: &mut ComponentDefinition,
    id: TerminalId,
) -> Result<FunctionId, &'static str> {
    let count = component
        .terminal(id)
        .and_then(Terminal::functions)
        .map(|f| f.functions.len())
        .ok_or("Only electrical terminals have functions")?;
    if count >= MAX_FUNCTIONS {
        return Err("Maximum 32 functions per terminal");
    }
    let function = FunctionId(allocate_local(component).ok_or("Function identity exhausted")?);
    let Some(TerminalClass::Electrical(functions)) = component
        .terminals
        .iter_mut()
        .find(|t| t.id == id)
        .map(|t| &mut t.class)
    else {
        return Err("Only electrical terminals have functions");
    };
    let label = (functions.functions.len() + 1..)
        .map(|n| format!("function {n}"))
        .find(|label| functions.functions.iter().all(|f| &f.label != label))
        .unwrap_or_default();
    functions.functions.push(Function {
        id: function,
        label,
        direction: Direction::Passive,
        signal: SignalKind::Unspecified,
        drive: Drive::None,
        resource_role: None,
    });
    Ok(function)
}

/// Parse a component default for a field: empty text is no default.
fn parse_default(text: &str, view: &FieldView<'_>) -> Result<Option<FieldValue>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    fields::parse_value(text, &view.field.definition.value_type, view.kind_body())
        .map(Some)
        .map_err(|e| e.to_string())
}

/// A default as the field displays it; empty for none.
fn default_text(value: Option<&FieldValue>, view: &FieldView<'_>) -> String {
    value
        .map(|v| fields::display(v, &view.field.definition.value_type, view.kind_body()))
        .unwrap_or_default()
}

/// Set the component default of a selected field from text. Invalid text
/// leaves the component unchanged. Returns the stored default as text.
fn set_field_default(
    component: &mut ComponentDefinition,
    library: &Library,
    lineage: DefinitionId,
    text: &str,
) -> Result<String, String> {
    let selection = component
        .fields
        .iter_mut()
        .find(|f| f.field.id == lineage)
        .ok_or("The component does not select this field")?;
    let view = library
        .field_view(selection.field)
        .ok_or("The field is not in the project library")?;
    let value = parse_default(text, &view)?;
    let shown = default_text(value.as_ref(), &view);
    selection.default = value;
    Ok(shown)
}

/// Published field lineages the component does not select yet, at their
/// latest revision.
fn addable_fields<'a>(
    component: &ComponentDefinition,
    library: &'a Library,
) -> Vec<&'a Published<FieldDefinition>> {
    let mut latest: Vec<&Published<FieldDefinition>> = Vec::new();
    for field in &library.fields {
        if component.field(field.id).is_some() {
            continue;
        }
        match latest.iter_mut().find(|f| f.id == field.id) {
            Some(slot) if slot.revision < field.revision => *slot = field,
            Some(_) => {}
            None => latest.push(field),
        }
    }
    latest.sort_by(|a, b| a.meta.name.cmp(&b.meta.name));
    latest
}

/// Text typed into a field default, kept across frames by field lineage.
#[derive(Clone, Debug, Default, PartialEq)]
struct FieldText {
    /// The default's text when the buffer last matched the working copy.
    loaded: String,
    text: String,
}

impl FieldText {
    fn new(text: String) -> Self {
        Self {
            loaded: text.clone(),
            text,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum View {
    #[default]
    Symbol,
    Footprint,
    Mapping,
    Model,
}

/// A request the editor cannot express as one operation; the app runs it
/// through the engine with public commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Reserve five new lineages and fill them from a template.
    Create(Template),
    /// Draft the lineages of a published component revision.
    Draft(ComponentRef),
}

pub struct PartEditor {
    /// Working copy edited by widgets.
    draft: Option<WorkingSet>,
    /// What the working copy was loaded from; a difference is uncommitted work.
    baseline: Option<WorkingSet>,
    /// The engine drafts last loaded, compared by identity to notice undo,
    /// redo, open and publish.
    source: Option<Arc<Drafts>>,
    /// The component lineage being authored.
    lineage: Option<DefinitionId>,
    /// A commit was sent; resync from the engine whatever its outcome.
    awaiting: bool,
    publishing: bool,
    /// Publish once the edit sent before it has been applied.
    publish_next: bool,
    request: Option<Request>,
    view: View,
    selected: Option<TerminalId>,
    notice: String,
    /// The pad the Footprint view and its properties edit.
    selected_pad: Option<PadId>,
    /// Footprint view pan and zoom; fitted when unset.
    footprint_view: Option<symbol_canvas::ViewState>,
    pad_drag: Option<footprint_canvas::PadDrag>,
    footprint_layers: footprint_canvas::Layers,
    findings: Option<(u64, Vec<String>)>,
    update: crate::update_preview::UpdatePreview,
    update_instances: std::collections::BTreeSet<Id>,
    /// Typed component defaults by field lineage.
    field_buffers: std::collections::BTreeMap<DefinitionId, FieldText>,
    /// The armed Symbol tool and its pending points.
    drawing: drawing::Drawing,
    /// Selected pins and graphics of the unit being drawn.
    selection: arrange::Selection,
    /// The unit being drawn; the first when unset.
    unit: Option<UnitId>,
    /// Symbol view pan and zoom; fitted when unset.
    symbol_view: Option<symbol_canvas::ViewState>,
    gesture: Option<symbol_canvas::Gesture>,
    /// A gesture finished by a key or button, placed on the next canvas frame.
    pending_place: Option<drawing::Placed>,
    snap: bool,
    show_grid: bool,
    /// Stroke width and fill for new shapes.
    stroke_width: f64,
    fill: Fill,
    /// What the Field tool places, when chosen.
    field_choice: Option<TextContent>,
    /// Focus the text content field once (after placing text).
    focus_text: bool,
    /// Space was pressed on the canvas (momentary pan) and is still down.
    space_held: bool,
    /// The drawn unit and its snap targets, kept while the working copy and
    /// engine revision are unchanged so idle frames redo nothing.
    drawn_cache: Option<symbol_canvas::DrawnCache>,
    /// The body rectangle a new terminal's pin row grows: the one the
    /// template or first unit laid out, tracked by ID so a rectangle the
    /// author drew is never stretched. `None` once it is gone.
    body: Option<(UnitId, GraphicId)>,
    /// The component's manufacturer parts.
    parts: part_props::PartProps,
}

impl Default for PartEditor {
    fn default() -> Self {
        Self {
            draft: None,
            baseline: None,
            source: None,
            lineage: None,
            awaiting: false,
            publishing: false,
            publish_next: false,
            request: None,
            view: View::Symbol,
            selected: Some(TerminalId(1)),
            notice: String::new(),
            selected_pad: None,
            footprint_view: None,
            pad_drag: None,
            footprint_layers: Default::default(),
            findings: None,
            update: Default::default(),
            update_instances: Default::default(),
            field_buffers: Default::default(),
            drawing: Default::default(),
            selection: Default::default(),
            unit: None,
            symbol_view: None,
            gesture: None,
            pending_place: None,
            snap: true,
            show_grid: true,
            stroke_width: SYMBOL_STROKE_MM,
            fill: Fill::None,
            field_choice: None,
            focus_text: false,
            space_held: false,
            drawn_cache: None,
            body: None,
            parts: Default::default(),
        }
    }
}

impl PartEditor {
    /// The package envelope the 3D preview shows.
    pub fn preview_package(&self) -> Option<&pcbcraft_core::library::PackageDefinition> {
        self.draft.as_ref().map(|set| &set.package)
    }
    pub fn is_model_view(&self) -> bool {
        self.view == View::Model
    }
    /// A multi-command request for the app to run, if one is pending.
    pub fn take_request(&mut self) -> Option<Request> {
        self.request.take()
    }

    fn unpublished(&self, engine: &Engine) -> bool {
        self.draft
            .as_ref()
            .is_some_and(|set| set.unpublished(engine))
    }

    /// Reload the working copy when the engine's drafts changed elsewhere.
    fn sync(&mut self, engine: &Engine) {
        let current = Arc::clone(&engine.document().drafts);
        let changed = self
            .source
            .as_ref()
            .is_none_or(|s| !Arc::ptr_eq(s, &current));
        if !changed && !self.awaiting {
            return;
        }
        self.awaiting = false;
        let set = WorkingSet::find(&current, self.lineage);
        let lineage = set.as_ref().map(|s| s.ids.component);
        if lineage != self.lineage {
            self.symbol_view = None;
            self.footprint_view = None;
            self.selected_pad = None;
            self.selection.clear();
            self.unit = None;
            self.drawing.arm(drawing::Tool::Select);
            self.body = set.as_ref().and_then(layout_body);
        }
        self.lineage = lineage;
        self.gesture = None;
        if let Some(set) = &set
            && self
                .selected
                .is_none_or(|id| set.component.terminal(id).is_none())
        {
            self.selected = set.component.terminals.first().map(|t| t.id);
        }
        if self.publishing
            && let Some(set) = &set
            && !set.unpublished(engine)
            && let Some(published) = set.published(engine)
        {
            self.notice = format!(
                "Revision {} published. Existing instances retain their snapshots.",
                published.revision.0
            );
        }
        self.publishing = false;
        self.baseline = set.clone();
        self.draft = set;
        self.source = Some(current);
        self.pad_drag = None;
    }

    /// The operation committing a finished edit, if any. Call once per frame
    /// after the editor's widgets ran.
    pub fn commit(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if let Some(publish) = self.publish_pending(engine) {
            return Some(publish);
        }
        let interacting =
            ctx.memory(|m| m.focused().is_some()) || ctx.input(|i| i.pointer.any_down());
        if let Some(lineage) = self.lineage
            && let Some(publish) = self.parts.publish_pending(engine, lineage)
        {
            return Some(publish);
        }
        if interacting {
            return None;
        }
        // A finished Part edit travels with the working set's, so neither
        // reload discards the other's uncommitted work.
        let part = self
            .lineage
            .and_then(|lineage| self.parts.take_edit(engine, lineage));
        if self.draft == self.baseline {
            return part.map(|edit| Operation::Library(LibraryCommand::Edit { edits: vec![edit] }));
        }
        let mut operation = self.send(engine);
        if let Some(Operation::Library(LibraryCommand::Edit { edits })) = &mut operation {
            edits.extend(part);
        }
        operation
    }

    /// Send the working copy with sibling references at their pending
    /// revisions; the next sync shows what the engine accepted.
    fn send(&mut self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?.retargeted(engine.drafts());
        self.baseline = self.draft.clone();
        self.awaiting = true;
        Some(Operation::Library(LibraryCommand::Edit {
            edits: set.edits(),
        }))
    }

    /// Send a deferred publish once the edit before it was applied.
    fn publish_pending(&mut self, engine: &Engine) -> Option<Operation> {
        if self.publish_next && !self.awaiting {
            self.publish_next = false;
            if self.draft == self.baseline
                && let Some(publish) = self.publish_set(engine)
            {
                self.publishing = true;
                self.awaiting = true;
                return Some(publish);
            }
        }
        None
    }

    /// Ribbon tabs are the four views of one component; groups are commands.
    pub fn ribbon(&mut self, engine: &Engine) -> crate::shell::Ribbon {
        use crate::shell::{Group, Ribbon, Tab, Tool};
        self.sync(engine);
        let tabs = [
            ("symbol", "Symbol", View::Symbol),
            ("footprint", "Footprint", View::Footprint),
            ("mapping", "Mapping", View::Mapping),
            ("model", "3D", View::Model),
        ]
        .into_iter()
        .map(|(id, label, view)| Tab {
            id,
            label,
            active: self.view == view,
        })
        .collect();
        if self.update.is_open() {
            return Ribbon {
                tabs,
                groups: vec![],
                notice: Some("Review the instance update in Properties"),
            };
        }
        let has_draft = self.draft.is_some();
        let selected_mapped = self.draft.as_ref().is_some_and(|set| {
            set.binding
                .mappings
                .iter()
                .any(|m| Some(m.terminal) == self.selected)
        });
        let library = Group {
            label: "Component",
            tools: vec![
                Tool::new("passive", "component", "Passive").tip("Two-terminal passive template"),
                Tool::new("connector", "component", "Connector").tip("Two-pin header template"),
                Tool::new("publish", "save", "Publish")
                    .enabled(self.unpublished(engine))
                    .tip("Publish the five drafts as new immutable revisions"),
            ],
        };
        if self.view == View::Symbol {
            return Ribbon {
                tabs,
                groups: self.symbol_groups(has_draft, library),
                notice: None,
            };
        }
        let mut edit = vec![
            Tool::new("terminal", "point", "Terminal")
                .enabled(has_draft)
                .tip("Add a terminal with its symbol pin, package terminal and pad"),
        ];
        edit.push(
            Tool::new("remove-terminal", "delete", "Remove")
                .enabled(has_draft && self.selected.is_some())
                .tip("Remove the selected terminal everywhere in this component"),
        );
        let mut groups = vec![Group {
            label: "Terminals",
            tools: edit,
        }];
        if self.view == View::Footprint {
            // Publish stays ahead of the layer toggles, so at narrow widths
            // the toggles fold into the overflow first.
            let [pads, show] = self.footprint_groups(has_draft, selected_mapped);
            groups.extend([pads, library, show]);
        } else {
            groups.push(library);
        }
        Ribbon {
            tabs,
            groups,
            notice: None,
        }
    }

    /// The Footprint tab's pad tools and layer toggles.
    fn footprint_groups(&self, has_draft: bool, selected_mapped: bool) -> [crate::shell::Group; 2] {
        use crate::shell::{Group, Tool};
        let explicit = self
            .draft
            .as_ref()
            .is_some_and(|set| set.footprint.source.is_none());
        let has_pad = self.draft.as_ref().is_some_and(|set| {
            footprint_props::current_pad(
                &set.footprint,
                &set.binding,
                self.selected_pad,
                self.selected,
            )
            .is_some()
        });
        let pads = vec![
            Tool::new("pad", "rect", "SMD")
                .enabled(selected_mapped && explicit)
                .tip("Add a surface pad to the selected terminal"),
            Tool::new("pad-pth", "circle", "Through-hole")
                .enabled(selected_mapped && explicit)
                .tip("Add a plated through-hole pad to the selected terminal"),
            Tool::new("pad-npth", "hole", "Hole")
                .enabled(has_draft && explicit)
                .tip("Add a non-plated mounting hole (lands no terminal)"),
            Tool::new("pad-fiducial", "point_center", "Fiducial")
                .enabled(has_draft && explicit)
                .tip("Add an optical fiducial (lands no terminal, no paste)"),
            Tool::new("remove-pad", "delete", "Remove pad")
                .enabled(has_pad && explicit)
                .tip("Remove the selected pad; its terminal stays"),
        ];
        let layers = footprint_canvas::LAYER_TOOLS
            .into_iter()
            .map(|(id, icon, label, tip)| {
                Tool::new(id, icon, label)
                    .selected(self.footprint_layers.get(id).unwrap_or(false))
                    .tip(tip)
            })
            .collect();
        [
            Group {
                label: "Pads",
                tools: pads,
            },
            Group {
                label: "Show",
                tools: layers,
            },
        ]
    }

    /// The Symbol tab's drawing ribbon: Select; Draw; Pins; Arrange; Grid
    /// and Snap; then the component's templates and Publish.
    fn symbol_groups(
        &self,
        has_draft: bool,
        library: crate::shell::Group,
    ) -> Vec<crate::shell::Group> {
        use crate::shell::{Group, Tool};
        use drawing::Tool as Draw;
        let armed = self.drawing.tool();
        let tool = |t: Draw| {
            Tool::new(t.id(), t.icon(), t.label())
                .enabled(has_draft)
                .selected(armed == t)
                .tip(t.tip())
        };
        let any = !self.selection.is_empty();
        vec![
            Group {
                label: "Select",
                tools: vec![tool(Draw::Select)],
            },
            Group {
                label: "Draw",
                // Variants fold into split buttons (Line ▾, Rectangle ▾,
                // Circle ▾, Text ▾) so Arrange stays on the ribbon at tiled
                // widths.
                tools: {
                    let family = |i: usize| {
                        let variants = Draw::FAMILIES[i];
                        let shown = if variants.contains(&armed) {
                            armed
                        } else {
                            self.drawing.shown(i)
                        };
                        crate::shell::Tool::family(
                            tool(shown),
                            variants.into_iter().map(tool).collect(),
                        )
                    };
                    vec![
                        family(0),
                        family(1),
                        family(2),
                        tool(Draw::SINGLE),
                        family(3),
                    ]
                },
            },
            Group {
                label: "Pins",
                tools: Draw::PINS.into_iter().map(tool).collect(),
            },
            Group {
                label: "Arrange",
                tools: symbol_canvas::Arrange::ALL
                    .into_iter()
                    .map(|a| {
                        Tool::new(a.id(), a.icon(), a.label())
                            .enabled(any)
                            .tip(a.tip())
                    })
                    .collect(),
            },
            Group {
                label: "Grid",
                tools: vec![
                    Tool::new("grid", "grid", "Grid")
                        .selected(self.show_grid)
                        .tip("Show the symbol grid; set its step in Properties"),
                    Tool::new("snap", "snap", "Snap").selected(self.snap).tip(
                        "Snap to ends, pins, midpoints, centres, curves, alignment and the grid",
                    ),
                ],
            },
            library,
        ]
    }

    /// Arm a Symbol tool. A drawing tool clears the selection, as
    /// Solidcraft's sketch tools do, so Properties edits the tool's options
    /// and nothing else looks editable.
    fn arm_tool(&mut self, tool: drawing::Tool) {
        self.drawing.arm(tool);
        if tool != drawing::Tool::Select {
            self.selection.clear();
            self.gesture = None;
        }
    }

    pub fn on_tab(&mut self, tab: &str) {
        self.view = match tab {
            "footprint" => View::Footprint,
            "mapping" => View::Mapping,
            "model" => View::Model,
            _ => View::Symbol,
        };
    }

    pub fn on_tool(&mut self, tool: &str, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if let Some(t) = drawing::Tool::from_id(tool) {
            self.arm_tool(t);
            return None;
        }
        if let Some(action) = symbol_canvas::Arrange::from_id(tool) {
            self.arrange(action, engine.drafts().grid.symbol_mm);
            return None;
        }
        if self.footprint_layers.toggle(tool) {
            return None;
        }
        match tool {
            "grid" => self.show_grid = !self.show_grid,
            "snap" => self.snap = !self.snap,
            "passive" => self.load_template(Template::TwoTerminal),
            "connector" => self.load_template(Template::Header),
            "terminal" => self.add_terminal(),
            "pad" => self.add_pad(footprint_geometry::NewPad::Smd),
            "pad-pth" => self.add_pad(footprint_geometry::NewPad::PlatedThrough),
            "pad-npth" => self.add_pad(footprint_geometry::NewPad::Hole),
            "pad-fiducial" => self.add_pad(footprint_geometry::NewPad::Fiducial),
            "remove-pad" => self.remove_pad(),
            "remove-terminal" => self.remove_selected(),
            "publish" => return self.publish(engine),
            _ => {}
        }
        None
    }

    /// The properties title band.
    pub fn header(&self, _engine: &Engine) -> crate::shell::Header {
        use crate::shell::Header;
        if self.update.is_open() {
            return Header::new("component", "Update placed instances", "Review");
        }
        match &self.draft {
            Some(set) => Header::new("component", set.metas.component.name.clone(), "Component"),
            None => Header::new("component", "No component", ""),
        }
    }

    /// The component lineage being authored.
    pub fn lineage(&self) -> Option<DefinitionId> {
        self.lineage
    }

    /// Author a component lineage, drafting it from its latest revision when
    /// it has no draft yet.
    pub fn open(&mut self, id: DefinitionId, engine: &Engine) {
        if engine.drafts().get(id).is_none()
            && let Some(published) = engine
                .project()
                .library
                .latest_components()
                .into_iter()
                .find(|c| c.id == id)
        {
            self.request = Some(Request::Draft(ComponentRef::new(id, published.revision)));
        }
        self.lineage = Some(id);
        self.source = None;
        self.notice.clear();
    }

    /// Open the component lineage `id` and draft a new Part of it rated at
    /// `values` (Resolve parts' New Part…).
    pub fn new_part_for(
        &mut self,
        id: DefinitionId,
        values: Vec<(pcbcraft_core::FieldRef, pcbcraft_core::fields::FieldValue)>,
        engine: &Engine,
    ) {
        self.open(id, engine);
        self.parts.request_new(values);
    }

    /// Return the 2D views to their fitted scale.
    pub fn fit(&mut self) {
        self.footprint_view = None;
        self.symbol_view = None;
    }

    fn publish(&mut self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let committed = engine.drafts();
        let pending_refs = set.retargeted(committed) != *set;
        // Part drafts publish with the cascade, pinning the pending revisions.
        let parts = match part_props::Pins::pending(set, committed) {
            Some(pins) => self.parts.retarget_edits(engine, set.ids.component, &pins),
            None => Vec::new(),
        };
        if self.draft != self.baseline || pending_refs || !parts.is_empty() {
            self.publish_next = true;
            let mut operation = self.send(engine);
            if let Some(Operation::Library(LibraryCommand::Edit { edits })) = &mut operation {
                edits.extend(parts);
            }
            return operation;
        }
        let publish = self.publish_set(engine)?;
        self.publishing = true;
        self.awaiting = true;
        Some(publish)
    }

    /// The working set's five drafts and its Part drafts, as one publish set.
    fn publish_set(&self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let mut publish = set.publish();
        if let Operation::Library(LibraryCommand::Publish { drafts }) = &mut publish {
            drafts.extend(part_props::PartProps::draft_ids(engine, set.ids.component));
        }
        Some(publish)
    }

    /// Replace the working copy with a template; the next commit makes it
    /// one undoable step. Without a working set the app creates the lineages.
    fn load_template(&mut self, template: Template) {
        match &mut self.draft {
            Some(set) => {
                set.load_template(template);
                self.selected = set.component.terminals.first().map(|t| t.id);
                self.body = layout_body(set);
            }
            None => self.request = Some(Request::Create(template)),
        }
        self.notice.clear();
        self.footprint_view = None;
        self.selected_pad = None;
        self.symbol_view = None;
        self.selection.clear();
        self.unit = None;
    }

    /// Add a terminal with its pin on the first unit's left edge, growing
    /// the body to meet it.
    fn add_terminal(&mut self) {
        let Some(electrical) = self.new_terminal() else {
            return;
        };
        let Some(set) = &mut self.draft else {
            return;
        };
        let row = set.component.terminals.len().saturating_sub(1) as f64;
        let tracked = self.body;
        if let Some(unit) = set.symbol.units.first_mut() {
            let position = [-2.0 * SYMBOL_MODULE_MM, -row * SYMBOL_MODULE_MM];
            // Grow the tracked body only; without it nothing is resized and
            // the pin faces what is drawn.
            let body = tracked
                .filter(|(u, _)| *u == unit.id)
                .and_then(|(_, body)| grow_body_to(unit, body, position[1]))
                .or_else(|| {
                    unit.graphics
                        .iter()
                        .filter_map(|g| match &g.item {
                            Item::Shape { shape, .. } => Some(shape.bounds(0.0)),
                            Item::Text(_) => None,
                        })
                        .reduce(Bounds::union)
                        .filter(|b| !b.contains(position))
                });
            let pin = match body {
                Some(body) => graphics::pin_toward(electrical, position, body),
                None => Pin::new(electrical, position, PinDirection::Right, SYMBOL_MODULE_MM),
            };
            unit.pins.push(pin);
        }
    }

    /// Add a passive terminal with its package terminal, pad and mapping,
    /// but no pin; the first terminal of an empty symbol also gets a first
    /// unit. Returns the terminal, or `None` with a notice at a limit.
    fn new_terminal(&mut self) -> Option<TerminalId> {
        let set = self.draft.as_mut()?;
        if set.component.terminals.len() >= pcbcraft_core::library::MAX_TERMINALS {
            self.notice = format!(
                "Maximum {} terminals in this edition",
                pcbcraft_core::library::MAX_TERMINALS
            );
            return None;
        }
        let allocate = |next: &mut u32| {
            let id = *next;
            next.checked_add(1).map(|n| {
                *next = n;
                id
            })
        };
        let electrical = allocate(&mut set.component.next_local);
        let function = allocate(&mut set.component.next_local);
        let physical = allocate(&mut set.package.next_local);
        let pad = allocate(&mut set.footprint.next_local);
        let (Some(electrical), Some(function), Some(physical), Some(pad)) =
            (electrical, function, physical, pad)
        else {
            self.notice = "Terminal identity exhausted".into();
            return None;
        };
        if set.symbol.units.is_empty() {
            let (Some(unit), Some(body), Some(designator), Some(name)) = (
                allocate(&mut set.symbol.next_local),
                allocate(&mut set.symbol.next_local),
                allocate(&mut set.symbol.next_local),
                allocate(&mut set.symbol.next_local),
            ) else {
                self.notice = "Unit identity exhausted".into();
                return None;
            };
            set.symbol.pin_text_height = PIN_TEXT_HEIGHT_MM;
            set.symbol.units.push(first_unit(
                UnitId(unit),
                [GraphicId(body), GraphicId(designator), GraphicId(name)],
            ));
            self.body = Some((UnitId(unit), GraphicId(body)));
        }
        let row = set.component.terminals.len() as f64;
        let electrical = TerminalId(electrical);
        let physical = PackageTerminalId(physical);
        // Named after its pin number: the next number no terminal or
        // package terminal uses yet, not the internal ID.
        let taken = |n: &String| {
            set.package.terminals.iter().any(|t| t.designation == *n)
                || set.component.terminals.iter().any(|t| t.name == *n)
        };
        let number = (set.package.terminals.len() + 1..)
            .map(|n| n.to_string())
            .find(|n| !taken(n))
            .unwrap_or_else(|| physical.to_string());
        if set.package.body.is_chip() && set.package.terminals.len() >= 2 {
            self.notice = format!(
                "A {} body has exactly two terminals; change Body on the 3D tab to Moulded, \
                 or remove this terminal",
                set.package.body.label().to_lowercase()
            );
        }
        set.component.terminals.push(Terminal {
            id: electrical,
            name: number.clone(),
            class: TerminalClass::Electrical(TerminalFunctions::fixed(
                FunctionId(function),
                Direction::Passive,
            )),
        });
        set.footprint.pads.push(Pad::smd_rect(
            PadId(pad),
            physical,
            number.clone(),
            [0.0, row * 2.0],
            [1.0, 1.4],
        ));
        set.package.terminals.push(PackageTerminal {
            id: physical,
            designation: number,
        });
        set.binding.mappings.push(TerminalMap {
            package_terminal: physical,
            terminal: electrical,
        });
        self.selected = Some(electrical);
        Some(electrical)
    }

    fn remove_selected(&mut self) {
        let (Some(selected), Some(set)) = (self.selected, &mut self.draft) else {
            return;
        };
        if set.component.terminals.len() <= 1 {
            self.notice = "A component needs at least one terminal".into();
            return;
        }
        let physical: Vec<_> = set
            .binding
            .mappings
            .iter()
            .filter(|m| m.terminal == selected)
            .map(|m| m.package_terminal)
            .collect();
        set.component.terminals.retain(|t| t.id != selected);
        for unit in &mut set.symbol.units {
            unit.pins.retain(|p| p.terminal != selected);
        }
        set.binding.mappings.retain(|m| m.terminal != selected);
        set.package.terminals.retain(|t| !physical.contains(&t.id));
        set.footprint
            .pads
            .retain(|p| p.terminal.is_none_or(|t| !physical.contains(&t)));
        self.selected = set.component.terminals.first().map(|t| t.id);
    }

    /// Add a pad of `which`: copper pads land the selected terminal's
    /// package terminal when no pad lands it yet (otherwise none, which
    /// publishing reports); holes and fiducials land none. Numbers never
    /// repeat an existing pad's. The pad goes to the right of the existing
    /// lands and becomes the selected pad.
    fn add_pad(&mut self, which: footprint_geometry::NewPad) {
        let Some(set) = &mut self.draft else {
            return;
        };
        if set.footprint.source.is_some() {
            self.notice = "Pads are derived from the footprint sketch".into();
            return;
        }
        let terminal = set
            .binding
            .mappings
            .iter()
            .find(|m| Some(m.terminal) == self.selected)
            .map(|m| m.package_terminal)
            .filter(|t| !set.footprint.pads.iter().any(|p| p.terminal == Some(*t)));
        if set.footprint.pads.len() >= pcbcraft_core::library::MAX_PADS {
            self.notice = format!(
                "Maximum {} pads in this edition",
                pcbcraft_core::library::MAX_PADS
            );
            return;
        }
        let id = set.footprint.next_local;
        let Some(next) = id.checked_add(1) else {
            self.notice = "Pad identity exhausted".into();
            return;
        };
        set.footprint.next_local = next;
        let used = |n: &str| set.footprint.pads.iter().any(|p| p.number == n);
        let designation = terminal
            .filter(|_| which.lands_terminal())
            .and_then(|terminal| set.package.terminals.iter().find(|t| t.id == terminal))
            .map(|t| t.designation.clone())
            .filter(|d| !d.is_empty() && !used(d));
        let number = match designation {
            Some(designation) => designation,
            None if which.lands_terminal() => (1..)
                .map(|n: usize| n.to_string())
                .find(|n| !used(n))
                .unwrap_or_default(),
            None => {
                let same = set
                    .footprint
                    .pads
                    .iter()
                    .filter(|p| p.kind == which.kind())
                    .count();
                format!("{}{}", which.prefix(), same + 1)
            }
        };
        let pad = footprint_geometry::new_pad(
            which,
            PadId(id),
            terminal,
            number,
            footprint_geometry::free_spot(&set.footprint.pads),
        );
        set.footprint.pads.push(pad);
        self.selected_pad = Some(PadId(id));
    }

    /// Remove the selected pad. Its package terminal stays; a terminal left
    /// without pads is reported when publishing.
    fn remove_pad(&mut self) {
        let Some(set) = &mut self.draft else {
            return;
        };
        if set.footprint.source.is_some() {
            return;
        }
        let Some(id) = footprint_props::current_pad(
            &set.footprint,
            &set.binding,
            self.selected_pad,
            self.selected,
        ) else {
            return;
        };
        set.footprint.pads.retain(|p| p.id != id);
        if set.footprint.polarity.as_ref().is_some_and(|p| p.pad == id) {
            set.footprint.polarity = None;
        }
        self.selected_pad = None;
    }

    /// Blocking publish findings of the committed drafts, from the engine.
    fn findings(&mut self, engine: &Engine) -> Vec<String> {
        if let Some((revision, findings)) = &self.findings
            && *revision == engine.revision()
        {
            return findings.clone();
        }
        let mut messages = Vec::new();
        if let Some(set) = &self.draft
            && let Ok(QueryResult::Drafts(reports)) = engine.query(Query::CheckDrafts {})
        {
            let mut ids = set.ids.all();
            ids.extend(part_props::PartProps::draft_ids(engine, set.ids.component));
            messages.extend(
                reports
                    .into_iter()
                    .filter(|r| matches!(r.target, CheckTarget::Draft(id) if ids.contains(&id)))
                    .flat_map(|r| r.findings)
                    .filter(|f| f.blocks_stage(Stage::Publish))
                    .map(|f| match fix_hint(&f.code) {
                        Some(hint) => format!("{} {hint}", f.message),
                        None => f.message,
                    }),
            );
        }
        self.findings = Some((engine.revision(), messages.clone()));
        messages
    }

    pub fn inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if self.update.is_open() {
            return self.update.show(ui, engine);
        }
        let unpublished = self.unpublished(engine);
        let findings = self.findings(engine);
        for message in findings.iter().take(6) {
            kit::status(ui, message, Tone::Error);
        }
        if findings.len() > 6 {
            kit::note(ui, &format!("{} more findings", findings.len() - 6));
        }
        if !self.notice.is_empty() {
            kit::status(ui, &self.notice, Tone::Note);
        }
        let Some(set) = &mut self.draft else {
            kit::status(
                ui,
                "Start a component from the Passive or Connector template, or open one in the Browser.",
                Tone::Dim,
            );
            return None;
        };
        let published = set.published(engine).map(|r| r.revision.0);
        let mut operation = None;
        kit::section(ui, "definition", "Definition", true, None, |ui| {
            kit::text_row(ui, "Name", &mut set.metas.component.name);
            let prefix = &mut set.component.designator_prefix;
            let invalid = !designator_prefix_valid(prefix);
            let response = kit::checked_text_row(ui, "Designator", prefix, "R", invalid)
                .on_hover_text(
                    "Prefix new instances are numbered under: 1–8 letters, e.g. R, C, U",
                );
            if response.changed() {
                prefix.make_ascii_uppercase();
            }
            let revision = match published {
                None => "Draft · new".to_owned(),
                Some(r) if unpublished => format!("r{r} · unpublished edits"),
                Some(r) => format!("r{r} · published"),
            };
            kit::value_row(ui, "Revision", &revision);
        });
        match self.view {
            View::Symbol => operation = self.symbol_inspector(ui, engine),
            View::Footprint => self.footprint_inspector(ui),
            View::Mapping => self.mapping_inspector(ui),
            View::Model => self.model_inspector(ui),
        }
        self.fields_section(ui, engine);
        if let Some(set) = &self.draft {
            match self.parts.section(ui, engine, set) {
                Some(part_props::PartsAction::Apply(op)) => operation = operation.or(Some(*op)),
                Some(part_props::PartsAction::PublishCascade) => {
                    operation = operation.or_else(|| self.publish(engine));
                }
                None => {}
            }
        }
        if let Some(set) = &mut self.draft {
            kit::section(ui, "provenance", "Provenance", false, None, |ui| {
                kit::text_row(ui, "Source", &mut set.metas.component.provenance);
                kit::note(
                    ui,
                    "Structural validation does not replace a datasheet review.",
                );
            });
        }
        if !unpublished {
            self.update_section(ui, engine);
        }
        // Library folders (S3): Promote to library… and Duplicate of the
        // published revision; the app runs them.
        if let Some(set) = &self.draft
            && let Some(component) = set.published(engine)
        {
            crate::library_sources::component_section(
                ui,
                engine,
                component,
                &set.metas.component.name,
            );
        }
        operation
    }

    /// Offer to move placed instances to the published revision.
    fn update_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let Some(target) = self.draft.as_ref().and_then(|s| s.published(engine)) else {
            return;
        };
        let candidates: Vec<_> = engine
            .project()
            .instances
            .iter()
            .filter(|i| i.component.id == target.id && i.component.revision != target.revision)
            .collect();
        self.update_instances
            .retain(|id| candidates.iter().any(|i| i.id == *id));
        if candidates.is_empty() {
            return;
        }
        let selected = &mut self.update_instances;
        let begin = kit::section(
            ui,
            "update-instances",
            "Update placed instances",
            true,
            Some(candidates.len()),
            |ui| {
                kit::note(
                    ui,
                    &format!(
                        "Use published revision {} for the selected instances.",
                        target.revision.0
                    ),
                );
                for instance in &candidates {
                    let mut checked = selected.contains(&instance.id);
                    let label = format!(
                        "{} · revision {}",
                        instance.designator, instance.component.revision.0
                    );
                    if kit::check_row(ui, &label, &mut checked) {
                        if checked {
                            selected.insert(instance.id);
                        } else {
                            selected.remove(&instance.id);
                        }
                    }
                }
                let mut begin = false;
                kit::actions(ui, |ui| {
                    let style = if selected.is_empty() {
                        Button::Enabled(false)
                    } else {
                        Button::Primary
                    };
                    begin = ui_kit::button(ui, "Preview update", style).clicked()
                        && !selected.is_empty();
                });
                begin
            },
        );
        if begin == Some(true) {
            self.update.begin(
                engine,
                pcbcraft_core::updates::Change::ComponentRevision {
                    instances: self.update_instances.iter().copied().collect(),
                    target,
                    resolutions: Default::default(),
                },
            );
        }
    }

    /// The component's selected fields with their component defaults.
    fn fields_section(&mut self, ui: &mut Ui, engine: &Engine) {
        let library = &engine.project().library;
        let Some(set) = &mut self.draft else {
            return;
        };
        let component = &mut set.component;
        let buffers = &mut self.field_buffers;
        buffers.retain(|lineage, _| component.field(*lineage).is_some());
        let count = component.fields.len();
        kit::section(ui, "fields", "Fields", true, Some(count), |ui| {
            let mut remove = None;
            for index in 0..component.fields.len() {
                let Some(selection) = component.fields.get(index).cloned() else {
                    continue;
                };
                let lineage = selection.field.id;
                if index > 0 {
                    kit::rule(ui);
                }
                ui.push_id(("field", lineage), |ui| {
                    let Some(view) = library.field_view(selection.field) else {
                        kit::value_row(ui, "Field", &selection.field.to_string());
                        kit::status(
                            ui,
                            "This field revision is not in the project library.",
                            Tone::Error,
                        );
                        return;
                    };
                    let shown = default_text(selection.default.as_ref(), &view);
                    let buffer = buffers
                        .entry(lineage)
                        .or_insert_with(|| FieldText::new(shown.clone()));
                    if buffer.loaded != shown {
                        *buffer = FieldText::new(shown);
                    }
                    let error = parse_default(&buffer.text, &view).err();
                    let hint = view
                        .field
                        .definition
                        .default
                        .as_ref()
                        .map(|d| default_text(Some(d), &view))
                        .unwrap_or_else(|| "No default".into());
                    let invalid = error.is_some();
                    let response = kit::row(ui, &view.field.meta.name, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui_kit::button(ui, "Remove", Button::Small)
                                .on_hover_text("Stop selecting this field")
                                .clicked()
                            {
                                remove = Some(index);
                            }
                            ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                            kit::text_field(ui, "default", &mut buffer.text, &hint, invalid)
                                .on_hover_text("Component default; empty uses the field default")
                        })
                        .inner
                    });
                    if response.changed()
                        && let Ok(stored) =
                            set_field_default(component, library, lineage, &buffer.text)
                    {
                        buffer.loaded = stored;
                    }
                    let error = parse_default(&buffer.text, &view).err();
                    if !response.has_focus() && error.is_none() && buffer.text != buffer.loaded {
                        buffer.text = buffer.loaded.clone();
                    }
                    if let Some(error) = &error {
                        kit::status(ui, error, Tone::Error);
                    }
                    if let Some(selection) = component.fields.get_mut(index) {
                        kit::check_row(ui, "Show on symbol", &mut selection.show_on_symbol);
                    }
                });
            }
            if let Some(index) = remove
                && index < component.fields.len()
            {
                component.fields.remove(index);
            }
            if count > 0 {
                kit::rule(ui);
            }
            let addable = addable_fields(component, library);
            if addable.is_empty() {
                kit::note(ui, "Every published field is selected.");
            } else if component.fields.len() >= MAX_FIELDS {
                kit::note(ui, "Maximum 64 fields per component.");
            } else {
                let mut chosen = None;
                kit::row(ui, "Add field", |ui| {
                    kit::dropdown_menu(ui, "add-field", "Choose a field", |ui| {
                        for field in &addable {
                            let label = format!("{} · r{}", field.meta.name, field.revision.0);
                            if ui.selectable_label(false, label).clicked() {
                                chosen = Some(FieldRef::new(field.id, field.revision));
                            }
                        }
                    });
                });
                if let Some(field) = chosen {
                    component.fields.push(FieldSelection {
                        field,
                        default: None,
                        show_on_symbol: true,
                    });
                }
            }
        });
    }

    /// One compact selectable row per terminal.
    fn terminal_list(&mut self, ui: &mut Ui) {
        let Some(set) = &self.draft else {
            return;
        };
        let terminals = &set.component.terminals;
        let selected = &mut self.selected;
        kit::section(
            ui,
            "terminals",
            "Terminals",
            true,
            Some(terminals.len()),
            |ui| {
                for terminal in terminals {
                    let summary = pin_summary(terminal);
                    let row = kit::Row {
                        icon: "point",
                        label: &terminal.name,
                        secondary: &summary,
                        selected: *selected == Some(terminal.id),
                        tip: "Select this terminal",
                        ..Default::default()
                    };
                    if kit::tree_row(ui, ui.id().with(("terminal", terminal.id.0)), &row).clicked {
                        *selected = Some(terminal.id);
                    }
                }
            },
        );
    }

    fn symbol_inspector(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.tool_section(ui, engine);
        self.selection_section(ui, engine);
        self.unit_section(ui);
        let operation = self.grid_section(ui, engine);
        self.terminal_list(ui);
        let (Some(id), Some(set)) = (self.selected, &mut self.draft) else {
            return operation;
        };
        let component = &mut set.component;
        let placed = set.symbol.pin(id).is_some();
        let notice = &mut self.notice;
        let mut remove = false;
        kit::section(
            ui,
            "selected-terminal",
            "Selected terminal",
            true,
            None,
            |ui| {
                if let Some(terminal) = component.terminals.iter_mut().find(|t| t.id == id) {
                    kit::text_row(ui, "Name", &mut terminal.name);
                }
                if let Some(mut class) = component.terminal(id).map(ClassChoice::of) {
                    let options: Vec<_> = CLASS_CHOICES
                        .iter()
                        .map(|(c, label)| (*c, (*label).to_owned()))
                        .collect();
                    if kit::choice_row(ui, "Class", &mut class, &options)
                        && let Err(message) = set_class(component, id, class)
                    {
                        *notice = message.into();
                    }
                }
                if placed {
                    kit::note(
                        ui,
                        "Select the pin on the canvas to edit it; drag to move it.",
                    );
                } else {
                    kit::note(ui, "No pin yet: place it with the Pin tool (N).");
                }
                kit::actions(ui, |ui| {
                    if ui_kit::button(ui, "Remove terminal", Button::Normal).clicked() {
                        remove = true;
                    }
                });
            },
        );
        functions_section(ui, component, id, notice);
        if remove {
            self.remove_selected();
        }
        operation
    }

    fn mapping_inspector(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let options: Vec<_> = std::iter::once((None, "Unmapped".to_owned()))
            .chain(
                set.component
                    .terminals
                    .iter()
                    .map(|t| (Some(t.id), t.name.clone())),
            )
            .collect();
        let count = set.package.terminals.len();
        let binding = &mut set.binding;
        let terminals = &mut set.package.terminals;
        kit::section(
            ui,
            "mapping",
            "Package terminals",
            true,
            Some(count),
            |ui| {
                kit::note(
                    ui,
                    "Designation, then the electrical terminal it carries. Matching names do not establish compatibility.",
                );
                for physical in terminals.iter_mut() {
                    let mut mapped = binding.terminal_of(physical.id);
                    let before = mapped;
                    ui.push_id(("package-terminal", physical.id.0), |ui| {
                        kit::row(ui, &format!("Package {}", physical.id), |ui| {
                            let t = Tokens::get();
                            let half = ((ui.available_width() - t.space_4) / 2.0).max(0.0);
                            ui.allocate_ui_with_layout(
                                egui::vec2(half, t.control_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().text_edit_width = half;
                                    kit::text_field(
                                        ui,
                                        "designation",
                                        &mut physical.designation,
                                        "",
                                        false,
                                    )
                                    .on_hover_text("Package terminal designation");
                                },
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(half, t.control_height),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| kit::dropdown(ui, "electrical", &mut mapped, &options),
                            );
                        });
                    });
                    if mapped != before {
                        binding
                            .mappings
                            .retain(|m| m.package_terminal != physical.id);
                        if let Some(terminal) = mapped {
                            binding.mappings.push(TerminalMap {
                                package_terminal: physical.id,
                                terminal,
                            });
                        }
                    }
                }
            },
        );
    }

    fn model_inspector(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let package = &mut set.package;
        kit::section(ui, "model-body", "Body", true, None, |ui| {
            let options: Vec<(PackageBody, String)> = PackageBody::ALL
                .into_iter()
                .map(|b| (b, b.label().to_owned()))
                .collect();
            kit::choice_row(ui, "Body", &mut package.body, &options);
            for (index, label) in ["Length", "Width", "Height"].into_iter().enumerate() {
                kit::number_row(
                    ui,
                    label,
                    &mut package.body_size[index],
                    "mm",
                    0.01..=10000.0,
                );
            }
            kit::note(
                ui,
                "Approximate parametric envelope, not a verified supplier model.",
            );
        });
        kit::section(ui, "model-offset", "Model offset", true, None, |ui| {
            for (index, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                kit::number_row(
                    ui,
                    label,
                    &mut package.model_offset[index],
                    "mm",
                    -10000.0..=10000.0,
                );
            }
            kit::note(ui, "The live model and native export use Solidcraft.");
        });
    }

    pub fn canvas(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        if self.update.is_open() {
            ui.disable();
        }
        if self.view == View::Model {
            ui.label("Native Solidcraft package preview");
            return None;
        }
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        if self.view == View::Mapping {
            self.mapping_canvas(ui, rect);
            return None;
        }
        if self.draft.is_none() {
            ui.put(rect, egui::Label::new("No component draft"));
            return None;
        }
        if self.view == View::Symbol {
            self.symbol_canvas(ui, rect, &response, engine);
            return None;
        }
        self.footprint_canvas(ui, rect, &response, engine.drafts().grid.footprint_mm);
        None
    }

    fn mapping_canvas(&mut self, ui: &mut Ui, rect: Rect) {
        let Some(set) = &self.draft else {
            return;
        };
        let t = Tokens::get();
        let painter = ui.painter().with_clip_rect(rect);
        let left = rect.left() + t.space_24;
        let right = rect.right() - t.space_24;
        painter.text(
            Pos2::new(left, rect.top() + t.space_16),
            Align2::LEFT_TOP,
            "Electrical terminal",
            fonts::semibold(t.font_panel),
            t.text,
        );
        painter.text(
            Pos2::new(right, rect.top() + t.space_16),
            Align2::RIGHT_TOP,
            "Package terminal / copper pads",
            fonts::semibold(t.font_panel),
            t.text,
        );
        let top = rect.top() + t.form_header_height + t.space_16;
        let mut clicked = None;
        for (index, physical) in set.package.terminals.iter().enumerate() {
            let y = top + index as f32 * (t.control_height + t.space_12);
            let electrical = set
                .binding
                .terminal_of(physical.id)
                .and_then(|id| set.component.terminal(id));
            let ink = if electrical.is_some_and(|e| Some(e.id) == self.selected) {
                t.body_selected
            } else {
                t.accent
            };
            painter.line_segment(
                [Pos2::new(left + 120.0, y), Pos2::new(right - 150.0, y)],
                Stroke::new(2.0, ink),
            );
            let pad_ids = set
                .footprint
                .pads
                .iter()
                .filter(|p| p.terminal == Some(physical.id))
                .map(|p| p.id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            painter.text(
                Pos2::new(left, y),
                Align2::LEFT_CENTER,
                electrical.map(|e| e.name.as_str()).unwrap_or("Unmapped"),
                FontId::proportional(t.font_label),
                t.text,
            );
            painter.text(
                Pos2::new(right, y),
                Align2::RIGHT_CENTER,
                format!("{}  ·  pads {}", physical.designation, pad_ids),
                FontId::proportional(t.font_label),
                t.text,
            );
            if ui
                .interact(
                    Rect::from_min_max(
                        Pos2::new(left, y - t.control_height / 2.0),
                        Pos2::new(right, y + t.control_height / 2.0),
                    ),
                    ui.id().with(("mapping-row", physical.id.0)),
                    Sense::click(),
                )
                .clicked()
            {
                clicked = Some(electrical.map(|e| e.id));
            }
        }
        if let Some(selected) = clicked {
            self.selected = selected;
        }
    }
}

/// Run a request through the engine with public commands. Each command is its
/// own undo step.
pub fn run(engine: &mut Engine, request: Request) -> pcbcraft_core::Result<()> {
    match request {
        Request::Create(template) => working_set::create(engine, template),
        Request::Draft(component) => working_set::draft_closure(engine, component),
    }
}

/// The function table of an electrical terminal: label, direction, signal
/// and drive per function, and the design default. The core checks the
/// declaration; its findings appear at the top of the panel.
fn functions_section(
    ui: &mut Ui,
    component: &mut ComponentDefinition,
    id: TerminalId,
    notice: &mut String,
) {
    let Some(count) = component
        .terminal(id)
        .and_then(Terminal::functions)
        .map(|f| f.functions.len())
    else {
        return;
    };
    let directions: Vec<_> = DIRECTIONS
        .iter()
        .map(|d| (*d, sentence(d.label())))
        .collect();
    let signals: Vec<_> = SIGNALS.iter().map(|(s, l)| (*s, (*l).to_owned())).collect();
    let drives: Vec<_> = DRIVES.iter().map(|(d, l)| (*d, (*l).to_owned())).collect();
    kit::section(ui, "functions", "Functions", true, Some(count), |ui| {
        let mut add = false;
        if let Some(TerminalClass::Electrical(table)) = component
            .terminals
            .iter_mut()
            .find(|t| t.id == id)
            .map(|t| &mut t.class)
        {
            let only = table.functions.len() <= 1;
            let mut remove = None;
            for (index, function) in table.functions.iter_mut().enumerate() {
                if index > 0 {
                    kit::rule(ui);
                }
                ui.push_id(("function", function.id.0), |ui| {
                    kit::row(ui, &format!("Function {}", index + 1), |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let response = ui
                                .add_enabled_ui(!only, |ui| {
                                    ui_kit::button(ui, "Remove", Button::Small)
                                })
                                .inner;
                            if response.clicked() {
                                remove = Some(index);
                            }
                            ui.spacing_mut().text_edit_width = ui.available_width().max(0.0);
                            kit::text_field(ui, "label", &mut function.label, "Label", false)
                                .on_hover_text("Function label, unique on this terminal");
                        });
                    });
                    kit::choice_row(ui, "Direction", &mut function.direction, &directions);
                    kit::choice_row(ui, "Signal", &mut function.signal, &signals);
                    kit::choice_row(ui, "Drive", &mut function.drive, &drives);
                });
            }
            if let Some(index) = remove
                && table.functions.len() > 1
                && index < table.functions.len()
            {
                let removed = table.functions.remove(index);
                if table.design_default == Some(removed.id) {
                    table.design_default = None;
                }
            }
            if table.functions.len() > 1 {
                kit::rule(ui);
                let options: Vec<_> = std::iter::once((None, "None".to_owned()))
                    .chain(
                        table
                            .functions
                            .iter()
                            .map(|f| (Some(f.id), f.label.clone())),
                    )
                    .collect();
                kit::choice_row(ui, "Default", &mut table.design_default, &options);
            }
            kit::actions(ui, |ui| {
                add = ui_kit::button(
                    ui,
                    "Add function",
                    Button::Enabled(table.functions.len() < MAX_FUNCTIONS),
                )
                .clicked();
            });
        }
        if add && let Err(message) = add_function(component, id) {
            *notice = message.into();
        }
    });
}

/// Where an author fixes a blocking finding the Component editor shows,
/// by finding code.
fn fix_hint(code: &str) -> Option<&'static str> {
    match code {
        "package.body" => Some("Change Body on the 3D tab, or remove terminals."),
        _ => None,
    }
}

/// The first unit of a part built by hand: a one-module body with the
/// designator above it and the component name below it, as the templates
/// draw them. `ids` are the body, designator and name graphics.
fn first_unit(id: UnitId, ids: [GraphicId; 3]) -> Unit {
    let body = Bounds {
        min: [-SYMBOL_MODULE_MM, -SYMBOL_MODULE_MM],
        max: [SYMBOL_MODULE_MM, SYMBOL_MODULE_MM],
    };
    let gap = 0.5;
    Unit {
        id,
        label: "A".into(),
        graphics: vec![
            graphics::body_rect(ids[0], body),
            graphics::text(
                ids[1],
                TextContent::Designator,
                [body.center()[0], body.max[1] + gap],
                Justify {
                    h: HAlign::Center,
                    v: VAlign::Bottom,
                },
            ),
            graphics::text(
                ids[2],
                TextContent::ComponentName,
                [body.center()[0], body.min[1] - gap],
                Justify {
                    h: HAlign::Center,
                    v: VAlign::Top,
                },
            ),
        ],
        pins: vec![],
    }
}

/// The body rectangle a template or first unit lays out: the first unit's
/// first graphic when it is a body-filled rectangle.
fn layout_body(set: &WorkingSet) -> Option<(UnitId, GraphicId)> {
    let unit = set.symbol.units.first()?;
    let first = unit.graphics.first()?;
    matches!(
        first.item,
        Item::Shape {
            shape: Shape::Rect { .. },
            fill: Fill::Body,
            ..
        }
    )
    .then_some((unit.id, first.id))
}

/// Grow the body rectangle `body` downward so a pin row at `y` meets it one
/// module above its lower edge, moving text below the body along with it.
/// Returns the body, or `None` when the unit has no such rectangle.
fn grow_body_to(unit: &mut Unit, body: GraphicId, y: f64) -> Option<Bounds> {
    fn rect(g: &mut pcbcraft_core::library::Graphic) -> Option<(&mut Point, &mut Point)> {
        match &mut g.item {
            Item::Shape {
                shape: Shape::Rect { a, b, .. },
                ..
            } => Some((a, b)),
            _ => None,
        }
    }
    let (a, b) = unit
        .graphics
        .iter_mut()
        .find(|g| g.id == body)
        .and_then(rect)?;
    let bottom = a[1].min(b[1]);
    let wanted = y - SYMBOL_MODULE_MM;
    let shift = wanted - bottom;
    if shift < 0.0 {
        if a[1] <= b[1] {
            a[1] = wanted;
        } else {
            b[1] = wanted;
        }
        for graphic in &mut unit.graphics {
            if let Item::Text(text) = &mut graphic.item
                && text.at[1] < bottom
            {
                text.at[1] += shift;
            }
        }
    }
    unit.graphics
        .iter_mut()
        .find(|g| g.id == body)
        .and_then(rect)
        .and_then(|(a, b)| Bounds::of([*a, *b]))
}

#[cfg(test)]
mod draft_tests {
    use super::*;
    use pcbcraft_core::library::TextMetric;
    use pcbcraft_core::schematic;

    fn component_name(engine: &Engine, editor: &PartEditor) -> Option<String> {
        let id = editor.draft.as_ref()?.ids.component;
        engine.drafts().get(id).map(|d| d.meta.name.clone())
    }

    #[test]
    fn finished_edits_commit_once_and_engine_changes_reload_the_working_copy()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        assert!(
            editor.commit(&ctx, &engine).is_none(),
            "loading is not an edit"
        );
        let original = editor.draft.clone();
        assert!(original.is_some());
        if let Some(set) = &mut editor.draft {
            set.metas.component.name = "Edited".into();
        }
        let operation = editor.commit(&ctx, &engine);
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(component_name(&engine, &editor).as_deref(), Some("Edited"));
        engine.undo()?;
        assert!(editor.commit(&ctx, &engine).is_none());
        assert_eq!(editor.draft, original);
        Ok(())
    }

    #[test]
    fn a_part_built_terminal_by_terminal_lints_clean_with_pins_on_the_body()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        if let Some(set) = &mut editor.draft {
            set.component.terminals.clear();
            set.symbol.units.clear();
            set.package.terminals.clear();
            set.footprint.pads.clear();
            set.binding.mappings.clear();
        }
        for _ in 0..4 {
            editor.add_terminal();
        }
        let symbol = editor
            .draft
            .as_ref()
            .map(|set| set.ids.symbol)
            .ok_or(pcbcraft_core::Error::Limit("no working set".into()))?;
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        let draft = engine
            .drafts()
            .get(symbol)
            .ok_or(pcbcraft_core::Error::Limit("no symbol draft".into()))?;
        let findings = pcbcraft_core::library::lint(
            draft,
            &engine.project().library,
            &pcbcraft_core::library::Grid::default(),
        );
        assert!(findings.is_empty(), "{findings:?}");
        let pcbcraft_core::library::Definition::Symbol(definition) = &draft.definition else {
            return Err(pcbcraft_core::Error::Limit("not a symbol".into()));
        };
        let unit = &definition.units[0];
        assert_eq!(unit.pins.len(), 4);
        let body = unit
            .graphics
            .iter()
            .find_map(|g| match &g.item {
                Item::Shape {
                    shape: Shape::Rect { a, b, .. },
                    ..
                } => Bounds::of([*a, *b]),
                _ => None,
            })
            .ok_or(pcbcraft_core::Error::Limit("no body".into()))?;
        for pin in &unit.pins {
            let end = pin.body_end();
            assert!(
                (end[0] - body.min[0]).abs() < 1e-9 && end[1] > body.min[1] && end[1] < body.max[1],
                "pin {pin:?} ends off the body {body:?}"
            );
        }
        // The component name stays below the grown body.
        assert!(unit.graphics.iter().any(|g| matches!(
            &g.item,
            Item::Text(t) if t.content == TextContent::ComponentName && t.at[1] < body.min[1]
        )));
        Ok(())
    }

    #[test]
    fn loading_a_template_is_an_undoable_draft_change() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        editor.load_template(Template::Header);
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(
            component_name(&engine, &editor).as_deref(),
            Some("Connector")
        );
        engine.undo()?;
        editor.commit(&ctx, &engine);
        assert_eq!(
            component_name(&engine, &editor).as_deref(),
            Some("Resistor")
        );
        Ok(())
    }

    #[test]
    fn publishing_the_working_set_is_one_cascade_through_public_commands()
    -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        assert!(
            !editor.unpublished(&engine),
            "the demo drafts are published"
        );
        if let Some(set) = &mut editor.draft {
            set.footprint.pads[0].position = [-1.5, 0.0];
        }
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        editor.sync(&engine);
        assert!(editor.unpublished(&engine));
        let operation = editor.publish(&engine);
        assert!(matches!(
            operation,
            Some(Operation::Library(LibraryCommand::Publish { .. }))
        ));
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        editor.sync(&engine);
        assert!(!editor.unpublished(&engine));
        assert_eq!(engine.project().library.components.len(), 2);
        assert_eq!(engine.project().library.footprints.len(), 2);
        assert!(editor.notice.contains("Revision 2 published"));
        Ok(())
    }

    #[test]
    fn a_new_component_is_created_from_a_template_in_an_empty_project() -> pcbcraft_core::Result<()>
    {
        let ctx = egui::Context::default();
        let mut engine = Engine::default();
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        assert!(editor.draft.is_none());
        editor.load_template(Template::TwoTerminal);
        let request = editor.take_request();
        assert_eq!(request, Some(Request::Create(Template::TwoTerminal)));
        if let Some(request) = request {
            run(&mut engine, request)?;
        }
        editor.commit(&ctx, &engine);
        assert_eq!(
            component_name(&engine, &editor).as_deref(),
            Some("Resistor")
        );
        if let Some(operation) = editor.publish(&engine) {
            engine.apply(engine.revision(), operation)?;
        }
        assert_eq!(engine.project().library.components.len(), 1);
        Ok(())
    }

    fn resistance(editor: &PartEditor) -> Option<DefinitionId> {
        editor
            .draft
            .as_ref()?
            .component
            .fields
            .first()
            .map(|f| f.field.id)
    }

    #[test]
    fn a_field_default_commits_once_as_one_undoable_edit() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let lineage = resistance(&editor).ok_or(pcbcraft_core::Error::Limit("no field".into()))?;
        let original = editor.draft.clone();
        if let Some(set) = &mut editor.draft {
            let stored = set_field_default(
                &mut set.component,
                &engine.project().library,
                lineage,
                "10k",
            );
            assert!(stored.is_ok_and(|text| !text.is_empty()));
        }
        let operation = editor.commit(&ctx, &engine);
        assert!(matches!(
            operation,
            Some(Operation::Library(LibraryCommand::Edit { .. }))
        ));
        assert!(editor.commit(&ctx, &engine).is_none(), "sent once");
        if let Some(operation) = operation {
            engine.apply(engine.revision(), operation)?;
        }
        editor.commit(&ctx, &engine);
        let committed = engine
            .drafts()
            .get(
                editor
                    .lineage
                    .ok_or(pcbcraft_core::Error::Limit("no lineage".into()))?,
            )
            .and_then(|d| match &d.definition {
                pcbcraft_core::library::Definition::Component(c) => c.field(lineage).cloned(),
                _ => None,
            });
        assert!(matches!(
            committed.and_then(|f| f.default),
            Some(FieldValue::Quantity { si, .. }) if (si - 10_000.0).abs() < 1e-9
        ));
        engine.undo()?;
        editor.commit(&ctx, &engine);
        assert_eq!(editor.draft, original);
        Ok(())
    }

    #[test]
    fn invalid_default_text_leaves_the_draft_unchanged() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let lineage = resistance(&editor).ok_or(pcbcraft_core::Error::Limit("no field".into()))?;
        let original = editor.draft.clone();
        if let Some(set) = &mut editor.draft {
            let result = set_field_default(
                &mut set.component,
                &engine.project().library,
                lineage,
                "ten volts",
            );
            assert!(result.is_err());
        }
        assert_eq!(editor.draft, original);
        assert!(editor.commit(&ctx, &engine).is_none(), "nothing to commit");
        Ok(())
    }

    #[test]
    fn adding_a_function_allocates_a_fresh_id_and_commits() -> pcbcraft_core::Result<()> {
        let ctx = egui::Context::default();
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let terminal = editor
            .selected
            .ok_or(pcbcraft_core::Error::Limit("no terminal".into()))?;
        let Some(set) = &mut editor.draft else {
            return Err(pcbcraft_core::Error::Limit("no draft".into()));
        };
        let next = set.component.next_local;
        let added = add_function(&mut set.component, terminal);
        assert_eq!(added, Ok(FunctionId(next)));
        assert_eq!(set.component.next_local, next + 1);
        let table = set
            .component
            .terminal(terminal)
            .and_then(Terminal::functions);
        assert_eq!(table.map(|t| t.functions.len()), Some(2));
        let labels: Vec<_> = table
            .map(|t| t.functions.iter().map(|f| f.label.clone()).collect())
            .unwrap_or_default();
        assert_ne!(labels[0], labels[1], "labels stay unique");
        if let Some(operation) = editor.commit(&ctx, &engine) {
            engine.apply(engine.revision(), operation)?;
        }
        editor.commit(&ctx, &engine);
        let committed = engine
            .drafts()
            .get(
                editor
                    .lineage
                    .ok_or(pcbcraft_core::Error::Limit("no lineage".into()))?,
            )
            .and_then(|d| match &d.definition {
                pcbcraft_core::library::Definition::Component(c) => c
                    .terminal(terminal)
                    .and_then(Terminal::functions)
                    .map(|f| f.functions.len()),
                _ => None,
            });
        assert_eq!(committed, Some(2));
        let findings = editor.findings(&engine);
        assert!(
            findings.is_empty(),
            "a second passive function is valid: {findings:?}"
        );
        Ok(())
    }

    /// Run one inspector frame. Tests have no renderer, so the texture
    /// deltas are cleared rather than applied.
    fn frame(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, Pos2)> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 2400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.inspector(ui, engine);
            },
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
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
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    /// Run one canvas frame in a 600 × 400 area; returns the drawn texts.
    fn canvas_frame(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<String> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
                events,
                ..Default::default()
            },
            |ui| {
                editor.canvas(ui, engine);
            },
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        out
    }

    /// The Symbol view draws what core resolves for the working set and
    /// picks pins through the core hit test; a dragged pin end lands on the
    /// symbol grid.
    #[test]
    fn the_symbol_view_draws_and_picks_through_core_queries()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let shown = canvas_frame(&mut editor, &engine, &ctx, vec![]);
        // The designator placeholder reads prefix + "?"; numbers come from
        // the working binding; the shown field reads its name (no default).
        for text in ["R?", "Resistor", "1", "2", "Resistance"] {
            assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
        }
        // The view fits the drawn bounds about the origin (3 mm margin).
        let set = editor.draft.as_ref().ok_or("no draft")?;
        let bounds = schematic::definition_unit(
            &engine.project().library,
            schematic::DefinitionUnit {
                symbol: &set.symbol,
                unit: set.symbol.units[0].id,
                component: &set.component,
                component_name: &set.metas.component.name,
                physical: Some((&set.binding, &set.package)),
            },
        )?
        .bounds(TextMetric::default())
        .ok_or("nothing drawn")?;
        let half = |lo: f64, hi: f64| lo.abs().max(hi.abs());
        let scale = (600.0 / (2.0 * half(bounds.min[0], bounds.max[0]) + 6.0))
            .min(400.0 / (2.0 * half(bounds.min[1], bounds.max[1]) + 6.0))
            as f32;
        let at = |p: Point| Pos2::new(300.0 + p[0] as f32 * scale, 200.0 - p[1] as f32 * scale);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Click pin 2's connection end.
        let end = at([5.0, 0.0]);
        for events in [
            vec![egui::Event::PointerMoved(end)],
            vec![button(end, true)],
            vec![button(end, false)],
        ] {
            canvas_frame(&mut editor, &engine, &ctx, events);
        }
        assert_eq!(editor.selected, Some(TerminalId(2)));
        // Drag pin 1's end to (−6.6, 1.4) mm: it lands on (−7.5, 2.5).
        let (from, to) = (at([-5.0, 0.0]), at([-6.6, 1.4]));
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(from + (to - from) * 0.5)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
        ] {
            canvas_frame(&mut editor, &engine, &ctx, events);
        }
        assert_eq!(editor.selected, Some(TerminalId(1)));
        let set = editor.draft.as_ref().ok_or("no draft")?;
        let pin = set.symbol.units[0]
            .pin(TerminalId(1))
            .ok_or("pin 1 missing")?;
        assert_eq!(pin.position, [-7.5, 2.5]);
        Ok(())
    }

    /// Click the text `label`, type `typed`, then leave the field.
    fn type_into(
        editor: &mut PartEditor,
        engine: &Engine,
        ctx: &egui::Context,
        label: &str,
        typed: &str,
    ) -> Result<Vec<(String, Pos2)>, String> {
        let shown = frame(editor, engine, ctx, vec![]);
        let point = shown
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, p)| *p)
            .ok_or_else(|| format!("missing {label}"))?;
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
        frame(editor, engine, ctx, vec![press(false)]);
        let typed = frame(editor, engine, ctx, vec![egui::Event::Text(typed.into())]);
        let key = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(editor, engine, ctx, vec![key]);
        Ok(typed)
    }

    #[test]
    fn typing_a_default_in_the_fields_section_commits_it() -> Result<(), Box<dyn std::error::Error>>
    {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let lineage = resistance(&editor).ok_or("no field")?;
        type_into(&mut editor, &engine, &ctx, "No default", "10k")?;
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        engine.apply(engine.revision(), operation)?;
        editor.commit(&ctx, &engine);
        let default = editor
            .draft
            .as_ref()
            .and_then(|s| s.component.field(lineage))
            .and_then(|f| f.default.clone());
        assert!(matches!(
            default,
            Some(FieldValue::Quantity { si, .. }) if (si - 10_000.0).abs() < 1e-9
        ));
        Ok(())
    }

    /// The Designator row edits the component's prefix like the Name row:
    /// letters are uppercased as typed, the edit commits through
    /// `library.edit`, and a prefix with a digit is a publish finding.
    #[test]
    fn the_designator_row_edits_the_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let mut engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let prefix = |editor: &PartEditor| {
            editor
                .draft
                .as_ref()
                .map(|s| s.component.designator_prefix.clone())
        };
        assert_eq!(prefix(&editor).as_deref(), Some("R"));
        type_into(&mut editor, &engine, &ctx, "R", "n")?;
        let typed = prefix(&editor).ok_or("no draft")?;
        assert!(typed == "RN" || typed == "NR", "{typed}");
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        engine.apply(engine.revision(), operation)?;
        editor.commit(&ctx, &engine);
        let id = editor.draft.as_ref().ok_or("no draft")?.ids.component;
        let stored = match engine.drafts().get(id).map(|d| &d.definition) {
            Some(pcbcraft_core::library::Definition::Component(c)) => c.designator_prefix.clone(),
            _ => return Err("no component draft".into()),
        };
        assert_eq!(stored, typed);
        let finding = "A component needs a designator prefix of 1–8 uppercase letters A–Z, such as R, C or U.";
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(!shown.iter().any(|(text, _)| text == finding));
        type_into(&mut editor, &engine, &ctx, &typed, "1")?;
        let operation = editor.commit(&ctx, &engine).ok_or("no edit")?;
        engine.apply(engine.revision(), operation)?;
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            shown.iter().any(|(text, _)| text == finding),
            "{:?}",
            shown.iter().map(|(t, _)| t).collect::<Vec<_>>()
        );
        Ok(())
    }

    #[test]
    fn invalid_typed_default_shows_an_error_and_sends_nothing()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.commit(&ctx, &engine);
        let original = editor.draft.clone();
        type_into(&mut editor, &engine, &ctx, "No default", "ten volts")?;
        let shown = frame(&mut editor, &engine, &ctx, vec![]);
        assert!(
            shown.iter().any(|(text, _)| text == "ten volts"),
            "text kept"
        );
        assert_eq!(editor.draft, original);
        assert!(editor.commit(&ctx, &engine).is_none());
        Ok(())
    }
}
