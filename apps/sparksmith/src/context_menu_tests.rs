//! The context menu's look and behaviour: Solidcraft's rows with their
//! keys, a disabled row dimmed and inert, and the ways it closes.
use super::*;
use crate::commands::Chord;
use crate::kit::text::tests::{Screen, painted};
use egui::{Event, PointerButton, Rect, pos2};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    Rotate,
    Mirror,
    Delete,
}

fn rows(ctx: &egui::Context) -> Rows<Act> {
    vec![
        Some(Entry::act("Rotate", "spin", Act::Rotate, true).key_of(ctx, "schematic.rotate")),
        Some(Entry::act("Mirror", "mirror", Act::Mirror, false).key_of(ctx, "schematic.mirror")),
        None,
        Some(Entry::command(ctx, "view.fit", "Fit view", true)),
        None,
        Some(Entry::act("Delete…", "delete", Act::Delete, true).key_of(ctx, "schematic.delete")),
    ]
}

fn press(at: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One menu frame on `screen`: returns its output and the row chosen.
fn frame(
    screen: &Screen,
    state: &mut State<()>,
    events: Vec<Event>,
) -> (egui::FullOutput, Option<Run<Act>>) {
    let mut chosen = None;
    let out = screen.run(events, |ui| {
        hold_keys(ui.ctx());
        let rows = rows(ui.ctx());
        chosen = state.show(ui.ctx(), &rows);
    });
    (out, chosen)
}

/// The output once the menu's fade-in has ended.
fn settled(screen: &Screen, state: &mut State<()>, first: Vec<Event>) -> egui::FullOutput {
    frame(screen, state, first);
    for _ in 0..30 {
        frame(screen, state, vec![]);
    }
    frame(screen, state, vec![]).0
}

/// The pixel at `at` of an RGBA picture `width` pixels wide.
fn pixel(rgba: &[u8], width: usize, at: Pos2) -> [u8; 3] {
    let start = (at.y as usize * width + at.x as usize) * 4;
    [rgba[start], rgba[start + 1], rgba[start + 2]]
}

/// How far the inkiest pixel in `rect` is from `background`.
fn contrast(rgba: &[u8], width: usize, rect: Rect, background: [u8; 3]) -> i32 {
    let mut most = 0;
    for y in rect.top() as usize..rect.bottom() as usize {
        for x in rect.left() as usize..rect.right() as usize {
            let c = pixel(rgba, width, pos2(x as f32, y as f32));
            let d = (0..3)
                .map(|i| (i32::from(c[i]) - i32::from(background[i])).abs())
                .sum();
            most = i32::max(most, d);
        }
    }
    most
}

/// Measured in pixels, once faded in: the menu's body is the dialog colour
/// the kit's menus use, an enabled label stands out more than a disabled
/// one, Delete is drawn in the error colour and a hovered row is lit.
#[test]
fn the_menus_look_measured_in_pixels() -> TestResult {
    use sparksmith_ui::theme::Tokens;
    let screen = Screen::new(egui::vec2(400.0, 320.0));
    let width = 400;
    let mut state = State::default();
    state.open(pos2(20.0, 20.0), ());
    let out = settled(&screen, &mut state, vec![]);
    let texts = painted(&out);
    let at = |text: &str| {
        texts
            .iter()
            .find(|(t, ..)| t == text)
            .map(|(_, rect, ..)| *rect)
            .ok_or(format!("{text} is not painted"))
    };
    let (rotate, mirror, delete, fit) = (
        at("Rotate")?,
        at("Mirror")?,
        at("Delete…")?,
        at("Fit view")?,
    );
    let rgba = screen.rgba(&out);
    let t = Tokens::get();
    let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
    let near = |a: [u8; 3], b: [u8; 3]| (0..3).all(|i| a[i].abs_diff(b[i]) <= 3);
    let body = pixel(&rgba, width, pos2(rotate.right() + 40.0, rotate.center().y));
    assert!(near(body, rgb(t.dialog_body)), "{body:?}");
    let enabled = contrast(&rgba, width, rotate, body);
    let disabled = contrast(&rgba, width, mirror, body);
    assert!(
        enabled > disabled + 60,
        "enabled {enabled}, disabled {disabled}"
    );
    // Delete's label moves towards the error colour: its inkiest pixel is
    // red-dominant, as the kit's danger row.
    let mut reddest = 0;
    for y in delete.top() as usize..delete.bottom() as usize {
        for x in delete.left() as usize..delete.right() as usize {
            let c = pixel(&rgba, width, pos2(x as f32, y as f32));
            reddest = i32::max(reddest, i32::from(c[0]) - i32::from(c[1]));
        }
    }
    assert!(reddest > 60, "Delete is not red: {reddest}");
    // Hovering Fit view lights its row.
    let spot = pos2(fit.right() + 40.0, fit.center().y);
    assert!(near(pixel(&rgba, width, spot), body), "not lit at rest");
    let out = settled(&screen, &mut state, vec![Event::PointerMoved(fit.center())]);
    let lit = pixel(&screen.rgba(&out), width, spot);
    assert!(!near(lit, body), "a hovered row is lit: {lit:?}");
    Ok(())
}

fn click(screen: &Screen, state: &mut State<()>, at: Pos2) -> Option<Run<Act>> {
    frame(
        screen,
        state,
        vec![
            Event::PointerMoved(at),
            press(at, PointerButton::Primary, true),
        ],
    );
    frame(
        screen,
        state,
        vec![press(at, PointerButton::Primary, false)],
    )
    .1
}

/// The menu is Solidcraft's list: rows about a control high, each key
/// right-aligned on its row from the registry, separators between groups,
/// about 250 points wide; a disabled row is drawn and chooses nothing, an
/// enabled one is chosen and closes the menu. With
/// `SPARKSMITH_TEXT_PICTURES=<dir>` it writes `context-menu.png`.
#[test]
fn the_menu_is_solidcrafts_rows_with_keys() -> TestResult {
    let screen = Screen::new(egui::vec2(400.0, 320.0));
    let mut state = State::default();
    state.open(pos2(20.0, 20.0), ());
    let out = settled(&screen, &mut state, vec![]);
    screen.picture("context-menu", &out, 2.0)?;
    let texts = painted(&out);
    let at = |text: &str| {
        texts
            .iter()
            .find(|(t, ..)| t == text)
            .map(|(_, rect, ..)| *rect)
            .ok_or(format!("{text} is not painted in {texts:?}"))
    };
    let rotate = at("Rotate")?;
    let mirror = at("Mirror")?;
    let fit = at("Fit view")?;
    let delete = at("Delete…")?;
    assert!(rotate.left() >= 20.0, "it opens right of the pointer");
    let pitch = mirror.center().y - rotate.center().y;
    assert!((24.0..=34.0).contains(&pitch), "row pitch {pitch}");
    assert!(
        fit.center().y - mirror.center().y > pitch + 5.0,
        "a separator before Fit view"
    );
    assert!(delete.center().y > fit.center().y + pitch);
    let key = at(&Chord::cmd(egui::Key::R).shown())?;
    assert!(
        (key.center().y - rotate.center().y).abs() < 1.0,
        "on its row"
    );
    assert!(key.left() > rotate.right() + 100.0, "right-aligned");
    // Delete and Backspace both delete; a menu row shows the one key.
    at(&Chord::key(egui::Key::Delete).shown())?;
    assert!(
        texts.iter().all(|(t, ..)| !t.contains(", ")),
        "two keys in a row: {texts:?}"
    );
    let right = texts
        .iter()
        .map(|(_, r, ..)| r.right())
        .fold(0.0_f32, f32::max);
    assert!(right <= 20.0 + LIST_W + 4.0, "{right}");

    // The disabled row: nothing, and the menu stays.
    assert_eq!(click(&screen, &mut state, mirror.center()), None);
    assert!(state.is_open());
    // An enabled row: chosen once, and the menu closes.
    assert_eq!(
        click(&screen, &mut state, rotate.center()),
        Some(Run::Act(Act::Rotate))
    );
    assert!(!state.is_open());
    // A registry row runs its command.
    state.open(pos2(20.0, 20.0), ());
    settled(&screen, &mut state, vec![]);
    assert_eq!(
        click(&screen, &mut state, fit.center()),
        Some(Run::Command("view.fit"))
    );
    Ok(())
}

/// Esc, a press outside it and an owner that stops drawing it close the
/// menu; the click that opened it does not.
#[test]
fn the_menu_closes_on_esc_a_press_outside_and_when_not_drawn() {
    let screen = Screen::new(egui::vec2(400.0, 320.0));
    let mut state = State::default();
    let outside = pos2(390.0, 310.0);
    // Opened in the frame of the right button's release, with the press
    // still in this frame's input: it stays open.
    state.open(pos2(20.0, 20.0), ());
    frame(
        &screen,
        &mut state,
        vec![press(pos2(20.0, 20.0), PointerButton::Secondary, true)],
    );
    assert!(state.is_open());
    frame(
        &screen,
        &mut state,
        vec![press(outside, PointerButton::Primary, true)],
    );
    assert!(!state.is_open(), "a press outside closes it");

    state.open(pos2(20.0, 20.0), ());
    frame(&screen, &mut state, vec![]);
    let esc = Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&screen, &mut state, vec![esc]);
    assert!(!state.is_open(), "Esc closes it");

    state.open(pos2(20.0, 20.0), ());
    frame(&screen, &mut state, vec![]);
    screen.run(vec![], |_| {});
    frame(&screen, &mut state, vec![]);
    assert!(!state.is_open(), "a menu its owner stopped drawing closes");

    // A menu near the corner flips to stay on screen.
    state.open(outside, ());
    let (out, _) = frame(&screen, &mut state, vec![]);
    let screen_rect = Rect::from_min_size(Pos2::ZERO, screen.size);
    for (text, rect, ..) in painted(&out) {
        assert!(screen_rect.contains_rect(rect), "{text} at {rect:?}");
    }
}

