//! Search sheet, field and result rows (upstream `ui_kit/search.rs`:
//! `CommandSearch::{field, sheet, footer, heading}` and
//! `MenuItem::search_row` with its `SearchBadge` looks). Upstream keeps them
//! crate-private, so their geometry is reproduced on the public tokens,
//! fonts and icons. Ranking and what a row does stay with the caller. The
//! compact `filter_field` and `search_message` are local additions in the
//! same look.
use egui::{FontId, Rect, Response, Sense, Stroke, Ui, pos2, vec2};
use solidcraft_ui_egui::{
    fonts,
    theme::Tokens,
    ui_kit::{Container, Surface},
};

/// Upstream search row: one control height plus 10 pt.
pub fn search_row_height() -> f32 {
    let t = Tokens::get();
    t.control_height + t.space_10
}

/// Keys a search sheet owns while it is open. They are consumed before any
/// text editor sees them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchKeys {
    pub up: bool,
    pub down: bool,
    pub enter: bool,
    /// Shift+Enter: act and keep the sheet open.
    pub keep: bool,
    pub escape: bool,
}

/// Consume the sheet's navigation keys for this frame.
pub fn search_keys(ctx: &egui::Context) -> SearchKeys {
    use egui::{Key, Modifiers};
    ctx.input_mut(|i| {
        // Shift+Enter first: a plain-Enter match ignores Shift.
        let keep = i.consume_key(Modifiers::SHIFT, Key::Enter);
        SearchKeys {
            keep,
            enter: keep || i.consume_key(Modifiers::NONE, Key::Enter),
            up: i.consume_key(Modifiers::NONE, Key::ArrowUp),
            down: i.consume_key(Modifiers::NONE, Key::ArrowDown),
            escape: i.consume_key(Modifiers::NONE, Key::Escape),
        }
    })
}

/// The radius of nested shapes inside the sheet (upstream `inner_radius`).
fn inner_radius() -> u8 {
    let t = Tokens::get();
    t.radius_xl.saturating_sub(t.space_6 as u8)
}

/// The upstream menu frame the search sheet paints behind its content.
fn sheet_frame() -> egui::Frame {
    let t = Tokens::get();
    egui::Frame::NONE
        .fill(t.dialog_body)
        .stroke(t.primary_border(t.dialog_border))
        .corner_radius(t.radius_xl)
        .shadow(egui::Shadow {
            offset: [0, t.space_14 as i8],
            blur: (t.space_24 + t.space_12) as u8,
            spread: 0,
            color: t.app_bar.gamma_multiply(0.18),
        })
}

/// A floating search sheet of `width` at `top` (its top-left corner), above
/// the canvas and cards, with the key-hint `footer` along its bottom edge
/// (upstream `CommandSearch::sheet`: content inset 6 pt at the sides and
/// 4 pt at the top, then an edge-to-edge footer). Returns the sheet's
/// rectangle.
pub fn search_sheet(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    top: egui::Pos2,
    width: f32,
    footer: &str,
    add: impl FnOnce(&mut Ui),
) -> Rect {
    let host = Container::new(ctx, Surface::Popup);
    host.area(egui::Id::unique(("search-sheet", id)))
        .fixed_pos(top)
        .movable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            let t = Tokens::get();
            let origin = ui.cursor().min;
            let background = ui.painter().add(egui::Shape::Noop);
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: t.space_6 as i8,
                    right: t.space_6 as i8,
                    top: t.space_4 as i8,
                    bottom: 0,
                })
                .show(ui, |ui| {
                    ui.set_width((width - t.space_12).max(0.0));
                    ui.spacing_mut().item_spacing.y = 0.0;
                    super::form(ui, add);
                });
            ui.add_space(t.space_6);
            search_footer(ui, width, footer);
            let sheet = Rect::from_min_size(origin, vec2(width, ui.min_rect().bottom() - origin.y));
            let frame = sheet_frame();
            ui.painter()
                .set(background, frame.paint(sheet.shrink(frame.stroke.width)));
            // The edge-to-edge footer shares the card silhouette; keep the
            // outer rule above its fill.
            ui.painter()
                .rect_stroke(sheet, t.radius_xl, frame.stroke, egui::StrokeKind::Inside);
            sheet
        })
        .inner
}

