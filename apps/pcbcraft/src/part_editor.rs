//! Component authoring content for the shared shell. Form spacing, controls and
//! typography follow Solidcraft ui_kit/layout.rs and fields.rs at the pinned revision.
//!
//! The editor authors a working set of five drafts (component, symbol,
//! package, footprint, binding). Widgets edit a working copy; a finished
//! gesture commits it as one `library.edit`, so every finished edit is one
//! shared undo step. Publishing sends one `library.publish` of the five.
mod working_set;

use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};
use pcbcraft_core::completeness::CheckTarget;
use pcbcraft_core::functions::{Direction, TerminalFunctions};
use pcbcraft_core::library::{
    Anchor, ComponentDefinition, Drafts, PackageTerminal, Pad, Terminal, TerminalClass,
    TerminalMap, Unit,
};
use pcbcraft_core::{
    ComponentRef, DefinitionId, Engine, FunctionId, Id, LibraryCommand, Operation,
    PackageTerminalId, PadId, Point, Query, QueryResult, Stage, TerminalId, UnitId,
};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{self, Button},
};
use std::sync::Arc;
pub use working_set::{Template, WorkingSet};

/// The terminal choices this editor offers: one fixed function of a
/// direction, or device NC. Multi-function terminals are authored through
/// `library.edit`; the pin-configuration table is later work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PinChoice {
    Fixed(Direction),
    DeviceNc,
}

const PIN_CHOICES: [PinChoice; 6] = [
    PinChoice::Fixed(Direction::Passive),
    PinChoice::Fixed(Direction::Input),
    PinChoice::Fixed(Direction::Output),
    PinChoice::Fixed(Direction::PowerInput),
    PinChoice::Fixed(Direction::PowerOutput),
    PinChoice::DeviceNc,
];

impl PinChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Fixed(direction) => direction.label(),
            Self::DeviceNc => "device NC",
        }
    }
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

/// Give a terminal one fixed function of `choice`, keeping the ID of its
/// existing fixed function or allocating one from the component.
fn set_pin_choice(component: &mut ComponentDefinition, id: TerminalId, choice: PinChoice) -> bool {
    let keep = component.terminal(id).and_then(|t| match t.functions() {
        Some(f) if f.functions.len() == 1 => f.functions.first().map(|f| f.id),
        _ => None,
    });
    let class = match choice {
        PinChoice::DeviceNc => TerminalClass::DeviceNc,
        PinChoice::Fixed(direction) => {
            let function = match keep {
                Some(function) => function,
                None => {
                    let next = component.next_local;
                    let Some(after) = next.checked_add(1) else {
                        return false;
                    };
                    component.next_local = after;
                    FunctionId(next)
                }
            };
            TerminalClass::Electrical(TerminalFunctions::fixed(function, direction))
        }
    };
    match component.terminals.iter_mut().find(|t| t.id == id) {
        Some(terminal) => {
            terminal.class = class;
            true
        }
        None => false,
    }
}

