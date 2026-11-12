//! The one row painter of every list (list-system spec §1, direction A
//! "Ledger"): one band, quiet and typographic, with no row rules. Left to
//! right: indent guides, fold chevron, reorder grip, kind icon or layer /
//! net swatch, name (and an optional relation), dim secondary text, a
//! status badge, the decimal-aligned value, and the trailing severity
//! glyph. Hover actions are an overlay at the row's end that reserves no
//! space, so nothing shifts when they appear; they are allocated every
//! frame, so they stay in the tab order and the accessibility tree.
//!
//! Selection is a tinted band with a bar at the left edge; the
//! cross-highlight (the object hovered in another list, the canvas or 3D)
//! is a yellow band with a bar at the right edge, so the two never look
//! alike and neither relies on colour alone.
//!
//! The row geometry follows Solidcraft 4f3ac07a `ui_kit/table.rs`
//! (`Table::row_content`) and `tree.rs` (`tree_row`) as `kit::item_row`
//! did, MIT option; see ATTRIBUTION.md.
use super::super::severity::{Level, severity_glyph};
use super::measure::{self, Slots};
use super::model::{Fold, RowKey};
use super::tokens::ListTokens;
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Galley, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use std::sync::Arc;

/// The arrow between the parts of a name ("Origin → U1 · x").
pub const ARROW: &str = " \u{2192} ";

/// A row's value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Value<'a> {
    #[default]
    None,
    /// A number with its unit ("12.700 mm"), decimal-aligned in the
    /// column's slots; text that is not a number is right-aligned.
    Number(&'a str),
    /// `fx expr = value`: the expression gives way first, the value never.
    Expr { expr: &'a str, value: &'a str },
    /// Dim read-only text ("Held", "Reference").
    Text(&'a str),
}

/// A layer or net identity mark before the name.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Swatch {
    /// A board side or copper layer: a small rounded square.
    Layer(Color32),
    /// A net: a dot.
    Net(Color32),
}

/// A hover action at the row's end: an icon button, or a small text button
/// when `icon` is empty. `label` names what it acts on ("Delete dimension
/// Origin → left edge").
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Action<'a> {
    pub icon: &'a str,
    pub text: &'a str,
    pub label: &'a str,
    pub tip: &'a str,
    pub danger: bool,
}

impl<'a> Action<'a> {
    pub fn icon(icon: &'a str, label: &'a str) -> Self {
        Self {
            icon,
            text: "",
            label,
            tip: "",
            danger: false,
        }
    }

    pub fn delete(label: &'a str) -> Self {
        Self {
            danger: true,
            ..Self::icon("delete", label)
        }
    }

    pub fn text(text: &'a str, label: &'a str) -> Self {
        Self {
            icon: "",
            text,
            label,
            tip: "",
            danger: false,
        }
    }
}

/// What one row shows.
#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    pub key: RowKey,
    pub depth: u8,
    /// Whether this list keeps a fold slot (so icons line up).
    pub fold_slot: bool,
    pub fold: Fold,
    /// Order is document data: the row keeps a grip slot.
    pub grip: bool,
    pub icon: Option<&'a str>,
    pub swatch: Option<Swatch>,
    pub name: &'a [&'a str],
    pub secondary: Option<&'a str>,
    pub badge: Option<(Level, &'a str)>,
    pub value: Value<'a>,
    /// A severity glyph and an optional count at the end.
    pub trailing: Option<(Level, usize)>,
    pub actions: &'a [Action<'a>],
    pub selected: bool,
    /// Cross-highlighted: hovered elsewhere (canvas, 3D, another list).
    pub highlighted: bool,
    /// The keyboard cursor, drawn as a focus ring.
    pub cursor: bool,
    /// The accessibility role (ListBoxOption, TreeItem, Row).
    pub role: egui::Role,
    /// A committed value's flash, 1 → 0.
    pub flash: f32,
}

impl<'a> Row<'a> {
    pub fn new(key: RowKey, name: &'a [&'a str]) -> Self {
        Self {
            key,
            depth: 0,
            fold_slot: false,
            fold: Fold::Leaf,
            grip: false,
            icon: None,
            swatch: None,
            name,
            secondary: None,
            badge: None,
            value: Value::None,
            trailing: None,
            actions: &[],
            selected: false,
            highlighted: false,
            cursor: false,
            role: egui::Role::ListItem,
            flash: 0.0,
        }
    }
}

