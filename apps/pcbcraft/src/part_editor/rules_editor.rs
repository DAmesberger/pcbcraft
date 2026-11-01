//! Slice C5: the Rules sub-view: one Rhai pin-rule script per row with a
//! live syntax check and an explain/test panel that runs
//! `functions::resolve` and the rule evaluator on a scratch selection
//! (founder decision F2). Nothing here changes the document except the
//! script text and labels the person types; the scratch selection lives
//! only in this editor.
use crate::kit;
use crate::refusal::shown_error;
use egui::Ui;
use pcbcraft_core::Error;
use pcbcraft_core::functions::rules::{
    self, MAX_RULE_SOURCE, PinRule, RuleBudget, RuleEvaluation, RuleFailureKind, RuleVerdict,
    RuleView, ViewFunction, ViewResource, ViewTerminal,
};
use pcbcraft_core::functions::{self, FunctionSelections, Resolution};
use pcbcraft_core::library::ComponentDefinition;
use pcbcraft_core::{FunctionId, RuleId, TerminalId};
use solidcraft_ui_egui::theme::Tokens;

/// What the syntax check says about one script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// The script compiles.
    Compiles,
    /// The compiler's message, or why the script cannot be checked.
    Error(String),
    /// This build has no rule compiler yet.
    Unavailable,
}

/// The syntax check of one rule: bounds first, then the core compiler
/// (`functions::rules::compile`) and its compile failures.
pub fn check(rule: &PinRule) -> Check {
    let source = &rule.script.source;
    if source.len() > MAX_RULE_SOURCE {
        return Check::Error(format!("A rule script has at most {MAX_RULE_SOURCE} bytes"));
    }
    if source.trim().is_empty() {
        return Check::Error("The script is empty: end it with ok() or a verdict".into());
    }
    let compiled = match rules::compile(std::slice::from_ref(rule)) {
        Ok(compiled) => compiled,
        Err(Error::Unsupported(_)) => return Check::Unavailable,
        Err(error) => return Check::Error(shown_error(&error)),
    };
    match rules::evaluate(&compiled, &RuleView::default(), RuleBudget::default()) {
        Ok(evaluation) => evaluation
            .failures
            .iter()
            .find(|f| f.rule == rule.id && f.kind == RuleFailureKind::Compile)
            .map_or(Check::Compiles, |f| Check::Error(f.message.clone())),
        Err(Error::Unsupported(_)) => Check::Unavailable,
        Err(error) => Check::Error(shown_error(&error)),
    }
}

/// The read-only view rules see of `component` resolved as `resolution`:
/// every terminal's name, declared labels and active function with its
/// peripheral and role, and every peripheral's active roles.
pub fn rule_view(component: &ComponentDefinition, resolution: &Resolution) -> RuleView {
    let mut view = RuleView::default();
    let mut active_roles: Vec<(pcbcraft_core::ResourceId, pcbcraft_core::RoleId)> = Vec::new();
    for terminal in &component.terminals {
        let Some(declared) = terminal.functions() else {
            continue;
        };
        let active = resolution.active(terminal.id).and_then(|a| {
            let function = declared.function(a.function)?;
            let names = super::pins_table::peripheral_of(component, function);
            if let Some(rr) = function.resource_role {
                active_roles.push((rr.resource, rr.role));
            }
            Some(ViewFunction {
                function: function.id,
                label: function.label.clone(),
                peripheral: names.map(|(p, _)| p.to_owned()),
                role: names.map(|(_, r)| r.to_owned()),
            })
        });
        view.terminals.push(ViewTerminal {
            terminal: terminal.id,
            name: terminal.name.clone(),
            active,
            functions: declared.functions.iter().map(|f| f.label.clone()).collect(),
            config: None,
        });
    }
    for resource in &component.resources {
        view.resources.push(ViewResource {
            resource: resource.id,
            label: resource.label.clone(),
            kind: resource.kind.clone(),
            active_roles: resource
                .roles
                .iter()
                .filter(|r| active_roles.contains(&(resource.id, r.id)))
                .map(|r| r.label.clone())
                .collect(),
        });
    }
    view
}

/// What the explain panel shows for one scratch selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Explained {
    pub resolution: Result<Resolution, String>,
    /// Every rule's verdict; `None` when this build cannot evaluate rules.
    pub evaluation: Option<Result<RuleEvaluation, String>>,
}

/// `scratch` without selections that no longer name a declared function.
pub fn valid_scratch(
    component: &ComponentDefinition,
    scratch: &FunctionSelections,
) -> FunctionSelections {
    scratch
        .iter()
        .filter(|(t, f)| {
            component
                .terminal(**t)
                .and_then(|t| t.functions())
                .is_some_and(|d| d.function(**f).is_some())
        })
        .map(|(t, f)| (*t, *f))
        .collect()
}