/// The field outline shared by the search field and the filter field: the
/// focus halo, the input fill and one error look (a red ring on the error
/// fill halo while focused).
fn field_frame(ui: &Ui, r: Rect, radius: f32, focused: bool, invalid: bool) {
    let t = Tokens::get();
    let ink = if invalid {
        t.form_error
    } else if focused {
        t.form_primary
    } else {
        t.control_border
    };
    if focused || invalid {
        let outset = t.space_3;
        ui.painter().rect_filled(
            r.expand(outset),
            radius + outset,
            if invalid {
                t.form_error_fill
            } else {
                t.accent_soft
            },
        );
    }
    ui.painter().rect(
        r,
        radius,
        t.input_fill(),
        if focused || invalid {
            Stroke::new(1.0, ink)
        } else {
            t.control_border_stroke(ink)
        },
        egui::StrokeKind::Inside,
    );
}

/// Cut `value` to at most `limit` bytes on a character boundary: the core
/// bounds queries in bytes, and `Ω` or `µ` take two.
fn bound_bytes(value: &mut String, limit: usize) {
    if value.len() > limit {
        let mut end = limit;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
}

/// The search field (upstream `CommandSearch::field`, fully expanded): a
/// zoom icon, the editor and a key cap. `focus` requests keyboard focus.
pub fn search_field(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut String,
    hint: &str,
    invalid: bool,
    focus: bool,
    limit: usize,
) -> Response {
    let t = Tokens::get();
    let height = search_row_height();
    let radius = f32::from(t.radius_xl);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let focused = focus || ui.memory(|m| m.has_focus(id));
    field_frame(ui, r, radius, focused, invalid);
    super::icon(
        ui.painter(),
        r.left_center() + vec2(t.space_12 + t.space_16 / 2.0, 0.0),
        t.space_16,
        "zoom",
        1.0,
    );
    let key_width = key_cap(
        ui,
        r.right_center() - vec2(t.space_10, 0.0),
        "Esc",
        t.form_dim,
    );
    let editor_left = (r.left() + t.space_12 + t.space_16 + t.space_12).min(r.right());
    let editor = Rect::from_min_max(
        pos2(editor_left, r.top()),
        pos2(
            (r.right() - t.space_10 - key_width - t.space_10).max(editor_left),
            r.bottom(),
        ),
    );
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(editor), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(editor));
            ui.add_sized(
                editor.size(),
                egui::TextEdit::singleline(value)
                    .align(egui::Align2::LEFT_CENTER)
                    .min_size(vec2(0.0, height))
                    .margin(vec2(t.form_inset, 0.0))
                    .id(id)
                    .char_limit(limit)
                    .frame(egui::Frame::NONE)
                    .hint_text(hint)
                    .font(FontId::proportional(t.font_button)),
            )
        })
        .inner;
    bound_bytes(value, limit);
    if focus {
        response.request_focus();
    }
    response
}

/// What the compact filter field reports for one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FilterResponse {
    /// The text changed: typed, cleared with Esc or with the clear button.
    pub changed: bool,
}

