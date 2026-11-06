//! Slice C5: the pin table (one row per terminal, function chips tinted by
//! peripheral, filters), the Peripherals view ("where can USART1 TX go")
//! and Paste table… (column mapping, then `library.pinout_preview`).
//!
//! Everything here reads explicit data. A pasted table becomes a typed
//! [`PinoutDocument`] only through the column mapping the person chose:
//! a function cell is split into peripheral and role only at the
//! separator they typed, and nothing is inferred from names.
use crate::kit;
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, Ui, pos2, vec2};
use pcbcraft_core::functions::{
    Direction, Drive, Function, GpioRef, SignalKind, SourceSpan, TerminalFunctions,
};
use pcbcraft_core::ids::MAX_LABEL;
use pcbcraft_core::library::pinout::{
    ImportReview, MAX_PINOUT_PINS, PINOUT_FORMAT, PinoutClass, PinoutDevice, PinoutDocument,
    PinoutFunction, PinoutOptions, PinoutPeripheral, PinoutPin, PinoutReference, PinoutRole,
    PinoutVariant,
};
use pcbcraft_core::library::{ComponentDefinition, Terminal, TerminalClass};
use pcbcraft_core::{FunctionId, ResourceId, RoleId, TerminalId};
use solidcraft_ui_egui::{fonts, theme::Tokens, ui_kit};
use std::collections::BTreeMap;

/// Which rows the table shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    /// Terminals with more than one function.
    Multi,
    /// Rows an import created that nobody confirmed, or flagged uncertain.
    Unreviewed,
    /// Terminals with a power function.
    Power,
    /// Device-NC and reserved terminals.
    NotConnected,
}

impl Filter {
    pub const ALL: [Filter; 5] = [
        Filter::All,
        Filter::Multi,
        Filter::Unreviewed,
        Filter::Power,
        Filter::NotConnected,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Multi => "Multi-function",
            Filter::Unreviewed => "Unreviewed",
            Filter::Power => "Power",
            Filter::NotConnected => "NC",
        }
    }
}

/// The table's filter and query.
#[derive(Default)]
pub struct PinsTable {
    pub filter: Filter,
    pub query: String,
    /// The open Paste table… flow.
    pub paste: Option<PasteFlow>,
}

/// Bytes of the filter text.
pub const MAX_QUERY: usize = 128;

/// Whether `id` waits for a person's review (F8).
pub fn unreviewed(review: Option<&ImportReview>, id: TerminalId) -> bool {
    review.is_some_and(|r| r.unreviewed.contains(&id) || r.uncertain.contains(&id))
}

/// How many terminals wait for review.
pub fn unreviewed_count(component: &ComponentDefinition) -> usize {
    component
        .terminals
        .iter()
        .filter(|t| unreviewed(component.review.as_deref(), t.id))
        .count()
}

fn is_power(functions: &TerminalFunctions) -> bool {
    functions.functions.iter().any(|f| {
        matches!(f.direction, Direction::PowerInput | Direction::PowerOutput)
            || f.signal == SignalKind::Power
    })
}

/// Whether `terminal` passes `filter` and the case-insensitive `query`
/// over its name, function labels, mux codes and peripheral labels.
pub fn matches(
    component: &ComponentDefinition,
    terminal: &Terminal,
    filter: Filter,
    query: &str,
) -> bool {
    let functions = terminal.functions();
    let kept = match filter {
        Filter::All => true,
        Filter::Multi => functions.is_some_and(|f| f.functions.len() > 1),
        Filter::Unreviewed => unreviewed(component.review.as_deref(), terminal.id),
        Filter::Power => functions.is_some_and(is_power),
        Filter::NotConnected => !terminal.is_electrical(),
    };
    let query = query.trim().to_lowercase();
    if !kept || query.is_empty() {
        return kept;
    }
    let hit = |text: &str| text.to_lowercase().contains(&query);
    hit(&terminal.name)
        || functions.is_some_and(|f| {
            f.functions.iter().any(|function| {
                hit(&function.label)
                    || function.mux.as_ref().is_some_and(|m| hit(&m.0))
                    || peripheral_of(component, function).is_some_and(|(p, _)| hit(p))
            })
        })
}

/// The peripheral and role labels a function takes, if any.
pub fn peripheral_of<'a>(
    component: &'a ComponentDefinition,
    function: &Function,
) -> Option<(&'a str, &'a str)> {
    let rr = function.resource_role?;
    let resource = component.resources.iter().find(|r| r.id == rr.resource)?;
    let role = resource.role(rr.role)?;
    Some((&resource.label, &role.label))
}

/// The chip fill of a function: its peripheral's categorical tint from
/// the kit's tab palette, or the control fill when it takes none.
pub fn chip_fill(component: &ComponentDefinition, function: &Function) -> Color32 {
    let t = Tokens::get();
    let palette = [
        t.accent_soft,
        t.sketch_soft,
        t.surface_soft,
        t.mesh_soft,
        t.form_soft,
        t.render_soft,
    ];
    function
        .resource_role
        .and_then(|rr| component.resources.iter().position(|r| r.id == rr.resource))
        .and_then(|index| palette.get(index % palette.len()).copied())
        .unwrap_or(t.control_fill)
}

/// A function's chip text: its label and mux code.
pub fn chip_text(function: &Function) -> String {
    match function.mux_text() {
        Some(mux) => format!("{} {mux}", function.label),
        None => function.label.clone(),
    }
}

