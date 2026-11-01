//! Slice C5: the Pins tab (between Symbol and Footprint) with its Table,
//! Peripherals and Rules sub-views, and its Properties card for the
//! selected terminal, function, peripheral or rule. Owned by C5, with
//! `pins_table.rs` and `rules_editor.rs` (docs/component-model.md, pin functions as built).
//!
//! Every edit changes the working copy and travels through the editor's
//! one commit (`library.edit`, one undo step per finished edit), exactly as
//! the Symbol tab's cards do. Paste table… plans with
//! `library.pinout_preview` and applies with `library.pinout_apply`, the
//! commands agents use. Confirming an imported row removes it from the
//! component's import review (founder decision F8).
use super::pins_table::{self, Filter, Lit, PasteFlow, PinsTable, TableEvent};
use super::rules_editor::{self, RulesEditor, RulesEvent};
use super::symbol_card::{
    CLASS_CHOICES, ClassChoice, DIRECTIONS, DRIVES, SIGNALS, add_function, allocate_local,
    set_class,
};
use super::{PartEditor, sentence};
use crate::kit::{self, severity::Level};
use crate::refusal::{self, Origin};
use egui::{Rect, Ui, UiBuilder, vec2};
use pcbcraft_core::functions::rules::{MAX_PIN_RULES, PinRule, PinRuleScript};
use pcbcraft_core::functions::{
    GpioRef, MAX_KIND, MAX_RESOURCES, MAX_ROLES, MuxCode, PinOption, Pull, Resource, ResourceRole,
    Role,
    pin_config::{MAX_GPIO_PORT, MAX_MUX_CODE, MAX_OPTION_VALUES},
};
use pcbcraft_core::ids::MAX_LABEL;
use pcbcraft_core::library::ComponentDefinition;
use pcbcraft_core::library::pinout::{PinoutAccept, PinoutApply, PinoutTarget};
use pcbcraft_core::{
    Engine, FunctionId, LibraryCommand, Operation, Query, QueryResult, ResourceId, RoleId, RuleId,
    TerminalId,
};
use solidcraft_ui_egui::theme::Tokens;
use solidcraft_ui_egui::ui_kit::{self, Button};

/// Whether the ribbon offers the Pins tab.
pub const AVAILABLE: bool = true;

/// The tab's three sub-views.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubView {
    #[default]
    Table,
    Peripherals,
    Rules,
}

impl SubView {
    const ALL: [(SubView, &'static str); 3] = [
        (SubView::Table, "Table"),
        (SubView::Peripherals, "Peripherals"),
        (SubView::Rules, "Rules"),
    ];
}

/// What the Pins card edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    Terminal(TerminalId),
    Function(TerminalId, FunctionId),
    Peripheral(ResourceId),
    Rule(RuleId),
}

/// The tab's state.
#[derive(Default)]
pub struct PinsTab {
    pub table: PinsTable,
    pub rules: RulesEditor,
    pub view: SubView,
    pub subject: Option<Subject>,
}

impl PinsTab {
    /// Show `terminal` in the table with its card (the Symbol tab's
    /// terminal card hands off here).
    pub fn show_terminal(&mut self, terminal: TerminalId) {
        self.view = SubView::Table;
        self.subject = Some(Subject::Terminal(terminal));
        self.table.paste = None;
    }

    fn lit(&self) -> Lit {
        match self.subject {
            Some(Subject::Terminal(t)) => Lit {
                terminal: Some(t),
                ..Lit::default()
            },
            Some(Subject::Function(t, f)) => Lit {
                terminal: Some(t),
                function: Some(f),
                ..Lit::default()
            },
            Some(Subject::Peripheral(p)) => Lit {
                peripheral: Some(p),
                ..Lit::default()
            },
            Some(Subject::Rule(_)) | None => Lit::default(),
        }
    }
}

/// Whether `subject` still names an object of `component`.
fn exists(component: &ComponentDefinition, subject: Subject) -> bool {
    match subject {
        Subject::Terminal(t) => component.terminal(t).is_some(),
        Subject::Function(t, f) => component
            .terminal(t)
            .and_then(|t| t.functions())
            .is_some_and(|d| d.function(f).is_some()),
        Subject::Peripheral(p) => component.resources.iter().any(|r| r.id == p),
        Subject::Rule(r) => component.pin_rules.iter().any(|rule| rule.id == r),
    }
}

/// A label `stem N` not used by `taken`.
fn unique_label<'a>(stem: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    (1..)
        .map(|n| format!("{stem} {n}"))
        .find(|label| !taken.clone().any(|t| t.eq_ignore_ascii_case(label)))
        .unwrap_or_default()
}

/// Add a peripheral with one role, both with fresh IDs.
pub fn add_peripheral(component: &mut ComponentDefinition) -> Result<ResourceId, &'static str> {
    if component.resources.len() >= MAX_RESOURCES {
        return Err("A component has at most 256 peripherals");
    }
    let id = ResourceId(allocate_local(component).ok_or("Peripheral identity exhausted")?);
    let role = RoleId(allocate_local(component).ok_or("Role identity exhausted")?);
    let label = unique_label(
        "peripheral",
        component.resources.iter().map(|r| r.label.as_str()),
    );
    component.resources.push(Resource {
        id,
        label,
        kind: None,
        roles: vec![Role {
            id: role,
            label: "role 1".into(),
            capacity: 1,
            required_when_used: false,
            signal: None,
        }],
    });
    Ok(id)
}

