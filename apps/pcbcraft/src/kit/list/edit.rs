//! The inline value editor of a list row (double-click, Enter or F2): a
//! field in the row's value rect. Enter commits, Esc cancels, Tab commits
//! and moves to the next editable row, and leaving the field commits (the
//! card's live-commit rule). Invalid text gets a red ring; the caller shows
//! why under the row (`row::field_error`) and sends nothing, so the
//! document is unchanged.
use super::measure;
use super::tokens::ListTokens;
use egui::{Rect, Stroke, Ui};

/// What the field did this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Still editing.
    Open,
    /// Enter, or the field lost the keyboard: send the text.
    Commit,
    /// Tab: send the text, then edit the next row.
    CommitNext,
    /// Esc: drop the text.
    Cancel,
}

/// The editor of the row value at `rect`, named `label` ("Origin → left
/// edge"). `start` gives it the keyboard (the frame the edit opens).
pub fn field(
    ui: &mut Ui,
    id: egui::Id,
    rect: Rect,
    text: &mut String,
    invalid: bool,
    start: bool,
    label: &str,
) -> Edit {
    let l = ListTokens::get(ui.ctx());
    let id = id.with("inline-edit");
    let r = rect.shrink2(egui::vec2(0.0, 3.0));
    let response = ui.put(
        r,
        egui::TextEdit::singleline(text)
            .id(id)
            .font(measure::mono(&l))
            .margin(egui::vec2(5.0, 1.0))
            .align(egui::Align2::RIGHT_CENTER)
            .char_limit(512)
            .desired_width(r.width()),
    );
    response.widget_info(|| egui::WidgetInfo::text_edit(true, "", text.as_str(), label));
    let ink = if invalid {
        super::super::severity::ink(super::super::severity::Level::Error)
    } else {
        l.focus
    };
    ui.painter()
        .rect_stroke(r, 5.0, Stroke::new(1.0, ink), egui::StrokeKind::Outside);
    ui.painter()
        .rect_stroke(r.expand(1.0), 6.0, Stroke::new(2.0, ink.gamma_multiply(0.22)), egui::StrokeKind::Outside);
    if start {
        response.request_focus();
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            );
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), id);
        }
        return Edit::Open;
    }
    if !response.lost_focus() {
        return Edit::Open;
    }
    let (escape, tab) = ui.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Tab),
        )
    });
    if escape {
        Edit::Cancel
    } else if tab {
        Edit::CommitNext
    } else {
        Edit::Commit
    }
}
