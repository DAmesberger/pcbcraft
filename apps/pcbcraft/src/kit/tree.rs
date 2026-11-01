//! Browser tree rows and card headers (upstream `ui_kit/tree.rs`,
//! `browser.rs::browser_header` and `ui_kit/panel.rs::inspector_header`).
use egui::{FontId, Rect, Sense, Ui, pos2, vec2};
use solidcraft_ui_egui::{fonts, theme::Tokens};

/// Upstream `TREE_ROW_HEIGHT` and Browser indent.
pub const TREE_ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 14.0;
const DIM_OPACITY: f32 = 0.4;

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
    pub fold: bool,
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
    let mut right = r.right() - t.space_6;
    if !row.secondary.is_empty() {
        let galley = ui.painter().layout_no_wrap(
            row.secondary.to_owned(),
            FontId::proportional(t.font_small),
            t.text_dim,
        );
        right -= galley.size().x;
        ui.painter()
            .galley(pos2(right, cy - galley.size().y / 2.0), galley, t.text_dim);
        right -= t.space_6;
    }
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
    let mut job =
        egui::text::LayoutJob::simple(row.label.to_owned(), font, ink, (right - x).max(0.0));
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('\u{2026}');
    let galley = ui.painter().layout_job(job);
    let elided = galley.elided;
    ui.painter()
        .with_clip_rect(ui.clip_rect().intersect(r))
        .galley(pos2(x, cy - galley.size().y / 2.0), galley, ink);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            true,
            row.selected,
            row.label,
        )
    });
    let response = match (elided, row.tip.is_empty()) {
        (true, true) => response.on_hover_text(row.label),
        (true, false) => response.on_hover_text(format!("{}\n{}", row.label, row.tip)),
        (false, false) => response.on_hover_text(row.tip),
        (false, true) => response,
    };
    out.clicked = response.clicked() && !on_fold(response.interact_pointer_pos());
    out
}

/// Browser-style card title band: the title, and a dim summary on the right.
pub fn card_title(ui: &mut Ui, title: &str, summary: &str) {
    let t = Tokens::get();
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), TREE_ROW_HEIGHT), Sense::hover());
    let galley =
        ui.painter()
            .layout_no_wrap(title.to_owned(), fonts::semibold(t.font_heading), t.text);
    let title_right = rect.left() + t.space_6 + galley.size().x;
    ui.painter().galley(
        pos2(
            rect.left() + t.space_6,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        t.text,
    );
    let mut job = egui::text::LayoutJob::simple(
        summary.to_owned(),
        FontId::proportional(t.font_body),
        t.text_dim,
        (rect.right() - t.space_6 - title_right - t.space_12).max(0.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('\u{2026}');
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(
            rect.right() - t.space_6 - galley.size().x,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        t.text_dim,
    );
}

/// Inspector header: icon, the inspected object's name and a kind chip
/// (upstream `inspector_header` and `workspace_chip`).
pub fn inspector_header(ui: &mut Ui, icon: &str, title: &str, kind: &str) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(
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
    let mut job = egui::text::LayoutJob::simple(
        title.to_owned(),
        fonts::semibold(t.font_heading),
        t.form_ink,
        (name_right - left).max(0.0),
    );
    job.wrap.max_rows = 1;
    job.wrap.overflow_character = Some('\u{2026}');
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        pos2(left, rect.center().y - galley.size().y / 2.0),
        galley,
        t.form_ink,
    );
    t.secondary_hline(ui.painter(), rect.x_range(), rect.bottom(), t.dialog_rule);
}
