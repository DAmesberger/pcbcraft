#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use egui::{Event, PointerButton, Pos2, RawInput};

/// One painted text: what it says and where.
#[derive(Clone, Debug)]
struct Painted {
    text: String,
    rect: Rect,
}

struct Harness {
    ctx: egui::Context,
    time: f64,
    width: f32,
}

impl Harness {
    fn new(width: f32) -> Self {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        Self {
            ctx,
            time: 0.0,
            width,
        }
    }

    fn frame<R>(
        &mut self,
        events: Vec<Event>,
        mut add: impl FnMut(&mut Ui) -> R,
    ) -> (R, Vec<Painted>) {
        self.time += 0.05;
        let mut result = None;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(self.width, 400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| result = Some(add(ui)),
        );
        output.textures_delta.clear();
        fn walk(shape: &egui::Shape, out: &mut Vec<Painted>) {
            match shape {
                egui::Shape::Text(t) => out.push(Painted {
                    text: t.galley.text().to_owned(),
                    rect: t.galley.rect.translate(t.pos.to_vec2()),
                }),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut painted = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut painted);
        }
        (result.expect("the frame ran its UI"), painted)
    }

    /// Move to `at`, press and release; the result of the release frame.
    fn click<R>(&mut self, at: Pos2, mut add: impl FnMut(&mut Ui) -> R) -> R {
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(vec![Event::PointerMoved(at)], &mut add);
        self.frame(vec![button(true)], &mut add);
        self.frame(vec![button(false)], &mut add).0
    }
}

fn find<'a>(painted: &'a [Painted], text: &str) -> Option<&'a Painted> {
    painted.iter().find(|p| p.text == text)
}

fn rect_of(painted: &[Painted], text: &str) -> Rect {
    find(painted, text)
        .unwrap_or_else(|| panic!("{text:?} is not painted: {painted:?}"))
        .rect
}

fn counts(error: usize, warning: usize, to_do: usize, info: usize) -> IssueCounts {
    IssueCounts {
        error,
        warning,
        to_do,
        info,
    }
}

#[test]
fn counts_are_never_dropped_except_the_badges_to_do_under_errors() {
    let c = counts(4, 2, 36, 1);
    assert_eq!(
        counts_words(&c, true, true),
        "4 errors, 2 warnings, 36 to do, 1 note"
    );
    assert_eq!(counts_words(&c, false, false), "4 errors, 2 warnings");
    assert_eq!(counts_words(&counts(0, 0, 0, 0), true, true), "No issues");
    assert_eq!(counts_words(&counts(0, 0, 3, 0), true, false), "3 to do");
}

#[test]
fn the_bar_is_28_points_and_shows_every_count() {
    let mut h = Harness::new(360.0);
    let (rect, painted) = h.frame(Vec::new(), |ui| {
        issues_bar(
            ui,
            &counts(4, 2, 36, 0),
            false,
            Some("Component: 2 block publishing"),
        )
        .rect
    });
    assert_eq!(rect.height(), ISSUES_BAR_HEIGHT);
    for text in ["Issues", "4", "2", "36", "Component: 2 block publishing"] {
        assert!(find(&painted, text).is_some(), "{text}: {painted:?}");
    }
    let (_, painted) = h.frame(Vec::new(), |ui| {
        issues_bar(ui, &counts(0, 0, 0, 0), true, None)
    });
    assert!(find(&painted, "No issues").is_some(), "{painted:?}");
    // With nothing here and blockers elsewhere, the two never overlap.
    let (_, painted) = h.frame(Vec::new(), |ui| {
        issues_bar(
            ui,
            &counts(0, 0, 0, 0),
            false,
            Some("2 blocking in Schematic, 3 blocking in Board, 1 in Component"),
        )
    });
    let none = rect_of(&painted, "No issues");
    let elsewhere = painted
        .iter()
        .find(|p| p.text.contains("blocking") || p.text.ends_with('\u{2026}'))
        .map(|p| p.rect)
        .unwrap_or_else(|| panic!("the elsewhere note: {painted:?}"));
    assert!(none.right() < elsewhere.left(), "{none:?} {elsewhere:?}");
    let clicked = h.click(rect.center(), |ui| {
        issues_bar(ui, &counts(1, 0, 0, 0), false, None).clicked()
    });
    assert!(clicked);
}

#[test]
fn the_badge_hides_to_do_only_while_errors_exist_and_when_asked() {
    let mut h = Harness::new(360.0);
    let numbers = |h: &mut Harness, c: IssueCounts, hide: bool| {
        let (_, painted) = h.frame(Vec::new(), |ui| count_badge(ui, &c, hide));
        painted.into_iter().map(|p| p.text).collect::<Vec<_>>()
    };
    assert_eq!(numbers(&mut h, counts(4, 2, 36, 5), true), ["4", "2"]);
    assert_eq!(
        numbers(&mut h, counts(4, 2, 36, 5), false),
        ["4", "2", "36"]
    );
    assert_eq!(numbers(&mut h, counts(0, 2, 36, 5), true), ["2", "36"]);
}

/// The slot keeps its row while empty, so the card never reflows, and an
/// empty slot does nothing on a click.
#[test]
fn the_issue_line_is_reserved_and_inert_while_empty() {
    let mut h = Harness::new(360.0);
    let (empty, painted) = h.frame(Vec::new(), |ui| {
        issue_line(ui, &counts(0, 0, 0, 0), "on R1").rect
    });
    assert!(painted.is_empty(), "{painted:?}");
    let (full, painted) = h.frame(Vec::new(), |ui| {
        issue_line(ui, &counts(1, 1, 0, 0), "on R1").rect
    });
    assert_eq!(empty.height(), full.height());
    assert!(empty.height() > 0.0);
    assert!(find(&painted, "on R1").is_some(), "{painted:?}");
    assert!(!h.click(empty.center(), |ui| {
        issue_line(ui, &counts(0, 0, 0, 0), "on R1").clicked()
    }));
    assert!(h.click(full.center(), |ui| {
        issue_line(ui, &counts(1, 0, 0, 0), "on R1").clicked()
    }));
}