/// The value column of a list, measured over every row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Columns {
    /// The value column's width; 0 for none.
    pub value: f32,
    pub slots: Slots,
}

impl Columns {
    /// The column that fits every one of `values` as shown at rest
    /// ([`Value`]), without its expressions (which give way).
    pub fn measure<'v>(ui: &Ui, l: &ListTokens, values: impl IntoIterator<Item = Value<'v>>) -> Self {
        let mut numbers = Vec::new();
        let mut texts = Vec::new();
        let mut fx = false;
        for v in values {
            match v {
                Value::None => {}
                Value::Number(n) => numbers.push(n),
                Value::Expr { value, .. } => {
                    fx = true;
                    numbers.push(value);
                }
                Value::Text(t) => texts.push(t),
            }
        }
        let slots = measure::slots(ui, l, numbers.iter().copied());
        let mut value = if numbers.is_empty() {
            0.0
        } else {
            slots.width(l.gap * 0.5)
        };
        if fx {
            value += fx_width(ui, l) + l.gap;
        }
        let text = measure::widest(ui, &FontId::proportional(l.fs_s), texts);
        Self {
            value: value.max(text).ceil(),
            slots,
        }
    }
}

fn fx_width(ui: &Ui, l: &ListTokens) -> f32 {
    measure::widest(ui, &fx_font(l), ["fx"])
}

fn fx_font(l: &ListTokens) -> FontId {
    FontId::monospace(l.fs_xs)
}

/// What a row did this frame.
#[derive(Clone, Debug)]
pub struct RowResponse {
    /// The band.
    pub response: Response,
    pub clicked: bool,
    pub double_clicked: bool,
    /// The pointer is anywhere on the row, its actions included.
    pub hovered: bool,
    /// The chevron was clicked.
    pub fold: bool,
    /// The hover action clicked, by index.
    pub action: Option<usize>,
    /// The reorder grip, when the row has one.
    pub grip: Option<Response>,
    /// Where the value is drawn: an inline editor goes here.
    pub value_rect: Rect,
}

/// The name as one line of text: "Front face → bottom edge".
pub fn name_text(parts: &[&str]) -> String {
    parts.join(ARROW)
}

fn name_galley(ui: &Ui, l: &ListTokens, parts: &[&str], width: f32, ink: Color32) -> Arc<Galley> {
    let layout = |size: f32, rows: usize| {
        let font = FontId::proportional(size);
        let mut job = LayoutJob::default();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                job.append(ARROW, 0.0, TextFormat::simple(font.clone(), l.fg_faint));
            }
            job.append(part, 0.0, TextFormat::simple(font.clone(), ink));
        }
        job.wrap.max_width = width.max(1.0);
        job.wrap.max_rows = rows;
        job.wrap.overflow_character = Some(super::super::text::ELLIPSIS);
        ui.painter().layout_job(job)
    };
    let one = layout(l.fs, 1);
    if one.elided { layout(l.fs_s, 2) } else { one }
}

fn line_galley(ui: &Ui, text: &str, font: FontId, ink: Color32, width: f32) -> Arc<Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, ink);
    job.wrap.max_width = width.max(1.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some(super::super::text::ELLIPSIS);
    ui.painter().layout_job(job)
}

/// The band's fill: selection, cross-highlight, both, or hover.
fn fill(l: &ListTokens, selected: bool, highlighted: bool, hovered: bool) -> Option<Color32> {
    match (selected, highlighted) {
        (true, true) => Some(lerp(l.sel, l.xh, 0.45)),
        (true, false) => Some(l.sel),
        (false, true) => Some(l.xh),
        (false, false) => hovered.then_some(l.hover),
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        mix(a.r(), b.r()),
        mix(a.g(), b.g()),
        mix(a.b(), b.b()),
        mix(a.a(), b.a()),
    )
}

/// The edge bars: selection at the left, cross-highlight at the right.
fn bars(ui: &Ui, l: &ListTokens, band: Rect, selected: bool, highlighted: bool) {
    let bar = |x: f32, ink: Color32| {
        let r = Rect::from_min_max(
            pos2(x, band.top() + l.bar_inset),
            pos2(x + l.bar_w, band.bottom() - l.bar_inset),
        );
        ui.painter().rect_filled(r, l.bar_w / 2.0, ink);
    };
    if selected {
        bar(band.left(), l.sel_line);
    }
    if highlighted {
        bar(band.right() - l.bar_w, l.xh_line);
    }
}