/// A compact search-style filter field (one control high): a zoom icon,
/// proportional text, and a clear button while it holds text. Esc clears a
/// non-empty filter. `limit` bounds the text in bytes.
pub fn filter_field(
    ui: &mut Ui,
    id: egui::Id,
    value: &mut String,
    hint: &str,
    invalid: bool,
    limit: usize,
) -> FilterResponse {
    let t = Tokens::get();
    let height = t.control_height;
    let radius = f32::from(t.control_radius);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    // egui drops focus on Esc before the field runs, so the field also
    // remembers whether it held focus in the previous frame.
    let held = id.with("held");
    let focused = ui.memory(|m| m.has_focus(id));
    let had_focus = focused || ui.data(|d| d.get_temp::<bool>(held).unwrap_or(false));
    let mut changed = false;
    if had_focus
        && !value.is_empty()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        value.clear();
        changed = true;
    }
    field_frame(ui, r, radius, focused, invalid);
    let icon_size = t.space_14;
    super::icon(
        ui.painter(),
        r.left_center() + vec2(t.space_8 + icon_size / 2.0, 0.0),
        icon_size,
        "zoom",
        0.8,
    );
    let clear_size = height - t.space_6;
    let clear = Rect::from_center_size(
        r.right_center() - vec2(t.space_4 + clear_size / 2.0, 0.0),
        vec2(clear_size, clear_size),
    );
    if !value.is_empty() {
        let response = ui
            .interact(clear, id.with("clear"), Sense::click())
            .on_hover_text("Clear the filter (Esc)");
        let ink = if response.hovered() {
            t.form_ink
        } else {
            t.form_dim
        };
        let arm = clear_size * 0.18;
        let c = clear.center();
        let stroke = Stroke::new(1.3, ink);
        ui.painter()
            .line_segment([c - vec2(arm, arm), c + vec2(arm, arm)], stroke);
        ui.painter()
            .line_segment([c + vec2(-arm, arm), c + vec2(arm, -arm)], stroke);
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Clear filter"));
        if response.clicked() {
            value.clear();
            changed = true;
        }
    }
    let editor_left = (r.left() + t.space_8 + icon_size + t.space_4).min(r.right());
    let editor = Rect::from_min_max(
        pos2(editor_left, r.top()),
        pos2((clear.left() - t.space_2).max(editor_left), r.bottom()),
    );
    let response = ui
        .scope_builder(egui::UiBuilder::new().max_rect(editor), |ui| {
            ui.set_clip_rect(ui.clip_rect().intersect(editor));
            ui.add_sized(
                editor.size(),
                egui::TextEdit::singleline(value)
                    .align(egui::Align2::LEFT_CENTER)
                    .min_size(vec2(0.0, height))
                    .margin(vec2(t.space_4, 0.0))
                    .id(id)
                    .char_limit(limit)
                    .frame(egui::Frame::NONE)
                    .hint_text(hint)
                    .font(FontId::proportional(t.font_label)),
            )
        })
        .inner;
    bound_bytes(value, limit);
    let focused = response.has_focus();
    ui.data_mut(|d| d.insert_temp(held, focused));
    FilterResponse {
        changed: changed || response.changed(),
    }
}

/// A message under a search field, inset like the group captions and
/// wrapped: the parse error (in the error ink) or the empty state.
pub fn search_message(ui: &mut Ui, text: &str, error: bool) {
    let t = Tokens::get();
    let ink = if error { t.form_error } else { t.form_dim };
    let inset = t.space_8;
    let width = (ui.available_width() - inset * 2.0).max(0.0);
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        FontId::proportional(t.font_small),
        ink,
        width,
    );
    job.wrap.max_rows = 3;
    let galley = ui.painter().layout_job(job);
    let (r, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), galley.size().y + t.space_6 * 2.0),
        Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, text));
    ui.painter()
        .galley(pos2(r.left() + inset, r.top() + t.space_6), galley, ink);
}

/// A key cap right-aligned at `right`; returns its width.
fn key_cap(ui: &Ui, right: egui::Pos2, label: &str, ink: egui::Color32) -> f32 {
    let t = Tokens::get();
    let padding = t.space_10;
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), FontId::monospace(t.font_small), ink);
    let size = galley.size() + vec2(padding, t.space_4);
    let r = Rect::from_min_size(pos2(right.x - size.x, right.y - size.y / 2.0), size);
    t.container_rect(
        ui.painter(),
        r,
        inner_radius(),
        t.dialog_footer,
        t.dialog_rule,
    );
    ui.painter()
        .galley(r.min + vec2(padding / 2.0, t.space_2), galley, ink);
    size.x
}

