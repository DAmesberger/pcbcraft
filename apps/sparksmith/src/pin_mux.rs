//! Slice C6: the schematic pin-mux panel. "Pin assignment…" in an
//! instance's Pin functions section opens a docked window (the
//! Resolve-parts container, `ui_kit::Window`) with three parts:
//!
//! - **Peripherals**: one tick per declared peripheral (resource). Ticking
//!   runs `functions.suggest` on a worker thread (the search may take
//!   seconds; the panel shows it running with Cancel) and stages its first
//!   assignment, every role that has a pin; Tab (or Next) cycles through
//!   the ranked alternatives while the pointer is over the panel. Each peripheral in use
//!   lists its roles with a choice of the pins whose functions declare
//!   that role. Unticking releases its pins.
//! - **Pins**: each multi-function or peripheral pin with its previewed
//!   function, mux code and net ("UART_TX → U3.RXD"); changed pins are
//!   marked.
//! - **Package**: the chosen footprint's pads in 2D, tinted by peripheral.
//!
//! Every staged change is previewed through the core's one resolution rule
//! (pin rules included); a rejected combination shows its rule message
//! inline and gates Apply, so nothing is ever applied half way. Apply sends
//! one `instance.select_functions` batch: one undo step. Candidates come
//! only from declared resource roles, never from matching names. State
//! lives in egui memory, so no shared editor struct changes. [`reveal`]
//! opens the panel at a pin (Issues locate of `config.*` rows).
mod model;
mod package;

