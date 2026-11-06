//! The Issues surface widgets: rows, the grouped list, the card's one-line
//! issue slot, the always-visible bar, the canvas count badge and the
//! disclosed details.
//!
//! docs/sidebar-ia.md, "Issues": these widgets lay out what core's
//! `project.issues` returns and never change a count. Rows are buttons (a
//! click locates, never selects text); the list is grouped, uncapped and
//! virtualised: rows outside the viewport only take their space and their
//! view is never built. Counts are drawn glyphs followed by numbers
//! ("× 4 ! 2 ○ 36"), with the plain words as the accessible name.
//!
//! Adapted from Solidcraft 4f3ac07a `crates/ui-egui/src/ui_kit/status.rs`
//! (`check_status_line`), `reports.rs` (`report_list`, the `report_header`
//! ×n pill), `tree.rs` (`tree_row` culling and stable row ids,
//! `tree_header`) and `checks.rs`, MIT option; see ATTRIBUTION.md. Upstream
//! keeps these crate-private, so the geometry is reproduced here on the
//! public tokens and fonts.
use super::severity::{Level, severity_glyph};
use egui::{
    Align, Color32, CursorIcon, FontId, Galley, Layout, Rect, Response, RichText, Sense, Stroke,
    Ui, UiBuilder, WidgetInfo, WidgetType, pos2, vec2,
};
use pcbcraft_core::issues::{IssueCounts, IssueDetail};
use solidcraft_ui_egui::{fonts, theme::Tokens, ui_kit};
use std::sync::Arc;

/// The Issues bar's height while collapsed, in points.
pub const ISSUES_BAR_HEIGHT: f32 = 28.0;

/// One closed row of the list (and a group header), in points: the Browser
/// row height, so both lists read alike.
pub const ISSUE_ROW_HEIGHT: f32 = super::TREE_ROW_HEIGHT;

/// How far a group's rows sit right of its header.
const INDENT: f32 = 14.0;

/// One issue row as shown: disclosure, glyph, one-line title, a ×n pill
/// when `count` is above one, an optional Fix and locate.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueRowView {
    pub level: Level,
    /// At most 80 characters (core's `MAX_TITLE`); clipped to the row with
    /// the full text on hover.
    pub title: String,
    pub count: usize,
    /// The Fix button's label, only when a typed command or a dialog that
    /// issues one resolves the issue.
    pub fix: Option<String>,
    /// Whether the details are disclosed.
    pub open: bool,
    /// Cause and fix, shown under the row while `open`.
    pub detail: IssueDetail,
    /// The user names of the objects the issue refers to, in the finding's
    /// `ObjectRef` order; shown as chips while `open`.
    pub objects: Vec<String>,
}

impl IssueRowView {
    /// Whether the row has anything to disclose.
    fn disclosable(&self) -> bool {
        self.detail.cause.is_some() || self.detail.fix.is_some() || !self.objects.is_empty()
    }
}

/// What a row's click did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct IssueRowResponse {
    /// The row itself or its locate button: select and frame every
    /// referenced object.
    pub locate: bool,
    pub fix: bool,
    /// The disclosure.
    pub toggle: bool,
    /// An object chip: select and frame only that object (an index into
    /// [`IssueRowView::objects`]).
    pub object: Option<usize>,
    /// A right-click on the row: where, for its context menu.
    pub menu: Option<egui::Pos2>,
}

/// One issue row. `id` keeps its interaction stable while the list scrolls.
#[allow(
    dead_code,
    reason = "S0 contract; a single row outside the list has no caller yet"
)]
pub fn issue_row(ui: &mut Ui, id: egui::Id, row: &IssueRowView) -> IssueRowResponse {
    row_at(ui, id, row, 0.0)
}

