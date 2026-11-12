#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::kit::rows::tests::{Harness, Painted, find, rect_of};
use egui::{Event, Key, Modifiers, PointerButton};
use std::collections::BTreeSet;

/// A list's rows held in memory, as the Issues card holds a report.
pub(crate) struct Rows {
    pub groups: Vec<IssueGroupView>,
    pub titles: Vec<Vec<String>>,
    pub detail: IssueDetail,
    pub objects: Vec<String>,
    /// Disclosed rows.
    pub open: BTreeSet<(usize, usize)>,
    pub fix: Option<&'static str>,
    pub count: usize,
}

impl Rows {
    /// One open group `title` of `titles`.
    pub(crate) fn one(title: &str, titles: &[&str]) -> Self {
        Self::of(vec![(
            title,
            Level::Error,
            titles.iter().map(|t| (*t).to_owned()).collect(),
        )])
    }

    pub(crate) fn of(groups: Vec<(&str, Level, Vec<String>)>) -> Self {
        Self {
            groups: groups
                .iter()
                .map(|(title, level, rows)| IssueGroupView {
                    key: RowKey::of(title),
                    level: *level,
                    title: (*title).to_owned(),
                    rows: rows.len(),
                    open: true,
                    fix: None,
                })
                .collect(),
            titles: groups.into_iter().map(|(_, _, rows)| rows).collect(),
            detail: IssueDetail::default(),
            objects: Vec::new(),
            open: BTreeSet::new(),
            fix: None,
            count: 1,
        }
    }
}

impl IssueSource for Rows {
    fn groups(&self) -> &[IssueGroupView] {
        &self.groups
    }

    fn key(&self, group: usize, row: usize) -> RowKey {
        RowKey::of(("row", group, row))
    }

    fn title(&self, group: usize, row: usize) -> &str {
        self.titles
            .get(group)
            .and_then(|g| g.get(row))
            .map_or("", String::as_str)
    }

    fn disclosed(&self, group: usize, row: usize) -> Option<bool> {
        let any = self.detail.cause.is_some() || !self.objects.is_empty();
        any.then(|| self.open.contains(&(group, row)))
    }

    fn row(&self, group: usize, row: usize) -> IssueRowView<'_> {
        IssueRowView {
            level: self.groups.get(group).map_or(Level::Info, |g| g.level),
            title: self.title(group, row),
            count: self.count,
            fix: self.fix,
            waive: true,
            detail: &self.detail,
            objects: &self.objects,
        }
    }
}

/// Draw `rows` in a list 300 pt high and keep what it did.
fn draw(ui: &mut Ui, rows: &Rows, out: &mut IssueListOutput) {
    *out = issues_list(
        ui,
        egui::Id::unique("issues-test"),
        300.0,
        rows,
        Waived {
            rows: &[],
            open: true,
        },
        None,
    );
}

fn row_event(group: usize, row: usize, action: RowAction) -> IssueListEvent {
    IssueListEvent::Row { group, row, action }
}

fn key(k: Key) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

/// A click on a row locates it; its hover actions (Locate, Fix…, Waive…)
/// and its chevron each do only their own thing, and none shows at rest.
#[test]
fn a_row_click_locates_and_its_controls_do_only_their_own_thing() {
    let mut h = Harness::new(360.0, 400.0);
    let mut rows = Rows::one("Clearance", &["R3 ↔ R6: 4 pads too close"]);
    rows.fix = Some("Assign\u{2026}");
    rows.count = 3;
    rows.detail.cause = Some("Pads of different nets are too close.".to_owned());
    let mut out = IssueListOutput::default();
    h.frame(vec![Event::PointerGone], |ui| draw(ui, &rows, &mut out));
    let rest = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let title = rect_of(&rest, "R3 ↔ R6: 4 pads too close");
    assert!(find(&rest, "Assign\u{2026}").is_none(), "{rest:?}");
    assert!(find(&rest, "Waive\u{2026}").is_none(), "{rest:?}");
    assert!(
        find(&rest, "3").is_some(),
        "the count of merged findings shows"
    );
    // Hovered, the actions show over the row's end without moving it.
    h.frame(vec![Event::PointerMoved(title.center())], |ui| {
        draw(ui, &rows, &mut out)
    });
    let hovered = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    assert_eq!(rect_of(&hovered, "R3 ↔ R6: 4 pads too close"), title);
    let fix = rect_of(&hovered, "Assign\u{2026}");
    let waive = rect_of(&hovered, "Waive\u{2026}");

    h.click(title.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 0, RowAction::Locate)]);
    h.click(fix.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 0, RowAction::Fix)]);
    h.click(waive.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 0, RowAction::Waive)]);
    // Locate is the icon left of Fix (its text has 7 pt each side, and
    // actions are 2 pt apart; the icon is the row's height less 8).
    let l = ListTokens::get(h.ctx());
    let locate = pos2(
        fix.left() - 7.0 - 2.0 - (l.row_h - 8.0) / 2.0,
        title.center().y,
    );
    h.click(locate, |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 0, RowAction::Locate)]);
    let chevron = pos2(l.pad_x + l.indent + l.fold_w / 2.0, title.center().y);
    h.click(chevron, |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 0, RowAction::Toggle)]);
}