/// Resolve `component` with the scratch selection and evaluate its rules
/// over the result, within the default budget.
pub fn explain(component: &ComponentDefinition, scratch: &FunctionSelections) -> Explained {
    let scratch = valid_scratch(component, scratch);
    let declarations = component.declarations();
    let resolution = functions::resolve(
        &declarations,
        &component.resources,
        &component.constraints,
        None,
        &scratch,
    )
    .map_err(|e| shown_error(&e));
    let evaluation = match &resolution {
        Err(_) => None,
        Ok(_) if component.pin_rules.is_empty() => Some(Ok(RuleEvaluation::default())),
        Ok(resolved) => {
            let view = rule_view(component, resolved);
            match rules::compile(&component.pin_rules)
                .and_then(|c| rules::evaluate(&c, &view, RuleBudget::default()))
            {
                Ok(evaluation) => Some(Ok(evaluation)),
                Err(Error::Unsupported(_)) => None,
                Err(error) => Some(Err(shown_error(&error))),
            }
        }
    };
    Explained {
        resolution,
        evaluation,
    }
}

/// One line for a rule's verdict ("Forbids: USART1 uses one pin set").
pub fn verdict_text(verdict: &RuleVerdict) -> String {
    match verdict {
        RuleVerdict::Ok => "OK".into(),
        RuleVerdict::Forbid { message, .. } => format!("Forbids: {message}"),
        RuleVerdict::Require { message, .. } => format!("Requires: {message}"),
        RuleVerdict::Warn { message, .. } => format!("Warns: {message}"),
    }
}

/// The source a new rule starts with.
pub const NEW_RULE_SOURCE: &str = "// Return ok(), forbid(msg), require(msg) or warn(msg).\n// Read pins with active(\"PA9\", \"usart1_tx\"), used(\"USART1\").\nok()\n";

/// The editor's buffers, caches and the explain panel's scratch selection.
#[derive(Default)]
pub struct RulesEditor {
    /// The explain panel's function choice per terminal; never saved.
    pub scratch: FunctionSelections,
    /// The last syntax check, for the source it was made for.
    checked: Option<(RuleId, String, Check)>,
    /// The last explanation, for the component and scratch it was made for.
    explained: Option<(ComponentDefinition, FunctionSelections, Explained)>,
}

impl RulesEditor {
    /// The syntax check of `rule`, recomputed when its source changed.
    pub fn check(&mut self, rule: &PinRule) -> Check {
        if let Some((id, source, check)) = &self.checked
            && *id == rule.id
            && *source == rule.script.source
        {
            return check.clone();
        }
        let result = check(rule);
        self.checked = Some((rule.id, rule.script.source.clone(), result.clone()));
        result
    }

    /// The explanation of `component` with the scratch selection,
    /// recomputed when either changed.
    pub fn explained(&mut self, component: &ComponentDefinition) -> Explained {
        if let Some((c, s, e)) = &self.explained
            && c == component
            && *s == self.scratch
        {
            return e.clone();
        }
        let result = explain(component, &self.scratch);
        self.explained = Some((component.clone(), self.scratch.clone(), result.clone()));
        result
    }
}

/// What a click in the Rules view asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesEvent {
    Select(RuleId),
}

/// The rule list: one row per rule with its check, the selected one lit.
pub fn rule_list(
    ui: &mut Ui,
    component: &ComponentDefinition,
    editor: &mut RulesEditor,
    selected: Option<RuleId>,
) -> Option<RulesEvent> {
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("pins-rule-list")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            kit::form(ui, |ui| {
                for rule in &component.pin_rules {
                    let lit = selected == Some(rule.id);
                    let (secondary, issue) = match editor.check(rule) {
                        Check::Compiles => ("compiles", None),
                        Check::Unavailable => ("", None),
                        Check::Error(_) => ("does not compile", Some(kit::severity::Level::Error)),
                    };
                    let row = kit::Row {
                        icon: "code",
                        label: &rule.label,
                        secondary,
                        selected: lit,
                        issue,
                        tip: "Edit the rule",
                        ..Default::default()
                    };
                    if kit::tree_row(ui, ui.id().with(("rule", rule.id.0)), &row).clicked {
                        event = Some(RulesEvent::Select(rule.id));
                    }
                }
                if component.pin_rules.is_empty() {
                    kit::hint(
                        ui,
                        "No rules yet",
                        "A rule is a Rhai script that returns ok, forbid, require or warn",
                    );
                }
            });
        });
    event
}

/// The script editor of `rule` with its live syntax check. Edits the
/// source in place; the editor's commit sends it when focus leaves.
pub fn script_editor(ui: &mut Ui, rule: &mut PinRule, editor: &mut RulesEditor) {
    let t = Tokens::get();
    kit::form(ui, |ui| {
        let id = ui.id().with(("script", rule.id.0));
        let response = ui.add(
            egui::TextEdit::multiline(&mut rule.script.source)
                .id(id)
                .code_editor()
                .desired_rows(9)
                .desired_width(ui.available_width())
                .text_color(t.text),
        );
        bound_bytes(&mut rule.script.source, MAX_RULE_SOURCE);
        let bytes = rule.script.source.len();
        match editor.check(rule) {
            Check::Compiles => {
                kit::note(
                    ui,
                    &format!("Script compiles · {bytes} of {MAX_RULE_SOURCE} bytes"),
                );
            }
            Check::Error(message) => {
                kit::field_error(ui, &response, &message, false);
            }
            Check::Unavailable => {
                kit::hint(
                    ui,
                    "This build cannot check rule scripts yet",
                    "The script is stored as typed; Issues reports it once checked",
                );
            }
        }
    });
}

