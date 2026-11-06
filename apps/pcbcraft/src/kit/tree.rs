//! Browser tree rows and card headers (upstream `ui_kit/tree.rs`,
//! `browser.rs::browser_header` and `ui_kit/panel.rs::inspector_header`).
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use solidcraft_ui_egui::{fonts, theme::Tokens};

/// Upstream `TREE_ROW_HEIGHT` and Browser indent.
pub const TREE_ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 14.0;
const DIM_OPACITY: f32 = 0.4;
/// Width added when a text is laid out at its own measured width, so
/// rounding never cuts a text that fits.
const FIT_SLACK: f32 = 0.5;

/// One Browser row. Feature code fills it; the kit draws it.
#[derive(Default)]
pub struct Row<'a> {
    pub depth: usize,
    /// `Some(open)` for rows that fold.
    pub fold: Option<bool>,
    pub icon: &'a str,
    pub label: &'a str,
    /// Dim trailing text: a count, a revision.
    pub secondary: &'a str,
    pub selected: bool,
    /// The row leads to the selection (an ancestor of it): a faint bar.
    pub branch: bool,
    pub dim: bool,
    /// Unpublished or unsaved changes: an accent dot.
    pub modified: bool,
    /// The most severe issue on the object: its severity glyph at the
    /// row's right (docs/sidebar-ia.md, "Canvas and Browser").
    pub issue: Option<super::severity::Level>,
    pub tip: &'a str,
}

#[derive(Default)]
pub struct RowResponse {
    pub clicked: bool,
    /// The second click of a double-click (its first click was `clicked`).
    pub double: bool,
    pub fold: bool,
    /// A right-click on the row: where, for its context menu.
    pub menu: Option<egui::Pos2>,
}

/// Draw one Browser row with a stable `id` (a press and its release stay on
/// the same row while rows above fold).
pub fn tree_row(ui: &mut Ui, id: egui::Id, row: &Row<'_>) -> RowResponse {
    let t = Tokens::get();
    let (r, _) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), TREE_ROW_HEIGHT),
        Sense::hover(),
    );
    if !ui.is_rect_visible(r) {
        return RowResponse::default();
    }
    let response = ui.interact(r, id, Sense::click());
    let accent = t.accent;
    if row.selected {
        ui.painter()
            .rect_filled(r, t.radius_sm.saturating_sub(1), t.selected_bg);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(r, t.radius_sm.saturating_sub(1), t.hover);
    }
    let bar = if row.selected {
        Some((3.0, accent))
    } else if row.branch {
        Some((2.0, accent.gamma_multiply(0.4)))
    } else {
        None
    };
    if let Some((width, colour)) = bar {
        ui.painter().rect_filled(
            Rect::from_min_size(
                r.left_top() + vec2(0.0, t.space_2),
                vec2(width, r.height() - 4.0),
            ),
            t.radius_sm / 4,
            colour,
        );
    }
    let cy = r.center().y;
    let mut x = r.left() + 2.0 + row.depth as f32 * INDENT;
    let fold_rect = Rect::from_min_max(pos2(x - 4.0, r.top()), pos2(x + 15.0, r.bottom()));
    let on_fold =
        |p: Option<egui::Pos2>| row.fold.is_some() && p.is_some_and(|p| fold_rect.contains(p));
    let mut out = RowResponse::default();
    if let Some(open) = row.fold {
        let ink = if on_fold(response.hover_pos()) {
            t.text
        } else {
            t.text_dim
        };
        super::disclosure(ui.painter(), pos2(x + 6.0, cy), open, ink);
        out.fold = response.clicked() && on_fold(response.interact_pointer_pos());
    }
    x += 16.0;
    if !row.icon.is_empty() {
        super::icon(
            ui.painter(),
            pos2(x + 8.0, cy),
            t.space_16,
            row.icon,
            if row.dim { DIM_OPACITY } else { 1.0 },
        );
        x += 21.0;
    }
    // The marks at the right are laid out first and always keep their
    // place; the label and the secondary text share what is left.
    let mut right = r.right() - t.space_6;
    if let Some(level) = row.issue {
        let size = t.space_12;
        right -= size / 2.0;
        super::severity::severity_glyph(ui.painter(), pos2(right, cy), size, level);
        right -= size / 2.0 + t.space_6;
    }
    if row.modified {
        right -= t.space_4;
        ui.painter()
            .circle_filled(pos2(right, cy), t.space_3, accent);
        right -= t.space_6;
    }
    let ink = if row.selected {
        t.selected_ink
    } else if row.dim {
        t.text_dim
    } else {
        t.text
    };
    let font = if row.selected {
        fonts::semibold(t.font_label)
    } else {
        FontId::proportional(t.font_label)
    };
    let secondary_font = FontId::proportional(t.font_small);
    let natural = |text: &str, font: &FontId| {
        ui.painter()
            .layout_no_wrap(text.to_owned(), font.clone(), ink)
            .size()
            .x
    };
    let room = (right - x).max(0.0);
    let split = share(
        room,
        natural(row.label, &font),
        if row.secondary.is_empty() {
            None
        } else {
            Some(natural(row.secondary, &secondary_font))
        },
        t.space_6,
    );
    let clip = ui.painter().with_clip_rect(ui.clip_rect().intersect(r));
    let mut secondary_cut = false;
    if !row.secondary.is_empty() {
        let galley = super::text::one_line(
            ui,
            row.secondary,
            secondary_font,
            t.text_dim,
            split.secondary + FIT_SLACK,
        );
        secondary_cut = galley.elided;
        clip.galley(
            pos2(right - galley.size().x, cy - galley.size().y / 2.0),
            galley,
            t.text_dim,
        );
    }
    let galley = super::text::one_line(ui, row.label, font, ink, split.label + FIT_SLACK);
    let elided = galley.elided;
    clip.galley(pos2(x, cy - galley.size().y / 2.0), galley, ink);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            true,
            row.selected,
            row.label,
        )
    });
    let full = match (elided, secondary_cut) {
        (true, true) => format!("{}\n{}", row.label, row.secondary),
        (true, false) => row.label.to_owned(),
        (false, true) => row.secondary.to_owned(),
        (false, false) => String::new(),
    };
    let response = match (full.is_empty(), row.tip.is_empty()) {
        (false, true) => response.on_hover_text(full),
        (false, false) => response.on_hover_text(format!("{full}\n{}", row.tip)),
        (true, false) => response.on_hover_text(row.tip),
        (true, true) => response,
    };
    out.clicked = response.clicked() && !on_fold(response.interact_pointer_pos());
    out.double = response.double_clicked() && !on_fold(response.interact_pointer_pos());
    out.menu = response
        .secondary_clicked()
        .then(|| response.interact_pointer_pos())
        .flatten();
    out
}