/// How a function's direction and signal read: "Bidirectional digital",
/// and only "Power input" or "Passive" where the signal adds nothing. A
/// ground pin is a power input in the model: it reads as one.
pub fn kind_text(direction: Direction, signal: SignalKind) -> String {
    let mut chars = direction.label().chars();
    let name: String = chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    match signal {
        SignalKind::Digital => format!("{name} digital"),
        SignalKind::Analog => format!("{name} analog"),
        SignalKind::Power | SignalKind::Unspecified => name,
    }
}

/// A terminal's type as the table shows it: what its class says when it
/// has no functions (`TerminalClass`: device NC, reserved), else the
/// direction its functions have ("Power input"), or "Mixed" when they
/// differ. The class "Electrical" alone would tell the rows nothing apart.
pub fn type_text(terminal: &Terminal) -> String {
    let functions = match &terminal.class {
        TerminalClass::Electrical(functions) => functions,
        TerminalClass::DeviceNc => return "Device NC".to_owned(),
        TerminalClass::Reserved => return "Reserved".to_owned(),
    };
    let mut directions = functions.functions.iter().map(|f| f.direction);
    match directions.next() {
        None => "Electrical".to_owned(),
        Some(first) if directions.all(|d| d == first) => kind_text(first, SignalKind::Unspecified),
        Some(_) => "Mixed".to_owned(),
    }
}

/// A terminal's GPIO identity as stated ("A·9"), or a dash.
pub fn gpio_text(terminal: &Terminal) -> String {
    terminal
        .functions()
        .and_then(|f| f.gpio.as_ref())
        .map_or_else(|| "—".to_owned(), |g| format!("{}·{}", g.port, g.index))
}

/// Pad numbers by electrical terminal, through the binding.
pub fn pads_by_terminal(set: &super::working_set::WorkingSet) -> BTreeMap<TerminalId, Vec<String>> {
    let mut out: BTreeMap<TerminalId, Vec<String>> = BTreeMap::new();
    for physical in &set.package.terminals {
        let Some(electrical) = set.binding.terminal_of(physical.id) else {
            continue;
        };
        let entry = out.entry(electrical).or_default();
        entry.extend(
            set.footprint
                .pads
                .iter()
                .filter(|p| p.terminal == Some(physical.id))
                .map(|p| p.number.clone()),
        );
    }
    out
}

/// What a click on the table or the Peripherals view asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableEvent {
    Terminal(TerminalId),
    Function(TerminalId, FunctionId),
    AddFunction(TerminalId),
    Peripheral(ResourceId),
}

/// What is selected, for highlighting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lit {
    pub terminal: Option<TerminalId>,
    pub function: Option<FunctionId>,
    pub peripheral: Option<ResourceId>,
}

const COLUMNS: [(&str, f32); 7] = [
    ("#", 36.0),
    ("Name", 110.0),
    ("Type", 84.0),
    ("GPIO", 56.0),
    ("Functions (default bold)", 0.0),
    ("Pads", 84.0),
    ("Src", 44.0),
];

/// Column widths for `width`: fixed columns, the functions column takes
/// the rest. Below the natural width every column shrinks in proportion,
/// the functions column keeping a share of at least 120 points, so the
/// row never runs past `width`.
pub fn widths(width: f32) -> [f32; 7] {
    const MIN_FUNCTIONS: f32 = 120.0;
    let width = width.max(0.0);
    let fixed: f32 = COLUMNS.iter().map(|(_, w)| w).sum();
    let scale = if width >= fixed + MIN_FUNCTIONS {
        1.0
    } else {
        width / (fixed + MIN_FUNCTIONS)
    };
    let mut out = [0.0; 7];
    for (slot, (_, w)) in out.iter_mut().zip(COLUMNS) {
        *slot = w * scale;
    }
    if let Some(functions) = out.get_mut(4) {
        *functions = (width - fixed * scale).max(0.0);
    }
    out
}

/// One cell's text on one line, cut with "…" and then shown in full on
/// hover (`kit::text`).
fn paint_cell(ui: &Ui, rect: Rect, text: &str, font: FontId, ink: Color32) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let galley = crate::kit::text::one_line(ui, text, font, ink, rect.width());
    let cut = galley.elided;
    painter.galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    if cut {
        let id = ui.scope_id().with((
            "pin-cell",
            text,
            rect.left().to_bits(),
            rect.top().to_bits(),
        ));
        ui.interact(rect, id, Sense::hover()).on_hover_text(text);
    }
}

