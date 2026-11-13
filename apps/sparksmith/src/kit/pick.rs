//! Rows that can be ticked: a list of findings where several are chosen
//! for one action (the production check's Waive). A row has an optional
//! fold chevron (a group header), a tick box, a severity glyph, a one-line
//! title with an optional count and one action shown while the row is
//! hovered, so the title has the row's width the rest of the time. The
//! title keeps its lines while the action shows, each cut with "…" where
//! it runs under the action, never re-wrapped into the narrower room, and
//! the title's hover gives the full title when it was cut, then `tip`.
//!
//! Geometry follows the Issues rows (`kit::issues`), so both lists read
//! alike.
use super::severity::{Level, severity_glyph};
use egui::{
    Align, CursorIcon, FontId, Layout, Rect, Role, Sense, Stroke, Ui, UiBuilder, WidgetInfo, pos2,
    vec2,
};
use sparksmith_ui::theme::Tokens;
use sparksmith_ui::ui_kit;
use sparksmith_ui::ui_kit::type_face;

/// How far a group's rows sit right of its header.
pub const PICK_INDENT: f32 = 14.0;

/// A tick box's state; a group's is `Some` when only part of it is ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tick {
    Off,
    Some,
    All,
}

/// One row as shown.
pub struct PickRow<'a> {
    pub level: Level,
    pub title: &'a str,
    /// Added to the title's hover ("Route the connection").
    pub tip: &'a str,
    /// Indent levels.
    pub depth: usize,
    /// `Some(open)` for a group header that folds.
    pub fold: Option<bool>,
    /// The row's count after its title (a group's rows); `None` for none.
    pub count: Option<usize>,
    /// The tick box; `None` for a row that cannot be ticked.
    pub tick: Option<Tick>,
    /// The action shown while the row is hovered.
    pub action: Option<&'a str>,
}

/// What a row did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PickResponse {
    /// The tick box was clicked; `shift` says whether Shift was held.
    pub ticked: bool,
    pub shift: bool,
    /// The fold chevron or the header itself was clicked.
    pub folded: bool,
    pub action: bool,
}