use crate::kit::{self, LIST_MAX_ROWS, severity::Level, severity_glyph};
use crate::refusal::{self, Origin};
use egui::{Align, FontId, Layout, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use model::{Candidate, Model, Preview, Staged};
use sparksmith_core::functions::suggest::{
    MAX_SUGGESTIONS, PeripheralRequest, SuggestReport, SuggestRequest, Suggestion,
};
use sparksmith_core::ids::{ResourceId, RoleId, TerminalId};
use sparksmith_core::{ComponentInstance, Engine, Id, InstanceCommand, Operation};
#[cfg(test)]
use sparksmith_core::{Query, QueryResult};
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::ui_kit::{self, Button};
use std::collections::BTreeSet;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

/// The panel's egui id: its window, its memory and its refusal origin.
fn panel_id() -> egui::Id {
    egui::Id::unique("pin-mux")
}

/// Ranked alternatives of the last suggestion, staged over `base`.
#[derive(Clone, Debug, Default)]
struct Alternatives {
    suggestions: Vec<Suggestion>,
    index: usize,
    base: Staged,
    exhausted: bool,
}

/// A `functions.suggest` running on a worker thread, and what its result
/// is staged over.
#[derive(Clone)]
struct Pending {
    receiver: Arc<Mutex<Receiver<Result<SuggestReport, String>>>>,
    base: Staged,
    revision: u64,
}

/// The model and preview of one document revision and staging.
#[derive(Clone)]
struct Cached {
    revision: u64,
    instance: Id,
    model: Result<Arc<Model>, String>,
    preview: Option<(Staged, Result<Arc<Preview>, String>)>,
}

/// The panel's state, kept in egui memory.
#[derive(Clone, Default)]
struct State {
    /// The instance the panel is open for.
    instance: Option<Id>,
    /// The last frame the panel was drawn or opened; a panel not drawn for
    /// a frame (its instance is no longer the subject) closes.
    frame: u64,
    revision: u64,
    staged: Staged,
    /// Peripherals ticked without a placement yet: their roles are shown.
    expanded: BTreeSet<ResourceId>,
    alternatives: Option<Alternatives>,
    /// What the last suggestion could not do, as the core words it.
    blockers: Vec<String>,
    /// The way out the core found for a blocked peripheral: its wording
    /// and the edits it stages over the base.
    way_out: Option<(String, Suggestion, Staged)>,
    /// Why the last suggestion did not run.
    suggest_note: Option<String>,
    focus: Option<TerminalId>,
    all_peripherals: bool,
    all_pins: bool,
    /// The engine's refusal of the last Apply.
    refusal: Option<String>,
    cache: Option<Cached>,
    /// The suggestion being searched for.
    pending: Option<Pending>,
    /// The pointer is over the panel (or a field in it has focus): only
    /// then do Enter and Tab act on it.
    engaged: bool,
}

fn load(ctx: &egui::Context) -> State {
    ctx.data_mut(|d| d.get_temp::<State>(panel_id()).unwrap_or_default())
}

fn store(ctx: &egui::Context, state: State) {
    ctx.data_mut(|d| d.insert_temp(panel_id(), state));
}

/// Open the panel for `instance`, with `terminal` as its focus pin. The
/// instance must be the selected subject by the next frame.
pub fn reveal(ctx: &egui::Context, instance: Id, terminal: Option<TerminalId>) {
    let mut state = load(ctx);
    if state.instance != Some(instance) {
        state = State::default();
    }
    state.instance = Some(instance);
    state.frame = ctx.cumulative_frame_nr();
    if terminal.is_some() {
        state.focus = terminal;
    }
    store(ctx, state);
}

/// Whether the panel is open for `instance`.
#[cfg(test)]
pub fn is_open(ctx: &egui::Context, instance: Id) -> bool {
    let state = load(ctx);
    state.instance == Some(instance) && ctx.cumulative_frame_nr() <= state.frame + 1
}

/// The model of `instance` at the engine's revision, cached in `state`.
fn model_of(
    state: &mut State,
    engine: &Engine,
    instance: &ComponentInstance,
) -> Result<Arc<Model>, String> {
    let fresh = state
        .cache
        .as_ref()
        .is_some_and(|c| c.revision == engine.revision() && c.instance == instance.id);
    if !fresh {
        state.cache = Some(Cached {
            revision: engine.revision(),
            instance: instance.id,
            model: model::build(engine.project(), instance)
                .map(Arc::new)
                .map_err(|e| refusal::shown_error(&e)),
            preview: None,
        });
    }
    match &state.cache {
        Some(cached) => cached.model.clone(),
        None => Err("The pin assignment could not be built".into()),
    }
}

/// The preview of the staged selections, cached in `state`.
fn preview_of(
    state: &mut State,
    engine: &Engine,
    instance: &ComponentInstance,
    model: &Model,
) -> Result<Arc<Preview>, String> {
    let staged = state.staged.clone();
    let Some(cached) = state.cache.as_mut() else {
        return Err("The pin assignment could not be built".into());
    };
    if let Some((key, preview)) = &cached.preview
        && *key == staged
    {
        return preview.clone();
    }
    let preview = model::preview(engine.project(), instance, model, &staged)
        .map(Arc::new)
        .map_err(|e| refusal::shown_error(&e));
    cached.preview = Some((staged, preview.clone()));
    preview
}

/// "Pin assignment…" in the Pin functions section of an instance whose
/// component declares peripherals, with the peripherals in use.
pub fn row(ui: &mut Ui, engine: &Engine, instance: &ComponentInstance) {
    let ctx = ui.ctx().clone();
    let mut state = load(&ctx);
    if state.instance.is_some_and(|i| i != instance.id) {
        state = State::default();
    }
    let Ok(model) = model_of(&mut state, engine, instance) else {
        store(&ctx, state);
        return;
    };
    if model.peripherals.is_empty() {
        store(&ctx, state);
        return;
    }
    let used: Vec<&str> = model
        .peripherals
        .iter()
        .filter(|p| {
            model.current.active.iter().any(|a| {
                model
                    .role_of(a.terminal, a.function)
                    .is_some_and(|r| r.resource == p.id)
            })
        })
        .map(|p| p.label.as_str())
        .collect();
    let summary = match used.len() {
        0 => "No peripheral in use".to_owned(),
        n if n <= 3 => format!("{} in use", used.join(", ")),
        n => format!("{n} peripherals in use"),
    };
    if kit::summary_row(ui, &summary, Some("Pin assignment…")) {
        state.instance = Some(instance.id);
        state.frame = ctx.cumulative_frame_nr();
    }
    store(&ctx, state);
}

/// What the panel's controls asked for this frame.
#[derive(Clone, Debug, PartialEq)]
enum Action {
    Apply,
    Close,
    Tick(ResourceId, bool),
    Place(ResourceId, RoleId, Option<Candidate>),
    Suggest,
    CancelSuggest,
    StageWayOut,
    Cycle(isize),
    Focus(TerminalId),
    ShowAllPeripherals,
    ShowAllPins,
}

/// The docked pin-mux panel for `instance` when it is open; returns the
/// Apply batch.
pub fn entry(ui: &mut Ui, engine: &Engine, instance: &ComponentInstance) -> Option<Operation> {
    let ctx = ui.ctx().clone();
    let now = ctx.cumulative_frame_nr();
    let mut state = load(&ctx);
    if state.instance != Some(instance.id) || now > state.frame + 1 {
        if state.instance.is_some() {
            store(&ctx, State::default());
        }
        return None;
    }
    state.frame = now;
    if let Some(message) = refusal::claim(&ctx, Origin::Dialog(panel_id())) {
        state.refusal = Some(message);
    }
    if state.revision != engine.revision() {
        model::settle(instance, &mut state.staged);
        state.alternatives = None;
        state.pending = None;
        state.blockers.clear();
        state.way_out = None;
        state.suggest_note = None;
        if state.revision != 0 {
            state.refusal = None;
        }
        state.revision = engine.revision();
    }
    let model = match model_of(&mut state, engine, instance) {
        Ok(model) => model,
        Err(message) => {
            state.refusal = Some(message);
            store(&ctx, state);
            return None;
        }
    };
    if poll(&mut state, false) {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    let preview = preview_of(&mut state, engine, instance, &model);
    let t = Tokens::get();
    let cards = ((ctx.content_rect().width() - t.space_24 * 2.0) / t.form_card_width.max(1.0))
        .clamp(1.5, 3.5);
    let title = format!("{} · Pin assignment", model.designator);
    let mut action = None;
    let closed = ui_kit::Window::new(panel_id(), "component", &title)
        .cards(cards)
        .show_body(&ctx, |ui| {
            state.engaged = ui.rect_contains_pointer(ui.max_rect());
            kit::form(ui, |ui| {
                action = body(ui, engine, instance, &model, &preview, &state)
            });
        });
    if closed {
        action = Some(Action::Close);
    }
    let output = match (action, &preview) {
        (Some(action), Ok(preview)) => {
            handle(&mut state, engine, instance, &model, preview, action)
        }
        (Some(Action::Close), Err(_)) => {
            state = State::default();
            None
        }
        _ => None,
    };
    if output.is_some() {
        refusal::mark(&ctx, Origin::Dialog(panel_id()));
        state.refusal = None;
    }
    store(&ctx, state);
    output
}

/// Apply `action` to the panel state; returns the operation to send.
fn handle(
    state: &mut State,
    engine: &Engine,
    instance: &ComponentInstance,
    model: &Model,
    preview: &Preview,
    action: Action,
) -> Option<Operation> {
    match action {
        Action::Apply => {
            if preview.gate().is_some() {
                return None;
            }
            state.alternatives = None;
            return Some(Operation::Instance(InstanceCommand::SelectFunctions {
                instance: instance.id,
                edits: preview.edits.clone(),
            }));
        }
        Action::Close => *state = State::default(),
        Action::StageWayOut => {
            if let Some((_, suggestion, base)) = state.way_out.take() {
                state.staged = model::stage_suggestion(&base, &suggestion.edits);
                state.blockers.clear();
                state.alternatives = None;
            }
        }
        Action::CancelSuggest => {
            state.pending = None;
            state.suggest_note = Some("Suggestion cancelled".into());
        }
        Action::Tick(peripheral, true) => {
            state.alternatives = None;
            state.expanded.insert(peripheral);
            let base = state.staged.clone();
            suggest(state, engine, instance, vec![peripheral], true, base);
        }
        Action::Tick(peripheral, false) => {
            state.alternatives = None;
            state.blockers.clear();
            state.way_out = None;
            state.expanded.remove(&peripheral);
            model::disable(model, preview, peripheral, &mut state.staged);
        }
        Action::Place(peripheral, role, candidate) => {
            state.alternatives = None;
            state.expanded.insert(peripheral);
            model::place(
                model,
                preview,
                peripheral,
                role,
                candidate.as_ref(),
                &mut state.staged,
            );
            if let Some(candidate) = candidate {
                focus(state, model, candidate.terminal);
            }
        }
        Action::Suggest => {
            let enable: Vec<ResourceId> = model
                .peripherals
                .iter()
                .map(|p| p.id)
                .filter(|p| state.expanded.contains(p) || !preview.placements(model, *p).is_empty())
                .collect();
            let base = state.staged.clone();
            suggest(state, engine, instance, enable, false, base);
        }
        Action::Cycle(step) => {
            if let Some(alt) = state.alternatives.as_mut()
                && !alt.suggestions.is_empty()
            {
                let len = alt.suggestions.len() as isize;
                alt.index = (alt.index as isize + step).rem_euclid(len) as usize;
                if let Some(s) = alt.suggestions.get(alt.index) {
                    state.staged = model::stage_suggestion(&alt.base, &s.edits);
                }
            }
        }
        Action::Focus(terminal) => focus(state, model, terminal),
        Action::ShowAllPeripherals => state.all_peripherals = true,
        Action::ShowAllPins => state.all_pins = true,
    }
    None
}

/// Make `terminal` the focus pin and keep it in the shown pin list.
fn focus(state: &mut State, model: &Model, terminal: TerminalId) {
    state.focus = Some(terminal);
    if listed(model)
        .iter()
        .position(|p| p.terminal == terminal)
        .is_some_and(|i| i >= LIST_MAX_ROWS)
    {
        state.all_pins = true;
    }
}

/// Start `functions.suggest` for `enable` on a worker thread over the
/// project as it is now; [`poll`] stages its first assignment over `base`
/// when it arrives. The core bounds the search (node and rule-operation
/// budgets), so the thread always ends; a cancelled or superseded search
/// is simply not read.
fn suggest(
    state: &mut State,
    engine: &Engine,
    instance: &ComponentInstance,
    enable: Vec<ResourceId>,
    keep_existing: bool,
    base: Staged,
) {
    state.blockers.clear();
    state.way_out = None;
    state.suggest_note = None;
    state.pending = None;
    if enable.is_empty() {
        state.suggest_note = Some("Tick a peripheral to suggest pins for it".into());
        return;
    }
    let request = SuggestRequest {
        enable: enable
            .into_iter()
            .map(|peripheral| PeripheralRequest {
                peripheral,
                roles: None,
            })
            .collect(),
        keep_existing,
        avoid: Vec::new(),
        limit: Some(MAX_SUGGESTIONS),
    };
    let project = engine.project_arc().clone();
    let id = instance.id;
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("pin-mux-suggest".into())
        .spawn(move || {
            let report = sparksmith_core::pins::suggest(&project, id, &request)
                .map_err(|e| refusal::shown_error(&e));
            let _ = sender.send(report);
        });
    match spawned {
        Ok(_) => {
            state.pending = Some(Pending {
                receiver: Arc::new(Mutex::new(receiver)),
                base,
                revision: engine.revision(),
            });
        }
        Err(error) => {
            state.suggest_note = Some(format!("The suggestion could not start: {error}"));
        }
    }
}

/// Read a finished suggestion into `state`; with `wait`, block until it
/// arrives (tests). Returns whether one is still running.
fn poll(state: &mut State, wait: bool) -> bool {
    let Some(pending) = state.pending.clone() else {
        return false;
    };
    if pending.revision != state.revision && state.revision != 0 {
        state.pending = None;
        return false;
    }
    let received = match pending.receiver.lock() {
        Ok(receiver) if wait => receiver.recv().map_err(|_| TryRecvError::Disconnected),
        Ok(receiver) => receiver.try_recv(),
        Err(_) => Err(TryRecvError::Disconnected),
    };
    match received {
        Err(TryRecvError::Empty) => true,
        Err(TryRecvError::Disconnected) => {
            state.pending = None;
            state.suggest_note = Some("The suggestion stopped without a result".into());
            false
        }
        Ok(result) => {
            state.pending = None;
            receive(state, result, pending.base);
            false
        }
    }
}

/// Stage a suggestion's first assignment over `base`; the core's blockers
/// and refusals are kept for display.
fn receive(state: &mut State, result: Result<SuggestReport, String>, base: Staged) {
    match result {
        Ok(report) => {
            // The core words a blocker as the reason, then the selection
            // in the way, then a way out: one line each, so none is cut off.
            state.blockers.clear();
            state.way_out = None;
            for blocker in &report.blockers {
                let (reason, rest) = blocker
                    .message
                    .split_once(" It conflicts with ")
                    .map_or((blocker.message.as_str(), None), |(r, c)| (r, Some(c)));
                // The refusal row cuts them, with the whole text on hover.
                state
                    .blockers
                    .push(refusal::clip(reason, refusal::MAX_SHOWN));
                let Some(rest) = rest else { continue };
                let (conflict, alternative) = rest
                    .split_once(" Alternative: ")
                    .map_or((rest, None), |(c, a)| (c, Some(a)));
                state.blockers.push(refusal::clip(
                    &format!("Conflicts with {conflict}"),
                    refusal::MAX_SHOWN,
                ));
                if let (Some(text), Some(suggestion), None) =
                    (alternative, &blocker.alternative, &state.way_out)
                {
                    state.way_out = Some((
                        format!("Way out: {}", text.trim_end_matches('.')),
                        suggestion.clone(),
                        base.clone(),
                    ));
                }
            }
            if let Some(first) = report.suggestions.first() {
                state.staged = model::stage_suggestion(&base, &first.edits);
                state.alternatives = Some(Alternatives {
                    suggestions: report.suggestions.clone(),
                    index: 0,
                    base,
                    exhausted: report.exhausted,
                });
            } else if report.blockers.is_empty() {
                state.suggest_note = Some(if report.exhausted {
                    "No assignment found within the search budget".into()
                } else {
                    "No assignment fits; choose pins below".into()
                });
            }
        }
        Err(message) => state.suggest_note = Some(message),
    }
}

/// The pins the Pins list shows: multi-function pins and pins with a
/// peripheral function, in declaration order.
fn listed(model: &Model) -> Vec<&model::Pin> {
    model
        .pins
        .iter()
        .filter(|p| p.multi() || p.functions.iter().any(|f| f.role.is_some()))
        .collect()
}

/// The panel body: the three parts, the explanation lines, the
/// suggestion's status, the refusal row and the footer.
fn body(
    ui: &mut Ui,
    engine: &Engine,
    instance: &ComponentInstance,
    model: &Model,
    preview: &Result<Arc<Preview>, String>,
    state: &State,
) -> Option<Action> {
    let preview = match preview {
        Ok(preview) => preview,
        Err(message) => {
            kit::refusal(ui, message);
            return footer(ui, None, state);
        }
    };
    let t = Tokens::get();
    // The parts scroll at one fixed height, and the refusal row and the
    // footer stay pinned under them, so Apply is always reachable. The
    // height never depends on the content: at most 16 rows, and no more
    // than the window's body shows (its clip, which the window bounds).
    // A refusal row takes its height from the parts, never adds to it.
    let row = t.control_height + t.form_gap_y;
    let refused = if state.refusal.is_some() { row } else { 0.0 };
    let reserve = row * 4.0 + t.space_8 + t.space_4;
    let shown = ui.clip_rect().bottom() - ui.cursor().top();
    let body = ui.ctx().content_rect().height()
        - t.form_header_height
        - t.form_footer_height
        - t.space_24 * 2.0;
    let room = ((shown.min(body) - reserve).min(row * 16.0) - refused).max(t.control_height * 4.0);
    let action = egui::ScrollArea::vertical()
        .id_salt("pin-mux-parts")
        .max_height(room)
        .min_scrolled_height(room)
        .auto_shrink([false, false])
        .show(ui, |ui| parts(ui, engine, instance, model, preview, state))
        .inner;
    if let Some(message) = &state.refusal {
        kit::refusal(ui, message);
    }
    action.or(footer(ui, Some(preview), state))
}

/// The explanation lines, the suggestion's status and the three parts.
fn parts(
    ui: &mut Ui,
    engine: &Engine,
    instance: &ComponentInstance,
    model: &Model,
    preview: &Preview,
    state: &State,
) -> Option<Action> {
    let t = Tokens::get();
    let mut action = None;
    if model.peripherals.is_empty() {
        kit::hint(ui, "This component declares no peripherals", "");
    }
    // The explanations come first so a rejected combination is in view.
    for (n, explanation) in preview.lines.iter().enumerate() {
        let show = (!explanation.terminals.is_empty()).then_some("Show");
        if ui
            .push_id(("line", n), |ui| {
                line(ui, Some(explanation.level), &explanation.text, show)
            })
            .inner
            && let Some(first) = explanation.terminals.first()
        {
            action = Some(Action::Focus(*first));
        }
    }
    if state.pending.is_some()
        && kit::summary_row(ui, "Searching for an assignment…", Some("Cancel"))
    {
        action = Some(Action::CancelSuggest);
    }
    if let Some(alt) = &state.alternatives
        && let Some(current) = alt.suggestions.get(alt.index)
        && !current.unplaced.is_empty()
    {
        let roles: Vec<String> = current
            .unplaced
            .iter()
            .filter_map(|r| {
                let peripheral = model.peripherals.iter().find(|p| p.id == r.resource)?;
                let role = peripheral.roles.iter().find(|x| x.id == r.role)?;
                Some(format!("{} {}", peripheral.label, role.label))
            })
            .collect();
        line(
            ui,
            Some(Level::Info),
            &format!("Optional roles left out: {}", roles.join(", ")),
            None,
        );
    }
    if let Some(alt) = &state.alternatives {
        let text = format!(
            "Suggestion {} of {}{} · Tab for the next",
            alt.index + 1,
            alt.suggestions.len(),
            if alt.exhausted {
                " (search budget reached)"
            } else {
                ""
            }
        );
        if alt.suggestions.len() > 1 && kit::summary_row(ui, &text, Some("Next")) {
            action = Some(Action::Cycle(1));
        } else if alt.suggestions.len() <= 1 {
            kit::summary_row(ui, "The only suggestion is staged", None);
        }
    }
    for (n, blocker) in state.blockers.iter().enumerate() {
        ui.push_id(("blocker", n), |ui| {
            line(ui, Some(Level::Warning), blocker, None)
        });
    }
    if let Some((text, _, _)) = &state.way_out
        && line(ui, Some(Level::Info), text, Some("Stage"))
    {
        action = Some(Action::StageWayOut);
    }
    if let Some(note) = &state.suggest_note {
        line(ui, Some(Level::Info), note, None);
    }
    if !preview.lines.is_empty()
        || state.pending.is_some()
        || state.alternatives.is_some()
        || !state.blockers.is_empty()
        || state.way_out.is_some()
        || state.suggest_note.is_some()
    {
        kit::rule(ui);
    }
    let width = ui.available_width();
    let wide = width >= t.form_card_width * 2.4;
    if wide {
        let gap = t.space_16;
        let inner = (width - gap * 2.0).max(0.0);
        let widths = [inner * 0.36, inner * 0.40, inner * 0.24];
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (column, w) in widths.into_iter().enumerate() {
                ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    let got = match column {
                        0 => peripherals(ui, model, preview, state),
                        1 => pins(ui, engine, instance, model, preview, state),
                        _ => package_part(ui, model, preview, state),
                    };
                    action = action.take().or(got);
                });
            }
        });
    } else {
        action = peripherals(ui, model, preview, state);
        action = action.or(pins(ui, engine, instance, model, preview, state));
        action = action.or(package_part(ui, model, preview, state));
    }
    // Tab and Shift+Tab cycle the alternatives while the pointer is over
    // the panel and no field has focus.
    let free = state.engaged && ui.ctx().memory(|m| m.focused().is_none());
    if free
        && state
            .alternatives
            .as_ref()
            .is_some_and(|a| a.suggestions.len() > 1)
    {
        let (next, back) = ui.ctx().input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
            )
        });
        if next {
            action = Some(Action::Cycle(1));
        } else if back {
            action = Some(Action::Cycle(-1));
        }
    }
    action
}