/// The pin table over `rows` (indices into the terminals), with a header
/// of column captions; rows scroll, drawing only the visible ones.
pub fn table(
    ui: &mut Ui,
    component: &ComponentDefinition,
    pads: &BTreeMap<TerminalId, Vec<String>>,
    rows: &[usize],
    lit: Lit,
) -> Option<TableEvent> {
    let t = Tokens::get();
    let row_height = t.control_height + t.space_4;
    let gap = t.space_6;
    let width = ui.available_width().max(0.0);
    let widths = widths(width - gap * COLUMNS.len() as f32);
    let (header, _) = ui.allocate_exact_size(vec2(width, row_height), Sense::hover());
    let mut x = header.left();
    for ((title, _), w) in COLUMNS.iter().zip(widths) {
        let caption = Rect::from_min_size(pos2(x, header.top()), vec2(w, header.height()));
        paint_cell(
            ui,
            caption,
            &title.to_uppercase(),
            fonts::semibold(t.font_caption),
            t.form_dim,
        );
        x += w + gap;
    }
    ui.painter().hline(
        header.x_range(),
        header.bottom(),
        Stroke::new(1.0, t.dialog_rule),
    );
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("pins-table")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, rows.len(), |ui, range| {
            for index in range {
                let Some(terminal) = rows.get(index).and_then(|i| component.terminals.get(*i))
                else {
                    continue;
                };
                let (rect, response) = ui.allocate_exact_size(
                    vec2(ui.available_width().max(0.0), row_height),
                    Sense::click(),
                );
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                if lit.terminal == Some(terminal.id) && lit.function.is_none() {
                    ui.painter().rect_filled(rect, t.radius_sm, t.selected_bg);
                } else if response.hovered() {
                    ui.painter().rect_filled(rect, t.radius_sm, t.hover);
                }
                if response.clicked() {
                    event = Some(TableEvent::Terminal(terminal.id));
                }
                if let Some(e) = row(ui, rect, component, terminal, index, pads, &widths, lit) {
                    event = Some(e);
                }
            }
        });
    event
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut Ui,
    rect: Rect,
    component: &ComponentDefinition,
    terminal: &Terminal,
    index: usize,
    pads: &BTreeMap<TerminalId, Vec<String>>,
    widths: &[f32; 7],
    lit: Lit,
) -> Option<TableEvent> {
    let t = Tokens::get();
    let gap = t.space_6;
    let font = FontId::proportional(t.font_label);
    let mut cells = Vec::with_capacity(widths.len());
    let mut x = rect.left();
    for w in widths {
        cells.push(Rect::from_min_size(
            pos2(x, rect.top()),
            vec2(*w, rect.height()),
        ));
        x += w + gap;
    }
    let cell = |i: usize| cells.get(i).copied().unwrap_or(Rect::NOTHING);
    paint_cell(
        ui,
        cell(0),
        &(index + 1).to_string(),
        font.clone(),
        t.form_dim,
    );
    paint_cell(ui, cell(1), &terminal.name, font.clone(), t.form_ink);
    paint_cell(ui, cell(2), &type_text(terminal), font.clone(), t.form_dim);
    paint_cell(ui, cell(3), &gpio_text(terminal), font.clone(), t.form_dim);
    let pad_text = pads
        .get(&terminal.id)
        .filter(|p| !p.is_empty())
        .map_or_else(|| "—".to_owned(), |p| p.join(", "));
    paint_cell(ui, cell(5), &pad_text, font.clone(), t.form_dim);
    let source = terminal
        .functions()
        .and_then(|f| f.functions.iter().find_map(|f| f.source.as_ref()))
        .and_then(|s| s.page)
        .map_or_else(String::new, |page| format!("p.{page}"));
    let src = cell(6);
    paint_cell(ui, src, &source, font.clone(), t.form_dim);
    if let Some(review) = component.review.as_deref() {
        let ink = if review.uncertain.contains(&terminal.id) {
            Some(t.warning)
        } else if review.unreviewed.contains(&terminal.id) {
            Some(t.accent)
        } else {
            None
        };
        if let Some(ink) = ink {
            ui.painter()
                .circle_filled(pos2(src.right() - t.space_6, src.center().y), 3.5, ink);
        }
    }
    chips(ui, cell(4), component, terminal, lit)
}

/// The function chips of one row, clipped to the cell with "+n" for the
/// rest, then "+" to add a function.
fn chips(
    ui: &mut Ui,
    cell: Rect,
    component: &ComponentDefinition,
    terminal: &Terminal,
    lit: Lit,
) -> Option<TableEvent> {
    let t = Tokens::get();
    let functions = terminal.functions()?;
    let height = t.control_height - t.space_6;
    let pad = t.space_6;
    let mut x = cell.left();
    let mut event = None;
    let plus_width = height + t.space_2;
    let more_width = 28.0;
    let can_add = functions.functions.len() < pcbcraft_core::functions::MAX_FUNCTIONS;
    for (index, function) in functions.functions.iter().enumerate() {
        let bold = functions.design_default == Some(function.id);
        let font = if bold {
            fonts::semibold(t.font_small)
        } else {
            FontId::proportional(t.font_small)
        };
        let text = chip_text(function);
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), font.clone(), t.form_ink);
        let width = (galley.size().x + pad * 2.0).min(cell.width());
        let reserve = plus_width + more_width;
        let last = index + 1 == functions.functions.len();
        if x + width > cell.right() - if last { plus_width } else { reserve } && index > 0 {
            let rest = functions.functions.len() - index;
            paint_cell(
                ui,
                Rect::from_min_max(pos2(x, cell.top()), pos2(x + more_width, cell.bottom())),
                &format!("+{rest}"),
                FontId::proportional(t.font_small),
                t.form_dim,
            );
            x += more_width;
            break;
        }
        let rect =
            Rect::from_min_size(pos2(x, cell.center().y - height / 2.0), vec2(width, height));
        let id = ui.scope_id().with(("chip", terminal.id.0, function.id.0));
        let response = ui
            .interact(rect, id, Sense::click())
            .on_hover_text(chip_tip(component, function, bold))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
        let selected = lit.terminal == Some(terminal.id) && lit.function == Some(function.id);
        let fill = chip_fill(component, function);
        let painter = ui.painter().with_clip_rect(cell.intersect(ui.clip_rect()));
        painter.rect_filled(rect, height / 2.0, fill);
        let stroke = if selected {
            Stroke::new(1.5, t.accent)
        } else if response.hovered() {
            Stroke::new(1.0, t.form_ink)
        } else {
            Stroke::new(1.0, t.control_border)
        };
        painter.rect_stroke(rect, height / 2.0, stroke, egui::StrokeKind::Inside);
        let text_rect = rect.shrink2(vec2(pad, 0.0));
        paint_cell(ui, text_rect, &text, font, t.form_ink);
        if response.clicked() {
            event = Some(TableEvent::Function(terminal.id, function.id));
        }
        x += width + t.space_4;
    }
    if can_add && x + plus_width <= cell.right() + 0.5 {
        let rect = Rect::from_min_size(
            pos2(x, cell.center().y - height / 2.0),
            vec2(height, height),
        );
        let response = ui
            .interact(rect, ui.scope_id().with(("add", terminal.id.0)), Sense::click())
            .on_hover_text("Add a function to this terminal")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, true, "Add function")
        });
        let ink = if response.hovered() {
            t.form_ink
        } else {
            t.form_dim
        };
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "+",
            FontId::proportional(t.font_label),
            ink,
        );
        if response.clicked() {
            event = Some(TableEvent::AddFunction(terminal.id));
        }
    }
    event
}

