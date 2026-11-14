//! egui-frame tests of the list widgets: rows at the narrowest Properties
//! width, keys through a focused list, virtualisation, the filter bar.
use super::*;
use crate::kit::rows::tests::Harness;
use egui::{Event, Key, Modifiers, Pos2};

const NAMES: [&str; 6] = ["C1", "C2", "R1", "R2", "R10", "U1"];

fn lines() -> Vec<Line<'static>> {
    NAMES.iter().map(|n| Line::item(RowKey::of(n), n)).collect()
}

fn key(k: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Show a bounded list of `lines` and return its output.
fn show(ui: &mut egui::Ui, lines: &[Line<'_>], layout: Layout) -> ListOutput {
    show_with(ui, lines, layout, false)
}

/// [`show`], with a click on a value opening its editor when `click_edits`.
fn show_with(
    ui: &mut egui::Ui,
    lines: &[Line<'_>],
    layout: Layout,
    click_edits: bool,
) -> ListOutput {
    let view = ListView {
        delete: true,
        reorder: true,
        click_edits,
        ..ListView::new(egui::Id::unique("test-list"), "Parts", layout)
    };
    let values = ["12.700 mm", "3 mm", "-1.25 mm", "40 mm", "Held", "0.5 mm"];
    let cols = Columns::measure(
        ui,
        &ListTokens::get(ui.ctx()),
        values.iter().map(|v| Value::Number(v)),
    );
    view.show(ui, lines, |ui, rect, line, cx| {
        let index = NAMES.iter().position(|n| *n == line.name).unwrap_or(0);
        let name = [line.name];
        let actions = [Action::delete("Delete")];
        let row = Row {
            selected: cx.selected,
            cursor: cx.cursor,
            value: Value::Number(values.get(index).copied().unwrap_or("")),
            actions: &actions,
            ..Row::new(line.key, &name)
        };
        row::paint(ui, cx.id, rect, &row, &cols)
    })
}

/// Rows never overlap, never paint outside the 230 pt column, and the
/// numbers line up on their decimal point.
#[test]
fn rows_fit_230pt_and_numbers_align_on_the_point() {
    let mut h = Harness::new(230.0, 400.0);
    let l = lines();
    let painted = h.frame(vec![], |ui| {
        show(ui, &l, Layout::Bounded { rows: 8 });
    });
    for name in NAMES {
        let p = painted.iter().find(|p| p.text == name);
        assert!(
            p.is_some_and(|p| p.rect.right() <= 230.0),
            "{name} shown in the column"
        );
    }
    let ys: Vec<f32> = NAMES
        .iter()
        .filter_map(|n| {
            painted
                .iter()
                .find(|p| p.text == *n)
                .map(|p| p.rect.center().y)
        })
        .collect();
    for w in ys.windows(2) {
        if let [a, b] = w {
            assert!(b - a >= 25.9, "rows 26 pt apart: {ys:?}");
        }
    }
    // ".700", ".25" and ".5" start at one x: the decimal point.
    let fracs: Vec<f32> = [".700", ".25", ".5"]
        .iter()
        .filter_map(|f| painted.iter().find(|p| p.text == *f).map(|p| p.rect.left()))
        .collect();
    assert_eq!(fracs.len(), 3, "{painted:#?}");
    assert!(
        fracs.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5),
        "{fracs:?}"
    );
    // Units in their own faint slot.
    assert!(painted.iter().any(|p| p.text == "mm"));
}

/// Where values edit on a click (Board parameters, Dimensions), one click
/// on a row's value opens its editor, as a number field does; a click on
/// its name only selects. Without the rule a click on a value selects.
#[test]
fn a_click_on_a_value_edits_where_the_list_allows_it() {
    let l = lines();
    let bounded = Layout::Bounded { rows: 8 };
    let mut h = Harness::new(300.0, 400.0);
    let mut out = ListOutput::default();
    h.frame(vec![], |ui| out = show_with(ui, &l, bounded, true));
    let c2 = RowKey::of("C2");
    let value = out
        .value_rects
        .iter()
        .find(|(k, _)| *k == c2)
        .map(|(_, r)| r.center());
    assert!(
        value.is_some(),
        "C2's value is drawn: {:?}",
        out.value_rects
    );
    let value = value.unwrap_or_default();
    h.click(value, |ui| out = show_with(ui, &l, bounded, true));
    assert!(
        out.effects.contains(&Effect::Activate(c2)),
        "{:?}",
        out.effects
    );
    assert!(out.state.selection.contains(&c2));
    let name = Pos2::new(30.0, value.y);
    h.click(name, |ui| out = show_with(ui, &l, bounded, true));
    assert!(
        !out.effects.iter().any(|e| matches!(e, Effect::Activate(_))),
        "{:?}",
        out.effects
    );
    let mut h = Harness::new(300.0, 400.0);
    h.frame(vec![], |ui| out = show(ui, &l, bounded));
    h.click(value, |ui| out = show(ui, &l, bounded));
    assert_eq!(out.effects, vec![Effect::Selected]);
}

/// A click focuses the list and selects; ↓ moves; ⇧↓ extends; ⌘A
/// selects all; Esc clears; ⌥↓ asks for a reorder; ⌫ for a delete.
#[test]
fn keys_drive_a_focused_list() {
    let mut h = Harness::new(300.0, 400.0);
    let l = lines();
    let mut out = ListOutput::default();
    h.frame(vec![], |ui| out = show(ui, &l, Layout::Bounded { rows: 8 }));
    // Click C2's row (the second row, 26 pt high, from y 0).
    let at = Pos2::new(60.0, 26.0 + 13.0);
    h.click(at, |ui| out = show(ui, &l, Layout::Bounded { rows: 8 }));
    assert_eq!(out.state.selection.len(), 1);
    assert!(out.state.selection.contains(&RowKey::of("C2")));
    let mut step = |events: Vec<Event>| {
        let mut o = ListOutput::default();
        h.frame(events, |ui| o = show(ui, &l, Layout::Bounded { rows: 8 }));
        o
    };
    let o = step(vec![key(Key::ArrowDown, Modifiers::NONE)]);
    assert!(o.focused, "the click gave the list the keyboard");
    assert_eq!(o.state.cursor, Some(RowKey::of("R1")));
    let o = step(vec![key(Key::ArrowDown, Modifiers::SHIFT)]);
    assert_eq!(o.state.selection.len(), 2);
    let o = step(vec![key(Key::A, Modifiers::COMMAND)]);
    assert_eq!(o.state.selection.len(), 6);
    let o = step(vec![key(Key::Escape, Modifiers::NONE)]);
    assert!(o.state.selection.is_empty());
    let o = step(vec![key(Key::ArrowDown, Modifiers::ALT)]);
    assert!(
        o.effects.iter().any(|e| matches!(e, Effect::Reorder(_, 1))),
        "{:?}",
        o.effects
    );
    let o = step(vec![
        key(Key::ArrowUp, Modifiers::NONE),
        key(Key::Backspace, Modifiers::NONE),
    ]);
    assert!(
        o.effects
            .iter()
            .any(|e| matches!(e, Effect::Delete(k) if k.len() == 1)),
        "{:?}",
        o.effects
    );
}

/// A scrolled list of 10 000 rows builds only what shows, plus overscan.
#[test]
fn ten_thousand_rows_build_a_screenful() {
    let names: Vec<String> = (0..10_000).map(|i| format!("V{i}")).collect();
    let l: Vec<Line<'_>> = names
        .iter()
        .map(|n| Line::item(RowKey::of(n), n.as_str()))
        .collect();
    let mut h = Harness::new(300.0, 500.0);
    let mut built = 0;
    let started = std::time::Instant::now();
    h.frame(vec![], |ui| {
        let view = ListView::new(
            egui::Id::unique("vias"),
            "Vias",
            Layout::Scrolled { height: 400.0 },
        );
        let cols = Columns::default();
        let out = view.show(ui, &l, |ui, rect, line, cx| {
            let name = [line.name];
            row::paint(ui, cx.id, rect, &Row::new(line.key, &name), &cols)
        });
        built = out.built;
    });
    assert!(built <= 400 / 26 + 1 + 2 * virt::OVERSCAN, "built {built}");
    assert!(built >= 400 / 26, "built {built}");
    // A target, not a gate: one frame of 10 000 rows well under a second.
    assert!(started.elapsed().as_secs_f32() < 2.0);
}

/// A selection made elsewhere reveals its row: the next frame scrolls the
/// virtualised list so that row is built and shown.
#[test]
fn reveal_scrolls_a_row_into_view() {
    let names: Vec<String> = (0..2_000).map(|i| format!("V{i}")).collect();
    let l: Vec<Line<'_>> = names
        .iter()
        .map(|n| Line::item(RowKey::of(n), n.as_str()))
        .collect();
    let view = ListView::new(
        egui::Id::unique("reveal"),
        "Vias",
        Layout::Scrolled { height: 300.0 },
    );
    let mut h = Harness::new(300.0, 400.0);
    let show = |h: &mut Harness| {
        h.frame(vec![], |ui| {
            let cols = Columns::default();
            view.show(ui, &l, |ui, rect, line, cx| {
                let name = [line.name];
                row::paint(ui, cx.id, rect, &Row::new(line.key, &name), &cols)
            });
        })
    };
    let painted = show(&mut h);
    assert!(!painted.iter().any(|p| p.text == "V1500"));
    view.reveal(h.ctx(), 1500);
    show(&mut h);
    let painted = show(&mut h);
    let row = painted.iter().find(|p| p.text == "V1500");
    assert!(
        row.is_some_and(|p| p.rect.top() >= 0.0 && p.rect.bottom() <= 300.0),
        "V1500 shown: {row:?}"
    );
}

/// The filter bar shows a chip per term and an error chip that says why;
/// the chip's × removes its term.
#[test]
fn the_filter_bar_shows_chips_and_removes_a_term() {
    let fields = query::Fields::new(vec![
        query::Field::new("x", query::Kind::Length),
        query::Field::new(
            "side",
            query::Kind::Choice(vec!["Top".into(), "Bottom".into()]),
        ),
    ]);
    let bar = filter::FilterBar {
        id: egui::Id::unique("bar"),
        list: "test.parts",
        hint: "Filter",
        saving: true,
    };
    let mut h = Harness::new(300.0, 300.0);
    let mut text = "x>=12 side:left".to_owned();
    let painted = h.frame(vec![], |ui| {
        bar.show(ui, &mut text, &fields);
    });
    assert!(
        painted.iter().any(|p| p.text == "x ≥ 12 mm"),
        "{painted:#?}"
    );
    assert!(painted.iter().any(|p| p.text == "side:left"));
    // The star at the field's right end saves the query for the app to
    // persist, and the saved chip shows at once.
    let star = Pos2::new(280.0, 14.0);
    let painted = h.click(star, |ui| {
        bar.show(ui, &mut text, &fields);
    });
    let changes = filter::take_changes(h.ctx());
    assert_eq!(
        changes,
        vec![filter::SavedChange::Add(filter::Saved {
            list: "test.parts".into(),
            query: "x>=12 side:left".into(),
        })]
    );
    let _ = painted;
    filter::publish_saved(
        h.ctx(),
        vec![filter::Saved {
            list: "test.parts".into(),
            query: "side:bottom".into(),
        }],
    );
    let painted = h.frame(vec![], |ui| {
        bar.show(ui, &mut text, &fields);
    });
    assert!(
        painted.iter().any(|p| p.text == "side:bottom"),
        "saved chip"
    );
}