/// A row is a button: dragging across its title selects no text, so Copy
/// afterwards copies nothing (B-b).
#[test]
fn dragging_across_a_row_selects_no_text() {
    let mut h = Harness::new(360.0, 400.0);
    let rows = Rows::one("Pins", &["R1 pin 1 unconnected"]);
    let mut out = IssueListOutput::default();
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let title = rect_of(&painted, "R1 pin 1 unconnected");
    let (from, to) = (title.left_center(), title.right_center());
    let press = |pos, pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    let mut copied = Vec::new();
    for events in [
        vec![Event::PointerMoved(from)],
        vec![press(from, true)],
        vec![Event::PointerMoved(title.center())],
        vec![Event::PointerMoved(to)],
        vec![press(to, false)],
        vec![Event::Copy],
    ] {
        let output = h.output(events, |ui| draw(ui, &rows, &mut out));
        copied.extend(
            output
                .platform_output
                .commands
                .into_iter()
                .filter_map(|c| match c {
                    egui::OutputCommand::CopyText(text) => Some(text),
                    _ => None,
                }),
        );
    }
    assert!(
        copied.iter().all(String::is_empty),
        "a drag selected text: {copied:?}"
    );
}

/// At the narrowest Properties width (230 pt) rows are one list row apart
/// and never overlap, a long title is cut to one line inside its row, and
/// every cut text shows in full on hover.
#[test]
fn rows_fit_230pt_and_cut_titles_show_in_full_on_hover() {
    let long = "Courtyards of R7 and R10 overlap on the top side of the board";
    let mut h = Harness::new(230.0, 400.0);
    let rows = Rows::one(
        "Courtyards overlap on the top side",
        &[
            long,
            "R1 ↔ R2 courtyards overlap",
            "C4 ↔ U1 courtyards overlap",
        ],
    );
    let mut out = IssueListOutput::default();
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let l = ListTokens::get(h.ctx());
    let titles: Vec<&Painted> = painted
        .iter()
        .filter(|p| p.text == long || p.text.contains("courtyards overlap"))
        .collect();
    assert_eq!(titles.len(), 3, "{painted:?}");
    for t in &titles {
        // A cut name may take two smaller lines; it stays in its row.
        assert!(t.rect.right() <= 230.0, "{t:?}");
        assert!(t.rows <= 2, "{t:?}");
        assert!(t.clip.height() <= l.row_h + 0.5, "{t:?}");
    }
    for w in titles.windows(2) {
        assert!(
            w[1].rect.top() - w[0].rect.top() >= l.row_h - 0.5,
            "{titles:?}"
        );
    }
    assert!(titles.iter().any(|t| t.elided), "the long title is cut");
    let ctx = h.ctx().clone();
    let (cut, missing) = crate::kit::text::tests::cuts_without_hover(&ctx, |events| {
        h.output(events, |ui| draw(ui, &rows, &mut out))
    });
    assert!(cut >= 1, "{cut}");
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
}

/// A disclosed row shows its cause, fix and object chips on lines of its
/// own; a chip locates only its object.
#[test]
fn an_open_row_shows_its_details_and_chips_that_locate_one_object() {
    let mut h = Harness::new(360.0, 400.0);
    let mut rows = Rows::one("Pads", &["2 pads have no package terminal", "Another"]);
    rows.detail = IssueDetail {
        cause: Some("Each pad needs a package terminal.".to_owned()),
        fix: Some("Assign the pads by number.".to_owned()),
    };
    rows.objects = vec!["Pad 1".to_owned(), "Pad 2".to_owned()];
    let mut out = IssueListOutput::default();
    let closed = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    assert!(find(&closed, "Pad 2").is_none());
    rows.open.insert((0, 0));
    // The rows below move down (FLIP) and then rest.
    h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    for text in [
        "Each pad needs a package terminal.",
        "Assign the pads by number.",
        "Pad 1",
    ] {
        assert!(find(&painted, text).is_some(), "{text}: {painted:?}");
    }
    // The next row moved down below the details, never under them.
    let fix = rect_of(&painted, "Assign the pads by number.");
    let chip = rect_of(&painted, "Pad 2");
    let next = rect_of(&painted, "Another");
    assert!(
        fix.bottom() <= chip.top() && chip.bottom() <= next.top(),
        "{painted:?}"
    );
    h.click(chip.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 0, RowAction::Object(1))]);
}