/// A group caption with its count (upstream `CommandSearch::heading`).
pub fn search_heading(ui: &mut Ui, label: &str, count: usize) {
    let t = Tokens::get();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), t.space_24), Sense::hover());
    let mut job = egui::text::LayoutJob::simple_singleline(
        format!("{}  {count}", label.to_uppercase()),
        fonts::semibold(t.font_small),
        t.form_dim,
    );
    for section in &mut job.sections {
        section.format.extra_letter_spacing = t.font_small * t.font_section_tracking;
    }
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(r.left() + t.space_8, r.center().y - galley.size().y / 2.0),
        galley,
        t.form_dim,
    );
}

/// The key-hint footer along the sheet's bottom edge (upstream
/// `CommandSearch::footer`): edge to edge, with the sheet's bottom corners
/// and a rule along its top.
fn search_footer(ui: &mut Ui, width: f32, text: &str) {
    let t = Tokens::get();
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::proportional(t.font_small),
        t.form_dim,
    );
    let (r, _) = ui.allocate_exact_size(
        vec2(width, galley.size().y + t.space_3 * 2.0),
        Sense::hover(),
    );
    ui.painter().rect_filled(
        r,
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: t.radius_xl,
            se: t.radius_xl,
        },
        t.dialog_footer,
    );
    t.secondary_hline(ui.painter(), r.x_range(), r.top(), t.dialog_rule);
    ui.painter().with_clip_rect(r).galley(
        pos2(r.left() + t.space_14, r.center().y - galley.size().y / 2.0),
        galley,
        t.form_dim,
    );
}

/// Parsed query chips under the field, and a dim trailing note.
pub fn search_chips(ui: &mut Ui, chips: &[String], trailing: &str) {
    let t = Tokens::get();
    if chips.is_empty() && trailing.is_empty() {
        return;
    }
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), t.space_24 + t.space_4),
        Sense::hover(),
    );
    let painter = ui.painter().with_clip_rect(r);
    let mut x = r.left() + t.space_4;
    let note = (!trailing.is_empty()).then(|| {
        painter.layout_no_wrap(
            trailing.to_owned(),
            FontId::proportional(t.font_small),
            t.form_dim,
        )
    });
    let limit = r.right() - note.as_ref().map_or(0.0, |g| g.size().x + t.space_12);
    for chip in chips {
        let galley =
            painter.layout_no_wrap(chip.clone(), FontId::proportional(t.font_small), t.form_ink);
        let size = galley.size() + vec2(t.space_16, t.space_6);
        if x + size.x > limit {
            break;
        }
        let rect = Rect::from_min_size(pos2(x, r.center().y - size.y / 2.0), size);
        t.container_rect(
            &painter,
            rect,
            inner_radius(),
            t.dialog_footer,
            t.dialog_rule,
        );
        painter.galley(rect.min + vec2(t.space_8, t.space_3), galley, t.form_ink);
        x = rect.right() + t.space_6;
    }
    if let Some(galley) = note {
        painter.galley(
            pos2(
                r.right() - t.space_4 - galley.size().x,
                r.center().y - galley.size().y / 2.0,
            ),
            galley,
            t.form_dim,
        );
    }
}

/// One completeness badge: present, or absent and dimmed with a dash.
pub struct SearchBadge<'a> {
    pub label: &'a str,
    pub present: bool,
    /// What the badge means, in the row's hover; empty for none.
    pub tip: &'a str,
}

/// One result row.
pub struct SearchRow<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    /// The typed text, for match highlighting.
    pub query: &'a str,
    /// The dim second line.
    pub detail: &'a str,
    pub badges: &'a [SearchBadge<'a>],
    pub highlighted: bool,
}

/// What a row marks besides its badges: a favourite star after the name,
/// and one coloured chip before the badges (an update or a changed record).
#[derive(Clone, Copy, Debug, Default)]
pub struct RowMarks<'a> {
    pub star: bool,
    pub chip: Option<RowChip<'a>>,
}

