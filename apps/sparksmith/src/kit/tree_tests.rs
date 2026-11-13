use super::*;
use crate::kit::rows::tests::{Harness, Painted, find};

const LONG_NAME: &str = "Precision thin-film resistor network, 8 elements, isolated";
const LONG_PART: &str = "Acme Components ACM-0603-10K-1%-THINFILM-REEL";

/// Draw one row `width` wide; returns the painted texts and the row's
/// rectangle.
fn draw(width: f32, row: &Row<'_>) -> (Vec<Painted>, Rect) {
    let mut harness = Harness::new(800.0, 200.0);
    let mut rect = Rect::NOTHING;
    let mut add = |ui: &mut Ui| {
        let area = Rect::from_min_size(pos2(10.0, 10.0), vec2(width, TREE_ROW_HEIGHT));
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            ui.set_clip_rect(area);
            tree_row(ui, egui::Id::unique("row"), row);
            rect = area;
        });
    };
    harness.frame(Vec::new(), &mut add);
    (harness.frame(Vec::new(), &mut add), rect)
}

/// The painted `text`, asserting that it was painted.
fn painted(painted: &[Painted], text: &str) -> Painted {
    let found = find(painted, text).cloned();
    assert!(found.is_some(), "{text} is painted: {painted:?}");
    found.unwrap_or(Painted {
        text: String::new(),
        rect: Rect::NOTHING,
        ink: egui::Color32::TRANSPARENT,
        clip: Rect::NOTHING,
        rows: 0,
        elided: false,
    })
}

fn inside(row: Rect, text: &Painted) -> bool {
    text.rect.left() >= row.left() - 0.5 && text.rect.right() <= row.right() + 0.5
}

/// An instance row in a narrow Browser: the designator is shown whole and
/// the long component name after it is cut with an ellipsis inside the
/// row, never pushing the designator out.
#[test]
fn a_long_secondary_text_is_cut_and_the_designator_stays() {
    for width in [146.0, 200.0, 240.0] {
        let (shown, row) = draw(
            width,
            &Row {
                depth: 3,
                icon: "component",
                label: "R1",
                secondary: LONG_NAME,
                issue: Some(crate::kit::severity::Level::Warning),
                ..Default::default()
            },
        );
        let designator = painted(&shown, "R1");
        let name = painted(&shown, LONG_NAME);
        assert!(!designator.elided, "{width}: R1 is whole");
        assert!(inside(row, &designator) && inside(row, &name), "{width}");
        assert!(name.elided, "{width}: the name is cut");
        assert!(
            designator.rect.right() < name.rect.left(),
            "{width}: the texts never overlap"
        );
    }
}

/// A catalog row: a long part name is cut, and its short revision at the
/// right is laid out first and shown whole.
#[test]
fn a_long_label_is_cut_before_the_trailing_text() {
    let (shown, row) = draw(
        200.0,
        &Row {
            depth: 1,
            icon: "library",
            label: LONG_PART,
            secondary: "r12",
            modified: true,
            ..Default::default()
        },
    );
    let label = painted(&shown, LONG_PART);
    let revision = painted(&shown, "r12");
    assert!(label.elided && !revision.elided);
    assert!(inside(row, &label) && inside(row, &revision));
    assert!(label.rect.right() < revision.rect.left());
}

/// Two long texts get half the room each; a wide row shows both whole.
#[test]
fn two_long_texts_share_the_row_and_a_wide_row_shows_both() {
    let row = Row {
        depth: 3,
        label: LONG_PART,
        secondary: LONG_NAME,
        ..Default::default()
    };
    let (shown, rect) = draw(220.0, &row);
    let (label, name) = (painted(&shown, LONG_PART), painted(&shown, LONG_NAME));
    assert!(label.elided && name.elided);
    assert!(inside(rect, &label) && inside(rect, &name));
    assert!(label.rect.right() < name.rect.left());
    let (shown, _) = draw(760.0, &row);
    assert!(!painted(&shown, LONG_PART).elided && !painted(&shown, LONG_NAME).elided);
}

#[test]
fn share_keeps_what_fits_and_splits_what_does_not() {
    let gap = 6.0;
    let s = |room, label, secondary| share(room, label, secondary, gap);
    assert_eq!(
        s(100.0, 300.0, None),
        Share {
            label: 100.0,
            secondary: 0.0
        }
    );
    assert_eq!(
        s(106.0, 40.0, Some(60.0)),
        Share {
            label: 40.0,
            secondary: 60.0
        }
    );
    // The short label keeps its width; the secondary takes the rest.
    assert_eq!(
        s(106.0, 20.0, Some(500.0)),
        Share {
            label: 20.0,
            secondary: 80.0
        }
    );
    // The short secondary keeps its width; the label takes the rest.
    assert_eq!(
        s(106.0, 500.0, Some(30.0)),
        Share {
            label: 70.0,
            secondary: 30.0
        }
    );
    assert_eq!(
        s(106.0, 500.0, Some(500.0)),
        Share {
            label: 50.0,
            secondary: 50.0
        }
    );
    let tiny = s(2.0, 50.0, Some(50.0));
    assert!(tiny.label >= 0.0 && tiny.secondary >= 0.0);
}

/// A long card title is cut too, never painted past the card.
#[test]
fn a_long_card_title_is_cut() {
    let mut harness = Harness::new(800.0, 200.0);
    let area = Rect::from_min_size(pos2(10.0, 10.0), vec2(120.0, TREE_ROW_HEIGHT));
    let mut add = |ui: &mut Ui| {
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            card_title(ui, LONG_PART, "3 components");
        });
    };
    harness.frame(Vec::new(), &mut add);
    let shown = harness.frame(Vec::new(), &mut add);
    let title = painted(&shown, LONG_PART);
    assert!(title.elided && inside(area, &title));
}