fn row_at(ui: &mut Ui, id: egui::Id, row: &IssueRowView, indent: f32) -> IssueRowResponse {
    let t = Tokens::get();
    let mut out = IssueRowResponse::default();
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), ISSUE_ROW_HEIGHT),
        Sense::hover(),
    );
    let response = ui.interact(rect, id, Sense::click());
    if response.contains_pointer() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let cy = rect.center().y;
    let mut x = rect.left() + t.space_6 + indent;

    let disclosure = Rect::from_min_size(
        pos2(x, cy - t.form_section_icon_size / 2.0),
        vec2(t.form_section_icon_size, t.form_section_icon_size),
    );
    if row.disclosable() {
        let toggle = ui
            .interact(
                disclosure.expand(t.space_2),
                id.with("toggle"),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        super::chevron(ui.painter(), disclosure, row.open, t.text_dim);
        out.toggle = toggle.clicked();
    }
    x = disclosure.right() + t.space_4;
    severity_glyph(
        ui.painter(),
        pos2(x + t.space_14 / 2.0, cy),
        t.space_14,
        row.level,
    );
    x += t.space_14 + t.space_6;

    let mut right = rect.right() - t.space_6;
    // Locate, Fix and the ×n pill show only while the row is hovered, so
    // the title has the row's width the rest of the time.
    if ui.rect_contains_pointer(rect) {
        if locate_button(ui, id.with("locate"), &mut right, cy).clicked() {
            out.locate = true;
        }
        if let Some(fix) = &row.fix {
            let fix = trailing_button(ui, rect, x, &mut right, fix);
            out.fix = fix.clicked();
        }
        if row.count > 1 {
            right = count_pill(ui, right, cy, row.count);
        }
    }
    let title = clipped(
        ui,
        &row.title,
        FontId::proportional(t.font_label),
        t.text,
        right - x,
    );
    let elided = title.elided;
    ui.painter()
        .galley(pos2(x, cy - title.size().y / 2.0), title, t.text);

    let label = format!("{}: {}", row.level.count_words(1), row.title);
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &label));
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let response = if elided {
        response.on_hover_text(&row.title)
    } else {
        response
    };
    out.locate |= response.clicked();
    out.menu = response
        .secondary_clicked()
        .then(|| response.interact_pointer_pos())
        .flatten();

    if row.open && row.disclosable() {
        let inset = x - rect.left();
        ui.horizontal(|ui| {
            ui.add_space(inset);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(t.space_4, t.space_4);
                ui.add_space(t.space_2);
                detail(ui, &row.detail);
                if !row.objects.is_empty() {
                    out.object = object_chips(ui, id, &row.objects);
                }
                ui.add_space(t.space_4);
            });
        });
    }
    out
}

/// The ⌖ button at the row's right end; moves `right` past it.
fn locate_button(ui: &Ui, id: egui::Id, right: &mut f32, cy: f32) -> Response {
    let t = Tokens::get();
    let size = t.space_16 + t.space_4;
    let rect = Rect::from_center_size(pos2(*right - size / 2.0, cy), vec2(size, size));
    *right = rect.left() - t.space_4;
    let response = ui
        .interact(rect, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Select and show on the canvas");
    if response.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let ink = if response.hovered() {
        t.text
    } else {
        t.text_dim
    };
    let c = rect.center();
    let r = t.space_6;
    let stroke = Stroke::new(1.2, ink);
    ui.painter().circle_stroke(c, r * 0.75, stroke);
    for d in [
        vec2(1.0, 0.0),
        vec2(-1.0, 0.0),
        vec2(0.0, 1.0),
        vec2(0.0, -1.0),
    ] {
        ui.painter()
            .line_segment([c + d * (r * 0.45), c + d * (r * 1.3)], stroke);
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), "Locate"));
    response
}

/// A small kit button right-aligned before `right` (and never left of
/// `left`), in the row band; moves `right` past it.
fn trailing_button(ui: &mut Ui, band: Rect, left: f32, right: &mut f32, label: &str) -> Response {
    let t = Tokens::get();
    let slot = Rect::from_min_max(
        pos2(left.min(*right), band.top()),
        pos2(*right, band.bottom()),
    );
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(slot)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let response = ui_kit::button(&mut child, label, ui_kit::Button::Small);
    *right = response.rect.left() - t.space_6;
    response
}

/// The ×n pill ending at `right`; returns the new right edge.
fn count_pill(ui: &Ui, right: f32, cy: f32, count: usize) -> f32 {
    let t = Tokens::get();
    let text = galley(
        ui,
        &format!("×{}", super::severity::compact(count)),
        FontId::proportional(t.font_small),
        t.form_dim,
    );
    let size = vec2(text.size().x + t.space_12, t.space_16 + t.space_2);
    let pill = Rect::from_min_size(pos2(right - size.x, cy - size.y / 2.0), size);
    ui.painter()
        .rect_filled(pill, t.control_radius, t.dialog_footer);
    ui.painter()
        .galley(pill.center() - text.size() / 2.0, text, t.form_dim);
    pill.left() - t.space_6
}