/// A coloured chip of [`RowMarks`].
#[derive(Clone, Copy, Debug)]
pub struct RowChip<'a> {
    pub label: &'a str,
    /// Draws as a warning (a changed source record) instead of the accent.
    pub warning: bool,
}

/// The 38 pt search row: icon tile, highlighted match, detail line and
/// badges (upstream `MenuItem::search_row`).
pub fn search_row(ui: &mut Ui, id: egui::Id, row: &SearchRow<'_>) -> Response {
    search_row_marked(ui, id, row, RowMarks::default())
}

/// [`search_row`] with a favourite star and a coloured chip.
pub fn search_row_marked(
    ui: &mut Ui,
    id: egui::Id,
    row: &SearchRow<'_>,
    marks: RowMarks<'_>,
) -> Response {
    let t = Tokens::get();
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), search_row_height()),
        Sense::hover(),
    );
    let response = ui.interact(r, id, Sense::click());
    // Only the highlighted row (the one Enter places and the preview shows)
    // takes the accent; hover is a neutral wash.
    let hot = row.highlighted;
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(r));
    if hot {
        painter.rect_filled(r, inner_radius(), t.accent_soft);
    } else if response.hovered() {
        painter.rect_filled(r, inner_radius(), t.dialog_footer);
    }
    let tile_size = t.space_24 + t.space_2;
    let tile = Rect::from_center_size(
        pos2(r.left() + t.space_8 + tile_size / 2.0, r.center().y),
        vec2(tile_size, tile_size),
    );
    painter.rect_filled(
        tile,
        inner_radius(),
        if hot { t.accent_soft } else { t.dialog_footer },
    );
    super::icon(
        &painter,
        tile.center(),
        tile_size - t.space_12,
        row.icon,
        1.0,
    );
    // Badges right to left.
    let mut right = r.right() - t.space_8;
    for badge in row.badges.iter().rev() {
        let (text, ink, fill) = if badge.present {
            (badge.label.to_owned(), t.form_ink, t.dialog_footer)
        } else {
            (
                format!("—{}", badge.label),
                t.form_dim.gamma_multiply(0.7),
                egui::Color32::TRANSPARENT,
            )
        };
        let galley = painter.layout_no_wrap(text, fonts::semibold(t.font_small), ink);
        let size = galley.size() + vec2(t.space_10, t.space_4);
        let rect = Rect::from_min_size(pos2(right - size.x, r.center().y - size.y / 2.0), size);
        if badge.present {
            painter.rect_filled(rect, inner_radius(), fill);
        } else {
            painter.rect_stroke(
                rect,
                inner_radius(),
                t.container_border(t.dialog_rule),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley(rect.min + vec2(t.space_10 / 2.0, t.space_2), galley, ink);
        right = rect.left() - t.space_4;
    }
    if let Some(chip) = marks.chip {
        let colour = if chip.warning { t.warning } else { t.accent };
        let galley =
            painter.layout_no_wrap(chip.label.to_owned(), fonts::semibold(t.font_small), colour);
        let size = galley.size() + vec2(t.space_10, t.space_4);
        let rect = Rect::from_min_size(pos2(right - size.x, r.center().y - size.y / 2.0), size);
        painter.rect_filled(rect, inner_radius(), colour.gamma_multiply(0.16));
        painter.rect_stroke(
            rect,
            inner_radius(),
            Stroke::new(1.0, colour.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
        painter.galley(rect.min + vec2(t.space_10 / 2.0, t.space_2), galley, colour);
        right = rect.left() - t.space_6;
    }
    let left = tile.right() + t.space_10;
    // The star sits at the end of the name line, inside the text column.
    let star = t.font_label + t.space_2;
    let right = if marks.star {
        super::icon(
            &painter,
            pos2(right - star / 2.0, r.top() + t.space_3 + star / 2.0),
            star,
            "star",
            1.0,
        );
        right - star - t.space_2
    } else {
        right
    };
    let width = (right - t.space_6 - left).max(0.0);
    let normal = egui::TextFormat::simple(FontId::proportional(t.font_label), t.form_ink);
    let mut job = egui::text::LayoutJob::default();
    match highlight(row.label, row.query) {
        Some((start, end)) => {
            job.append(
                row.label.get(..start).unwrap_or_default(),
                0.0,
                normal.clone(),
            );
            job.append(
                row.label.get(start..end).unwrap_or_default(),
                0.0,
                egui::TextFormat::simple(fonts::bold(t.font_label), t.selected_ink),
            );
            job.append(row.label.get(end..).unwrap_or_default(), 0.0, normal);
        }
        None => job.append(row.label, 0.0, normal),
    }
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    let galley = painter.layout_job(job);
    let mut cut = galley.elided;
    painter.galley(pos2(left, r.top() + t.space_3), galley, t.form_ink);
    let detail = super::text::one_line(
        ui,
        row.detail,
        FontId::proportional(t.font_small),
        t.form_dim,
        width,
    );
    cut |= detail.elided;
    painter.galley(
        pos2(left, r.bottom() - t.space_3 - detail.size().y),
        detail,
        t.form_dim,
    );
    let tips: Vec<String> = row
        .badges
        .iter()
        .filter(|b| !b.tip.is_empty())
        .map(|b| format!("{}: {}", b.label, b.tip))
        .collect();
    let response = super::text::hover(
        response,
        &full_row(row.label, row.detail),
        &tips.join("\n"),
        cut,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, row.label));
    response
}

/// A search row's full text for its hover: the name, then the detail.
fn full_row(label: &str, detail: &str) -> String {
    if detail.is_empty() {
        label.to_owned()
    } else {
        format!("{label}\n{detail}")
    }
}

/// One command palette row (upstream `MenuItem::search_row` with a
/// shortcut): icon tile, highlighted name, its context or why it is
/// unavailable on the second line, and its key cap at the right.
pub struct CommandRow<'a> {
    pub icon: &'a str,
    pub label: &'a str,
    pub query: &'a str,
    pub detail: &'a str,
    pub shortcut: Option<&'a str>,
    pub enabled: bool,
    pub highlighted: bool,
}

/// Draw a [`CommandRow`]. A disabled row is dimmed and only hovers; its
/// reason is its detail line and its hover text.
pub fn command_row(ui: &mut Ui, id: egui::Id, row: &CommandRow<'_>) -> Response {
    let t = Tokens::get();
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), search_row_height()),
        Sense::hover(),
    );
    let sense = if row.enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(r, id, sense);
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(r));
    if row.highlighted {
        painter.rect_filled(r, inner_radius(), t.accent_soft);
    } else if response.hovered() && row.enabled {
        painter.rect_filled(r, inner_radius(), t.dialog_footer);
    }
    let opacity = if row.enabled { 1.0 } else { 0.4 };
    let tile_size = t.space_24 + t.space_2;
    let tile = Rect::from_center_size(
        pos2(r.left() + t.space_8 + tile_size / 2.0, r.center().y),
        vec2(tile_size, tile_size),
    );
    painter.rect_filled(
        tile,
        inner_radius(),
        if row.highlighted {
            t.accent_soft
        } else {
            t.dialog_footer
        },
    );
    super::icon(
        &painter,
        tile.center(),
        tile_size - t.space_12,
        row.icon,
        opacity,
    );
    let (ink, dim) = if row.enabled {
        (t.form_ink, t.form_dim)
    } else {
        (
            t.form_dim.gamma_multiply(0.8),
            t.form_dim.gamma_multiply(0.7),
        )
    };
    let right = match row.shortcut {
        Some(key) => {
            let width = key_cap(ui, r.right_center() - vec2(t.space_8, 0.0), key, dim);
            r.right() - t.space_8 - width - t.space_8
        }
        None => r.right() - t.space_8,
    };
    let left = tile.right() + t.space_10;
    let width = (right - left).max(0.0);
    let normal = egui::TextFormat::simple(FontId::proportional(t.font_label), ink);
    let mut job = egui::text::LayoutJob::default();
    match highlight(row.label, row.query) {
        Some((start, end)) if row.enabled => {
            job.append(
                row.label.get(..start).unwrap_or_default(),
                0.0,
                normal.clone(),
            );
            job.append(
                row.label.get(start..end).unwrap_or_default(),
                0.0,
                egui::TextFormat::simple(fonts::bold(t.font_label), t.selected_ink),
            );
            job.append(row.label.get(end..).unwrap_or_default(), 0.0, normal);
        }
        _ => job.append(row.label, 0.0, normal),
    }
    job.wrap.max_width = width;
    job.wrap.max_rows = 1;
    let galley = painter.layout_job(job);
    let mut cut = galley.elided;
    painter.galley(pos2(left, r.top() + t.space_3), galley, ink);
    let detail = super::text::one_line(
        ui,
        row.detail,
        FontId::proportional(t.font_small),
        dim,
        width,
    );
    cut |= detail.elided;
    painter.galley(
        pos2(left, r.bottom() - t.space_3 - detail.size().y),
        detail,
        dim,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, row.enabled, row.label));
    if row.enabled {
        super::text::hover(response, &full_row(row.label, row.detail), "", cut)
    } else {
        // A disabled row's reason is its detail: it shows even when it fits.
        response.on_hover_text(row.detail)
    }
}