/// Run 3, finding 6: a canvas runs before its menu in a frame and takes Esc
/// as its own cancel, which is why the menu never saw it. With the app's
/// `hold_keys` first, Esc closes the menu and nothing underneath hears it,
/// and ⌘Z does nothing underneath an open menu.
#[test]
fn an_open_menu_takes_esc_and_shortcuts_from_what_is_underneath() {
    let screen = Screen::new(egui::vec2(400.0, 320.0));
    let key = |key, modifiers| Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    let mut state = State::default();
    let run = |state: &mut State<()>, events: Vec<Event>, hold: bool| {
        let mut heard = Vec::new();
        screen.run(events, |ui| {
            if hold {
                hold_keys(ui.ctx());
            }
            // A canvas: it takes its keys before the menu draws.
            for k in [egui::Key::Escape, egui::Key::Z] {
                let press = ui.input_mut(|i| {
                    i.consume_key(egui::Modifiers::NONE, k)
                        || i.consume_key(egui::Modifiers::COMMAND, k)
                });
                if press {
                    heard.push(k);
                }
            }
            let rows = rows(ui.ctx());
            let _ = state.show(ui.ctx(), &rows);
        });
        heard
    };
    let esc = || vec![key(egui::Key::Escape, egui::Modifiers::NONE)];
    // The bug: without the hold the canvas takes Esc and the menu stays.
    state.open(pos2(20.0, 20.0), ());
    run(&mut state, vec![], false);
    assert_eq!(run(&mut state, esc(), false), [egui::Key::Escape]);
    assert!(state.is_open());
    state.close();
    run(&mut state, vec![], true);

    state.open(pos2(20.0, 20.0), ());
    run(&mut state, vec![], true);
    let undo = vec![key(egui::Key::Z, egui::Modifiers::COMMAND)];
    assert_eq!(
        run(&mut state, undo, true),
        [],
        "⌘Z acts on nothing underneath"
    );
    assert!(state.is_open(), "⌘Z leaves the menu open");
    assert_eq!(
        run(&mut state, esc(), true),
        [],
        "Esc reaches nothing underneath"
    );
    assert!(!state.is_open(), "Esc closes it");
    // Closed: the keys go through again.
    assert_eq!(run(&mut state, esc(), true), [egui::Key::Escape]);
}
