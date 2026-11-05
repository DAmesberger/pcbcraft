use super::*;
use egui::{Event, Pos2, RawInput};
use std::cell::Cell;

/// A headless egui frame loop that records the painted text, for the kit's
/// interaction tests.
pub(in crate::kit) struct Harness {
    ctx: egui::Context,
    time: f64,
    size: egui::Vec2,
    /// egui's font atlas, kept from every frame, for pictures.
    pub atlas: crate::kit::text::tests::Atlas,
}

/// One painted text: its string, screen rectangle, ink and clip.
#[derive(Clone, Debug)]
pub(in crate::kit) struct Painted {
    pub text: String,
    pub rect: Rect,
    pub ink: Color32,
    pub clip: Rect,
    /// The rows it was laid out as (1 when cut to one line).
    pub rows: usize,
    /// Cut short with an ellipsis.
    pub elided: bool,
}

impl Harness {
    pub fn new(width: f32, height: f32) -> Self {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        Self {
            ctx,
            time: 0.0,
            size: vec2(width, height),
            atlas: Default::default(),
        }
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    /// Run one frame with `events`; `add` draws into the screen's root Ui.
    pub fn frame(&mut self, events: Vec<Event>, mut add: impl FnMut(&mut Ui)) -> Vec<Painted> {
        self.time += 0.4;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| add(ui),
        );
        self.atlas.take(&mut output);
        fn walk(shape: &egui::Shape, clip: Rect, out: &mut Vec<Painted>) {
            match shape {
                egui::Shape::Text(t) => out.push(Painted {
                    text: t.galley.text().to_owned(),
                    rect: t.galley.rect.translate(t.pos.to_vec2()),
                    ink: t
                        .galley
                        .job
                        .sections
                        .first()
                        .map_or(t.fallback_color, |s| s.format.color),
                    clip,
                    rows: t.galley.rows.len(),
                    elided: t.galley.elided,
                }),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, clip, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, shape.clip_rect, &mut out);
        }
        out
    }

    /// Run one frame with `events` and return egui's whole output.
    pub fn output(&mut self, events: Vec<Event>, mut add: impl FnMut(&mut Ui)) -> egui::FullOutput {
        self.time += 0.4;
        let mut output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| add(ui),
        );
        self.atlas.take(&mut output);
        output
    }

    /// Settle, then click at `at` (press and release on separate frames).
    pub fn click(&mut self, at: Pos2, mut add: impl FnMut(&mut Ui)) -> Vec<Painted> {
        self.frame(vec![Event::PointerMoved(at)], &mut add);
        self.frame(vec![press(at, true)], &mut add);
        self.frame(vec![press(at, false)], &mut add)
    }
}

fn press(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

pub(in crate::kit) fn key(key: egui::Key) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The first painted text equal to `text`.
pub(in crate::kit) fn find<'a>(painted: &'a [Painted], text: &str) -> Option<&'a Painted> {
    painted.iter().find(|p| p.text == text)
}

/// The centre of the first painted `text`, asserting that it was painted.
pub(in crate::kit) fn at(painted: &[Painted], text: &str) -> Pos2 {
    let found = find(painted, text);
    assert!(found.is_some(), "{text} is painted: {painted:?}");
    found.map_or(Pos2::ZERO, |p| p.rect.center())
}

/// The rectangle of the first painted `text`, asserting that it was painted.
pub(in crate::kit) fn rect_of(painted: &[Painted], text: &str) -> Rect {
    let found = find(painted, text);
    assert!(found.is_some(), "{text} is painted: {painted:?}");
    found.map_or(Rect::NOTHING, |p| p.rect)
}

#[test]
fn lists_and_tables_share_column_widths_and_bounds() {
    assert_eq!(shown_rows(3, LIST_MAX_ROWS), 3);
    assert_eq!(shown_rows(23, LIST_MAX_ROWS), 8);
    assert_eq!(shown_rows(5, 0), 1, "a zero bound still shows a row");
    let columns = [
        TableColumn {
            title: "Name",
            weight: 1.0,
        },
        TableColumn {
            title: "Value",
            weight: 3.0,
        },
    ];
    assert_eq!(column_widths(&columns, 200.0), vec![50.0, 150.0]);
    let degenerate = [
        TableColumn {
            title: "A",
            weight: f32::NAN,
        },
        TableColumn {
            title: "B",
            weight: -1.0,
        },
    ];
    assert_eq!(column_widths(&degenerate, 100.0), vec![50.0, 50.0]);
}