fn current_choice(terminal: &Terminal) -> Option<PinChoice> {
    match &terminal.class {
        TerminalClass::Electrical(functions) => match functions.functions.as_slice() {
            [only] => PIN_CHOICES
                .into_iter()
                .find(|c| *c == PinChoice::Fixed(only.direction)),
            _ => None,
        },
        TerminalClass::DeviceNc => Some(PinChoice::DeviceNc),
        TerminalClass::Reserved => None,
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
    drag_scale: Option<f32>,
    findings: Option<(u64, Vec<String>)>,
    update: crate::update_preview::UpdatePreview,
    update_instances: std::collections::BTreeSet<Id>,
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
            drag_scale: None,
            findings: None,
            update: Default::default(),
            update_instances: Default::default(),
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
        self.lineage = set.as_ref().map(|s| s.ids.component);
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
        self.drag_scale = None;
    }

    /// The operation committing a finished edit, if any. Call once per frame
    /// after the editor's widgets ran.
    pub fn commit(&mut self, ctx: &egui::Context, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        let interacting =
            ctx.memory(|m| m.focused().is_some()) || ctx.input(|i| i.pointer.any_down());
        if interacting || self.draft == self.baseline {
            return None;
        }
        self.send(engine)
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

    pub fn tools(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        self.sync(engine);
        if self.publish_next && !self.awaiting {
            self.publish_next = false;
            if self.draft == self.baseline
                && let Some(set) = &self.draft
            {
                self.publishing = true;
                self.awaiting = true;
                return Some(set.publish());
            }
        }
        let unpublished = self.unpublished(engine);
        if self.update.is_open() {
            ui.label("Review the update in properties");
            return None;
        }
        let mut command = None;
        ui.horizontal(|ui| {
            for (icon, label, view) in [
                ("sketch", "Symbol", View::Symbol),
                ("component", "Footprint", View::Footprint),
                ("params", "Mapping", View::Mapping),
                ("box", "3D", View::Model),
            ] {
                if crate::shell::tool(ui, icon, label, true, self.view == view).clicked() {
                    self.view = view;
                }
            }
            ui.separator();
            for (label, template) in [
                ("Passive", Template::TwoTerminal),
                ("Connector", Template::Header),
            ] {
                if crate::shell::tool(ui, "component", label, true, false).clicked() {
                    self.load_template(template);
                }
            }
            if crate::shell::tool(ui, "point", "Terminal", self.draft.is_some(), false).clicked() {
                self.add_terminal();
            }
            if crate::shell::tool(ui, "save", "Publish", unpublished, false).clicked() {
                command = self.publish(engine);
            }
        });
        command
    }

    fn publish(&mut self, engine: &Engine) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let committed = engine.drafts();
        let pending_refs = set.retargeted(committed) != *set;
        if self.draft != self.baseline || pending_refs {
            self.publish_next = true;
            return self.send(engine);
        }
        self.publishing = true;
        self.awaiting = true;
        Some(set.publish())
    }

    /// Replace the working copy with a template; the next commit makes it
    /// one undoable step. Without a working set the app creates the lineages.
    fn load_template(&mut self, template: Template) {
        match &mut self.draft {
            Some(set) => {
                set.load_template(template);
                self.selected = set.component.terminals.first().map(|t| t.id);
            }
            None => self.request = Some(Request::Create(template)),
        }
        self.notice.clear();
        self.drag_scale = None;
    }

    fn add_terminal(&mut self) {
        let Some(set) = &mut self.draft else {
            return;
        };
        if set.component.terminals.len() >= pcbcraft_core::library::MAX_TERMINALS {
            self.notice = format!(
                "Maximum {} terminals in this edition",
                pcbcraft_core::library::MAX_TERMINALS
            );
            return;
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
            return;
        };
        if set.symbol.units.is_empty() {
            let Some(unit) = allocate(&mut set.symbol.next_local) else {
                self.notice = "Unit identity exhausted".into();
                return;
            };
            set.symbol.units.push(Unit {
                id: UnitId(unit),
                label: "A".into(),
                body_size: [6.0, 4.0],
                anchors: vec![],
            });
        }
        let row = set.component.terminals.len() as f64;
        let electrical = TerminalId(electrical);
        let physical = PackageTerminalId(physical);
        set.component.terminals.push(Terminal {
            id: electrical,
            name: electrical.to_string(),
            class: TerminalClass::Electrical(TerminalFunctions::fixed(
                FunctionId(function),
                Direction::Passive,
            )),
        });
        if let Some(unit) = set.symbol.units.first_mut() {
            unit.anchors.push(Anchor {
                terminal: electrical,
                position: [-5.0, -row * 2.5],
            });
        }
        set.package.terminals.push(PackageTerminal {
            id: physical,
            designation: physical.to_string(),
        });
        set.footprint.pads.push(Pad {
            id: PadId(pad),
            terminal: physical,
            position: [0.0, row * 2.0],
            size: [1.0, 1.4],
        });
        set.binding.mappings.push(TerminalMap {
            package_terminal: physical,
            terminal: electrical,
        });
        self.selected = Some(electrical);
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
            unit.anchors.retain(|a| a.terminal != selected);
        }
        set.binding.mappings.retain(|m| m.terminal != selected);
        set.package.terminals.retain(|t| !physical.contains(&t.id));
        set.footprint
            .pads
            .retain(|p| !physical.contains(&p.terminal));
        self.selected = set.component.terminals.first().map(|t| t.id);
    }

    fn add_pad(&mut self) {
        let Some(set) = &mut self.draft else {
            return;
        };
        let Some(terminal) = set
            .binding
            .mappings
            .iter()
            .find(|m| Some(m.terminal) == self.selected)
            .map(|m| m.package_terminal)
        else {
            return;
        };
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
        set.footprint.pads.push(Pad {
            id: PadId(id),
            terminal,
            position: [0.0, 0.0],
            size: [1.0, 1.4],
        });
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
            let ids = set.ids.all();
            messages.extend(
                reports
                    .into_iter()
                    .filter(|r| matches!(r.target, CheckTarget::Draft(id) if ids.contains(&id)))
                    .flat_map(|r| r.findings)
                    .filter(|f| f.blocks_stage(Stage::Publish))
                    .map(|f| f.message),
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
        let t = Tokens::get();
        ui.spacing_mut().item_spacing = egui::vec2(t.space_4, t.form_gap_y);
        ui.spacing_mut().interact_size.y = t.control_height;
        ui.label(egui::RichText::new("Component definition").font(fonts::semibold(t.font_panel)));
        let unpublished = self.unpublished(engine);
        let findings = self.findings(engine);
        let Some(set) = &mut self.draft else {
            ui.label("Start a component from the Passive or Connector template.");
            self.snapshots(ui, engine);
            return None;
        };
        text_row(ui, "Name", &mut set.metas.component.name);
        let published = set.published(engine);
        ui.weak(match published {
            None => "New project-local draft".into(),
            Some(r) => format!(
                "Definition {} · revision {}{}",
                r.id.0,
                r.revision.0,
                if unpublished {
                    " · unpublished edits"
                } else {
                    ""
                }
            ),
        });
        ui.label(
            "Publishing creates immutable revisions. Placed components keep their existing revision.",
        );
        if !self.notice.is_empty() {
            ui.colored_label(t.form_primary, &self.notice);
        }
        for message in findings.iter().take(6) {
            ui.colored_label(t.form_error, message);
        }
        ui.separator();
        match self.view {
            View::Symbol => self.symbol_inspector(ui),
            View::Footprint => self.footprint_inspector(ui),
            View::Mapping => self.mapping_inspector(ui),
            View::Model => {
                if let Some(set) = &mut self.draft {
                    ui.label("Parametric package envelope");
                    ui.weak("Approximate body; not a verified supplier model.");
                    for (index, label) in ["Length", "Width", "Height"].into_iter().enumerate() {
                        number_row(ui, label, &mut set.package.body_size[index], 0.01..=10000.0);
                    }
                    for (index, label) in
                        ["Offset X", "Offset Y", "Offset Z"].into_iter().enumerate()
                    {
                        number_row(
                            ui,
                            label,
                            &mut set.package.model_offset[index],
                            -10000.0..=10000.0,
                        );
                    }
                    ui.weak("The live model and native export use Solidcraft.");
                }
            }
        }
        ui.separator();
        self.snapshots(ui, engine);
        if !unpublished && let Some(target) = self.draft.as_ref().and_then(|s| s.published(engine))
        {
            let candidates: Vec<_> = engine
                .project()
                .instances
                .iter()
                .filter(|i| i.component.id == target.id && i.component.revision != target.revision)
                .collect();
            self.update_instances
                .retain(|id| candidates.iter().any(|i| i.id == *id));
            if !candidates.is_empty() {
                ui.separator();
                ui.strong("Update placed instances");
                ui.weak(format!(
                    "Use published revision {} for the selected instances.",
                    target.revision.0
                ));
                for instance in candidates {
                    let mut checked = self.update_instances.contains(&instance.id);
                    if ui
                        .checkbox(
                            &mut checked,
                            format!(
                                "{} · revision {}",
                                instance.designator, instance.component.revision.0
                            ),
                        )
                        .changed()
                    {
                        if checked {
                            self.update_instances.insert(instance.id);
                        } else {
                            self.update_instances.remove(&instance.id);
                        }
                    }
                }
                if ui_kit::button(
                    ui,
                    "Preview instance update",
                    Button::Enabled(!self.update_instances.is_empty()),
                )
                .clicked()
                {
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
        }
        if let Some(set) = &mut self.draft {
            ui.collapsing("Provenance", |ui| {
                text_row(ui, "Source", &mut set.metas.component.provenance);
                ui.weak("Structural validation does not replace a datasheet review.");
            });
        }
        None
    }

    /// The project's published components; choosing one authors its lineage.
    fn snapshots(&mut self, ui: &mut Ui, engine: &Engine) {
        ui.collapsing("Project components", |ui| {
            let library = &engine.project().library;
            for component in library.latest_components() {
                let label = format!("{} · r{}", component.meta.name, component.revision.0);
                let current = self.lineage == Some(component.id);
                if ui_kit::button(
                    ui,
                    label,
                    if current {
                        Button::SmallSelected
                    } else {
                        Button::Ghost
                    },
                )
                .on_hover_text("Author this component's drafts")
                .clicked()
                {
                    let reference = ComponentRef::new(component.id, component.revision);
                    if engine.drafts().get(component.id).is_none() {
                        self.request = Some(Request::Draft(reference));
                    }
                    self.lineage = Some(component.id);
                    self.source = None;
                }
            }
            if library.components.is_empty() {
                ui.weak("Publish a draft to make it available in the schematic.");
            }
        });
    }

    fn terminal_choices(&mut self, ui: &mut Ui) {
        let Some(set) = &self.draft else {
            return;
        };
        for terminal in &set.component.terminals {
            if ui_kit::button(
                ui,
                format!("{} · {}", terminal.name, pin_summary(terminal)),
                if self.selected == Some(terminal.id) {
                    Button::SmallSelected
                } else {
                    Button::Ghost
                },
            )
            .clicked()
            {
                self.selected = Some(terminal.id);
            }
        }
    }

    fn symbol_inspector(&mut self, ui: &mut Ui) {
        if let Some(unit) = self.draft.as_mut().and_then(|s| s.symbol.units.first_mut()) {
            pair_row(ui, "Body size", &mut unit.body_size, 0.01..=10000.0);
        }
        self.terminal_choices(ui);
        let mut remove = false;
        if let (Some(id), Some(set)) = (self.selected, &mut self.draft) {
            if let Some(terminal) = set.component.terminals.iter_mut().find(|t| t.id == id) {
                ui.separator();
                text_row(ui, "Pin name", &mut terminal.name);
                let current = current_choice(terminal);
                let mut chosen = current;
                egui::ComboBox::from_id_salt(("pin-kind", id.0))
                    .selected_text(pin_summary(terminal))
                    .show_ui(ui, |ui| {
                        for choice in PIN_CHOICES {
                            ui.selectable_value(&mut chosen, Some(choice), choice.label());
                        }
                    });
                if chosen != current
                    && let Some(choice) = chosen
                    && !set_pin_choice(&mut set.component, id, choice)
                {
                    self.notice = "Function identity exhausted".into();
                }
            }
            if let Some(anchor) = set
                .symbol
                .units
                .iter_mut()
                .flat_map(|u| u.anchors.iter_mut())
                .find(|a| a.terminal == id)
            {
                pair_row(ui, "Pin anchor", &mut anchor.position, -10000.0..=10000.0);
            }
            remove = ui_kit::button(ui, "Remove terminal", Button::Small).clicked();
        }
        if remove {
            self.remove_selected();
        }
        ui.weak("Drag a pin anchor on the canvas; publish when the draft is ready.");
    }

    fn footprint_inspector(&mut self, ui: &mut Ui) {
        if let Some(set) = &mut self.draft {
            pair_row(
                ui,
                "Package datum",
                &mut set.footprint.datum_offset,
                -10000.0..=10000.0,
            );
        }
        self.terminal_choices(ui);
        let mut add = false;
        if let Some(set) = &mut self.draft {
            let physical: Vec<_> = set
                .binding
                .mappings
                .iter()
                .filter(|m| Some(m.terminal) == self.selected)
                .map(|m| m.package_terminal)
                .collect();
            for pad in set
                .footprint
                .pads
                .iter_mut()
                .filter(|p| physical.contains(&p.terminal))
            {
                ui.separator();
                ui.label(format!(
                    "Pad {} · package terminal {}",
                    pad.id, pad.terminal
                ));
                egui::ComboBox::from_id_salt(("pad-terminal", pad.id.0))
                    .selected_text(format!("Package terminal {}", pad.terminal))
                    .show_ui(ui, |ui| {
                        for terminal in &set.package.terminals {
                            ui.selectable_value(
                                &mut pad.terminal,
                                terminal.id,
                                &terminal.designation,
                            )
                            .changed();
                        }
                    });
                pair_row(ui, "Position", &mut pad.position, -10000.0..=10000.0);
                pair_row(ui, "Land size", &mut pad.size, 0.01..=10000.0);
            }
            add = ui_kit::button(ui, "Add pad to selected terminal", Button::Small).clicked();
        }
        if add {
            self.add_pad();
        }
        ui.weak("Pad size is independent of the package body. Pads remain individual routing obligations.");
    }

    fn mapping_inspector(&mut self, ui: &mut Ui) {
        let Some(set) = &mut self.draft else {
            return;
        };
        ui.label("Electrical → package terminal");
        ui.weak("Mappings are explicit; matching names do not establish compatibility.");
        for physical in &mut set.package.terminals {
            ui.separator();
            text_row(
                ui,
                &format!("Terminal {}", physical.id),
                &mut physical.designation,
            );
            if let Some(mapping) = set
                .binding
                .mappings
                .iter_mut()
                .find(|m| m.package_terminal == physical.id)
            {
                let selected = set
                    .component
                    .terminal(mapping.terminal)
                    .map(|t| t.name.as_str())
                    .unwrap_or("Unmapped");
                egui::ComboBox::from_id_salt(("mapping", physical.id.0))
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for terminal in &set.component.terminals {
                            ui.selectable_value(&mut mapping.terminal, terminal.id, &terminal.name)
                                .changed();
                        }
                    });
            }
        }
    }

    pub fn canvas(&mut self, ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        if self.update.is_open() {
            ui.disable();
        }
        if self.view == View::Model {
            ui.label("Native Solidcraft package preview");
            return None;
        }
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return None;
        }
        if self.view == View::Mapping {
            self.mapping_canvas(ui, rect);
            return None;
        }
        let Some(set) = &mut self.draft else {
            ui.put(rect, egui::Label::new("No component draft"));
            return None;
        };
        let t = Tokens::get();
        let painter = ui.painter().with_clip_rect(rect);
        let empty_unit = Unit {
            id: UnitId(0),
            label: String::new(),
            body_size: [1.0, 1.0],
            anchors: vec![],
        };
        let body_size = set.symbol.units.first().unwrap_or(&empty_unit).body_size;
        let (extent_x, extent_y) = if self.view == View::Symbol {
            set.symbol
                .units
                .first()
                .unwrap_or(&empty_unit)
                .anchors
                .iter()
                .fold((body_size[0] / 2.0, body_size[1] / 2.0), |(x, y), a| {
                    (x.max(a.position[0].abs()), y.max(a.position[1].abs()))
                })
        } else {
            set.footprint.pads.iter().fold(
                (
                    set.package.body_size[0] / 2.0,
                    set.package.body_size[1] / 2.0,
                ),
                |(x, y), p| {
                    (
                        x.max(p.position[0].abs() + p.size[0] / 2.0),
                        y.max(p.position[1].abs() + p.size[1] / 2.0),
                    )
                },
            )
        };
        if !ui.input(|input| input.pointer.any_down()) {
            self.drag_scale = None;
        }
        let fitted_scale = (rect.width() as f64 / (2.0 * extent_x + 6.0))
            .min(rect.height() as f64 / (2.0 * extent_y + 6.0))
            .clamp(0.001, 80.0) as f32;
        let scale = self.drag_scale.unwrap_or(fitted_scale);
        let transform =
            |p: Point| rect.center() + egui::vec2(p[0] as f32 * scale, -p[1] as f32 * scale);
        let origin = rect.center();
        painter.line_segment(
            [
                origin - egui::vec2(10.0, 0.0),
                origin + egui::vec2(10.0, 0.0),
            ],
            Stroke::new(1.0, t.text_dim),
        );
        painter.line_segment(
            [
                origin - egui::vec2(0.0, 10.0),
                origin + egui::vec2(0.0, 10.0),
            ],
            Stroke::new(1.0, t.text_dim),
        );
        let title = if self.view == View::Symbol {
            "Symbol · electrical terminals"
        } else {
            "Footprint · copper lands"
        };
        painter.text(
            rect.left_top() + egui::vec2(t.space_16, t.space_16),
            Align2::LEFT_TOP,
            title,
            fonts::semibold(t.font_panel),
            t.text,
        );
        let mut selected = self.selected;
        let mut drag_scale = self.drag_scale;
        if self.view == View::Symbol {
            let body = Rect::from_center_size(
                origin,
                egui::vec2(body_size[0] as f32, body_size[1] as f32) * scale,
            );
            painter.rect(
                body,
                t.radius_sm,
                t.panel,
                Stroke::new(1.5, t.accent),
                StrokeKind::Inside,
            );
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                &set.metas.component.name,
                FontId::proportional(t.font_label),
                t.text,
            );
            let names: Vec<(TerminalId, String)> = set
                .component
                .terminals
                .iter()
                .map(|t| (t.id, t.name.clone()))
                .collect();
            if let Some(unit) = set.symbol.units.first_mut() {
                for anchor in &mut unit.anchors {
                    let at = transform(anchor.position);
                    let edge = body.clamp(at);
                    let ink = if selected == Some(anchor.terminal) {
                        t.body_selected
                    } else {
                        t.accent
                    };
                    painter.line_segment([at, edge], Stroke::new(2.0, ink));
                    painter.circle_filled(at, 4.0, ink);
                    let label = names
                        .iter()
                        .find(|(id, _)| *id == anchor.terminal)
                        .map(|(_, name)| name.as_str())
                        .unwrap_or("?");
                    painter.text(
                        at + egui::vec2(0.0, -t.space_10),
                        Align2::CENTER_BOTTOM,
                        label,
                        FontId::proportional(t.font_label),
                        t.text,
                    );
                    let response = ui.interact(
                        Rect::from_center_size(at, Vec2::splat(18.0)),
                        ui.id().with(("symbol-pin", anchor.terminal.0)),
                        Sense::click_and_drag(),
                    );
                    if response.clicked() || response.drag_started() {
                        selected = Some(anchor.terminal);
                    }
                    if response.drag_started() {
                        drag_scale = Some(scale);
                    }
                    if response.dragged()
                        && let Some(p) = response.interact_pointer_pos()
                    {
                        anchor.position = [
                            f64::from((p.x - origin.x) / scale),
                            f64::from((origin.y - p.y) / scale),
                        ];
                    }
                }
            }
        } else {
            let body_origin = transform(set.footprint.datum_offset);
            let body = Rect::from_center_size(
                body_origin,
                egui::vec2(
                    set.package.body_size[0] as f32,
                    set.package.body_size[1] as f32,
                ) * scale,
            );
            painter.rect_stroke(
                body,
                t.radius_sm,
                Stroke::new(1.0, t.text_dim),
                StrokeKind::Inside,
            );
            for pad in &mut set.footprint.pads {
                let electrical = set.binding.terminal_of(pad.terminal);
                let at = transform(pad.position);
                let size = egui::vec2(pad.size[0] as f32, pad.size[1] as f32) * scale;
                let land = Rect::from_center_size(at, size);
                let ink = if electrical.is_some() && electrical == selected {
                    t.body_selected
                } else {
                    t.accent
                };
                painter.rect_filled(land, 0, ink);
                let number = set
                    .package
                    .terminal(pad.terminal)
                    .map(|p| p.designation.as_str())
                    .unwrap_or("?");
                painter.text(
                    land.center(),
                    Align2::CENTER_CENTER,
                    number,
                    fonts::semibold(t.font_label),
                    t.on_accent,
                );
                let response = ui.interact(
                    land.expand(3.0),
                    ui.id().with(("footprint-pad", pad.id.0)),
                    Sense::click_and_drag(),
                );
                if response.clicked() || response.drag_started() {
                    selected = electrical;
                }
                if response.drag_started() {
                    drag_scale = Some(scale);
                }
                if response.dragged()
                    && let Some(p) = response.interact_pointer_pos()
                {
                    pad.position = [
                        f64::from((p.x - origin.x) / scale),
                        f64::from((origin.y - p.y) / scale),
                    ];
                }
            }
        }
        self.selected = selected;
        self.drag_scale = drag_scale;
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
                .filter(|p| p.terminal == physical.id)
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

fn text_row(ui: &mut Ui, label: &str, value: &mut String) -> bool {
    let t = Tokens::get();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [t.form_label_width, t.control_height],
            egui::Label::new(label),
        );
        changed = ui_kit::cell_editor(ui, ui.id().with(label), value, "", false).changed();
    });
    changed
}
fn number_row(
    ui: &mut Ui,
    label: &str,
    value: &mut f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let t = Tokens::get();
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [t.form_label_width, t.control_height],
            egui::Label::new(label),
        );
        changed = ui
            .add(
                egui::DragValue::new(value)
                    .range(range)
                    .speed(0.05)
                    .suffix(" mm"),
            )
            .changed();
    });
    changed
}
fn pair_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Point,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let t = Tokens::get();
    let mut changed = false;
    ui.label(label);
    ui.horizontal(|ui| {
        for (index, axis) in ["X", "Y"].iter().enumerate() {
            ui.label(*axis);
            changed |= ui
                .add_sized(
                    [t.form_label_width, t.control_height],
                    egui::DragValue::new(&mut value[index])
                        .range(range.clone())
                        .speed(0.05),
                )
                .changed();
        }
        ui.weak("mm");
    });
    changed
}

#[cfg(test)]
mod draft_tests {
    use super::*;

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
}