/// One row; `id` keeps its interaction stable as rows come and go.
pub fn pick_row(ui: &mut Ui, id: egui::Id, row: &PickRow<'_>) -> PickResponse {
    let t = Tokens::get();
    let mut out = PickResponse::default();
    let icon = t.form_section_icon_size;
    let count = row.count.map(|n| {
        ui.painter().layout_no_wrap(
            super::severity::grouped(n),
            FontId::proportional(t.font_small),
            t.form_dim,
        )
    });
    let count_width = count.as_ref().map_or(0.0, |c| c.size().x + t.space_6);
    let font = if row.fold.is_some() {
        type_face::semibold(t.font_label)
    } else {
        FontId::proportional(t.font_label)
    };
    // A group header's title wraps to a second line rather than being cut
    // at rest: the row is as tall as the title needs (two lines at most).
    let width = ui.available_width().max(0.0);
    let lead = t.space_6
        + PICK_INDENT * row.depth as f32
        + row.fold.map_or(0.0, |_| icon + t.space_4)
        + row.tick.map_or(0.0, |_| t.space_14 + t.space_6)
        + t.space_14
        + t.space_6;
    let max_rows = if row.fold.is_some() { 2 } else { 1 };
    let rest = super::text::wrapped(
        ui,
        row.title,
        font.clone(),
        t.text,
        width - lead - t.space_6 - count_width,
        max_rows,
    );
    let line = rest.size().y / rest.rows.len().max(1) as f32;
    let extra = line * rest.rows.len().saturating_sub(1) as f32;
    let (rect, _) = ui.allocate_exact_size(
        vec2(width, super::issues::ISSUE_ROW_HEIGHT + extra),
        Sense::hover(),
    );
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ui.painter().rect_filled(rect, t.radius_sm, t.hover);
    }
    // The chevron, tick, glyph and action keep to the first line.
    let first = Rect::from_min_size(
        rect.min,
        vec2(rect.width(), super::issues::ISSUE_ROW_HEIGHT),
    );
    let cy = first.center().y;
    let mut x = rect.left() + t.space_6 + PICK_INDENT * row.depth as f32;
    if let Some(open) = row.fold {
        let chevron = Rect::from_min_size(pos2(x, cy - icon / 2.0), vec2(icon, icon));
        super::chevron(ui.painter(), chevron, open, t.text_dim);
        let fold = ui
            .interact(chevron.expand(t.space_2), id.with("fold"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        fold.widget_info(|| WidgetInfo::selected(Role::DisclosureTriangle, true, open, row.title));
        out.folded |= fold.clicked();
        x += icon + t.space_4;
    }
    if let Some(tick) = row.tick {
        let size = t.space_14;
        let rect = Rect::from_min_size(pos2(x, cy - size / 2.0), vec2(size, size));
        let response = ui
            .interact(rect.expand(t.space_2), id.with("tick"), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        paint_tick(ui, rect, tick, response.hovered());
        response.widget_info(|| {
            WidgetInfo::selected(Role::CheckBox, true, tick == Tick::All, row.title)
        });
        if response.clicked() {
            out.ticked = true;
            out.shift = ui.input(|i| i.modifiers.shift);
        }
        x += size + t.space_6;
    }
    severity_glyph(
        ui.painter(),
        pos2(x + t.space_14 / 2.0, cy),
        t.space_14,
        row.level,
    );
    x += t.space_14 + t.space_6;
    let mut right = rect.right() - t.space_6;
    let mut action_button = None;
    if let Some(action) = row.action.filter(|_| hovered) {
        let slot = Rect::from_min_max(pos2(x.min(right), first.top()), pos2(right, first.bottom()));
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(id.with("action"))
                .max_rect(slot)
                .layout(Layout::right_to_left(Align::Center)),
        );
        let button = ui_kit::button(&mut child, action, ui_kit::Button::Small);
        out.action = button.clicked();
        right = button.rect.left() - t.space_6;
        action_button = Some(button);
    }
    let top = cy - line / 2.0;
    let mut cut = rest.elided;
    // The count follows the title's last line.
    let end = if row.action.is_some() && hovered {
        // The action takes the first line's end: the lines stay as at rest.
        let lines: Vec<String> = rest.rows.iter().map(|r| r.row.text()).collect();
        let mut end = pos2(x, cy);
        for (index, text) in lines.iter().enumerate() {
            let last = index + 1 == lines.len();
            let room = if index == 0 {
                right
            } else {
                rect.right() - t.space_6
            };
            let room = room - x - if last { count_width } else { 0.0 };
            let shown = super::text::one_line(ui, text, font.clone(), t.text, room);
            cut |= shown.elided;
            let at = pos2(x, top + line * index as f32);
            end = pos2(at.x + shown.size().x, at.y + line / 2.0);
            ui.painter().galley(at, shown, t.text);
        }
        end
    } else {
        let end = rest.rows.last().map_or(pos2(x, cy), |r| {
            pos2(x + r.rect().right(), top + r.rect().center().y)
        });
        ui.painter().galley(pos2(x, top), rest, t.text);
        end
    };
    if let Some(count) = count {
        ui.painter().galley(
            pos2(end.x + t.space_6, end.y - count.size().y / 2.0),
            count,
            t.form_dim,
        );
    }
    // The title's own hover, so the tick box and the action keep theirs.
    let sense = if row.fold.is_some() {
        Sense::click()
    } else {
        Sense::hover()
    };
    let text_area = Rect::from_min_max(pos2(x, rect.top()), pos2(right.max(x), rect.bottom()));
    let title_response = ui.interact(text_area, id.with("title"), sense);
    if row.fold.is_some() {
        out.folded |= title_response.clicked();
    }
    super::text::hover(title_response, row.title, row.tip, cut);
    // In a narrow column the action covers where the cut title was: it
    // then shows the whole title too, so the row's text is never lost.
    if cut && let Some(button) = action_button {
        button.on_hover_text(if row.tip.is_empty() {
            row.title.to_owned()
        } else {
            format!("{}\n{}", row.title, row.tip)
        });
    }
    out
}

/// The tick box: an outlined square, filled with a tick when `All` and a
/// bar when `Some`.
fn paint_tick(ui: &Ui, rect: Rect, tick: Tick, hovered: bool) {
    let t = Tokens::get();
    let painter = ui.painter();
    match tick {
        Tick::Off => {
            let border = if hovered {
                t.form_primary
            } else {
                t.control_border
            };
            painter.rect(
                rect,
                t.radius_sm,
                t.control_fill,
                t.control_border_stroke(border),
                egui::StrokeKind::Inside,
            );
        }
        Tick::Some | Tick::All => {
            painter.rect_filled(rect, t.radius_sm, t.form_primary);
            let stroke = Stroke::new(1.6, t.on_accent);
            let c = rect.center();
            let s = rect.width() * 0.28;
            if tick == Tick::All {
                painter.line_segment([c + vec2(-s, 0.0), c + vec2(-s * 0.3, s * 0.7)], stroke);
                painter.line_segment([c + vec2(-s * 0.3, s * 0.7), c + vec2(s, -s * 0.7)], stroke);
            } else {
                painter.line_segment([c + vec2(-s, 0.0), c + vec2(s, 0.0)], stroke);
            }
        }
    }
}