/// Cut `text` to at most `limit` bytes at a character boundary.
fn bound_bytes(text: &mut String, limit: usize) {
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

/// The explain/test panel: a function choice per multi-function terminal
/// (the scratch selection), then what the resolution and every rule say.
pub fn explain_panel(ui: &mut Ui, component: &ComponentDefinition, editor: &mut RulesEditor) {
    kit::form(ui, |ui| {
        kit::section(ui, "explain", "Explain", true, None, |ui| {
            let multi: Vec<_> = component
                .terminals
                .iter()
                .filter(|t| t.functions().is_some_and(|f| f.functions.len() > 1))
                .collect();
            if multi.is_empty() {
                kit::hint(
                    ui,
                    "No terminal has several functions to choose between",
                    "Add a second function to a pin in the Table view",
                );
            }
            let all_id = ui.id().with("explain-all");
            let all = ui.data(|d| d.get_temp::<bool>(all_id)).unwrap_or(false);
            let max = if all { multi.len() } else { kit::LIST_MAX_ROWS };
            let shown = kit::list_frame(ui, multi.len(), max, |ui, index| {
                let Some(terminal) = multi.get(index) else {
                    return;
                };
                let Some(declared) = terminal.functions() else {
                    return;
                };
                let mut choice: Option<FunctionId> = editor.scratch.get(&terminal.id).copied();
                let default = declared
                    .implicit()
                    .map_or("Unresolved".to_owned(), |(f, _)| {
                        format!("Default ({})", f.label)
                    });
                let options: Vec<(Option<FunctionId>, String)> = std::iter::once((None, default))
                    .chain(
                        declared
                            .functions
                            .iter()
                            .map(|f| (Some(f.id), super::pins_table::chip_text(f))),
                    )
                    .collect();
                if kit::choice_row(ui, &terminal.name, &mut choice, &options) {
                    set_scratch(&mut editor.scratch, terminal.id, choice);
                }
            });
            if shown.show_all {
                ui.data_mut(|d| d.insert_temp(all_id, true));
            }
            kit::actions(ui, |ui| {
                let clear = solidcraft_ui_egui::ui_kit::button(
                    ui,
                    "Reset choices",
                    solidcraft_ui_egui::ui_kit::Button::Enabled(!editor.scratch.is_empty()),
                )
                .on_hover_text("Back to each terminal's default; the document is unchanged")
                .clicked();
                if clear {
                    editor.scratch.clear();
                }
            });
            let explained = editor.explained(component);
            outcome(ui, component, &explained);
        });
    });
}

fn set_scratch(scratch: &mut FunctionSelections, terminal: TerminalId, f: Option<FunctionId>) {
    match f {
        Some(f) => {
            scratch.insert(terminal, f);
        }
        None => {
            scratch.remove(&terminal);
        }
    }
}

/// The resolution's buckets and every rule's verdict, as status lines.
fn outcome(ui: &mut Ui, component: &ComponentDefinition, explained: &Explained) {
    use kit::Tone;
    kit::rule(ui);
    let resolution = match &explained.resolution {
        Ok(resolution) => resolution,
        Err(message) => {
            kit::refusal(ui, message);
            return;
        }
    };
    if resolution.conflicts.is_empty()
        && resolution.incomplete.is_empty()
        && resolution.notes.is_empty()
    {
        kit::status(ui, "This choice is allowed and complete", Tone::Dim);
    }
    for conflict in &resolution.conflicts {
        kit::status(ui, &format!("Rejected: {}", conflict.message()), Tone::Dim);
    }
    for requirement in &resolution.incomplete {
        kit::status(
            ui,
            &format!("Incomplete: {}", requirement.message()),
            Tone::Dim,
        );
    }
    for note in &resolution.notes {
        kit::status(ui, &format!("Note: {}", note.message), Tone::Dim);
    }
    if component.pin_rules.is_empty() {
        return;
    }
    kit::rule(ui);
    match &explained.evaluation {
        None => {
            kit::hint(
                ui,
                "This build cannot evaluate rule scripts yet",
                "The resolution above uses functions, peripherals and pair rules",
            );
        }
        Some(Err(message)) => {
            kit::refusal(ui, message);
        }
        Some(Ok(evaluation)) => {
            for rule in &component.pin_rules {
                let text = evaluation
                    .outcomes
                    .iter()
                    .find(|o| o.rule == rule.id)
                    .map(|o| verdict_text(&o.verdict))
                    .or_else(|| {
                        evaluation
                            .failures
                            .iter()
                            .find(|f| f.rule == rule.id)
                            .map(|f| format!("No verdict: {}", f.message))
                    })
                    .unwrap_or_else(|| "Not evaluated".into());
                kit::value_row(ui, &rule.label, &text);
            }
        }
    }
}