/// The object chips of an open row; returns the clicked chip.
fn object_chips(ui: &mut Ui, id: egui::Id, objects: &[String]) -> Option<usize> {
    let t = Tokens::get();
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(t.space_4, t.space_4);
        for (i, name) in objects.iter().enumerate() {
            let text = clipped(
                ui,
                name,
                FontId::proportional(t.font_small),
                t.text,
                ui.available_width().max(t.space_24) - t.space_16,
            );
            let size = vec2(text.size().x + t.space_16, t.space_16 + t.space_4);
            let cut = text.elided;
            let (rect, response) = ui.allocate_exact_size(size, Sense::click());
            let response = ui
                .interact(rect, id.with(("object", i)), Sense::click())
                .union(response)
                .on_hover_cursor(CursorIcon::PointingHand);
            let response = super::text::hover(response, name, "", cut);
            let fill = if response.hovered() {
                t.hover
            } else {
                t.control_fill
            };
            ui.painter().rect(
                rect,
                t.radius_xl,
                fill,
                t.control_border_stroke(t.control_border),
                egui::StrokeKind::Inside,
            );
            ui.painter()
                .galley(rect.center() - text.size() / 2.0, text, t.text);
            response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, name));
            if response.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// One group of the list: its header and how many rows it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct IssueGroupView {
    pub level: Level,
    pub title: String,
    pub rows: usize,
    pub open: bool,
    /// A group-wide Fix, such as "Update PCB…" for "Not placed".
    pub fix: Option<String>,
}

/// What happened in the list this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IssueListEvent {
    /// A group header's disclosure.
    Group(usize),
    /// A group's Fix.
    GroupFix(usize),
    Row {
        group: usize,
        row: usize,
        action: IssueRowResponse,
    },
}

/// The grouped list: every group and every row, never capped. `row(group,
/// index)` gives a row's view only when it is drawn; rows outside the
/// viewport take their last measured height without being built. The list
/// adds no scroll of its own: it lives in the card body's one scroll.
pub fn issues_list(
    ui: &mut Ui,
    id: egui::Id,
    groups: &[IssueGroupView],
    mut row: impl FnMut(usize, usize) -> IssueRowView,
) -> Option<IssueListEvent> {
    let mut event = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut skip = 0.0_f32;
        for (g, group) in groups.iter().enumerate() {
            let header = group_height(group);
            if shown(ui, skip, header) {
                flush(ui, &mut skip);
                let (toggle, fix) = group_header(ui, id.with(("group", g)), group);
                if toggle {
                    event = Some(IssueListEvent::Group(g));
                } else if fix {
                    event = Some(IssueListEvent::GroupFix(g));
                }
            } else {
                skip += header;
            }
            if !group.open {
                continue;
            }
            for r in 0..group.rows {
                let row_id = id.with(("row", g, r));
                let height = ui
                    .data(|d| d.get_temp::<f32>(row_id))
                    .unwrap_or(ISSUE_ROW_HEIGHT);
                if !shown(ui, skip, height) {
                    skip += height;
                    continue;
                }
                flush(ui, &mut skip);
                let top = ui.cursor().top();
                let action = row_at(ui, row_id, &row(g, r), INDENT);
                let used = ui.cursor().top() - top;
                if used.is_finite() && used > 0.0 {
                    ui.data_mut(|d| d.insert_temp(row_id, used));
                }
                if action != IssueRowResponse::default() {
                    event = Some(IssueListEvent::Row {
                        group: g,
                        row: r,
                        action,
                    });
                }
            }
        }
        flush(ui, &mut skip);
    });
    event
}

/// Whether an item of `height`, `skip` below the cursor, is in view.
fn shown(ui: &Ui, skip: f32, height: f32) -> bool {
    let min = ui.cursor().min + vec2(0.0, skip);
    ui.is_rect_visible(Rect::from_min_size(
        min,
        vec2(ui.available_width().max(1.0), height),
    ))
}