fn chip_tip(component: &ComponentDefinition, function: &Function, default: bool) -> String {
    let mut parts = vec![format!(
        "{} · {}",
        function.label,
        super::sentence(function.direction.label())
    )];
    if let Some((peripheral, role)) = peripheral_of(component, function) {
        parts.push(format!("{peripheral} {role}"));
    }
    if default {
        parts.push("design default".into());
    }
    parts.join(" · ")
}

/// A role of a peripheral, as a map key.
pub type RoleKey = (ResourceId, RoleId);

/// The functions that offer each role of each peripheral, in terminal
/// order: (terminal, function) pairs by (peripheral, role).
pub fn offers(component: &ComponentDefinition) -> BTreeMap<RoleKey, Vec<(TerminalId, FunctionId)>> {
    let mut out: BTreeMap<RoleKey, Vec<(TerminalId, FunctionId)>> = BTreeMap::new();
    for terminal in &component.terminals {
        let Some(functions) = terminal.functions() else {
            continue;
        };
        for function in &functions.functions {
            if let Some(rr) = function.resource_role {
                out.entry((rr.resource, rr.role))
                    .or_default()
                    .push((terminal.id, function.id));
            }
        }
    }
    out
}

/// The Peripherals view: each peripheral with its kind and roles, and the
/// functions that offer each role across terminals.
pub fn peripherals(ui: &mut Ui, component: &ComponentDefinition, lit: Lit) -> Option<TableEvent> {
    let t = Tokens::get();
    let offers = offers(component);
    let mut event = None;
    egui::ScrollArea::vertical()
        .id_salt("pins-peripherals")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            kit::form(ui, |ui| {
                for resource in &component.resources {
                    ui.push_id(("peripheral", resource.id.0), |ui| {
                        let kind = resource.kind.as_deref().unwrap_or("no kind");
                        let secondary = format!(
                            "{kind} · {} {}",
                            resource.roles.len(),
                            if resource.roles.len() == 1 {
                                "role"
                            } else {
                                "roles"
                            }
                        );
                        let row = kit::Row {
                            icon: "component",
                            label: &resource.label,
                            secondary: &secondary,
                            selected: lit.peripheral == Some(resource.id),
                            tip: "Edit the peripheral",
                            ..Default::default()
                        };
                        if kit::tree_row(ui, ui.scope_id().with("head"), &row).clicked {
                            event = Some(TableEvent::Peripheral(resource.id));
                        }
                        for role in &resource.roles {
                            let key = (resource.id, role.id);
                            let offered = offers.get(&key).map(Vec::as_slice).unwrap_or(&[]);
                            if let Some(e) = role_line(ui, component, resource, role, offered) {
                                event = Some(e);
                            }
                        }
                        kit::rule(ui);
                    });
                }
                if component.resources.is_empty() {
                    kit::hint(
                        ui,
                        "No peripherals yet: Add peripheral groups functions",
                        "A peripheral is one hardware instance, such as USART1, with roles",
                    );
                }
                let _ = t;
            });
        });
    event
}

fn role_line(
    ui: &mut Ui,
    component: &ComponentDefinition,
    resource: &pcbcraft_core::functions::Resource,
    role: &pcbcraft_core::functions::Role,
    offered: &[(TerminalId, FunctionId)],
) -> Option<TableEvent> {
    let mut event = None;
    let mut caption = role.label.clone();
    if let Some(signal) = &role.signal {
        caption.push_str(&format!(" ({signal})"));
    }
    if role.capacity != 1 {
        caption.push_str(&format!(" ×{}", role.capacity));
    }
    if role.required_when_used {
        caption.push_str(" · required");
    }
    kit::row(ui, &caption, |ui| {
        if offered.is_empty() {
            kit::note(ui, "No function offers this role");
            return;
        }
        ui.horizontal(|ui| {
            for (terminal, function) in offered {
                let Some(found) = component
                    .terminal(*terminal)
                    .and_then(|t| Some((t, t.functions()?.function(*function)?)))
                else {
                    continue;
                };
                let label = format!("{} {}", found.0.name, chip_text(found.1));
                let clicked = ui_kit::button(ui, label, ui_kit::Button::Small)
                    .on_hover_text(format!("Edit {}'s function", found.0.name))
                    .clicked();
                if clicked {
                    event = Some(TableEvent::Function(*terminal, *function));
                }
            }
        });
    });
    let _ = resource;
    event
}

// ---------------------------------------------------------------------
// Paste table…

/// Bytes of pasted text.
pub const MAX_PASTE: usize = 1024 * 1024;
/// Columns of a pasted table.
pub const MAX_PASTE_COLUMNS: usize = 64;

/// How cells are separated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delimiter {
    /// Tab when any line holds one, else semicolon, else comma.
    #[default]
    Auto,
    Tab,
    Comma,
    Semicolon,
}