/// A row's band: the selection, cross-highlight or fading hover fill, the
/// value flash, the edge bars and the keyboard focus ring. Shared by list
/// rows and table rows.
#[allow(clippy::too_many_arguments)]
pub(super) fn band(
    ui: &Ui,
    l: &ListTokens,
    rect: Rect,
    id: egui::Id,
    selected: bool,
    highlighted: bool,
    cursor: bool,
    flash: f32,
) {
    let hovered = ui.is_enabled() && ui.rect_contains_pointer(rect);
    let painter = ui.painter_at(rect);
    let fade = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), hovered, l.motion_fast);
    if let Some(color) = fill(l, selected, highlighted, false) {
        painter.rect_filled(rect, 0.0, color);
    } else if fade > 0.0 {
        painter.rect_filled(rect, 0.0, l.hover.gamma_multiply(fade));
    }
    if flash > 0.0 {
        painter.rect_filled(rect, 0.0, l.flash.gamma_multiply(flash.min(1.0)));
    }
    bars(ui, l, rect, selected, highlighted);
    if cursor {
        painter.rect_stroke(
            rect.shrink(1.0),
            2.0,
            Stroke::new(1.0, l.focus),
            egui::StrokeKind::Inside,
        );
    }
}

/// Paint one row into `rect` (a list line) and return what it did. The
/// row's id comes from its key, so a press survives a fold above it.
pub fn paint(ui: &mut Ui, id: egui::Id, rect: Rect, row: &Row<'_>, cols: &Columns) -> RowResponse {
    let l = ListTokens::get(ui.ctx());
    let id = id.with(row.key);
    let response = ui.interact(rect, id, Sense::click());
    let hovered = ui.is_enabled() && ui.rect_contains_pointer(rect);
    let painter = ui.painter_at(rect);
    band(ui, &l, rect, id, row.selected, row.highlighted, row.cursor, row.flash);
    let cy = rect.center().y;
    let mut x = rect.left() + l.pad_x;
    // Indent guides.
    for level in 0..row.depth {
        let gx = x + f32::from(level) * l.indent + 6.0;
        painter.line_segment(
            [pos2(gx, rect.top()), pos2(gx, rect.bottom())],
            Stroke::new(1.0, if hovered { l.line_strong } else { l.line }),
        );
    }
    x += f32::from(row.depth) * l.indent;
    let mut fold = false;
    if row.fold_slot || row.fold.folds() {
        let at = Rect::from_center_size(pos2(x + l.fold_w / 2.0, cy), vec2(l.fold_w, l.fold_w));
        if row.fold.folds() {
            let open = ui.ctx().animate_bool_with_time(
                id.with("fold"),
                row.fold == Fold::Open,
                l.motion,
            );
            chevron(&painter, at, open, l.fg_faint);
            fold = ui
                .interact(at, id.with("chevron"), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked();
        }
        x += l.fold_w + l.gap;
    }
    let mut grip = None;
    if row.grip {
        let at = Rect::from_min_size(pos2(x, rect.top()), vec2(l.grip_w, rect.height()));
        let r = ui
            .interact(at, id.with("grip"), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        if hovered || row.selected || r.dragged() {
            grip_dots(&painter, at.center(), l.fg_faint);
        }
        grip = Some(r);
        x += l.grip_w + l.gap;
    }
    if let Some(icon) = row.icon {
        super::super::icon(&painter, pos2(x + l.icon / 2.0, cy), l.icon, icon, 0.75);
        x += l.icon + l.gap;
    }
    if let Some(swatch) = row.swatch {
        let c = pos2(x + l.swatch / 2.0, cy);
        match swatch {
            Swatch::Layer(color) => {
                painter.rect_filled(
                    Rect::from_center_size(c, vec2(l.swatch - 2.0, l.swatch - 2.0)),
                    2.0,
                    color,
                );
            }
            Swatch::Net(color) => {
                painter.circle_filled(c, l.swatch / 2.0, color);
                painter.circle_stroke(c, l.swatch / 2.0, Stroke::new(1.0, l.line_strong));
            }
        }
        x += l.swatch + l.gap;
    }
    // From the right: trailing glyph, value, badge.
    let mut right = rect.right() - l.pad_x;
    if let Some((level, count)) = row.trailing {
        let glyph = l.icon - 2.0;
        if count > 1 {
            let g = painter.layout_no_wrap(count.to_string(), measure::mono(&l), l.fg_faint);
            right -= g.size().x;
            painter.galley(pos2(right, cy - g.size().y / 2.0), g, l.fg_faint);
            right -= 3.0;
        }
        severity_glyph(&painter, pos2(right - glyph / 2.0, cy), glyph, level);
        right -= glyph + l.gap;
    }
    let value_w = cols.value.min((rect.width() * l.value_share).max(0.0));
    let value_rect = Rect::from_min_max(
        pos2(right - value_w, rect.top()),
        pos2(right, rect.bottom()),
    );
    if value_w > 0.0 {
        paint_value(ui, &l, &painter, value_rect, row.value, &cols.slots, id);
        right -= value_w + l.gap;
    }
    if let Some((level, text)) = row.badge {
        let (ink, soft) = l.severity(level);
        let g = painter.layout_no_wrap(text.to_owned(), FontId::proportional(l.fs_xs), ink);
        let size = vec2(g.size().x + 10.0, l.chip_h);
        let chip = Rect::from_min_size(pos2(right - size.x, cy - size.y / 2.0), size);
        painter.rect_filled(chip, l.chip_r, soft);
        painter.galley(chip.center() - g.size() / 2.0, g, ink);
        right -= size.x + l.gap;
    }
    // The name keeps its own width before the secondary text gets any.
    let room = (right - x).max(1.0);
    let ink = l.fg;
    let galley = name_galley(ui, &l, row.name, room, ink);
    let name_w = galley.size().x;
    let name_rect = Rect::from_min_size(pos2(x, cy - galley.size().y / 2.0), galley.size());
    let cut = galley.elided;
    painter.galley(name_rect.min, galley, ink);
    if let Some(secondary) = row.secondary {
        let left = x + name_w + l.gap;
        let width = right - left;
        if width > l.fs * 2.0 {
            let g = line_galley(ui, secondary, FontId::proportional(l.fs_s), l.fg_dim, width);
            painter.galley(pos2(left, cy - g.size().y / 2.0), g, l.fg_dim);
        }
    }
    // The accessible name is the row's name; what else it shows is its
    // description-like value, so a screen reader and a test find the row
    // by what it is.
    let full = name_text(row.name);
    let mut rest = Vec::new();
    if let Some(s) = row.secondary {
        rest.push(s.to_owned());
    }
    if let Some((_, text)) = row.badge {
        rest.push(text.to_owned());
    }
    match row.value {
        Value::Number(v) | Value::Text(v) => rest.push(v.to_owned()),
        Value::Expr { expr, value } => rest.push(format!("{expr} = {value}")),
        Value::None => {}
    }
    if let Some((level, count)) = row.trailing {
        rest.push(level.count_words(count.max(1)));
    }
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::selected(row.role, true, row.selected, &full);
        if !rest.is_empty() {
            info.current_text_value = Some(rest.join(", "));
        }
        info
    });
    if cut {
        ui.interact(name_rect, id.with("name"), Sense::hover())
            .on_hover_text(&full);
    }
    let action = actions(ui, &l, rect, id, row, hovered);
    let actions_hovered = action.1;
    RowResponse {
        clicked: response.clicked(),
        double_clicked: response.double_clicked(),
        hovered: hovered || actions_hovered,
        fold,
        action: action.0,
        grip,
        value_rect,
        response,
    }
}

