//! Search sheet, field and result rows (upstream `ui_kit/search.rs`:
//! `CommandSearch::{field, sheet, footer, heading}` and
//! `MenuItem::search_row` with its `SearchBadge` looks). Upstream keeps them
//! crate-private, so their geometry is reproduced on the public tokens,
//! fonts and icons. Ranking and what a row does stay with the caller.
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

/// The upstream menu frame the search sheet uses.
fn sheet_frame() -> egui::Frame {
    let t = Tokens::get();
    egui::Frame::NONE
        .fill(t.dialog_body)
        .stroke(t.primary_border(t.dialog_border))
        .corner_radius(t.radius_xl)
        .inner_margin(egui::Margin::symmetric(t.space_6 as i8, t.space_6 as i8))
        .shadow(egui::Shadow {
            offset: [0, t.space_14 as i8],
            blur: (t.space_24 + t.space_12) as u8,
            spread: 0,
            color: t.app_bar.gamma_multiply(0.18),
        })
}

/// A floating search sheet of `width` at `top` (its top-left corner), above
/// the canvas and cards. Returns the sheet's rectangle.
pub fn search_sheet(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    top: egui::Pos2,
    width: f32,
    add: impl FnOnce(&mut Ui),
) -> Rect {
    let host = Container::new(ctx, Surface::Popup);
    host.area(egui::Id::new(("search-sheet", id)))
        .fixed_pos(top)
        .movable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            sheet_frame()
                .show(ui, |ui| {
                    let t = Tokens::get();
                    ui.set_width((width - t.space_6 * 2.0 - 2.0).max(0.0));
                    ui.spacing_mut().item_spacing.y = 0.0;
                    super::form(ui, add);
                })
                .response
                .rect
        })
        .inner
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
    let ink = if invalid {
        t.form_error
    } else if focused {
        t.form_primary
    } else {
        t.control_border
    };
    if focused {
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
    super::icon(
        ui.painter(),
        r.left_center() + vec2(t.space_12 + t.space_16 / 2.0, 0.0),
        t.space_16,
        "zoom",
        1.0,
    );
    let key_width = key_cap(ui, r.right_center() - vec2(t.space_10, 0.0), "Esc");
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
                    .vertical_align(egui::Align::Center)
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
    if focus {
        response.request_focus();
    }
    response
}

/// A key cap right-aligned at `right`; returns its width.
fn key_cap(ui: &Ui, right: egui::Pos2, label: &str) -> f32 {
    let t = Tokens::get();
    let padding = t.space_10;
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        FontId::monospace(t.font_small),
        t.form_dim,
    );
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
        .galley(r.min + vec2(padding / 2.0, t.space_2), galley, t.form_dim);
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
/// `CommandSearch::footer`).
pub fn search_footer(ui: &mut Ui, text: &str) {
    let t = Tokens::get();
    ui.add_space(t.space_6);
    let width = ui.available_width();
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        FontId::proportional(t.font_small),
        t.form_dim,
    );
    let (r, _) = ui.allocate_exact_size(
        vec2(width, galley.size().y + t.space_3 * 2.0),
        Sense::hover(),
    );
    ui.painter().rect_filled(r, inner_radius(), t.dialog_footer);
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

/// The 38 pt search row: icon tile, highlighted match, detail line and
/// badges (upstream `MenuItem::search_row`).
pub fn search_row(ui: &mut Ui, id: egui::Id, row: &SearchRow<'_>) -> Response {
    let t = Tokens::get();
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), search_row_height()),
        Sense::hover(),
    );
    let response = ui.interact(r, id, Sense::click());
    let hot = row.highlighted || response.hovered();
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(r));
    if hot {
        painter.rect_filled(r, inner_radius(), t.accent_soft);
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
    let left = tile.right() + t.space_10;
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
    painter.galley(pos2(left, r.top() + t.space_3), galley, t.form_ink);
    let mut detail = egui::text::LayoutJob::simple_singleline(
        row.detail.to_owned(),
        FontId::proportional(t.font_small),
        t.form_dim,
    );
    detail.wrap.max_width = width;
    detail.wrap.max_rows = 1;
    let detail = painter.layout_job(detail);
    painter.galley(
        pos2(left, r.bottom() - t.space_3 - detail.size().y),
        detail,
        t.form_dim,
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, row.label));
    response
}

/// The first case-insensitive occurrence of `query` in `label`, by
/// character boundaries (case mappings may change UTF-8 lengths).
fn highlight(label: &str, query: &str) -> Option<(usize, usize)> {
    let q = query.trim().to_lowercase();
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
    }
}