/// Add a role to a peripheral with a fresh ID.
pub fn add_role(
    component: &mut ComponentDefinition,
    resource: ResourceId,
) -> Result<(), &'static str> {
    let count = component
        .resources
        .iter()
        .find(|r| r.id == resource)
        .map(|r| r.roles.len())
        .ok_or("No such peripheral")?;
    if count >= MAX_ROLES {
        return Err("A peripheral has at most 16 roles");
    }
    let id = RoleId(allocate_local(component).ok_or("Role identity exhausted")?);
    let resource = component
        .resources
        .iter_mut()
        .find(|r| r.id == resource)
        .ok_or("No such peripheral")?;
    let label = unique_label("role", resource.roles.iter().map(|r| r.label.as_str()));
    resource.roles.push(Role {
        id,
        label,
        capacity: 1,
        required_when_used: false,
        signal: None,
    });
    Ok(())
}

/// Remove a peripheral, or one of its roles, and every function's claim on
/// what was removed, so no function names a role that is gone.
pub fn remove_peripheral(
    component: &mut ComponentDefinition,
    resource: ResourceId,
    role: Option<RoleId>,
) {
    match role {
        None => component.resources.retain(|r| r.id != resource),
        Some(role) => {
            if let Some(r) = component.resources.iter_mut().find(|r| r.id == resource) {
                r.roles.retain(|x| x.id != role);
            }
        }
    }
    for terminal in &mut component.terminals {
        if let pcbcraft_core::library::TerminalClass::Electrical(functions) = &mut terminal.class {
            for function in &mut functions.functions {
                let gone = function.resource_role.is_some_and(|rr| {
                    rr.resource == resource && role.is_none_or(|role| rr.role == role)
                });
                if gone {
                    function.resource_role = None;
                }
            }
        }
    }
}

/// Add a pin rule with a fresh ID, a unique label and the starter script.
pub fn add_rule(component: &mut ComponentDefinition) -> Result<RuleId, &'static str> {
    if component.pin_rules.len() >= MAX_PIN_RULES {
        return Err("A component has at most 1024 pin rules");
    }
    let id = RuleId(allocate_local(component).ok_or("Rule identity exhausted")?);
    let label = unique_label("rule", component.pin_rules.iter().map(|r| r.label.as_str()));
    component.pin_rules.push(PinRule {
        id,
        label,
        script: PinRuleScript {
            source: rules_editor::NEW_RULE_SOURCE.into(),
        },
        source: None,
    });
    Ok(id)
}

/// Confirm an imported row: it leaves the import review (F8). An empty
/// review is removed.
pub fn confirm_row(component: &mut ComponentDefinition, terminal: TerminalId) {
    if let Some(review) = component.review.as_deref_mut() {
        review.unreviewed.remove(&terminal);
        review.uncertain.remove(&terminal);
        if review.is_empty() {
            component.review = None;
        }
    }
}

/// Move function `index` of a terminal one place earlier (`-1`) or later
/// (`1`); IDs are kept, order is presentation only.
pub fn move_function(
    component: &mut ComponentDefinition,
    terminal: TerminalId,
    function: FunctionId,
    later: bool,
) {
    let Some(pcbcraft_core::library::TerminalClass::Electrical(functions)) = component
        .terminals
        .iter_mut()
        .find(|t| t.id == terminal)
        .map(|t| &mut t.class)
    else {
        return;
    };
    let Some(index) = functions.functions.iter().position(|f| f.id == function) else {
        return;
    };
    let other = if later {
        index.checked_add(1)
    } else {
        index.checked_sub(1)
    };
    if let Some(other) = other.filter(|o| *o < functions.functions.len()) {
        functions.functions.swap(index, other);
    }
}

/// A text row whose value is stored only when `problem` finds none:
/// invalid text stays in the field, marked, until fixed or Esc. Returns
/// the new valid text when it differs from `current`.
fn checked_row(
    ui: &mut Ui,
    label: &str,
    current: &str,
    hint: &str,
    problem: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let key = ui.id().with(("checked", label));
    let mut text = ui
        .data(|d| d.get_temp::<String>(key))
        .unwrap_or_else(|| current.to_owned());
    let invalid = problem(&text).is_some();
    let response = kit::row(ui, label, |ui| {
        kit::text_field(ui, label, &mut text, hint, invalid)
    });
    let escaped = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escaped {
        ui.data_mut(|d| d.remove::<String>(key));
        return None;
    }
    match problem(&text) {
        None => {
            ui.data_mut(|d| d.remove::<String>(key));
            (text != current).then_some(text)
        }
        Some(message) => {
            ui.data_mut(|d| d.insert_temp(key, text.clone()));
            kit::field_error(ui, &response, &message, true);
            None
        }
    }
}

fn byte_limit(limit: usize, what: &'static str) -> impl Fn(&str) -> Option<String> {
    move |text: &str| {
        (text.trim().len() > limit).then(|| format!("{what} has at most {limit} bytes"))
    }
}

impl PartEditor {
    /// Drop a Pins subject that no longer exists (undo, another component).
    fn pins_validate(&mut self) {
        let Some(set) = &self.draft else {
            self.pins.subject = None;
            return;
        };
        if let Some(subject) = self.pins.subject
            && !exists(&set.component, subject)
        {
            self.pins.subject = None;
        }
    }

    /// The Pins tab canvas: sub-view switch and actions, then the table,
    /// the peripherals or the rules, or the open Paste table… flow.
    pub(super) fn pins_canvas(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        engine: &Engine,
    ) -> Option<Operation> {
        self.pins_validate();
        if self.draft.is_none() {
            ui.put(rect, egui::Label::new("No component draft"));
            return None;
        }
        let t = Tokens::get();
        let inner = rect.shrink2(vec2(t.space_16, t.space_12));
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(inner)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(inner.intersect(ui.clip_rect()));
        self.pins_toolbar(&mut child);
        child.add_space(t.space_8);
        if self.pins.table.paste.is_some() {
            return self.paste_dialog(&mut child, engine);
        }
        match self.pins.view {
            SubView::Table => self.pins_table_view(&mut child),
            SubView::Peripherals => self.peripherals_view(&mut child),
            SubView::Rules => self.rules_view(&mut child),
        }
        None
    }