/// The value, right-aligned in its column: numbers on the decimal point
/// with a dim unit; an expression gives way before its value.
fn paint_value(
    ui: &Ui,
    l: &ListTokens,
    painter: &egui::Painter,
    rect: Rect,
    value: Value<'_>,
    slots: &Slots,
    id: egui::Id,
) {
    let cy = rect.center().y;
    let mono = measure::mono(l);
    let unit_font = FontId::monospace(l.fs_s);
    let number = |text: &str, right: f32| -> f32 {
        let Some(n) = measure::split(text) else {
            let g = line_galley(ui, text, mono.clone(), l.fg, rect.width());
            let left = right - g.size().x;
            painter.galley(pos2(left, cy - g.size().y / 2.0), g, l.fg);
            return left;
        };
        let unit_left = right - slots.unit;
        let frac_left = unit_left - if slots.unit > 0.0 { l.gap * 0.5 } else { 0.0 } - slots.frac;
        if !n.unit.is_empty() {
            let g = painter.layout_no_wrap(n.unit.to_owned(), unit_font.clone(), l.fg_faint);
            painter.galley(pos2(unit_left, cy - g.size().y / 2.0), g, l.fg_faint);
        }
        if !n.frac.is_empty() {
            let g = painter.layout_no_wrap(n.frac.to_owned(), mono.clone(), l.fg);
            painter.galley(pos2(frac_left, cy - g.size().y / 2.0), g, l.fg);
        }
        let g = painter.layout_no_wrap(n.int.to_owned(), mono.clone(), l.fg);
        let left = frac_left - g.size().x;
        painter.galley(pos2(left, cy - g.size().y / 2.0), g, l.fg);
        right - slots.width(l.gap * 0.5)
    };
    match value {
        Value::None => {}
        Value::Number(text) => {
            number(text, rect.right());
        }
        Value::Text(text) => {
            let g = line_galley(ui, text, FontId::proportional(l.fs_s), l.fg_dim, rect.width());
            let at = pos2(rect.right() - g.size().x, cy - g.size().y / 2.0);
            let cut = g.elided;
            painter.galley(at, g, l.fg_dim);
            if cut {
                ui.interact(rect, id.with("value"), Sense::hover())
                    .on_hover_text(text);
            }
        }
        Value::Expr { expr, value } => {
            let left = number(value, rect.right());
            let fx = painter.layout_no_wrap("fx".to_owned(), fx_font(l), l.focus);
            let eq = painter.layout_no_wrap(" = ".to_owned(), FontId::monospace(l.fs_s), l.fg_faint);
            let room = left - rect.left() - fx.size().x - eq.size().x - l.gap;
            let mut at = left;
            if room > l.fs * 2.0 {
                at -= eq.size().x;
                painter.galley(pos2(at, cy - eq.size().y / 2.0), eq, l.fg_faint);
                let g = line_galley(ui, expr, FontId::monospace(l.fs_s), l.fg_dim, room);
                at -= g.size().x;
                painter.galley(pos2(at, cy - g.size().y / 2.0), g, l.fg_dim);
            }
            at -= fx.size().x + l.gap * 0.5;
            painter.galley(pos2(at, cy - fx.size().y / 2.0), fx, l.focus);
            ui.interact(rect, id.with("value"), Sense::hover())
                .on_hover_text(format!("{expr} = {value}"));
        }
    }
}