/// Nothing is capped: 10 000 rows take their full height, but only the
/// lines in view are built.
#[test]
fn the_list_is_uncapped_and_builds_only_visible_rows() {
    let mut h = Harness::new(360.0, 400.0);
    let mut rows = Rows::of(vec![
        (
            "Clearance",
            Level::Error,
            (0..10_000).map(|r| format!("Row {r}")).collect(),
        ),
        (
            "Not placed",
            Level::ToDo,
            (0..6).map(|r| format!("R{r} is not placed")).collect(),
        ),
    ]);
    rows.groups[1].open = false;
    rows.groups[1].fix = Some("Update PCB\u{2026}".to_owned());
    let mut out = IssueListOutput::default();
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let l = ListTokens::get(h.ctx());
    let total = (10_000.0 + 2.0) * l.row_h;
    assert!(
        (out.height - total).abs() < 1.0,
        "{} vs {total}",
        out.height
    );
    let most = (300.0 / l.row_h).ceil() as usize + 1 + 2 * list::virt::OVERSCAN;
    assert!(out.built <= most, "built {}", out.built);
    assert!(find(&painted, "CLEARANCE").is_some(), "{painted:?}");
    assert!(
        find(&painted, "10000").is_some(),
        "the group count is shown"
    );
    assert!(find(&painted, "Row 0").is_some());
}

/// A header click folds its group, the group's Fix line asks for the
/// group's Fix and a row click locates that row.
#[test]
fn list_events_name_the_group_or_row() {
    let mut h = Harness::new(360.0, 400.0);
    let mut rows = Rows::one("Not placed", &["R20 is not placed", "R21 is not placed"]);
    rows.groups[0].fix = Some("Update PCB\u{2026}".to_owned());
    let mut out = IssueListOutput::default();
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let header = rect_of(&painted, "NOT PLACED");
    let fix = rect_of(&painted, "Update PCB\u{2026}");
    let second = rect_of(&painted, "R21 is not placed");
    h.click(header.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![IssueListEvent::Group(0)]);
    h.click(fix.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![IssueListEvent::GroupFix(0)]);
    h.click(second.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 1, RowAction::Locate)]);
}

/// Enter locates the row the keyboard is on, as a click does; ↓ moves.
#[test]
fn enter_locates_the_keyboards_row() {
    let mut h = Harness::new(360.0, 400.0);
    let rows = Rows::one("Pins", &["R1 pin 1 unconnected", "R2 pin 2 unconnected"]);
    let mut out = IssueListOutput::default();
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let first = rect_of(&painted, "R1 pin 1 unconnected");
    h.click(first.center(), |ui| draw(ui, &rows, &mut out));
    h.frame(vec![key(Key::ArrowDown)], |ui| draw(ui, &rows, &mut out));
    assert!(out.events.is_empty(), "↓ only moves: {:?}", out.events);
    h.frame(vec![key(Key::Enter)], |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![row_event(0, 1, RowAction::Locate)]);
    // ← on a row with nothing to disclose goes to its group; Enter there
    // folds it.
    h.frame(vec![key(Key::ArrowLeft)], |ui| draw(ui, &rows, &mut out));
    h.frame(vec![key(Key::Enter)], |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![IssueListEvent::Group(0)]);
}

/// Scrolled down a long group, its header sticks to the top of the list,
/// over the rows, and a click on it folds the group.
#[test]
fn the_group_header_sticks_at_a_scrolled_offset() {
    let mut h = Harness::new(360.0, 400.0);
    let titles: Vec<String> = (0..200).map(|r| format!("Row {r}")).collect();
    let refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let rows = Rows::one("Clearance", &refs);
    let mut out = IssueListOutput::default();
    h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    let wheel = Event::MouseWheel {
        source: egui::MouseWheelSource::Unknown,
        phase: egui::TouchPhase::Move,
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -1000.0),
        modifiers: Modifiers::NONE,
    };
    h.frame(vec![Event::PointerMoved(pos2(180.0, 150.0)), wheel], |ui| {
        draw(ui, &rows, &mut out)
    });
    let painted = h.frame(vec![], |ui| draw(ui, &rows, &mut out));
    assert!(
        find(&painted, "Row 0").is_none(),
        "scrolled past the first rows"
    );
    let l = ListTokens::get(h.ctx());
    let header = rect_of(&painted, "CLEARANCE");
    assert!(
        header.top() >= 0.0 && header.bottom() <= l.row_h,
        "{header:?}"
    );
    h.click(header.center(), |ui| draw(ui, &rows, &mut out));
    assert_eq!(out.events, vec![IssueListEvent::Group(0)]);
}