/// The first free-text term of `query` that occurs in `label`,
/// case-insensitively and by character boundaries (case mappings may change
/// UTF-8 lengths). Predicates and filters (`key>=…`, `pkg:…`) are skipped;
/// quotes are dropped.
fn highlight(label: &str, query: &str) -> Option<(usize, usize)> {
    query
        .split_whitespace()
        .filter(|term| !term.contains(['>', '<', '=', ':']))
        .find_map(|term| occurrence(label, term.trim_matches('"')))
}

fn occurrence(label: &str, term: &str) -> Option<(usize, usize)> {
    let q = term.to_lowercase();
    if q.is_empty() {
        return None;
    }
    label.char_indices().find_map(|(start, _)| {
        let mut lowered = String::new();
        for (offset, c) in label.get(start..)?.char_indices() {
            lowered.extend(c.to_lowercase());
            if lowered == q {
                return Some((start, start + offset + c.len_utf8()));
            }
            if !q.starts_with(&lowered) {
                break;
            }
        }
        None
    })
}

#[cfg(test)]
mod tests {
    use super::highlight;

    #[test]
    fn highlight_finds_the_first_match_by_character_boundaries() {
        assert_eq!(highlight("EX-R0603-10K0-F", "10k"), Some((9, 12)));
        assert_eq!(highlight("EX-C0402", "c0603"), None);
        assert_eq!(highlight("µ-Part", "µ-p"), Some((0, 4)));
        assert_eq!(highlight("anything", "  "), None);
        // Several terms: the first one found in the label; predicates skip.
        assert_eq!(highlight("EX-R0603-10K0-F", "10k 0603"), Some((9, 12)));
        assert_eq!(highlight("EX-R0603-10K0-F", "res>=1k 0603"), Some((4, 8)));
        assert_eq!(highlight("EX-R0603-10K0-F", "zz 0603"), Some((4, 8)));
        assert_eq!(highlight("EX-R0603", "pkg:0603"), None);
    }

    #[test]
    fn bound_bytes_cuts_on_a_character_boundary() {
        let mut text = "ΩΩΩ".to_owned();
        super::bound_bytes(&mut text, 5);
        assert_eq!(text, "ΩΩ");
        let mut short = "10k".to_owned();
        super::bound_bytes(&mut short, 5);
        assert_eq!(short, "10k");
    }
}