impl Delimiter {
    pub const ALL: [(Delimiter, &'static str); 4] = [
        (Delimiter::Auto, "Detect"),
        (Delimiter::Tab, "Tab"),
        (Delimiter::Comma, "Comma"),
        (Delimiter::Semicolon, "Semicolon"),
    ];

    fn resolve(self, text: &str) -> char {
        match self {
            Delimiter::Tab => '\t',
            Delimiter::Comma => ',',
            Delimiter::Semicolon => ';',
            Delimiter::Auto if text.contains('\t') => '\t',
            Delimiter::Auto if text.contains(';') => ';',
            Delimiter::Auto => ',',
        }
    }
}

/// What one pasted column holds, as the person maps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColumnMap {
    #[default]
    Ignore,
    /// The package designation ("12").
    Number,
    /// The terminal name ("PA9").
    Name,
    /// One function per cell (or several, at the list separator).
    Function,
    /// The label of the design-default function.
    Default,
    /// The mux (alternate-function) codes of the row's functions, in the
    /// same order, split at the list separator ("AF7").
    Mux,
    /// The pin's type: power in or out, ground, input, output, I/O, open
    /// drain, passive, analog, NC or reserved ([`pin_type`]); power and
    /// ground rows get power functions. An unknown type imports as
    /// passive, flagged uncertain.
    Type,
    /// The GPIO port, stated as data ("A").
    Port,
    /// The GPIO index within its port ("9").
    Index,
}

impl ColumnMap {
    pub const ALL: [(ColumnMap, &'static str); 9] = [
        (ColumnMap::Ignore, "Ignore"),
        (ColumnMap::Number, "Number"),
        (ColumnMap::Name, "Name"),
        (ColumnMap::Function, "Function"),
        (ColumnMap::Default, "Default"),
        (ColumnMap::Mux, "Mux / AF"),
        (ColumnMap::Type, "Pin type"),
        (ColumnMap::Port, "GPIO port"),
        (ColumnMap::Index, "GPIO index"),
    ];
}

/// What a Pin type cell says about its row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinType {
    /// Electrical, with every function of this direction, signal and drive.
    Electrical(Direction, SignalKind, Drive),
    /// Electrical with the mapping's direction, signal analog.
    Analog,
    /// Electrical with the mapping's direction and signal.
    Unstated,
    DeviceNc,
    Reserved,
}

/// The direction, signal and drive of a pin type the cell did not state
/// in words [`pin_type`] knows: passive, which drives nothing.
pub const NEUTRAL: (Direction, SignalKind, Drive) =
    (Direction::Passive, SignalKind::Unspecified, Drive::None);

/// The drive a function of `direction` gets unless its type says
/// otherwise: push-pull for outputs and bidirectional pins (the usual
/// GPIO), none for the rest, so every import is a valid combination.
pub fn default_drive(direction: Direction) -> Drive {
    match direction {
        Direction::Output | Direction::Bidirectional => Drive::PushPull,
        _ => Drive::None,
    }
}

/// Read a Pin type cell, ignoring case, spacing, `-` and `_`: power
/// (`power`, `power input`, `power in`, `supply`, `pwr`, `ground`,
/// `gnd`, `vss`, `vdd`), `power output`/`power out`, `input`/`in`,
/// `output`/`out`, `i/o`/`io`/`bidirectional`/`bidir`, `open drain`/
/// `open collector` (output, open-drain), `passive`, `analog`, `nc`/
/// `no connect`, `reserved`/`rsv`, or empty. The person maps the column;
/// the text is a stated pin type, not a name to guess from.
pub fn pin_type(text: &str) -> Option<PinType> {
    let text = text
        .to_ascii_lowercase()
        .replace(['-', '_'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let electrical =
        |direction| PinType::Electrical(direction, SignalKind::Digital, default_drive(direction));
    Some(match text.as_str() {
        "" => PinType::Unstated,
        "power" | "power input" | "power in" | "pwr" | "pwr in" | "supply" | "supply input"
        | "s" | "p" | "ground" | "gnd" => {
            PinType::Electrical(Direction::PowerInput, SignalKind::Power, Drive::None)
        }
        "power output" | "power out" | "pwr out" | "supply output" => {
            PinType::Electrical(Direction::PowerOutput, SignalKind::Power, Drive::None)
        }
        "input" | "in" | "i" | "digital input" | "digital in" => electrical(Direction::Input),
        "output" | "out" | "o" | "digital output" | "digital out" | "push pull" => {
            electrical(Direction::Output)
        }
        "i/o" | "io" | "bidirectional" | "bidir" | "bi" | "b" | "inout" | "in/out" => {
            electrical(Direction::Bidirectional)
        }
        "open drain" | "open collector" | "od" | "oc" => {
            PinType::Electrical(Direction::Output, SignalKind::Digital, Drive::OpenDrain)
        }
        "passive" => PinType::Electrical(NEUTRAL.0, NEUTRAL.1, NEUTRAL.2),
        "analog" | "analogue" | "a" => PinType::Analog,
        "nc" | "n/c" | "not connected" | "no connect" | "no connection" => PinType::DeviceNc,
        "reserved" | "rsv" => PinType::Reserved,
        _ => return None,
    })
}

/// Split `text` into rows of cells at `delimiter`, honouring double quotes
/// (`""` inside quotes is one quote). Blank lines are skipped. Bounded by
/// [`MAX_PASTE`] bytes, [`MAX_PINOUT_PINS`] + 1 rows and
/// [`MAX_PASTE_COLUMNS`] columns.
pub fn split_table(text: &str, delimiter: Delimiter) -> Result<Vec<Vec<String>>, String> {
    if text.len() > MAX_PASTE {
        return Err("The pasted text is larger than 1 MiB".into());
    }
    let delimiter = delimiter.resolve(text);
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if rows.len() > MAX_PINOUT_PINS {
            return Err(format!("A table has at most {MAX_PINOUT_PINS} pin rows"));
        }
        let mut cells = Vec::new();
        let mut cell = String::new();
        let mut quoted = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    cell.push('"');
                    chars.next();
                }
                '"' if quoted => quoted = false,
                '"' if cell.trim().is_empty() => {
                    cell.clear();
                    quoted = true;
                }
                c if c == delimiter && !quoted => cells.push(std::mem::take(&mut cell)),
                c => cell.push(c),
            }
        }
        cells.push(cell);
        if cells.len() > MAX_PASTE_COLUMNS {
            return Err(format!("A table has at most {MAX_PASTE_COLUMNS} columns"));
        }
        rows.push(cells.into_iter().map(|c| c.trim().to_owned()).collect());
    }
    Ok(rows)
}

