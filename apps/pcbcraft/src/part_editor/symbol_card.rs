//! The Symbol tab's Properties card: tool, selection, unit and grid
//! sections, the terminal list and the selected terminal with its
//! functions.
use super::{PartEditor, sentence};
use crate::kit;
use egui::Ui;
use pcbcraft_core::functions::{
    Direction, Drive, Function, MAX_FUNCTIONS, SignalKind, TerminalFunctions,
};
use pcbcraft_core::library::{ComponentDefinition, Terminal, TerminalClass};
use pcbcraft_core::{Engine, FunctionId, Operation, TerminalId};
use solidcraft_ui_egui::ui_kit::{self, Button};

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

impl PartEditor {
    pub(super) fn symbol_card(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
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

    /// One compact selectable row per terminal, the selected one lit.
    fn terminal_list(&mut self, ui: &mut Ui) {
        if let Some(clicked) = self.terminal_rows(ui, self.selected) {
            self.selected = Some(clicked);
        }
    }

    /// The terminal rows with `lit` highlighted; the clicked terminal.
    pub(super) fn terminal_rows(&self, ui: &mut Ui, lit: Option<TerminalId>) -> Option<TerminalId> {
        let set = self.draft.as_ref()?;
        let terminals = &set.component.terminals;
        let mut clicked = None;
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
                        selected: lit == Some(terminal.id),
                        tip: "Select this terminal",
                        ..Default::default()
                    };
                    if kit::tree_row(ui, ui.id().with(("terminal", terminal.id.0)), &row).clicked {
                        clicked = Some(terminal.id);
                    }
                }
            },
        );
        clicked
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
pub(super) fn add_function(
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

/// Take the next local ID of a component, with checked arithmetic.
fn allocate_local(component: &mut ComponentDefinition) -> Option<u32> {
    let id = component.next_local;
    component.next_local = id.checked_add(1)?;
    Some(id)
}