/// The summary and Cancel / Apply. Enter applies while the pointer is over
/// the panel and no field has focus, so Enter on the canvas beside it
/// never applies.
fn footer(ui: &mut Ui, preview: Option<&Preview>, state: &State) -> Option<Action> {
    let gate = preview.map_or(Some("The preview could not be computed"), Preview::gate);
    let summary = match preview.map_or(0, |p| p.changed.len()) {
        0 if preview.is_some_and(|p| !p.edits.is_empty()) => {
            "Selections change; no active function changes".to_owned()
        }
        0 => "No changes staged".to_owned(),
        1 => "1 pin changes function; nets, pads and copper unchanged".to_owned(),
        n => format!("{n} pins change function; nets, pads and copper unchanged"),
    };
    kit::rule(ui);
    kit::summary_row(ui, &summary, None);
    let mut action = None;
    kit::actions(ui, |ui| {
        let apply = kit::primary(ui, "Apply", gate.is_none());
        let apply = match gate {
            Some(reason) => apply.on_disabled_hover_text(reason),
            None => apply,
        };
        if apply.clicked() {
            action = Some(Action::Apply);
        }
        if ui_kit::button(ui, "Cancel", Button::Normal)
            .on_hover_text("Close without applying the staged pins")
            .clicked()
        {
            action = Some(Action::Close);
        }
    });
    let enter = gate.is_none()
        && state.engaged
        && ui.ctx().memory(|m| m.focused().is_none())
        && ui
            .ctx()
            .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
    if enter {
        action = Some(Action::Apply);
    }
    action
}

