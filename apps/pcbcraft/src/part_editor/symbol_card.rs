//! The Symbol tab's Properties cards: SYMBOL with nothing selected (unit
//! and terminal list), the PIN card of a selected pin (electrical, then
//! graphic) and the TERMINAL card of a terminal without a pin on the drawn
//! unit (docs/sidebar-ia.md, "Properties card by subject").
use super::{PartEditor, Selected, sentence};
use crate::kit::{self, severity::Level};
use egui::Ui;
use pcbcraft_core::functions::{
    Direction, Drive, Function, MAX_FUNCTIONS, SignalKind, TerminalFunctions,
};
use pcbcraft_core::library::symbol_style::SymbolKind;
use pcbcraft_core::library::{ComponentDefinition, Terminal, TerminalClass};
use pcbcraft_core::{Engine, FunctionId, LibraryCommand, Operation, TerminalId};

/// A terminal's class as the inspector offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClassChoice {
    Electrical,
    DeviceNc,
    Reserved,
}

impl ClassChoice {
    pub(super) fn of(terminal: &Terminal) -> Self {
        match terminal.class {
            TerminalClass::Electrical(_) => Self::Electrical,
            TerminalClass::DeviceNc => Self::DeviceNc,
            TerminalClass::Reserved => Self::Reserved,
        }
    }
}

pub(super) const CLASS_CHOICES: [(ClassChoice, &str); 3] = [
    (ClassChoice::Electrical, "Electrical"),
    (ClassChoice::DeviceNc, "Device NC"),
    (ClassChoice::Reserved, "Reserved"),
];

pub(super) const DIRECTIONS: [Direction; 6] = [
    Direction::Passive,
    Direction::Input,
    Direction::Output,
    Direction::Bidirectional,
    Direction::PowerInput,
    Direction::PowerOutput,
];

pub(super) const SIGNALS: [(SignalKind, &str); 4] = [
    (SignalKind::Unspecified, "Unspecified"),
    (SignalKind::Digital, "Digital"),
    (SignalKind::Analog, "Analog"),
    (SignalKind::Power, "Power"),
];

pub(super) const DRIVES: [(Drive, &str); 4] = [
    (Drive::None, "None"),
    (Drive::PushPull, "Push-pull"),
    (Drive::OpenDrain, "Open drain"),
    (Drive::OpenSource, "Open source"),
];

impl PartEditor {
    /// The Symbol tab with nothing selected: SYMBOL, with the unit being
    /// drawn and the terminal list. A row selects its pin, or opens the
    /// TERMINAL card when the drawn unit has no pin for it.
    pub(super) fn symbol_card(&mut self, ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        self.unit_section(ui);
        let operation = self.restyle_section(ui);
        if let Some(clicked) = self.terminal_rows(ui, None) {
            self.show_terminal(clicked);
        }
        operation
    }

    /// Redraw the symbol as another kind: one `library.restyle_symbol`. Only
    /// kinds drawn for the component's terminal count are offered.
    fn restyle_section(&self, ui: &mut Ui) -> Option<Operation> {
        let set = self.draft.as_ref()?;
        let count = set.component.terminals.len();
        let kinds = SymbolKind::ALL
            .into_iter()
            .filter(|k| k.terminal_count().is_none_or(|n| n == count));
        let mut chosen = None;
        kit::row(ui, "Redraw as", |ui| {
            kit::dropdown_menu(ui, "symbol-restyle", "Choose a kind", |ui| {
                for kind in kinds {
                    if ui.selectable_label(false, kind.label()).clicked() {
                        chosen = Some(kind);
                    }
                }
            })
            .on_hover_text("Replaces the drawing; terminals and mappings stay")
        });
        chosen.map(|kind| {
            Operation::Library(LibraryCommand::RestyleSymbol {
                draft: set.ids.symbol,
                kind,
                terminals: None,
            })
        })
    }

    /// Select `terminal`'s pin on the drawn unit, or show its TERMINAL card
    /// when the unit has none.
    pub(super) fn show_terminal(&mut self, terminal: TerminalId) {
        self.selected = Some(terminal);
        let on_unit = self
            .unit_index()
            .and_then(|i| self.draft.as_ref()?.symbol.units.get(i))
            .is_some_and(|u| u.pin(terminal).is_some());
        if on_unit {
            self.terminal_card = None;
            self.select([Selected::Pin(terminal)].into_iter().collect());
            self.frame_selection();
        } else {
            self.selection.clear();
            self.terminal_card = Some(terminal);
        }
    }

    /// A selected pin: its terminal's electrical properties, then the pin's
    /// drawing (docs/sidebar-ia.md wireframe "Pin selected").
    pub(super) fn pin_card(&mut self, ui: &mut Ui, terminal: TerminalId) {
        let refused = self.electrical_section(ui, terminal, "pin-electrical", "Pin · Electrical");
        if let Some(message) = refused {
            self.tell(Level::Error, message);
        }
        self.pin_graphic_section(ui, terminal);
    }

    /// A terminal the drawn unit has no pin for: TERMINAL, and where its
    /// pin comes from.
    pub(super) fn terminal_without_pin_card(&mut self, ui: &mut Ui, terminal: TerminalId) {
        let refused = self.electrical_section(ui, terminal, "terminal-electrical", "Terminal");
        if let Some(message) = refused {
            self.tell(Level::Error, message);
        }
        let elsewhere = self.draft.as_ref().and_then(|set| {
            set.symbol
                .units
                .iter()
                .find(|u| u.pin(terminal).is_some())
                .map(|u| u.label.clone())
        });
        match elsewhere {
            Some(unit) => kit::hint(
                ui,
                &format!("Its pin is on unit {unit}"),
                "Choose that unit under Symbol to edit the pin",
            ),
            None => kit::hint(
                ui,
                "No pin yet: place one with the Pin tool (N)",
                "The Pin tool places the next terminal without a pin",
            ),
        };
    }