/// Take the space of the items skipped so far.
fn flush(ui: &mut Ui, skip: &mut f32) {
    if *skip > 0.0 {
        ui.allocate_space(vec2(ui.available_width().max(0.0), *skip));
        *skip = 0.0;
    }
}

/// The height a group header takes: one row, and a second for the group's
/// Fix while the group is open.
fn group_height(group: &IssueGroupView) -> f32 {
    if group.open && group.fix.is_some() {
        2.0 * ISSUE_ROW_HEIGHT
    } else {
        ISSUE_ROW_HEIGHT
    }
}

/// A group header: disclosure, glyph, title and row count, and the group's
/// Fix: on its own line under the header while the group is open, and at
/// the header's end on hover while it is folded, so the title keeps the
/// row's width. A click anywhere but Fix folds or unfolds it.
fn group_header(ui: &mut Ui, id: egui::Id, group: &IssueGroupView) -> (bool, bool) {
    let t = Tokens::get();
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), ISSUE_ROW_HEIGHT),
        Sense::hover(),
    );
    let response = ui.interact(rect, id, Sense::click());
    if response.contains_pointer() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let cy = rect.center().y;
    let disclosure = Rect::from_min_size(
        pos2(rect.left() + t.space_6, cy - t.form_section_icon_size / 2.0),
        vec2(t.form_section_icon_size, t.form_section_icon_size),
    );
    super::chevron(ui.painter(), disclosure, group.open, t.text_dim);
    let mut x = disclosure.right() + t.space_4;
    severity_glyph(
        ui.painter(),
        pos2(x + t.space_14 / 2.0, cy),
        t.space_14,
        group.level,
    );
    x += t.space_14 + t.space_6;
    let mut right = rect.right() - t.space_6;
    let mut fix = !group.open
        && ui.rect_contains_pointer(rect)
        && group
            .fix
            .as_ref()
            .is_some_and(|fix| trailing_button(ui, rect, x, &mut right, fix).clicked());
    let count = galley(
        ui,
        &super::severity::grouped(group.rows),
        FontId::proportional(t.font_small),
        t.form_dim,
    );
    let title = clipped(
        ui,
        &group.title,
        fonts::semibold(t.font_label),
        t.text,
        right - x - count.size().x - t.space_6,
    );
    let elided = title.elided;
    let count_x = x + title.size().x + t.space_6;
    ui.painter()
        .galley(pos2(x, cy - title.size().y / 2.0), title, t.text);
    ui.painter()
        .galley(pos2(count_x, cy - count.size().y / 2.0), count, t.form_dim);
    let label = format!("{}: {}", group.title, group.level.count_words(group.rows));
    response.widget_info(|| {
        WidgetInfo::selected(WidgetType::CollapsingHeader, true, group.open, &label)
    });
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let response = if elided {
        response.on_hover_text(&group.title)
    } else {
        response
    };
    if group.open
        && let Some(label) = &group.fix
    {
        let (line, _) = ui.allocate_exact_size(
            vec2(ui.available_width().max(0.0), ISSUE_ROW_HEIGHT),
            Sense::hover(),
        );
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2(x, line.top()), line.max))
                .layout(Layout::left_to_right(Align::Center)),
        );
        fix = ui_kit::button(&mut child, label, ui_kit::Button::Small).clicked();
    }
    (response.clicked(), fix)
}