/// The mapping a person chose for a pasted table.
#[derive(Clone, Debug, PartialEq)]
pub struct Mapping {
    pub columns: Vec<ColumnMap>,
    /// The first row holds captions.
    pub header: bool,
    /// Cells of a function column hold several functions split at this
    /// text; empty for one function per cell.
    pub list_separator: String,
    /// A function cell is "peripheral<sep>role" split at the first
    /// occurrence of this text; empty for no peripheral.
    pub role_separator: String,
    /// A function cell is "label<sep>mux" split at the last occurrence of
    /// this text ("USART1_TX:AF7" at ":"); empty for no mux code in the
    /// cell.
    pub mux_separator: String,
    pub direction: Direction,
    pub signal: SignalKind,
    /// The datasheet the table comes from (required provenance).
    pub source_title: String,
    pub source_revision: String,
    pub page: String,
}

impl Default for Mapping {
    fn default() -> Self {
        Self {
            columns: Vec::new(),
            header: true,
            list_separator: String::new(),
            role_separator: String::new(),
            mux_separator: String::new(),
            direction: Direction::Bidirectional,
            signal: SignalKind::Digital,
            source_title: String::new(),
            source_revision: String::new(),
            page: String::new(),
        }
    }
}

/// The starting mapping for `columns` columns: number, name, then
/// functions, except that the first column captioned as a type ("Type",
/// "Pin type", "Type (power, input)") is the Pin type column. `captions`
/// is the table's first row when it holds captions. The person changes any
/// of them before anything is planned.
pub fn initial_columns(columns: usize, captions: Option<&[String]>) -> Vec<ColumnMap> {
    let type_column = captions
        .into_iter()
        .flatten()
        .position(|caption| is_type_caption(caption));
    (0..columns)
        .map(|i| match i {
            0 => ColumnMap::Number,
            i if Some(i) == type_column => ColumnMap::Type,
            1 => ColumnMap::Name,
            _ => ColumnMap::Function,
        })
        .collect()
}

/// Whether a column's caption has the word "type". This only starts the
/// mapping the person reviews; a pin's name is never read this way.
fn is_type_caption(caption: &str) -> bool {
    caption
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("type"))
}

/// One row the mapping cannot turn into a pin as stated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowProblem {
    /// 1-based line among the pasted rows.
    pub line: usize,
    pub message: String,
    /// The row still imports, flagged uncertain (an unknown pin type
    /// imports as passive); otherwise it is left out.
    pub kept: bool,
}

/// The import's account of `problems` for the toast: rows that need a
/// type and rows left out, by line ("2 pins need a type: rows 3, 5");
/// empty when there are none.
pub fn problem_summary(problems: &[RowProblem]) -> String {
    let lines = |kept: bool| -> Vec<String> {
        problems
            .iter()
            .filter(|p| p.kept == kept && p.line > 0)
            .map(|p| p.line.to_string())
            .collect()
    };
    let rows = |lines: &[String]| {
        format!(
            "{} {}",
            if lines.len() == 1 { "row" } else { "rows" },
            lines.join(", ")
        )
    };
    let mut parts = Vec::new();
    let typed = lines(true);
    if !typed.is_empty() {
        let pins = if typed.len() == 1 {
            "pin needs"
        } else {
            "pins need"
        };
        parts.push(format!("{} {pins} a type: {}", typed.len(), rows(&typed)));
    }
    let dropped = lines(false);
    if !dropped.is_empty() {
        let left = if dropped.len() == 1 {
            "row left out"
        } else {
            "rows left out"
        };
        parts.push(format!("{} {left}: {}", dropped.len(), rows(&dropped)));
    }
    parts.join("; ")
}

