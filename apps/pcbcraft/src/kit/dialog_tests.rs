use super::*;
use crate::kit::rows::tests::{Harness, at, find, key, rect_of};
use std::cell::Cell;

#[test]
fn the_body_is_capped_by_its_rows_and_the_slot() {
    assert_eq!(body_height(4.0, 24.0, 16.0, 1000.0), 4.0 * 24.0 + 16.0);
    assert_eq!(body_height(4.0, 24.0, 16.0, 50.0), 50.0, "the slot wins");
    assert_eq!(
        body_height(0.0, 24.0, 0.0, 1000.0),
        24.0,
        "at least one row"
    );
    assert_eq!(body_height(f32::NAN, 24.0, 0.0, 1e6), 32.0 * 24.0);
    assert_eq!(body_height(4.0, 24.0, 0.0, -5.0), 0.0);
}

#[test]
fn a_refusal_is_revealed_once() {
    let reason = "Not published: 3 issues block it";
    assert!(reveal_refusal(None, reason));
    assert!(
        !reveal_refusal(Some(reason), reason),
        "the user may scroll away"
    );
    assert!(reveal_refusal(Some("Another refusal"), reason));
}

fn dialog<'a>(id: &str, disabled: Option<&'a str>, refusal: Option<&'a str>) -> DialogPanel<'a> {
    let mut panel = DialogPanel::new(
        egui::Id::new(id),
        "library",
        "Assign pads by number",
        "Assign 2 pads",
    );
    panel.kind = "New";
    panel.body_rows = 6.0;
    panel.primary_disabled = disabled;
    panel.refusal = refusal;
    panel
}

#[test]
fn the_footer_is_pinned_and_the_refusal_is_the_last_row_scrolled_into_view() {
    let mut ui = Harness::new(360.0, 700.0);
    let refusal = "Pad 3 has no terminal 3 to take";
    let panel = dialog("pinned", None, Some(refusal));
    let mut painted = Vec::new();
    for _ in 0..4 {
        painted = ui.frame(vec![], |ui| {
            panel.show(ui, |ui| {
                for n in 0..30 {
                    ui.label(format!("Pad {n} → terminal {n}"));
                }
            });
        });
    }
    let cancel = rect_of(&painted, "Cancel");
    let primary = rect_of(&painted, "Assign 2 pads");
    assert!(
        cancel.top() > 700.0 - FOOTER_HEIGHT,
        "pinned to the slot's bottom: {cancel:?}"
    );
    assert!(primary.left() > cancel.right(), "primary last");
    let Some(shown) = find(&painted, refusal) else {
        return assert!(find(&painted, refusal).is_some(), "{painted:?}");
    };
    assert!(
        shown.clip.contains_rect(shown.rect),
        "the refusal is scrolled into view: {shown:?}"
    );
    let header = rect_of(&painted, "Assign pads by number");
    assert!(header.bottom() <= DIALOG_HEADER_HEIGHT + 1.0);
    let body_rows: Vec<_> = painted
        .iter()
        .filter(|p| p.text.starts_with("Pad ") && p.text.contains('→'))
        .collect();
    assert!(
        body_rows
            .iter()
            .all(|p| p.clip.bottom() <= 700.0 - FOOTER_HEIGHT + 0.5),
        "the body scrolls above the footer"
    );
}

#[test]
fn the_footer_and_keys_answer_and_a_disabled_primary_does_not() {
    let mut ui = Harness::new(360.0, 500.0);
    let enabled = dialog("keys", None, None);
    let action = Cell::new(None);
    let mut add = |ui: &mut Ui| {
        action.set(enabled.show(ui, |ui| {
            ui.label("body");
        }))
    };
    let painted = ui.frame(vec![], &mut add);
    ui.click(at(&painted, "Assign 2 pads"), &mut add);
    assert_eq!(action.get(), Some(DialogAction::Primary));
    ui.click(at(&painted, "Cancel"), &mut add);
    assert_eq!(action.get(), Some(DialogAction::Cancel));
    ui.frame(vec![key(egui::Key::Enter)], &mut add);
    assert_eq!(
        action.get(),
        Some(DialogAction::Primary),
        "Enter with no field focused"
    );
    ui.frame(vec![key(egui::Key::Escape)], &mut add);
    assert_eq!(action.get(), Some(DialogAction::Cancel));

    let disabled = dialog("disabled", Some("Pick the pads to assign"), None);
    let action = Cell::new(None);
    let mut add = |ui: &mut Ui| {
        action.set(disabled.show(ui, |ui| {
            ui.label("body");
        }))
    };
    let painted = ui.frame(vec![], &mut add);
    ui.click(at(&painted, "Assign 2 pads"), &mut add);
    assert_eq!(action.get(), None, "a disabled primary does nothing");
    ui.frame(vec![key(egui::Key::Enter)], &mut add);
    assert_eq!(action.get(), None, "nor does Enter");
}

#[test]
fn esc_in_a_focused_field_reverts_the_field_and_keeps_the_dialog() {
    let mut ui = Harness::new(360.0, 500.0);
    let panel = dialog("field", None, None);
    let mut text = String::from("10k");
    let action = Cell::new(None);
    let field = Cell::new(egui::Rect::NOTHING);
    let mut add = |ui: &mut Ui| {
        action.set(panel.show(ui, |ui| field.set(ui.text_edit_singleline(&mut text).rect)));
    };
    ui.frame(vec![], &mut add);
    let centre = field.get().center();
    ui.click(centre, &mut add);
    ui.frame(vec![key(egui::Key::Escape)], &mut add);
    assert_eq!(action.get(), None, "Esc belonged to the field");
}

#[test]
fn a_confirmation_proceeds_only_on_its_destructive_primary() {
    let mut ui = Harness::new(800.0, 600.0);
    let id = egui::Id::new("confirm");
    let answer = Cell::new(None);
    let ctx = ui.ctx().clone();
    let mut add = |_: &mut Ui| {
        answer.set(confirm(
            &ctx,
            id,
            "Clear footprint",
            "Removes 6 pads; one undo step",
            "Clear",
        ));
    };
    let mut painted = Vec::new();
    for _ in 0..3 {
        painted = ui.frame(vec![], &mut add);
    }
    assert!(
        find(&painted, "Removes 6 pads; one undo step").is_some(),
        "{painted:?}"
    );
    ui.frame(vec![key(egui::Key::Enter)], &mut add);
    assert_eq!(answer.get(), None, "Enter never proceeds");
    let clear = painted
        .iter()
        .find(|p| p.text == "Clear")
        .map(|p| (p.rect.center(), p.ink));
    assert!(
        clear.is_some_and(|(_, ink)| ink == Tokens::get().on_accent),
        "{clear:?}"
    );
    ui.click(clear.map_or(egui::Pos2::ZERO, |c| c.0), &mut add);
    assert_eq!(answer.get(), Some(true));
    ui.click(at(&painted, "Cancel"), &mut add);
    assert_eq!(answer.get(), Some(false));
}