/// The levels a count summary shows, most severe first, skipping zeros.
fn segments(counts: &IssueCounts, to_do: bool, info: bool) -> Vec<(Level, usize)> {
    [
        (Level::Error, counts.error),
        (Level::Warning, counts.warning),
        (Level::ToDo, if to_do { counts.to_do } else { 0 }),
        (Level::Info, if info { counts.info } else { 0 }),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .collect()
}

/// The counts in words, the accessible name of a count summary:
/// "4 errors, 2 warnings, 36 to do", or "No issues".
pub fn counts_words(counts: &IssueCounts, to_do: bool, info: bool) -> String {
    let parts: Vec<String> = segments(counts, to_do, info)
        .into_iter()
        .map(|(level, n)| level.count_words(n))
        .collect();
    if parts.is_empty() {
        Level::Ok.count_words(0)
    } else {
        parts.join(", ")
    }
}

/// Glyph and number pairs laid out from `left` at height `cy`; returns the
/// right edge. With nothing to count it draws the passed glyph alone.
fn paint_counts(ui: &Ui, left: f32, cy: f32, segs: &[(Level, usize)], font: &FontId) -> f32 {
    let t = Tokens::get();
    let glyph = font.size;
    if segs.is_empty() {
        severity_glyph(ui.painter(), pos2(left + glyph / 2.0, cy), glyph, Level::Ok);
        return left + glyph;
    }
    let mut x = left;
    for (i, (level, n)) in segs.iter().enumerate() {
        if i > 0 {
            x += t.space_10;
        }
        severity_glyph(ui.painter(), pos2(x + glyph / 2.0, cy), glyph, *level);
        x += glyph + t.space_4;
        let number = galley(ui, &super::severity::compact(*n), font.clone(), t.text);
        let width = number.size().x;
        ui.painter()
            .galley(pos2(x, cy - number.size().y / 2.0), number, t.text);
        x += width;
    }
    x
}

/// The width [`paint_counts`] takes.
fn counts_width(ui: &Ui, segs: &[(Level, usize)], font: &FontId) -> f32 {
    let t = Tokens::get();
    let glyph = font.size;
    if segs.is_empty() {
        return glyph;
    }
    segs.iter()
        .map(|(_, n)| glyph + t.space_4 + galley(ui, &n.to_string(), font.clone(), t.text).size().x)
        .sum::<f32>()
        + t.space_10 * (segs.len().saturating_sub(1)) as f32
}

/// The card's reserved one-line object-issue slot ("× 1 ! 1 on R1 ›").
/// It always takes its row, so the card does not reflow when issues appear;
/// it is blank and inert while the object has none. A click opens Issues
/// scoped to this object.
pub fn issue_line(ui: &mut Ui, counts: &IssueCounts, label: &str) -> Response {
    let t = Tokens::get();
    let segs = segments(counts, true, true);
    let size = vec2(ui.available_width().max(0.0), t.control_height);
    if segs.is_empty() {
        return ui.allocate_exact_size(size, Sense::hover()).1;
    }
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    let cy = rect.center().y;
    let font = FontId::proportional(t.font_label);
    let end = paint_counts(ui, rect.left() + t.space_6, cy, &segs, &font);
    let chevron_size = t.form_section_icon_size;
    let chevron = Rect::from_center_size(
        pos2(rect.right() - t.space_6 - chevron_size / 2.0, cy),
        vec2(chevron_size, chevron_size),
    );
    super::chevron(ui.painter(), chevron, false, t.text_dim);
    let words = clipped(
        ui,
        label,
        font,
        t.text_dim,
        chevron.left() - t.space_6 - end - t.space_8,
    );
    ui.painter().galley(
        pos2(end + t.space_8, cy - words.size().y / 2.0),
        words,
        t.text_dim,
    );
    let name = format!("{} {label}", counts_words(counts, true, true));
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &name));
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Show in Issues")
}