/// Build the typed import document from `rows` and `mapping`: one variant
/// named `variant`, a pin per data row, functions only from mapped
/// columns, and peripherals only from the person's role separator. Rows
/// that cannot become pins are reported and left out; the document is
/// `None` when the mapping lacks a number column or a source title.
pub fn document(
    rows: &[Vec<String>],
    mapping: &Mapping,
    device: &str,
    variant: &str,
) -> (Option<PinoutDocument>, Vec<RowProblem>) {
    let mut problems = Vec::new();
    let column = |kind: ColumnMap| mapping.columns.iter().position(|c| *c == kind);
    let Some(number_column) = column(ColumnMap::Number) else {
        return (None, problems);
    };
    let name_column = column(ColumnMap::Name);
    let default_column = column(ColumnMap::Default);
    let mux_column = column(ColumnMap::Mux);
    let type_column = column(ColumnMap::Type);
    let port_column = column(ColumnMap::Port);
    let index_column = column(ColumnMap::Index);
    let function_columns: Vec<usize> = mapping
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == ColumnMap::Function)
        .map(|(i, _)| i)
        .collect();
    let page = mapping.page.trim();
    let page = if page.is_empty() {
        None
    } else {
        match page.parse::<u16>() {
            Ok(page) if page > 0 => Some(page),
            _ => {
                problems.push(RowProblem {
                    line: 0,
                    message: "The page is not a whole number from 1 to 65535".into(),
                    kept: false,
                });
                return (None, problems);
            }
        }
    };
    let mut peripherals: Vec<PinoutPeripheral> = Vec::new();
    let mut pins = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let skip = usize::from(mapping.header);
    for (offset, cells) in rows.iter().enumerate().skip(skip) {
        let line = offset + 1;
        let cell = |i: usize| cells.get(i).map(String::as_str).unwrap_or("");
        let number = cell(number_column);
        if number.is_empty() {
            problems.push(RowProblem {
                line,
                message: format!("Row {line} has no number"),
                kept: false,
            });
            continue;
        }
        if !seen.insert(number.to_owned()) {
            problems.push(RowProblem {
                line,
                message: format!("Row {line} repeats number {number}"),
                kept: false,
            });
            continue;
        }
        let name = name_column
            .map(cell)
            .filter(|n| !n.is_empty())
            .unwrap_or(number);
        if name.len() > MAX_LABEL || number.len() > MAX_LABEL {
            problems.push(RowProblem {
                line,
                message: format!("Row {line} has a name longer than {MAX_LABEL} bytes"),
                kept: false,
            });
            continue;
        }
        let stated = type_column.map(cell).unwrap_or("");
        let kind = pin_type(stated);
        let untyped = kind.is_none();
        let (direction, signal, drive) = match kind {
            Some(PinType::Electrical(d, s, drive)) => (d, s, drive),
            Some(PinType::Analog) => (
                mapping.direction,
                SignalKind::Analog,
                default_drive(mapping.direction),
            ),
            None => NEUTRAL,
            _ => (
                mapping.direction,
                mapping.signal,
                default_drive(mapping.direction),
            ),
        };
        let kind = kind.unwrap_or(PinType::Unstated);
        let mut functions = Vec::new();
        let mut row_ok = true;
        for text in function_columns.iter().map(|i| cell(*i)) {
            let parts: Vec<&str> = if mapping.list_separator.is_empty() {
                vec![text]
            } else {
                text.split(mapping.list_separator.as_str()).collect()
            };
            for label in parts.into_iter().map(str::trim).filter(|l| !l.is_empty()) {
                if label.len() > MAX_LABEL {
                    row_ok = false;
                    break;
                }
                let (label, mux) = split_mux(label, &mapping.mux_separator);
                let (peripheral, role) = split_role(label, &mapping.role_separator);
                if let (Some(p), Some(r)) = (peripheral, role) {
                    add_role(&mut peripherals, p, r);
                }
                functions.push(PinoutFunction {
                    label: label.to_owned(),
                    direction,
                    signal,
                    drive,
                    peripheral: peripheral.map(str::to_owned),
                    role: role.map(str::to_owned),
                    mux: mux.map(str::to_owned),
                    mux_table: None,
                });
            }
        }
        if !row_ok {
            problems.push(RowProblem {
                line,
                message: format!("Row {line} has a function longer than {MAX_LABEL} bytes"),
                kept: false,
            });
            continue;
        }
        if let Some(text) = mux_column.map(cell).filter(|t| !t.is_empty()) {
            let codes: Vec<&str> = if mapping.list_separator.is_empty() {
                vec![text]
            } else {
                text.split(mapping.list_separator.as_str())
                    .map(str::trim)
                    .collect()
            };
            if codes.len() != functions.len() {
                problems.push(RowProblem {
                    line,
                    message: format!(
                        "Row {line} has {} mux codes for {} functions",
                        codes.len(),
                        functions.len()
                    ),
                    kept: false,
                });
                continue;
            }
            for (function, code) in functions.iter_mut().zip(codes) {
                if !code.is_empty() {
                    function.mux = Some(code.to_owned());
                }
            }
        }
        let class = match kind {
            PinType::DeviceNc => PinoutClass::DeviceNc,
            PinType::Reserved => PinoutClass::Reserved,
            _ => PinoutClass::Electrical,
        };
        if class != PinoutClass::Electrical && !functions.is_empty() {
            problems.push(RowProblem {
                line,
                message: format!("Row {line} is NC or reserved but lists functions"),
                kept: false,
            });
            continue;
        }
        // An electrical row without a function cell gets one named after
        // the pin with the row's type, so a power row imports as power,
        // never as I/O, and no terminal is left without a function.
        if class == PinoutClass::Electrical && functions.is_empty() {
            functions.push(PinoutFunction {
                label: name.to_owned(),
                direction,
                signal,
                drive,
                peripheral: None,
                role: None,
                mux: None,
                mux_table: None,
            });
        }
        let port = port_column.map(cell).filter(|p| !p.is_empty());
        let index = index_column.map(cell).filter(|i| !i.is_empty());
        let gpio = match (port, index) {
            (None, None) => None,
            (Some(port), Some(index)) => match index.parse::<u16>() {
                Ok(index) if port.len() <= pcbcraft_core::functions::pin_config::MAX_GPIO_PORT => {
                    Some(GpioRef {
                        port: port.to_owned(),
                        index,
                    })
                }
                _ => {
                    problems.push(RowProblem {
                        line,
                        message: format!(
                            "Row {line}'s GPIO needs a port of at most 8 bytes and an index from 0 to 65535"
                        ),
                        kept: false,
                    });
                    continue;
                }
            },
            _ => {
                problems.push(RowProblem {
                    line,
                    message: format!("Row {line} states a GPIO port or index without the other"),
                    kept: false,
                });
                continue;
            }
        };
        let default = default_column.map(cell).filter(|d| !d.is_empty());
        if let Some(default) = default
            && !functions.iter().any(|f| f.label == default)
        {
            problems.push(RowProblem {
                line,
                message: format!("Row {line}'s default {default} is not one of its functions"),
                kept: false,
            });
            continue;
        }
        pins.push(PinoutPin {
            row: number.to_owned(),
            number: BTreeMap::from([(variant.to_owned(), number.to_owned())]),
            name: name.to_owned(),
            class,
            gpio,
            options: PinoutOptions::default(),
            functions,
            default: default.map(str::to_owned),
            source: page.map(|page| SourceSpan {
                page: Some(page),
                table: None,
            }),
            uncertain: untyped,
            note: None,
        });
        if untyped {
            problems.push(RowProblem {
                line,
                message: format!(
                    "Row {line}: pin type ‘{stated}’ is not known; it imports as passive, set its type"
                ),
                kept: true,
            });
        }
    }
    let title = mapping.source_title.trim();
    if title.is_empty() || pins.is_empty() {
        return (None, problems);
    }
    let revision = mapping.source_revision.trim();
    let count = u32::try_from(pins.len()).unwrap_or(u32::MAX);
    (
        Some(PinoutDocument {
            format: PINOUT_FORMAT,
            device: PinoutDevice {
                name: device.to_owned(),
                reference: PinoutReference {
                    title: title.to_owned(),
                    document_revision: (!revision.is_empty()).then(|| revision.to_owned()),
                    url: None,
                },
            },
            variants: vec![PinoutVariant {
                name: variant.to_owned(),
                package_row: None,
                terminal_count: count,
            }],
            peripherals,
            pins,
            rules: Vec::new(),
        }),
        problems,
    )
}