/// PERIPHERALS: a tick per peripheral and, for one in use or ticked, a
/// choice per role among the pins that declare it.
fn peripherals(ui: &mut Ui, model: &Model, preview: &Preview, state: &State) -> Option<Action> {
    let mut action = None;
    let count = model.peripherals.len();
    if count == 0 {
        return None;
    }
    kit::section(
        ui,
        "pin-mux-peripherals",
        "Peripherals",
        true,
        Some(count),
        |ui| {
            let max = if state.all_peripherals {
                count
            } else {
                LIST_MAX_ROWS
            };
            let listed = kit::list_frame(ui, count, max, |ui, index| {
                let Some(peripheral) = model.peripherals.get(index) else {
                    return;
                };
                let placed = preview.placements(model, peripheral.id);
                let mut on = !placed.is_empty() || state.expanded.contains(&peripheral.id);
                let names: Vec<&str> = placed
                    .iter()
                    .filter_map(|(_, t)| model.pin(*t).map(|p| p.name.as_str()))
                    .collect();
                let label = if names.is_empty() {
                    peripheral.label.clone()
                } else {
                    format!("{} · {}", peripheral.label, names.join(", "))
                };
                if kit::check_row(ui, &label, &mut on) {
                    action = Some(Action::Tick(peripheral.id, on));
                }
                if !on {
                    return;
                }
                for role in &peripheral.roles {
                    let mut value =
                        placed
                            .iter()
                            .find(|(r, _)| *r == role.id)
                            .and_then(|(_, terminal)| {
                                let active = preview.resolution.active(*terminal)?;
                                Some(Candidate {
                                    terminal: *terminal,
                                    function: active.function,
                                })
                            });
                    let none = if role.required { "— required" } else { "—" };
                    let mut options = vec![(None, none.to_owned())];
                    options.extend(
                        role.candidates
                            .iter()
                            .map(|c| (Some(c.clone()), model::candidate_label(model, c))),
                    );
                    let caption = if role.label.is_empty() {
                        "Role".to_owned()
                    } else {
                        role.label.clone()
                    };
                    if ui
                        .push_id(("role", role.id), |ui| {
                            kit::choice_row(ui, &caption, &mut value, &options)
                        })
                        .inner
                    {
                        action = Some(Action::Place(peripheral.id, role.id, value));
                    }
                }
            });
            if listed.show_all {
                action = Some(Action::ShowAllPeripherals);
            }
            kit::actions(ui, |ui| {
                if ui_kit::button(ui, "Suggest assignment", Button::Normal)
                    .on_hover_text("Choose pins for every ticked peripheral")
                    .clicked()
                {
                    action = Some(Action::Suggest);
                }
            });
        },
    );
    action
}