/// The always-visible Issues bar: disclosure, title and every count (the
/// bar never hides To do), then the `elsewhere` blocker line ("Component:
/// 2 block publishing") at its right. A click expands or collapses the
/// card.
pub fn issues_bar(
    ui: &mut Ui,
    counts: &IssueCounts,
    expanded: bool,
    elsewhere: Option<&str>,
) -> Response {
    let t = Tokens::get();
    let (rect, response) = ui.allocate_exact_size(
        vec2(ui.available_width().max(0.0), ISSUES_BAR_HEIGHT),
        Sense::click(),
    );
    if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    t.secondary_hline(ui.painter(), rect.x_range(), rect.top(), t.dialog_rule);
    let cy = rect.center().y;
    let disclosure = Rect::from_min_size(
        pos2(rect.left() + t.space_8, cy - t.form_section_icon_size / 2.0),
        vec2(t.form_section_icon_size, t.form_section_icon_size),
    );
    super::chevron(ui.painter(), disclosure, expanded, t.text_dim);
    let title = galley(ui, "Issues", fonts::semibold(t.font_label), t.text);
    let mut x = disclosure.right() + t.space_6;
    let title_width = title.size().x;
    let segs = segments(counts, true, true);
    let font = FontId::proportional(t.font_label);
    // A narrow column drops the word "Issues" before it clips the
    // blockers elsewhere; the counts and the note always show.
    let note = elsewhere.filter(|e| !e.is_empty()).map_or(0.0, |e| {
        galley(ui, e, FontId::proportional(t.font_small), t.text_dim)
            .size()
            .x
            + t.space_16
            + t.space_10
    });
    // With nothing to count the bar says "No issues" after the glyph.
    let none_width = if segs.is_empty() {
        t.space_4
            + galley(ui, &Level::Ok.count_words(0), font.clone(), t.text_dim)
                .size()
                .x
    } else {
        0.0
    };
    let fits = x + title_width + t.space_10 + counts_width(ui, &segs, &font) + none_width + note
        <= rect.right() - t.space_8;
    if fits || note == 0.0 {
        ui.painter()
            .galley(pos2(x, cy - title.size().y / 2.0), title, t.text);
        x += title_width + t.space_10;
    }
    x = paint_counts(ui, x, cy, &segs, &font);
    if segs.is_empty() {
        let none = galley(ui, &Level::Ok.count_words(0), font, t.text_dim);
        let width = none.size().x;
        ui.painter().galley(
            pos2(x + t.space_4, cy - none.size().y / 2.0),
            none,
            t.text_dim,
        );
        // The elsewhere note clips against the end of "No issues".
        x += t.space_4 + width;
    }
    let mut name = format!("Issues: {}", counts_words(counts, true, true));
    if let Some(elsewhere) = elsewhere.filter(|e| !e.is_empty()) {
        let right = rect.right() - t.space_8;
        let dot = t.space_6;
        let text = clipped(
            ui,
            elsewhere,
            FontId::proportional(t.font_small),
            t.text_dim,
            (right - x - t.space_16 - dot - t.space_4).max(0.0),
        );
        let text_x = right - text.size().x;
        ui.painter().circle_filled(
            pos2(text_x - t.space_4 - dot / 2.0, cy),
            dot / 2.0,
            super::severity::ink(Level::Error),
        );
        ui.painter()
            .galley(pos2(text_x, cy - text.size().y / 2.0), text, t.text_dim);
        name = format!("{name}; {elsewhere}");
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), expanded, &name));
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&name)
}

/// The counts as a compact badge (ribbon toggle, canvas). Info is not
/// counted here. To do is hidden while errors exist when
/// `hide_to_do_with_errors`; the bar never hides it.
pub fn count_badge(ui: &mut Ui, counts: &IssueCounts, hide_to_do_with_errors: bool) -> Response {
    let t = Tokens::get();
    let to_do = !(hide_to_do_with_errors && counts.error > 0);
    let segs = segments(counts, to_do, false);
    let font = FontId::proportional(t.font_small);
    let size = vec2(
        counts_width(ui, &segs, &font) + t.space_8 * 2.0,
        t.space_16 + t.space_6,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.hovered() { t.hover } else { t.panel };
    ui.painter().rect(
        rect,
        t.radius_xl,
        fill,
        t.control_border_stroke(t.border),
        egui::StrokeKind::Inside,
    );
    paint_counts(ui, rect.left() + t.space_8, rect.center().y, &segs, &font);
    let name = format!("Issues: {}", counts_words(counts, to_do, false));
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), &name));
    response
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(&name)
}

/// A row's disclosed details: the cause, dim, then the fix. Each is one
/// sentence and wraps; neither is selectable, so a click never starts a
/// text selection.
pub fn detail(ui: &mut Ui, detail: &IssueDetail) {
    let t = Tokens::get();
    for (line, ink) in [(&detail.cause, t.text_dim), (&detail.fix, t.text)] {
        if let Some(line) = line.as_deref().filter(|l| !l.is_empty()) {
            ui.add(
                egui::Label::new(RichText::new(line).size(t.font_label).color(ink))
                    .wrap()
                    .selectable(false),
            );
        }
    }
}

fn galley(ui: &Ui, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    ui.painter().layout_no_wrap(text.to_owned(), font, color)
}

/// One line clipped to `width` with an ellipsis; `elided` says whether it
/// was cut (`kit::text`).
fn clipped(ui: &Ui, text: &str, font: FontId, color: Color32, width: f32) -> Arc<Galley> {
    super::text::one_line(ui, text, font, color, width.max(0.0))
}

#[cfg(test)]
#[path = "issues_tests.rs"]
mod tests;