/// "USART1_TX:AF7" at ":" → ("USART1_TX", Some("AF7")), split at the
/// last occurrence; no split without a separator or when either side
/// would be empty.
pub fn split_mux<'a>(label: &'a str, separator: &str) -> (&'a str, Option<&'a str>) {
    if separator.is_empty() {
        return (label, None);
    }
    match label.rsplit_once(separator) {
        Some((l, m)) if !l.trim().is_empty() && !m.trim().is_empty() => (l.trim(), Some(m.trim())),
        _ => (label, None),
    }
}

/// "USART1_TX" at "_" → ("USART1", "TX"); no split without a separator
/// or when either side would be empty.
pub fn split_role<'a>(label: &'a str, separator: &str) -> (Option<&'a str>, Option<&'a str>) {
    if separator.is_empty() {
        return (None, None);
    }
    match label.split_once(separator) {
        Some((p, r)) if !p.trim().is_empty() && !r.trim().is_empty() => {
            (Some(p.trim()), Some(r.trim()))
        }
        _ => (None, None),
    }
}

fn add_role(peripherals: &mut Vec<PinoutPeripheral>, peripheral: &str, role: &str) {
    let index = match peripherals.iter().position(|p| p.label == peripheral) {
        Some(index) => index,
        None => {
            peripherals.push(PinoutPeripheral {
                label: peripheral.to_owned(),
                kind: None,
                roles: Vec::new(),
            });
            peripherals.len() - 1
        }
    };
    if let Some(p) = peripherals.get_mut(index)
        && !p.roles.iter().any(|r| r.label == role)
    {
        p.roles.push(PinoutRole {
            label: role.to_owned(),
            signal: None,
            capacity: 1,
            required_when_used: false,
        });
    }
}

/// The open Paste table… flow: pasted text, the column mapping, and the
/// last preview.
#[derive(Default)]
pub struct PasteFlow {
    pub text: String,
    pub delimiter: Delimiter,
    pub mapping: Mapping,
    /// The plan of the last preview, for the document it was made for.
    pub plan: Option<(PinoutDocument, pcbcraft_core::library::pinout::PinoutPlan)>,
    /// The last preview's or apply's refusal, in shown wording.
    pub refusal: Option<String>,
    /// The engine revision an apply was sent at, until it lands.
    pub sent: Option<u64>,
    /// The flow opened this frame.
    pub opened: bool,
    /// Row keys the person unticked in the preview: applied as
    /// `PinoutAccept::Except`.
    pub rejected: std::collections::BTreeSet<String>,
    /// The preview lists every row, not the first few.
    pub all_rows: bool,
}

impl PasteFlow {
    /// What the apply takes: every row, or all but the unticked ones.
    pub fn accept(&self) -> pcbcraft_core::library::pinout::PinoutAccept {
        use pcbcraft_core::library::pinout::PinoutAccept;
        if self.rejected.is_empty() {
            PinoutAccept::All
        } else {
            PinoutAccept::Except(self.rejected.iter().cloned().collect())
        }
    }
}

impl PasteFlow {
    pub fn new() -> Self {
        Self {
            opened: true,
            ..Default::default()
        }
    }

    /// The pasted rows and the mapping's columns resized to them.
    pub fn rows(&mut self) -> Result<Vec<Vec<String>>, String> {
        let rows = split_table(&self.text, self.delimiter)?;
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        if self.mapping.columns.len() != width {
            let captions = rows.first().filter(|_| self.mapping.header);
            let initial = initial_columns(width, captions.map(Vec::as_slice));
            self.mapping.columns.resize(width, ColumnMap::Ignore);
            if self.mapping.columns.iter().all(|c| *c == ColumnMap::Ignore) {
                self.mapping.columns = initial;
            }
        }
        Ok(rows)
    }
}