#[test]
fn a_long_list_shows_its_bound_and_count_and_offers_show_all() {
    let mut ui = Harness::new(360.0, 600.0);
    let mut drawn = Vec::new();
    let mut response = ListFrameResponse::default();
    let mut add = |ui: &mut Ui| {
        drawn.clear();
        response = list_frame(ui, 23, LIST_MAX_ROWS, |ui, i| {
            drawn.push(i);
            ui.label(format!("row {i}"));
        });
    };
    let painted = ui.frame(vec![], &mut add);
    assert!(find(&painted, "8 of 23").is_some(), "the count is shown");
    let show_all = at(&painted, "Show all \u{203a}");
    ui.click(show_all, &mut add);
    assert_eq!(drawn, (0..8).collect::<Vec<_>>());
    assert!(response.show_all);

    let painted = ui.frame(vec![], |ui| {
        list_frame(ui, 3, LIST_MAX_ROWS, |ui, i| {
            ui.label(format!("row {i}"));
        });
    });
    assert!(
        painted.iter().all(|p| !p.text.contains(" of ")),
        "a short list has no count"
    );
}

#[test]
fn a_long_table_scrolls_its_visible_rows_and_its_add_row_clicks() {
    let mut ui = Harness::new(360.0, 900.0);
    let columns = [
        TableColumn {
            title: "Parameter",
            weight: 1.0,
        },
        TableColumn {
            title: "Value",
            weight: 1.0,
        },
    ];
    let cells = Cell::new(0);
    let mut out = TableResponse::default();
    let mut add = |ui: &mut Ui| {
        cells.set(0);
        out = table(
            ui,
            &columns,
            40,
            |ui, row, column| {
                cells.set(cells.get() + 1);
                ui.label(format!("{row}:{column}"));
            },
            Some("+ Add parameter"),
        );
    };
    let painted = ui.frame(vec![], &mut add);
    assert!(find(&painted, "PARAMETER").is_some(), "captions are drawn");
    assert!(
        cells.get() < 40 * 2,
        "only the visible rows are drawn ({})",
        cells.get()
    );
    assert!(
        cells.get() >= LIST_MAX_ROWS * 2,
        "at least the bound is drawn ({})",
        cells.get()
    );
    let plus = at(&painted, "+ Add parameter");
    ui.click(plus, &mut add);
    assert!(out.add);
}

#[test]
fn a_field_error_and_a_refusal_are_one_red_line() {
    let mut ui = Harness::new(240.0, 300.0);
    let long = "R13 must stay inside the board, which is sixty by forty millimetres in size";
    let painted = ui.frame(vec![], |ui| {
        let field = ui.label("field");
        field_error(ui, &field, long, true);
        refusal(ui, long);
    });
    let red: Vec<_> = painted.iter().filter(|p| p.text == long).collect();
    assert_eq!(red.len(), 2, "{painted:?}");
    for p in red {
        assert_eq!(p.ink, Tokens::get().error);
        assert_eq!(p.rows, 1, "cut to one line");
        assert!(
            p.rect.right() <= 240.0 + 0.5,
            "inside the column: {:?}",
            p.rect
        );
    }
}

#[test]
fn a_tool_card_ends_with_done_and_finishes_through_its_footer() {
    let mut ui = Harness::new(360.0, 400.0);
    let card = ToolCard {
        icon: "polygon",
        name: "Polygon",
        key: "P",
        hint: "Click points; double-click or Finish to close the shape",
        tip: "",
        finish: Some("Finish"),
    };
    let out = Cell::new(ToolCardResponse::default());
    let mut add = |ui: &mut Ui| {
        out.set(tool_card(ui, &card, |ui| {
            ui.label("Stroke");
        }));
    };
    let painted = ui.frame(vec![], &mut add);
    for text in ["Polygon", "P", "Stroke", "Cancel", "Finish"] {
        assert!(find(&painted, text).is_some(), "{text}: {painted:?}");
    }
    let (done, finish) = (at(&painted, "Done"), at(&painted, "Finish"));
    ui.click(done, &mut add);
    assert_eq!(
        out.get(),
        ToolCardResponse {
            done: true,
            ..Default::default()
        }
    );
    ui.click(finish, &mut add);
    assert_eq!(
        out.get(),
        ToolCardResponse {
            finish: true,
            ..Default::default()
        }
    );
}

#[test]
fn a_summary_row_puts_its_action_at_the_right_and_a_hint_wraps_in_the_column() {
    let mut ui = Harness::new(300.0, 200.0);
    let long = "Click a pad, then points; Esc ends the route and keeps what was placed so far";
    let painted = ui.frame(vec![], |ui| {
        summary_row(ui, "Derived from the sketch", Some("Edit sketch \u{203a}"));
        hint(ui, long, "");
    });
    let summary = rect_of(&painted, "Derived from the sketch");
    let action = rect_of(&painted, "Edit sketch \u{203a}");
    assert!(
        action.left() > summary.right(),
        "the action follows the summary"
    );
    assert!(
        action.right() > 300.0 - Tokens::get().space_24 * 2.0,
        "at the right: {action:?}"
    );
    let hint = painted.iter().find(|p| p.text == long);
    assert!(
        hint.is_some_and(|h| h.rows > 1 && !h.elided && h.rect.right() <= 300.5),
        "{hint:?}"
    );
}