/// PINS: each listed pin with its previewed function, mux code and net.
fn pins(
    ui: &mut Ui,
    engine: &Engine,
    instance: &ComponentInstance,
    model: &Model,
    preview: &Preview,
    state: &State,
) -> Option<Action> {
    let t = Tokens::get();
    let listed = listed(model);
    let mut action = None;
    kit::section(ui, "pin-mux-pins", "Pins", true, Some(listed.len()), |ui| {
        if listed.is_empty() {
            kit::hint(ui, "No pin has a choice of function", "");
            return;
        }
        let max = if state.all_pins {
            listed.len()
        } else {
            LIST_MAX_ROWS
        };
        let frame = kit::list_frame(ui, listed.len(), max, |ui, index| {
            let Some(pin) = listed.get(index) else {
                return;
            };
            let active = preview.resolution.active(pin.terminal);
            let function = active.and_then(|a| pin.function(a.function));
            let mut text = pin.name.clone();
            match function {
                Some(f) => {
                    text.push_str(" · ");
                    text.push_str(&f.label);
                    if let Some(mux) = &f.mux {
                        text.push_str(" · ");
                        text.push_str(mux);
                    }
                }
                None => text.push_str(" · unresolved"),
            }
            if let Some(net) = model::net_line(engine.project(), instance, pin.terminal, 2) {
                text.push_str(" · ");
                text.push_str(&net);
            }
            let changed = preview.changed.contains(&pin.terminal);
            let tint = preview
                .role(model, pin.terminal)
                .and_then(|r| model.tint_index(r.resource))
                .map(package::tint);
            let focused = state.focus == Some(pin.terminal);
            let pads = model.pads_of(pin.terminal);
            let tip = if pads.is_empty() {
                text.clone()
            } else {
                format!("{text} · pad {}", pads.join(", "))
            };
            let (rect, response) = ui.allocate_exact_size(
                vec2(ui.available_width().max(0.0), t.control_height),
                Sense::click(),
            );
            let painter = ui.painter_at(rect);
            if focused {
                painter.rect_filled(rect, t.radius_sm, t.selected_bg);
            } else if response.hovered() {
                painter.rect_filled(rect, t.radius_sm, t.hover);
            }
            let swatch = Rect::from_center_size(
                pos2(rect.left() + t.space_6, rect.center().y),
                vec2(t.space_8, t.space_8),
            );
            match tint {
                Some(color) => {
                    painter.rect_filled(swatch, 2.0, color);
                }
                None => {
                    painter.rect_stroke(
                        swatch,
                        2.0,
                        t.control_border_stroke(t.form_dim),
                        egui::StrokeKind::Inside,
                    );
                }
            }
            let ink = if changed { t.form_primary } else { t.form_ink };
            let shown = if changed {
                format!("{text}  ●")
            } else {
                text
            };
            let galley = painter.layout_no_wrap(shown, FontId::proportional(t.font_label), ink);
            painter.galley(
                pos2(
                    rect.left() + t.space_16,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                ink,
            );
            if response.clicked() {
                action = Some(Action::Focus(pin.terminal));
            }
            response.on_hover_text(tip);
        });
        if frame.show_all {
            action = Some(Action::ShowAllPins);
        }
    });
    action
}

/// PACKAGE: the footprint's pads tinted by peripheral; a click focuses a
/// pin.
fn package_part(ui: &mut Ui, model: &Model, preview: &Preview, state: &State) -> Option<Action> {
    let mut action = None;
    kit::section(ui, "pin-mux-package", "Package", true, None, |ui| {
        if !model.physical {
            kit::hint(ui, "Choose a footprint to see the package", "");
            return;
        }
        if model.pads.is_empty() {
            kit::hint(ui, "The footprint has no pads", "");
            return;
        }
        let max = Tokens::get().form_card_width * 0.8;
        if let Some(terminal) = package::show(ui, model, preview, state.focus, max) {
            action = Some(Action::Focus(terminal));
        }
    });
    action
}

/// One clipped body line with an optional severity glyph and action (the
/// review's line); returns whether the action was clicked.
fn line(ui: &mut Ui, level: Option<Level>, text: &str, action: Option<&str>) -> bool {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), t.control_height),
        Sense::hover(),
    );
    let mut left = rect.left();
    if let Some(level) = level {
        severity_glyph(
            ui.painter(),
            pos2(left + t.font_label / 2.0, rect.center().y),
            t.font_label,
            level,
        );
        left += t.font_label + t.space_8;
    }
    let mut right = rect.right();
    let mut clicked = false;
    if let Some(action) = action {
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("line_action", action))
                .max_rect(rect)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                let button = ui_kit::button(ui, action, Button::Enabled(true));
                clicked = button.clicked();
                right = button.rect.left() - t.space_8;
            },
        );
    }
    let area = Rect::from_min_max(pos2(left, rect.top()), pos2(right.max(left), rect.bottom()));
    let painter = ui.painter_at(area);
    let galley = painter.layout_no_wrap(
        text.to_owned(),
        FontId::proportional(t.font_label),
        t.form_ink,
    );
    let cut = galley.size().x > area.width() + 0.5;
    painter.galley(
        pos2(area.left(), area.center().y - galley.size().y / 2.0),
        galley,
        t.form_ink,
    );
    if cut {
        response.on_hover_text(text);
    }
    clicked
}

#[cfg(test)]
mod tests;