    fn pins_toolbar(&mut self, ui: &mut Ui) {
        let t = Tokens::get();
        let paste_open = self.pins.table.paste.is_some();
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), t.control_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                for (view, label) in SubView::ALL {
                    let on = self.pins.view == view && !paste_open;
                    let look = if on {
                        Button::SmallSelected
                    } else {
                        Button::Small
                    };
                    if ui_kit::button(ui, label, look).clicked() {
                        self.pins.view = view;
                        self.pins.table.paste = None;
                    }
                }
                // The canvas keeps its top-right corner for the issue pill.
                ui.add_space(t.space_24);
                {
                    match self.pins.view {
                        SubView::Table => {
                            let open = ui_kit::button(
                                ui,
                                "Paste table…",
                                Button::Enabled(!paste_open),
                            )
                            .on_hover_text(
                                "Map the columns of a pasted pin table, preview, then apply",
                            )
                            .clicked();
                            if open {
                                self.pins.table.paste = Some(PasteFlow::new());
                            }
                        }
                        SubView::Peripherals => {
                            let add = ui_kit::button(ui, "Add peripheral", Button::Enabled(true))
                                .on_hover_text("A hardware instance, such as USART1, with roles")
                                .clicked();
                            if add && let Some(set) = self.draft.as_mut() {
                                match add_peripheral(&mut set.component) {
                                    Ok(id) => self.pins.subject = Some(Subject::Peripheral(id)),
                                    Err(message) => self.tell(Level::Error, message),
                                }
                            }
                        }
                        SubView::Rules => {
                            let add = ui_kit::button(ui, "Add rule", Button::Enabled(true))
                                .on_hover_text("A Rhai script over the pin assignment")
                                .clicked();
                            if add && let Some(set) = self.draft.as_mut() {
                                match add_rule(&mut set.component) {
                                    Ok(id) => self.pins.subject = Some(Subject::Rule(id)),
                                    Err(message) => self.tell(Level::Error, message),
                                }
                            }
                        }
                    }
                }
            },
        );
    }

    fn pins_table_view(&mut self, ui: &mut Ui) {
        let t = Tokens::get();
        let Some(set) = self.draft.as_ref() else {
            return;
        };
        let component = &set.component;
        let waiting = pins_table::unreviewed_count(component);
        let wide = ui.available_width() >= 720.0;
        let field_width = if wide { 240.0 } else { ui.available_width() };
        let mut filter = self.pins.table.filter;
        let chips = |ui: &mut Ui, filter: &mut Filter| {
            for choice in Filter::ALL {
                let label = if choice == Filter::Unreviewed {
                    format!("{} {waiting}", choice.label())
                } else {
                    choice.label().to_owned()
                };
                let look = if *filter == choice {
                    Button::SmallSelected
                } else {
                    Button::Small
                };
                if ui_kit::button(ui, label, look).clicked() {
                    *filter = choice;
                }
            }
        };
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), t.control_height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.allocate_ui(vec2(field_width, t.control_height), |ui| {
                    kit::filter_field(
                        ui,
                        ui.id().with("pins-filter"),
                        &mut self.pins.table.query,
                        "Filter pins and functions",
                        false,
                        pins_table::MAX_QUERY,
                    );
                });
                if wide {
                    ui.add_space(t.space_8);
                    chips(ui, &mut filter);
                }
            },
        );
        if !wide {
            ui.add_space(t.space_4);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), t.control_height),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| chips(ui, &mut filter),
            );
        }
        self.pins.table.filter = filter;
        ui.add_space(t.space_4);
        let rows: Vec<usize> = component
            .terminals
            .iter()
            .enumerate()
            .filter(|(_, terminal)| {
                pins_table::matches(
                    component,
                    terminal,
                    self.pins.table.filter,
                    &self.pins.table.query,
                )
            })
            .map(|(i, _)| i)
            .collect();
        if rows.is_empty() {
            kit::hint(
                ui,
                if component.terminals.is_empty() {
                    "No terminals yet: place pins on the Symbol tab or paste a table"
                } else {
                    "No pin matches this filter"
                },
                "Choose All to see every terminal",
            );
            return;
        }
        let pads = pins_table::pads_by_terminal(set);
        let event = pins_table::table(ui, component, &pads, &rows, self.pins.lit());
        self.pins_event(event);
    }

    fn peripherals_view(&mut self, ui: &mut Ui) {
        let Some(set) = self.draft.as_ref() else {
            return;
        };
        let event = pins_table::peripherals(ui, &set.component, self.pins.lit());
        self.pins_event(event);
    }

    fn pins_event(&mut self, event: Option<TableEvent>) {
        match event {
            None => {}
            Some(TableEvent::Terminal(t)) => self.pins.subject = Some(Subject::Terminal(t)),
            Some(TableEvent::Function(t, f)) => self.pins.subject = Some(Subject::Function(t, f)),
            Some(TableEvent::Peripheral(p)) => self.pins.subject = Some(Subject::Peripheral(p)),
            Some(TableEvent::AddFunction(t)) => {
                let Some(set) = self.draft.as_mut() else {
                    return;
                };
                match add_function(&mut set.component, t) {
                    Ok(f) => self.pins.subject = Some(Subject::Function(t, f)),
                    Err(message) => self.tell(Level::Error, message),
                }
            }
        }
    }

    fn rules_view(&mut self, ui: &mut Ui) {
        let t = Tokens::get();
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let full = ui.available_rect_before_wrap();
        let list_width = (full.width() * 0.3).clamp(160.0, 280.0);
        let list = Rect::from_min_size(full.min, vec2(list_width, full.height()));
        let body = Rect::from_min_max(
            egui::pos2((list.right() + t.space_16).min(full.right()), full.top()),
            full.max,
        );
        let selected = match self.pins.subject {
            Some(Subject::Rule(r)) => Some(r),
            _ => None,
        };
        let event = ui
            .scope_builder(UiBuilder::new().max_rect(list), |ui| {
                rules_editor::rule_list(ui, &set.component, &mut self.pins.rules, selected)
            })
            .inner;
        if let Some(RulesEvent::Select(r)) = event {
            self.pins.subject = Some(Subject::Rule(r));
        }
        ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("pins-rule-body")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let selected = match self.pins.subject {
                        Some(Subject::Rule(r)) => Some(r),
                        _ => None,
                    };
                    match selected
                        .and_then(|r| set.component.pin_rules.iter_mut().find(|rule| rule.id == r))
                    {
                        Some(rule) => {
                            kit::form(ui, |ui| {
                                kit::section(ui, "script", "Script", true, None, |ui| {
                                    rules_editor::script_editor(ui, rule, &mut self.pins.rules);
                                });
                            });
                        }
                        None => {
                            kit::form(ui, |ui| {
                                kit::hint(
                                    ui,
                                    "Choose a rule to edit its script",
                                    "Each rule returns ok(), forbid(), require() or warn()",
                                );
                            });
                        }
                    }
                    rules_editor::explain_panel(ui, &set.component, &mut self.pins.rules);
                });
        });
    }

    /// The Paste table… flow in the canvas: paste, map the columns, then
    /// Preview (`library.pinout_preview`) and Apply (`library.pinout_apply`).
    fn paste_dialog(&mut self, ui: &mut Ui, engine: &Engine) -> Option<Operation> {
        let id = egui::Id::new("pins-paste-table");
        let set = self.draft.as_ref()?;
        let lineage = self.lineage?;
        let device = set.metas.component.name.clone();
        let variant = Some(set.metas.package.name.trim())
            .filter(|n| !n.is_empty())
            .unwrap_or("package")
            .to_owned();
        let clean = self.draft == self.baseline && !self.awaiting;
        let ctx = ui.ctx().clone();
        let flow = self.pins.table.paste.as_mut()?;
        if let Some(message) = refusal::claim(&ctx, Origin::Dialog(id)) {
            flow.refusal = Some(message);
            flow.sent = None;
        }
        if let Some(sent) = flow.sent
            && engine.revision() != sent
        {
            let rows = flow.plan.as_ref().map_or(0, |(d, _)| d.pins.len());
            self.pins.table.paste = None;
            self.pins.table.filter = Filter::Unreviewed;
            self.tell(
                Level::Ok,
                format!("Imported {rows} pin rows; confirm each under Unreviewed"),
            );
            return None;
        }
        let rows = flow.rows();
        let (document, problems) = match &rows {
            Ok(rows) => pins_table::document(rows, &flow.mapping, &device, &variant),
            Err(_) => (None, Vec::new()),
        };
        if flow
            .plan
            .as_ref()
            .is_some_and(|(planned, _)| Some(planned) != document.as_ref())
        {
            flow.plan = None;
        }
        let planned = flow.plan.is_some();
        let disabled = match (&rows, &document) {
            (Err(_), _) => Some("Paste a table the mapping can read"),
            _ if flow.mapping.source_title.trim().is_empty() => {
                Some("Name the datasheet the table comes from")
            }
            (_, None) => Some("Map a Number column and paste at least one pin row"),
            _ if !clean => Some("Wait until your last edit is saved"),
            _ if flow.sent.is_some() => Some("Applying…"),
            _ => None,
        };
        let primary = if planned { "Apply" } else { "Preview" };
        let mut panel = kit::DialogPanel::new(id, "component", "Paste pin table", primary);
        panel.kind = "Import";
        panel.body_rows = 32.0;
        panel.primary_disabled = disabled;
        let shown_refusal = flow.refusal.clone();
        panel.refusal = shown_refusal.as_deref();
        panel.reset_scroll = std::mem::take(&mut flow.opened);
        let action = panel.show(ui, |ui| {
            paste_body(ui, flow, &rows, &problems);
        });
        match action {
            Some(kit::dialog::DialogAction::Cancel) => {
                self.pins.table.paste = None;
                None
            }
            Some(kit::dialog::DialogAction::Primary) if disabled.is_none() => {
                let document = document?;
                let target = PinoutTarget::Draft { id: lineage };
                match flow.plan.take() {
                    None => {
                        flow.refusal = None;
                        let query = Query::PinoutPreview {
                            target,
                            document: Box::new(document.clone()),
                        };
                        match engine.query(query) {
                            Ok(QueryResult::PinoutPlan(plan)) => {
                                flow.plan = Some((document, *plan))
                            }
                            Ok(_) => flow.refusal = Some("The preview returned no plan".into()),
                            Err(error) => flow.refusal = Some(refusal::shown_error(&error)),
                        }
                        None
                    }
                    Some((document, plan)) => {
                        let base = plan.base;
                        flow.plan = Some((document.clone(), plan));
                        flow.refusal = None;
                        flow.sent = Some(engine.revision());
                        refusal::mark(&ctx, Origin::Dialog(id));
                        Some(Operation::Library(LibraryCommand::PinoutApply(Box::new(
                            PinoutApply {
                                target,
                                document,
                                base,
                                accept: PinoutAccept::All,
                            },
                        ))))
                    }
                }
            }
            _ => None,
        }
    }

    /// The Pins tab's Properties card: the selected terminal, function,
    /// peripheral or rule, or the component's pin summary.
    pub(super) fn pins_card(&mut self, ui: &mut Ui, _engine: &Engine) -> Option<Operation> {
        self.pins_validate();
        match self.pins.subject {
            None => self.pins_summary(ui),
            Some(Subject::Terminal(t)) => self.terminal_props(ui, t),
            Some(Subject::Function(t, f)) => self.function_props(ui, t, f),
            Some(Subject::Peripheral(p)) => self.peripheral_props(ui, p),
            Some(Subject::Rule(r)) => self.rule_props(ui, r),
        }
        None
    }

    fn pins_summary(&mut self, ui: &mut Ui) {
        let Some(set) = self.draft.as_ref() else {
            return;
        };
        let c = &set.component;
        let functions: usize = c
            .terminals
            .iter()
            .filter_map(|t| t.functions())
            .map(|f| f.functions.len())
            .sum();
        let multi = c
            .terminals
            .iter()
            .filter(|t| t.functions().is_some_and(|f| f.functions.len() > 1))
            .count();
        kit::section(ui, "pins-summary", "Pins", true, None, |ui| {
            kit::value_row(ui, "Terminals", &c.terminals.len().to_string());
            kit::value_row(ui, "Muxed", &multi.to_string());
            kit::value_row(ui, "Functions", &functions.to_string());
            kit::value_row(ui, "Peripherals", &c.resources.len().to_string());
            kit::value_row(ui, "Rules", &c.pin_rules.len().to_string());
            kit::value_row(
                ui,
                "Unreviewed",
                &pins_table::unreviewed_count(c).to_string(),
            );
            kit::hint(
                ui,
                "Click a row or a function chip to edit it",
                "Peripherals and Rules are the other two views",
            );
        });
    }

    fn terminal_props(&mut self, ui: &mut Ui, terminal: TerminalId) {
        let mut refused = None;
        let mut confirm = false;
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let component = &mut set.component;
        let review = component.review.as_deref().cloned().unwrap_or_default();
        kit::section(ui, "pins-terminal", "Terminal", true, None, |ui| {
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
            let Some(functions) = component
                .terminals
                .iter_mut()
                .find(|t| t.id == terminal)
                .and_then(|t| match &mut t.class {
                    pcbcraft_core::library::TerminalClass::Electrical(f) => Some(f),
                    _ => None,
                })
            else {
                return;
            };
            let port = functions
                .gpio
                .as_ref()
                .map_or("", |g| g.port.as_str())
                .to_owned();
            let port_problem = |text: &str| {
                (text.trim().len() > MAX_GPIO_PORT)
                    .then(|| format!("A GPIO port has at most {MAX_GPIO_PORT} bytes"))
            };
            if let Some(port) = checked_row(ui, "GPIO port", &port, "A, P0", port_problem) {
                let port = port.trim().to_owned();
                functions.gpio = if port.is_empty() {
                    None
                } else {
                    Some(GpioRef {
                        port,
                        index: functions.gpio.as_ref().map_or(0, |g| g.index),
                    })
                };
            }
            if let Some(gpio) = functions.gpio.as_mut() {
                let mut index = f64::from(gpio.index);
                if kit::number_row(ui, "GPIO index", &mut index, "", 0.0..=65535.0) {
                    gpio.index = index.round().clamp(0.0, 65535.0) as u16;
                }
            }
            kit::hint(
                ui,
                &format!(
                    "{} {}: click a chip to edit one",
                    functions.functions.len(),
                    if functions.functions.len() == 1 {
                        "function"
                    } else {
                        "functions"
                    }
                ),
                "The GPIO identity is stated here, never read from the name",
            );
        });
        if let Some(functions) = component
            .terminals
            .iter_mut()
            .find(|t| t.id == terminal)
            .and_then(|t| match &mut t.class {
                pcbcraft_core::library::TerminalClass::Electrical(f) => Some(f),
                _ => None,
            })
        {
            kit::section(ui, "pins-options", "Options", true, None, |ui| {
                options_rows(ui, &mut functions.options);
            });
        }
        let pending = review.unreviewed.contains(&terminal) || review.uncertain.contains(&terminal);
        if pending {
            kit::section(ui, "pins-review", "Review", true, None, |ui| {
                let text = if review.uncertain.contains(&terminal) {
                    "The import flagged this row uncertain: check it against the datasheet"
                } else {
                    "Imported and not reviewed yet"
                };
                kit::status(ui, text, kit::Tone::Dim);
                kit::actions(ui, |ui| {
                    confirm = ui_kit::button(ui, "Confirm row", Button::Enabled(true))
                        .on_hover_text("You checked this row's functions against the datasheet")
                        .clicked();
                });
            });
        }
        if confirm {
            confirm_row(component, terminal);
        }
        if let Some(message) = refused {
            self.tell(Level::Error, message);
        }
    }

    fn function_props(&mut self, ui: &mut Ui, terminal: TerminalId, function: FunctionId) {
        let mut action = None;
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let component = &mut set.component;
        let roles: Vec<(Option<ResourceRole>, String)> = std::iter::once((None, "None".to_owned()))
            .chain(component.resources.iter().flat_map(|r| {
                r.roles.iter().map(move |role| {
                    (
                        Some(ResourceRole {
                            resource: r.id,
                            role: role.id,
                        }),
                        format!("{} · {}", r.label, role.label),
                    )
                })
            }))
            .collect();
        let Some(table) = component
            .terminals
            .iter_mut()
            .find(|t| t.id == terminal)
            .and_then(|t| match &mut t.class {
                pcbcraft_core::library::TerminalClass::Electrical(f) => Some(f),
                _ => None,
            })
        else {
            return;
        };
        let count = table.functions.len();
        let mut default = table.design_default == Some(function);
        let Some(f) = table.functions.iter_mut().find(|f| f.id == function) else {
            return;
        };
        let directions: Vec<_> = DIRECTIONS
            .iter()
            .map(|d| (*d, sentence(d.label())))
            .collect();
        let signals: Vec<_> = SIGNALS.iter().map(|(s, l)| (*s, (*l).to_owned())).collect();
        let drives: Vec<_> = DRIVES.iter().map(|(d, l)| (*d, (*l).to_owned())).collect();
        kit::section(ui, "pins-function", "Function", true, None, |ui| {
            kit::text_row(ui, "Label", &mut f.label)
                .on_hover_text("Function label, unique on this terminal");
            kit::choice_row(ui, "Direction", &mut f.direction, &directions);
            kit::choice_row(ui, "Signal", &mut f.signal, &signals);
            kit::choice_row(ui, "Drive", &mut f.drive, &drives);
            kit::choice_row(ui, "Peripheral", &mut f.resource_role, &roles);
            let mux = f.mux.as_ref().map_or("", |m| m.0.as_str()).to_owned();
            if let Some(mux) = checked_row(
                ui,
                "Mux code",
                &mux,
                "AF7, ALT2",
                byte_limit(MAX_MUX_CODE, "A mux code"),
            ) {
                let mux = mux.trim().to_owned();
                f.mux = (!mux.is_empty()).then_some(MuxCode(mux));
            }
            if let Some(source) = &f.source {
                let text = match (source.page, &source.table) {
                    (Some(page), Some(table)) => format!("p. {page} · {table}"),
                    (Some(page), None) => format!("p. {page}"),
                    (None, Some(table)) => table.clone(),
                    (None, None) => "Imported".into(),
                };
                kit::value_row(ui, "Source", &text);
            }
            if count > 1 && kit::check_row(ui, "Design default", &mut default) {
                action = Some(FunctionAction::Default(default));
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Remove", Button::Enabled(count > 1))
                    .on_hover_text("A terminal keeps at least one function")
                    .clicked()
                {
                    action = Some(FunctionAction::Remove);
                }
                if ui_kit::button(ui, "Later", Button::Enabled(count > 1))
                    .on_hover_text("Move the chip one place right")
                    .clicked()
                {
                    action = Some(FunctionAction::Move(true));
                }
                if ui_kit::button(ui, "Earlier", Button::Enabled(count > 1))
                    .on_hover_text("Move the chip one place left")
                    .clicked()
                {
                    action = Some(FunctionAction::Move(false));
                }
            });
        });
        match action {
            None => {}
            Some(FunctionAction::Default(on)) => {
                table.design_default = if on {
                    Some(function)
                } else if table.design_default == Some(function) {
                    None
                } else {
                    table.design_default
                };
            }
            Some(FunctionAction::Remove) if table.functions.len() > 1 => {
                table.functions.retain(|f| f.id != function);
                if table.design_default == Some(function) {
                    table.design_default = None;
                }
                self.pins.subject = Some(Subject::Terminal(terminal));
            }
            Some(FunctionAction::Remove) => {}
            Some(FunctionAction::Move(later)) => {
                move_function(component, terminal, function, later);
            }
        }
    }

    fn peripheral_props(&mut self, ui: &mut Ui, resource: ResourceId) {
        let mut add = false;
        let mut remove: Option<Option<RoleId>> = None;
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let component = &mut set.component;
        let Some(r) = component.resources.iter_mut().find(|r| r.id == resource) else {
            return;
        };
        kit::section(ui, "pins-peripheral", "Peripheral", true, None, |ui| {
            kit::text_row(ui, "Label", &mut r.label)
                .on_hover_text("The peripheral instance, such as USART1");
            let kind = r.kind.clone().unwrap_or_default();
            if let Some(kind) = checked_row(
                ui,
                "Kind",
                &kind,
                "uart, spi",
                byte_limit(MAX_KIND, "A kind"),
            ) {
                let kind = kind.trim().to_owned();
                r.kind = (!kind.is_empty()).then_some(kind);
            }
            kit::hint(
                ui,
                "Kind groups peripherals for search and firmware",
                "It is never used to decide electrical compatibility",
            );
        });
        let roles = r.roles.len();
        kit::section(ui, "pins-roles", "Roles", true, Some(roles), |ui| {
            for role in &mut r.roles {
                ui.push_id(("role", role.id.0), |ui| {
                    kit::rule(ui);
                    kit::text_row(ui, "Role", &mut role.label)
                        .on_hover_text("The role a function takes, such as tx");
                    let signal = role.signal.clone().unwrap_or_default();
                    if let Some(signal) = checked_row(
                        ui,
                        "Signal",
                        &signal,
                        "TX, SCK",
                        byte_limit(MAX_KIND, "A signal name"),
                    ) {
                        let signal = signal.trim().to_owned();
                        role.signal = (!signal.is_empty()).then_some(signal);
                    }
                    let mut capacity = f64::from(role.capacity);
                    if kit::number_row(ui, "Capacity", &mut capacity, "", 1.0..=65535.0) {
                        role.capacity = capacity.round().clamp(1.0, 65535.0) as u16;
                    }
                    kit::check_row(ui, "Required when used", &mut role.required_when_used);
                    kit::actions(ui, |ui| {
                        if ui_kit::button(ui, "Remove role", Button::Enabled(roles > 1))
                            .on_hover_text("Functions that take it lose their peripheral")
                            .clicked()
                        {
                            remove = Some(Some(role.id));
                        }
                    });
                });
            }
            kit::rule(ui);
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Remove peripheral", Button::Enabled(true))
                    .on_hover_text("Functions that take its roles lose their peripheral")
                    .clicked()
                {
                    remove = Some(None);
                }
                add = ui_kit::button(ui, "Add role", Button::Enabled(roles < MAX_ROLES)).clicked();
            });
        });
        let refused = if add {
            add_role(component, resource).err()
        } else {
            None
        };
        if let Some(role) = remove {
            remove_peripheral(component, resource, role);
            if role.is_none() {
                self.pins.subject = None;
            }
        }
        if let Some(message) = refused {
            self.tell(Level::Error, message);
        }
    }

    fn rule_props(&mut self, ui: &mut Ui, rule: RuleId) {
        let mut remove = false;
        let Some(set) = self.draft.as_mut() else {
            return;
        };
        let component = &mut set.component;
        let others: Vec<String> = component
            .pin_rules
            .iter()
            .filter(|r| r.id != rule)
            .map(|r| r.label.trim().to_lowercase())
            .collect();
        let Some(r) = component.pin_rules.iter_mut().find(|r| r.id == rule) else {
            return;
        };
        kit::section(ui, "pins-rule", "Rule", true, None, |ui| {
            let problem = |text: &str| {
                if !pcbcraft_core::ids::printable(text) {
                    Some("A rule label is printable text".to_owned())
                } else if text.len() > MAX_LABEL {
                    Some(format!("A rule label has at most {MAX_LABEL} bytes"))
                } else if others.contains(&text.trim().to_lowercase()) {
                    Some("Another rule has this label".to_owned())
                } else {
                    None
                }
            };
            let label = r.label.clone();
            if let Some(label) = checked_row(ui, "Label", &label, "usart1_map", problem) {
                r.label = label.trim().to_owned();
            }
            kit::value_row(
                ui,
                "Script",
                &format!(
                    "{} of {} bytes",
                    r.script.source.len(),
                    pcbcraft_core::functions::rules::MAX_RULE_SOURCE
                ),
            );
            if let Some(source) = &r.source {
                let text = match (source.page, &source.table) {
                    (Some(page), Some(table)) => format!("p. {page} · {table}"),
                    (Some(page), None) => format!("p. {page}"),
                    (None, Some(table)) => table.clone(),
                    (None, None) => "Imported".into(),
                };
                kit::value_row(ui, "Source", &text);
            }
            kit::hint(
                ui,
                "Edit the script in the canvas; Explain tries it",
                "Rules are stored with the component revision",
            );
            kit::actions(ui, |ui| {
                remove = ui_kit::button(ui, "Remove rule", Button::Enabled(true)).clicked();
            });
        });
        if remove {
            component.pin_rules.retain(|r| r.id != rule);
            self.pins.subject = None;
        }
    }
}