/// The hover actions: an overlay at the row's end with a fading backdrop.
/// Returns the clicked action and whether the pointer is on one.
fn actions(
    ui: &mut Ui,
    l: &ListTokens,
    rect: Rect,
    id: egui::Id,
    row: &Row<'_>,
    hovered: bool,
) -> (Option<usize>, bool) {
    if row.actions.is_empty() {
        return (None, false);
    }
    let size = rect.height() - 8.0;
    let font = FontId::proportional(l.fs_s);
    let widths: Vec<f32> = row
        .actions
        .iter()
        .map(|a| {
            if a.icon.is_empty() {
                measure::widest(ui, &font, [a.text]) + 14.0
            } else {
                size
            }
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 2.0 * (widths.len() as f32 - 1.0).max(0.0);
    let mut x = rect.right() - 6.0 - total;
    let rects: Vec<Rect> = widths
        .iter()
        .map(|w| {
            let r = Rect::from_min_size(pos2(x, rect.center().y - size / 2.0), vec2(*w, size));
            x += w + 2.0;
            r
        })
        .collect();
    let responses: Vec<Response> = rects
        .iter()
        .enumerate()
        .map(|(i, r)| ui.interact(*r, id.with(("action", i)), Sense::click()))
        .collect();
    let focused = responses.iter().any(Response::has_focus);
    let on = responses.iter().any(Response::hovered);
    let shown = hovered || row.selected || focused;
    let alpha = ui
        .ctx()
        .animate_bool_with_time(id.with("actions"), shown, l.motion_fast);
    let mut clicked = None;
    if alpha > 0.0 {
        let back = fill(l, row.selected, row.highlighted, true).unwrap_or(l.hover);
        let left = rects.first().map_or(rect.right(), |r| r.left()) - 16.0;
        backdrop(ui, Rect::from_min_max(pos2(left, rect.top()), rect.max), back, alpha);
    }
    for (i, (a, response)) in row.actions.iter().zip(&responses).enumerate() {
        let r = response.rect;
        let lit = response.hovered() || response.has_focus();
        if alpha > 0.0 {
            if lit {
                let hot = if a.danger {
                    super::super::severity::ink(Level::Error).gamma_multiply(0.15)
                } else {
                    l.line
                };
                ui.painter().rect_filled(r, 4.0, hot.gamma_multiply(alpha));
            }
            if a.icon.is_empty() {
                ui.painter().rect_stroke(
                    r,
                    4.0,
                    Stroke::new(1.0, l.line_strong.gamma_multiply(alpha)),
                    egui::StrokeKind::Inside,
                );
                let g = ui.painter().layout_no_wrap(a.text.to_owned(), font.clone(), l.fg);
                ui.painter()
                    .galley(r.center() - g.size() / 2.0, g, l.fg.gamma_multiply(alpha));
            } else {
                super::super::icon(
                    ui.painter(),
                    r.center(),
                    size - 8.0,
                    a.icon,
                    alpha * if lit { 1.0 } else { 0.7 },
                );
            }
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                r,
                4.0,
                Stroke::new(1.0, l.focus),
                egui::StrokeKind::Inside,
            );
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, a.label));
        let response = response.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
        let response = if a.tip.is_empty() {
            response.on_hover_text(a.label)
        } else {
            response.on_hover_text(format!("{}\n{}", a.label, a.tip))
        };
        if response.clicked() {
            clicked = Some(i);
        }
    }
    (clicked, on)
}