    /// Name, class and functions of `terminal`: the electrical half of the
    /// PIN card and the whole TERMINAL card. Returns a refused change's
    /// message.
    fn electrical_section(
        &mut self,
        ui: &mut Ui,
        terminal: TerminalId,
        id: &str,
        title: &str,
    ) -> Option<&'static str> {
        let set = self.draft.as_mut()?;
        let component = &mut set.component;
        let mut refused = None;
        let mut hand_off = false;
        kit::section(ui, id, title, true, None, |ui| {
            if let Some(t) = component.terminals.iter_mut().find(|t| t.id == terminal) {
                kit::text_row(ui, "Name", &mut t.name)
                    .on_hover_text("The terminal's name, shown at the pin");
            }
            if let Some(mut class) = component.terminal(terminal).map(ClassChoice::of) {
                let options: Vec<_> = CLASS_CHOICES
                    .iter()
                    .map(|(c, label)| (*c, (*label).to_owned()))
                    .collect();
                if kit::choice_row(ui, "Class", &mut class, &options)
                    && let Err(message) = set_class(component, terminal, class)
                {
                    refused = Some(message);
                }
            }
            hand_off = function_summary(ui, component, terminal);
        });
        if hand_off {
            self.on_tab("pins");
            self.pins.show_terminal(terminal);
        }
        refused
    }

    /// One compact selectable row per terminal, the `lit` one highlighted,
    /// bounded at [`kit::LIST_MAX_ROWS`] with "Show all"; the clicked
    /// terminal.
    pub(super) fn terminal_rows(&self, ui: &mut Ui, lit: Option<TerminalId>) -> Option<TerminalId> {
        let set = self.draft.as_ref()?;
        let terminals = &set.component.terminals;
        let unit = self.unit_index().and_then(|i| set.symbol.units.get(i));
        let mut clicked = None;
        kit::section(
            ui,
            "terminals",
            "Terminals",
            true,
            Some(terminals.len()),
            |ui| {
                let all_id = ui.scope_id().with("show-all");
                let all = ui.data(|d| d.get_temp::<bool>(all_id)).unwrap_or(false);
                let max = if all {
                    terminals.len()
                } else {
                    kit::LIST_MAX_ROWS
                };
                let shown = kit::list_frame(ui, terminals.len(), max, |ui, index| {
                    let Some(terminal) = terminals.get(index) else {
                        return;
                    };
                    let placed = unit.is_some_and(|u| u.pin(terminal.id).is_some());
                    let summary = if placed {
                        pin_summary(terminal)
                    } else {
                        format!("{} · no pin here", pin_summary(terminal))
                    };
                    let row = kit::Row {
                        icon: "point",
                        label: &terminal.name,
                        secondary: &summary,
                        selected: lit == Some(terminal.id),
                        tip: if placed {
                            "Select its pin"
                        } else {
                            "Show the terminal"
                        },
                        ..Default::default()
                    };
                    if kit::tree_row(ui, ui.scope_id().with(("terminal", terminal.id.0)), &row)
                        .clicked
                    {
                        clicked = Some(terminal.id);
                    }
                });
                if shown.show_all {
                    ui.data_mut(|d| d.insert_temp(all_id, true));
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
pub(super) fn set_class(
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
        mux: None,
        mux_table: None,
        source: None,
    });
    Ok(function)
}

/// The functions of an electrical terminal, inside its section: a single
/// function's direction, signal and drive inline, then the function count
/// with "Edit in Pins tab", where chips, peripherals and rules are edited
/// (slice C5's terminal card hand-off). Returns whether it was clicked.
fn function_summary(ui: &mut Ui, component: &mut ComponentDefinition, id: TerminalId) -> bool {
    let Some(TerminalClass::Electrical(table)) = component
        .terminals
        .iter_mut()
        .find(|t| t.id == id)
        .map(|t| &mut t.class)
    else {
        return false;
    };
    if let [only] = table.functions.as_mut_slice() {
        let directions: Vec<_> = DIRECTIONS
            .iter()
            .map(|d| (*d, sentence(d.label())))
            .collect();
        let signals: Vec<_> = SIGNALS.iter().map(|(s, l)| (*s, (*l).to_owned())).collect();
        let drives: Vec<_> = DRIVES.iter().map(|(d, l)| (*d, (*l).to_owned())).collect();
        kit::choice_row(ui, "Direction", &mut only.direction, &directions);
        kit::choice_row(ui, "Signal", &mut only.signal, &signals);
        kit::choice_row(ui, "Drive", &mut only.drive, &drives);
    }
    let summary = match table.functions.as_slice() {
        [only] => format!("1 function · {}", only.label),
        many => format!("{} functions", many.len()),
    };
    kit::summary_row(ui, &summary, Some("Edit in Pins tab"))
}

/// Take the next local ID of a component, with checked arithmetic.
pub(super) fn allocate_local(component: &mut ComponentDefinition) -> Option<u32> {
    let id = component.next_local;
    component.next_local = id.checked_add(1)?;
    Some(id)
}
