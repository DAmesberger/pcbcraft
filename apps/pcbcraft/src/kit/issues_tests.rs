#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use egui::{Event, PointerButton, Pos2, RawInput};
use std::cell::Cell;

/// One painted text: what it says, where, and whether it was clipped.
#[derive(Clone, Debug)]
struct Painted {
    text: String,
    rect: Rect,
    elided: bool,
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
                    elided: t.galley.elided,
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

fn row_view(title: &str) -> IssueRowView {
    IssueRowView {
        level: Level::Error,
        title: title.to_owned(),
        count: 1,
        fix: None,
        open: false,
        detail: IssueDetail::default(),
        objects: Vec::new(),
    }
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

#[test]
fn a_row_click_locates_and_its_controls_do_only_their_own_thing() {
    let mut h = Harness::new(360.0);
    let id = egui::Id::new("row");
    let mut view = row_view("R3 ↔ R6: 4 pads closer than 0.2 mm");
    view.fix = Some("Assign…".to_owned());
    view.count = 3;
    view.detail.cause = Some("Pads of different nets are too close.".to_owned());
    let (_, painted) = h.frame(Vec::new(), |ui| issue_row(ui, id, &view));
    let (title, fix) = (rect_of(&painted, &view.title), rect_of(&painted, "Assign…"));
    assert!(find(&painted, "×3").is_some(), "{painted:?}");
    let row = Rect::from_min_size(pos2(0.0, title.top() - 4.0), vec2(360.0, ISSUE_ROW_HEIGHT));

    let on_title = h.click(title.center(), |ui| issue_row(ui, id, &view));
    assert_eq!(
        on_title,
        IssueRowResponse {
            locate: true,
            ..Default::default()
        }
    );
    let on_fix = h.click(fix.center(), |ui| issue_row(ui, id, &view));
    assert_eq!(
        on_fix,
        IssueRowResponse {
            fix: true,
            ..Default::default()
        }
    );
    let tokens = Tokens::get();
    let chevron = pos2(
        row.left() + tokens.space_6 + tokens.form_section_icon_size / 2.0,
        title.center().y,
    );
    let on_chevron = h.click(chevron, |ui| issue_row(ui, id, &view));
    assert_eq!(
        on_chevron,
        IssueRowResponse {
            toggle: true,
            ..Default::default()
        }
    );
    let locate = pos2(row.right() - tokens.space_6 - 10.0, title.center().y);
    let on_locate = h.click(locate, |ui| issue_row(ui, id, &view));
    assert_eq!(
        on_locate,
        IssueRowResponse {
            locate: true,
            ..Default::default()
        }
    );
}

#[test]
fn a_long_title_is_clipped_to_one_line_inside_the_row() {
    let mut h = Harness::new(240.0);
    let view = row_view(&"Courtyards of R7 and R10 overlap on the top side ".repeat(2));
    let ((), painted) = h.frame(Vec::new(), |ui| {
        issue_row(ui, egui::Id::new("long"), &view);
    });
    let title = find(&painted, view.title.as_str()).expect("the title is painted");
    assert!(title.elided);
    assert!(title.rect.right() <= 240.0, "{:?}", title.rect);
    assert!(title.rect.height() < ISSUE_ROW_HEIGHT);
}

#[test]
fn an_open_row_shows_its_details_and_chips_that_locate_one_object() {
    let mut h = Harness::new(360.0);
    let id = egui::Id::new("open");
    let mut view = row_view("2 pads have no package terminal");
    view.detail = IssueDetail {
        cause: Some("Each pad needs a package terminal.".to_owned()),
        fix: Some("Assign the pads by number.".to_owned()),
    };
    view.objects = vec!["Pad 1".to_owned(), "Pad 2".to_owned()];
    let (_, closed) = h.frame(Vec::new(), |ui| issue_row(ui, id, &view));
    assert!(find(&closed, "Pad 2").is_none());
    view.open = true;
    let (_, painted) = h.frame(Vec::new(), |ui| issue_row(ui, id, &view));
    for text in [
        "Each pad needs a package terminal.",
        "Assign the pads by number.",
        "Pad 1",
    ] {
        assert!(find(&painted, text).is_some(), "{text}: {painted:?}");
    }
    let chip = rect_of(&painted, "Pad 2");
    let response = h.click(chip.center(), |ui| issue_row(ui, id, &view));
    assert_eq!(response.object, Some(1));
    assert!(!response.locate);
}

/// Nothing is capped: 10 000 rows take their full height, but only the rows
/// in view are built.
#[test]
fn the_list_is_uncapped_and_builds_only_visible_rows() {
    let mut h = Harness::new(360.0);
    let groups = [
        IssueGroupView {
            level: Level::Error,
            title: "Clearance".to_owned(),
            rows: 10_000,
            open: true,
            fix: None,
        },
        IssueGroupView {
            level: Level::ToDo,
            title: "Not placed".to_owned(),
            rows: 6,
            open: false,
            fix: Some("Update PCB…".to_owned()),
        },
    ];
    let built = Cell::new(0_usize);
    let (content, painted) = h.frame(Vec::new(), |ui| {
        built.set(0);
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .show(ui, |ui| {
                issues_list(ui, egui::Id::new("list"), &groups, |g, r| {
                    built.set(built.get() + 1);
                    row_view(&format!("Row {g}.{r}"))
                })
            })
            .content_size
    });
    let total = (10_000.0 + 2.0) * ISSUE_ROW_HEIGHT;
    assert!((content.y - total).abs() < 1.0, "{content:?} vs {total}");
    assert!(
        built.get() <= 300 / ISSUE_ROW_HEIGHT as usize + 2,
        "{}",
        built.get()
    );
    assert!(find(&painted, "Clearance").is_some());
    assert!(
        find(&painted, "10000").is_some(),
        "the group count is shown"
    );
    assert!(find(&painted, "Row 0.0").is_some());
}

#[test]
fn list_events_name_the_group_or_row() {
    let mut h = Harness::new(360.0);
    let groups = [IssueGroupView {
        level: Level::ToDo,
        title: "Not placed".to_owned(),
        rows: 2,
        open: true,
        fix: Some("Update PCB…".to_owned()),
    }];
    let mut draw = |ui: &mut Ui| {
        issues_list(ui, egui::Id::new("events"), &groups, |_, r| {
            row_view(&format!("R2{r} is not placed"))
        })
    };
    let (_, painted) = h.frame(Vec::new(), &mut draw);
    let header = rect_of(&painted, "Not placed");
    let fix = rect_of(&painted, "Update PCB…");
    let second = rect_of(&painted, "R21 is not placed");
    assert_eq!(
        h.click(header.center(), &mut draw),
        Some(IssueListEvent::Group(0))
    );
    assert_eq!(
        h.click(fix.center(), &mut draw),
        Some(IssueListEvent::GroupFix(0))
    );
    assert_eq!(
        h.click(second.center(), &mut draw),
        Some(IssueListEvent::Row {
            group: 0,
            row: 1,
            action: IssueRowResponse {
                locate: true,
                ..Default::default()
            },
        })
    );
}