/// A band from transparent to `back` over 16 pt, then solid: the actions'
/// backdrop, so text under them fades out instead of being cut.
fn backdrop(ui: &Ui, rect: Rect, back: Color32, alpha: f32) {
    let solid = back.gamma_multiply(alpha);
    let clear = Color32::TRANSPARENT;
    let mid = (rect.left() + 16.0).min(rect.right());
    let mut mesh = Mesh::default();
    let mut quad = |a: Rect, left: Color32, right: Color32| {
        let base = mesh.vertices.len() as u32;
        for (pos, color) in [
            (a.left_top(), left),
            (a.right_top(), right),
            (a.right_bottom(), right),
            (a.left_bottom(), left),
        ] {
            mesh.vertices.push(Vertex {
                pos,
                uv: WHITE_UV,
                color,
            });
        }
        mesh.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    quad(
        Rect::from_min_max(rect.min, pos2(mid, rect.bottom())),
        clear,
        solid,
    );
    quad(
        Rect::from_min_max(pos2(mid, rect.top()), rect.max),
        solid,
        solid,
    );
    ui.painter().add(egui::Shape::mesh(mesh));
}

/// The chevron: right when folded, turning down as `open` goes to 1.
fn chevron(painter: &egui::Painter, rect: Rect, open: f32, ink: Color32) {
    let c = rect.center();
    let h = rect.width().min(rect.height()) * 0.18;
    let angle = open * std::f32::consts::FRAC_PI_2;
    let (s, co) = angle.sin_cos();
    let turn = |v: egui::Vec2| c + vec2(v.x * co - v.y * s, v.x * s + v.y * co);
    let stroke = Stroke::new(1.4, ink);
    let a = turn(vec2(-h * 0.8, -h * 1.6));
    let b = turn(vec2(h * 0.8, 0.0));
    let d = turn(vec2(-h * 0.8, h * 1.6));
    painter.line_segment([a, b], stroke);
    painter.line_segment([b, d], stroke);
}

/// Six dots, the reorder grip.
fn grip_dots(painter: &egui::Painter, c: egui::Pos2, ink: Color32) {
    for dx in [-2.0, 2.0] {
        for dy in [-4.0, 0.0, 4.0] {
            painter.circle_filled(c + vec2(dx, dy), 1.1, ink);
        }
    }
}

/// A group header line: chevron, uppercase caption with tracking and the
/// count. Returns the header's response (a click folds).
#[allow(clippy::too_many_arguments)]
pub fn header(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    key: RowKey,
    caption: &str,
    count: usize,
    folded: bool,
    cursor: bool,
) -> Response {
    let l = ListTokens::get(ui.ctx());
    let id = id.with(key);
    let response = ui.interact(rect, id, Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, l.surface);
    if response.hovered() {
        painter.rect_filled(rect, 0.0, l.hover);
    }
    painter.line_segment(
        [
            pos2(rect.left(), rect.bottom() - 0.5),
            pos2(rect.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, l.line),
    );
    if cursor {
        painter.rect_stroke(
            rect.shrink(1.0),
            2.0,
            Stroke::new(1.0, l.focus),
            egui::StrokeKind::Inside,
        );
    }
    let cy = rect.center().y;
    let mut x = rect.left() + l.pad_x;
    let open = ui
        .ctx()
        .animate_bool_with_time(id.with("fold"), !folded, l.motion);
    chevron(
        &painter,
        Rect::from_center_size(pos2(x + l.fold_w / 2.0, cy), vec2(l.fold_w, l.fold_w)),
        open,
        l.fg_faint,
    );
    x += l.fold_w + l.gap;
    let font = FontId::proportional(l.fs_xs);
    let mut job = LayoutJob::default();
    let format = TextFormat {
        font_id: font.clone(),
        color: l.fg_dim,
        extra_letter_spacing: l.fs_xs * l.caption_tracking,
        ..Default::default()
    };
    job.append(&caption.to_uppercase(), 0.0, format);
    let g = painter.layout_job(job);
    let w = g.size().x;
    painter.galley(pos2(x, cy - g.size().y / 2.0), g, l.fg_dim);
    let n = painter.layout_no_wrap(count.to_string(), FontId::monospace(l.fs_xs), l.fg_faint);
    painter.galley(pos2(x + w + l.gap, cy - n.size().y / 2.0), n, l.fg_faint);
    let label = format!("{caption}, {count}");
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::Role::TreeItem, true, &label);
        info.selected = Some(!folded);
        info
    });
    response
}