/// The reason field: typed text and Enter send the waiver, Esc cancels,
/// and a refusal shows under the field.
#[test]
fn the_reason_field_commits_on_enter_and_shows_a_refusal() {
    let mut h = Harness::new(230.0, 400.0);
    let rows = Rows::one("Holes", &["J1 peg hole is unplated"]);
    let mut text = String::new();
    let mut error: Option<String> = None;
    let mut start = true;
    let mut out = IssueListOutput::default();
    let mut frame = |h: &mut Harness,
                     events: Vec<Event>,
                     text: &mut String,
                     start: &mut bool,
                     error: &Option<String>| {
        let painted = h.frame(events, |ui| {
            out = issues_list(
                ui,
                egui::Id::unique("issues-test"),
                300.0,
                &rows,
                Waived {
                    rows: &[],
                    open: true,
                },
                Some(Reason {
                    group: 0,
                    row: 0,
                    text: &mut *text,
                    error: error.as_deref(),
                    required: true,
                    start: *start,
                }),
            );
        });
        *start = false;
        (out.events.clone(), painted)
    };
    let (_, painted) = frame(&mut h, vec![], &mut text, &mut start, &error);
    assert!(
        find(&painted, "Why it is acceptable (required)").is_some(),
        "{painted:?}"
    );
    frame(
        &mut h,
        vec![Event::Text("Pegs locate the jack".into())],
        &mut text,
        &mut start,
        &error,
    );
    assert_eq!(text, "Pegs locate the jack");
    let (events, _) = frame(&mut h, vec![key(Key::Enter)], &mut text, &mut start, &error);
    assert_eq!(events, vec![IssueListEvent::Commit]);
    // Refused: the reason stays and the refusal shows under it in red.
    error = Some("A reason is required to waive findings (Settings)".to_owned());
    start = true;
    let (_, painted) = frame(&mut h, vec![], &mut text, &mut start, &error);
    let refusal = find(
        &painted,
        "A reason is required to waive findings (Settings)",
    )
    .unwrap_or_else(|| panic!("{painted:?}"));
    assert_eq!(refusal.ink, severity::ink(Level::Error));
    assert!(refusal.rect.right() <= 230.0, "{refusal:?}");
    let (events, _) = frame(
        &mut h,
        vec![key(Key::Escape)],
        &mut text,
        &mut start,
        &error,
    );
    assert_eq!(events, vec![IssueListEvent::Cancel]);
}

/// The Waived group lists each waived finding with its reason; Restore
/// asks for it back.
#[test]
fn waived_findings_show_their_reason_and_restore() {
    let mut h = Harness::new(230.0, 400.0);
    let rows = Rows::one("Holes", &["R1 pad hole too small"]);
    let waived = [WaivedView {
        key: RowKey::of("w"),
        name: "J1 \u{b7} Hole 2".to_owned(),
        reason: "Locating pegs are unplated by design".to_owned(),
    }];
    let mut out = IssueListOutput::default();
    let mut draw = |ui: &mut Ui| {
        out = issues_list(
            ui,
            egui::Id::unique("issues-test"),
            300.0,
            &rows,
            Waived {
                rows: &waived,
                open: true,
            },
            None,
        );
    };
    let painted = h.frame(vec![], &mut draw);
    let name = rect_of(&painted, "J1 \u{b7} Hole 2");
    assert!(find(&painted, "WAIVED").is_some(), "{painted:?}");
    let reason = rect_of(&painted, "Locating pegs are unplated by design");
    assert!(
        reason.top() >= name.bottom(),
        "the reason is under the name"
    );
    h.frame(vec![Event::PointerMoved(name.center())], &mut draw);
    let hovered = h.frame(vec![], &mut draw);
    let restore = rect_of(&hovered, "Restore");
    h.click(restore.center(), &mut draw);
    assert_eq!(out.events, vec![IssueListEvent::Restore(0)]);
}