/// How a row's label and secondary text share `room`: their widths, a
/// gap apart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Share {
    label: f32,
    secondary: f32,
}

/// Split `room` between a label of natural width `label` and an optional
/// secondary text of natural width `secondary`, `gap` apart. What fits is
/// shown whole. Otherwise the shorter one keeps its width while it needs
/// at most half, and the other is cut to the rest; two long texts get half
/// each. Neither ever pushes the other out of the row (upstream caps the
/// trailing texts at a share of the row the same way).
fn share(room: f32, label: f32, secondary: Option<f32>, gap: f32) -> Share {
    let room = room.max(0.0);
    let Some(secondary) = secondary else {
        return Share {
            label: room,
            secondary: 0.0,
        };
    };
    let text = (room - gap).max(0.0);
    let half = text / 2.0;
    let (label, secondary) = if label + secondary <= text {
        (label, secondary)
    } else if label <= half {
        (label, text - label)
    } else if secondary <= half {
        (text - secondary, secondary)
    } else {
        (half, half)
    };
    Share {
        label: label.max(0.0),
        secondary: secondary.max(0.0),
    }
}

/// Browser-style card title band: the title, and a dim summary on the right.
pub fn card_title(ui: &mut Ui, title: &str, summary: &str) {
    let t = Tokens::get();
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), TREE_ROW_HEIGHT), Sense::hover());
    let galley = super::text::one_line(
        ui,
        title,
        fonts::semibold(t.font_heading),
        t.text,
        (rect.width() - t.space_6 * 2.0).max(0.0),
    );
    let mut cut = galley.elided;
    let title_right = rect.left() + t.space_6 + galley.size().x;
    ui.painter().galley(
        pos2(
            rect.left() + t.space_6,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        t.text,
    );
    let galley = super::text::one_line(
        ui,
        summary,
        FontId::proportional(t.font_body),
        t.text_dim,
        (rect.right() - t.space_6 - title_right - t.space_12).max(0.0),
    );
    cut |= galley.elided;
    ui.painter().galley(
        pos2(
            rect.right() - t.space_6 - galley.size().x,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        t.text_dim,
    );
    let full = if summary.is_empty() {
        title.to_owned()
    } else {
        format!("{title} · {summary}")
    };
    super::text::hover(response, &full, "", cut);
}

/// Inspector header: icon, the inspected object's name and a kind chip
/// (upstream `inspector_header` and `workspace_chip`).
pub fn inspector_header(ui: &mut Ui, icon: &str, title: &str, kind: &str) {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width(), t.form_header_height),
        Sense::hover(),
    );
    let icon_center = pos2(
        rect.left() + t.space_12 + t.form_icon_size / 2.0,
        rect.center().y,
    );
    super::icon(ui.painter(), icon_center, t.form_icon_size, icon, 1.0);
    let mut name_right = rect.right() - t.space_12;
    if !kind.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            kind.to_owned(),
            fonts::semibold(t.font_small),
            t.selected_ink,
        );
        let chip = Rect::from_center_size(
            pos2(
                name_right - (galley.size().x + t.space_16) / 2.0,
                rect.center().y,
            ),
            vec2(galley.size().x + t.space_16, t.space_16 + t.space_4),
        );
        ui.painter().rect_filled(chip, t.radius_xl, t.selected_bg);
        ui.painter()
            .galley(chip.center() - galley.size() / 2.0, galley, t.selected_ink);
        name_right = chip.left() - t.space_8;
    }
    let left = icon_center.x + t.form_icon_size / 2.0 + t.space_8;
    let galley = super::text::one_line(
        ui,
        title,
        fonts::semibold(t.font_heading),
        t.form_ink,
        (name_right - left).max(0.0),
    );
    let cut = galley.elided;
    ui.painter().galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        t.form_ink,
    );
    super::text::hover(response, title, "", cut);
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