/// "4 of 7 · Show 3 more": the one way a bounded list in a card grows.
pub fn more(ui: &mut Ui, shown: usize, total: usize) -> bool {
    let l = ListTokens::get(ui.ctx());
    let rest = total.saturating_sub(shown);
    let text = format!("{shown} of {total} \u{b7} Show {rest} more");
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), l.row_h), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 0.0, l.hover);
    }
    let g = ui
        .painter()
        .layout_no_wrap(text.clone(), FontId::proportional(l.fs_s), l.fg_dim);
    ui.painter().galley(
        pos2(rect.left() + l.pad_x, rect.center().y - g.size().y / 2.0),
        g,
        if hovered { l.fg } else { l.fg_dim },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(1.0),
            2.0,
            Stroke::new(1.0, l.focus),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
    response.clicked()
}

/// A dashed full-width button whose text says what it adds.
pub fn add_row(ui: &mut Ui, text: &str) -> bool {
    let l = ListTokens::get(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), l.row_h - 2.0), Sense::click());
    let r = rect.shrink2(vec2(l.pad_x - 2.0, 2.0));
    let ink = if response.hovered() || response.has_focus() {
        l.fg
    } else {
        l.fg_dim
    };
    let stroke = Stroke::new(1.0, if response.hovered() { l.fg_faint } else { l.line_strong });
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        if let [a, b] = w {
            ui.painter()
                .extend(egui::Shape::dashed_line(&[*a, *b], stroke, 4.0, 3.0));
        }
    }
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(l.fs_s), ink);
    ui.painter().galley(r.center() - g.size() / 2.0, g, ink);
    if response.has_focus() {
        ui.painter()
            .rect_stroke(r, 4.0, Stroke::new(1.0, l.focus), egui::StrokeKind::Inside);
    }
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text));
    response.clicked()
}

/// One red line under a row, aligned with the name: what is wrong and how
/// to fix it. The document is unchanged.
pub fn field_error(ui: &mut Ui, text: &str, indent: f32) {
    let l = ListTokens::get(ui.ctx());
    let ink = super::super::severity::ink(Level::Error);
    let width = (ui.available_width() - indent - l.pad_x).max(1.0);
    let mut job = LayoutJob::simple(text.to_owned(), FontId::proportional(l.fs_s), ink, width);
    job.wrap.max_rows = 3;
    let g = ui.painter().layout_job(job);
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), g.size().y + 5.0), Sense::hover());
    ui.painter()
        .galley(pos2(rect.left() + l.pad_x + indent, rect.top()), g, ink);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
}