enum FunctionAction {
    Default(bool),
    Remove,
    Move(bool),
}

/// The settable options of a terminal and their allowed values.
fn options_rows(ui: &mut Ui, options: &mut Vec<PinOption>) {
    let mut pulls: Vec<Pull> = options
        .iter()
        .find_map(|o| match o {
            PinOption::Pull(v) => Some(v.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut changed_pull = false;
    for (pull, label) in [
        (Pull::Up, "Pull-up"),
        (Pull::Down, "Pull-down"),
        (Pull::None, "No pull"),
    ] {
        let mut on = pulls.contains(&pull);
        if kit::check_row(ui, label, &mut on) {
            changed_pull = true;
            if on {
                pulls.push(pull);
            } else {
                pulls.retain(|p| *p != pull);
            }
        }
    }
    if changed_pull {
        options.retain(|o| !matches!(o, PinOption::Pull(_)));
        if !pulls.is_empty() {
            options.push(PinOption::Pull(pulls));
        }
    }
    let speeds = options
        .iter()
        .find_map(|o| match o {
            PinOption::Speed(v) => Some(v.join(", ")),
            _ => None,
        })
        .unwrap_or_default();
    let problem = |text: &str| {
        let values = speed_values(text);
        if values.len() > MAX_OPTION_VALUES {
            Some(format!("At most {MAX_OPTION_VALUES} speeds"))
        } else if values.iter().any(|v| v.len() > MAX_LABEL) {
            Some(format!("A speed has at most {MAX_LABEL} bytes"))
        } else {
            None
        }
    };
    if let Some(text) = checked_row(ui, "Speeds", &speeds, "low, high", problem) {
        options.retain(|o| !matches!(o, PinOption::Speed(_)));
        let values = speed_values(&text);
        if !values.is_empty() {
            options.push(PinOption::Speed(values));
        }
    }
    for (option, label) in [
        (PinOption::InitialLevel, "Initial level"),
        (PinOption::Schmitt, "Schmitt trigger"),
    ] {
        let mut on = options.contains(&option);
        if kit::check_row(ui, label, &mut on) {
            options.retain(|o| *o != option);
            if on {
                options.push(option);
            }
        }
    }
}

/// Speed grades from comma-separated text, as data.
pub fn speed_values(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The Paste table… body: pasted text, delimiter and header, provenance,
/// the column mapping, problems and the last plan.
fn paste_body(
    ui: &mut Ui,
    flow: &mut PasteFlow,
    rows: &Result<Vec<Vec<String>>, String>,
    problems: &[pins_table::RowProblem],
) {
    let text_id = ui.id().with("paste-text");
    kit::note(
        ui,
        "Paste the pin table; imported rows stay unreviewed until confirmed",
    );
    let response = ui.add(
        egui::TextEdit::multiline(&mut flow.text)
            .id(text_id)
            .font(egui::TextStyle::Monospace)
            .desired_rows(5)
            .desired_width(ui.available_width())
            .hint_text("12\tPA9\tUSART1_TX\tTIM1_CH2"),
    );
    if flow.text.len() > pins_table::MAX_PASTE {
        let mut end = pins_table::MAX_PASTE;
        while !flow.text.is_char_boundary(end) {
            end -= 1;
        }
        flow.text.truncate(end);
    }
    if let Err(message) = rows {
        kit::field_error(ui, &response, message, false);
    }
    let delimiters: Vec<_> = pins_table::Delimiter::ALL
        .iter()
        .map(|(d, l)| (*d, (*l).to_owned()))
        .collect();
    kit::choice_row(ui, "Separator", &mut flow.delimiter, &delimiters);
    kit::check_row(ui, "First row holds captions", &mut flow.mapping.header);
    let m = &mut flow.mapping;
    kit::text_row(ui, "Datasheet", &mut m.source_title)
        .on_hover_text("The document the table comes from, recorded as provenance");
    kit::text_row(ui, "Revision", &mut m.source_revision);
    kit::text_row(ui, "Page", &mut m.page);
    let directions: Vec<_> = DIRECTIONS
        .iter()
        .map(|d| (*d, sentence(d.label())))
        .collect();
    let signals: Vec<_> = SIGNALS.iter().map(|(s, l)| (*s, (*l).to_owned())).collect();
    kit::choice_row(ui, "Direction", &mut m.direction, &directions);
    kit::choice_row(ui, "Signal", &mut m.signal, &signals);
    kit::text_row(ui, "Split cells at", &mut m.list_separator)
        .on_hover_text("Split a cell into several functions at this text; empty for one per cell");
    kit::text_row(ui, "Split role at", &mut m.role_separator).on_hover_text(
        "Split a function into peripheral and role at this text, such as _; empty for none",
    );
    let Ok(rows) = rows else {
        return;
    };
    let captions = m.header;
    kit::section(
        ui,
        "paste-columns",
        "Columns",
        true,
        Some(m.columns.len()),
        |ui| {
            let header = rows.first();
            let sample = rows.get(usize::from(captions));
            let maps: Vec<_> = pins_table::ColumnMap::ALL
                .iter()
                .map(|(c, l)| (*c, (*l).to_owned()))
                .collect();
            for (index, column) in m.columns.iter_mut().enumerate() {
                let caption = header
                    .filter(|_| captions)
                    .and_then(|h| h.get(index))
                    .filter(|c| !c.is_empty())
                    .cloned()
                    .unwrap_or_else(|| format!("Column {}", index + 1));
                let example = sample
                    .and_then(|r| r.get(index))
                    .cloned()
                    .unwrap_or_default();
                ui.push_id(("column", index), |ui| {
                    kit::choice_row(ui, &format!("{caption} ({example})"), column, &maps);
                });
            }
        },
    );
    let m = &flow.mapping;
    if let Some(cells) = rows.get(usize::from(m.header)) {
        let functions: Vec<String> = m
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == pins_table::ColumnMap::Function)
            .filter_map(|(i, _)| cells.get(i))
            .filter(|c| !c.is_empty())
            .flat_map(|cell| {
                let parts: Vec<String> = if m.list_separator.is_empty() {
                    vec![cell.clone()]
                } else {
                    cell.split(m.list_separator.as_str())
                        .map(|p| p.trim().to_owned())
                        .filter(|p| !p.is_empty())
                        .collect()
                };
                parts.into_iter().map(|label| {
                    match pins_table::split_role(&label, &m.role_separator) {
                        (Some(p), Some(r)) => format!("{label} → {p} · {r}"),
                        _ => label,
                    }
                })
            })
            .collect();
        kit::value_row(
            ui,
            "First pin",
            &if functions.is_empty() {
                "no functions".to_owned()
            } else {
                functions.join(", ")
            },
        );
    }
    for problem in problems.iter().take(kit::LIST_MAX_ROWS) {
        kit::status(ui, &problem.message, kit::Tone::Dim);
    }
    if problems.len() > kit::LIST_MAX_ROWS {
        kit::note(
            ui,
            &format!("{} more rows left out", problems.len() - kit::LIST_MAX_ROWS),
        );
    }
    if let Some((document, plan)) = &flow.plan {
        kit::section(ui, "paste-plan", "Preview", true, None, |ui| {
            kit::value_row(ui, "Pins", &document.pins.len().to_string());
            kit::value_row(ui, "Peripherals", &document.peripherals.len().to_string());
            kit::value_row(ui, "Changes", &plan.changes.len().to_string());
            kit::value_row(ui, "Findings", &plan.findings.len().to_string());
            kit::value_row(ui, "To review", &plan.review.len().to_string());
            for item in plan.review.iter().take(kit::LIST_MAX_ROWS) {
                kit::status(
                    ui,
                    &format!("{}: {}", item.row, item.message),
                    kit::Tone::Dim,
                );
            }
            if plan.review.len() > kit::LIST_MAX_ROWS {
                kit::note(
                    ui,
                    &format!(
                        "{} more review items",
                        plan.review.len() - kit::LIST_MAX_ROWS
                    ),
                );
            }
        });
    }
}

#[cfg(test)]
#[path = "pins_tests.rs"]
mod tests;
