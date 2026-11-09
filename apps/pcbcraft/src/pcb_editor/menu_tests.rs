//! The Layout canvas's right-click menus: what each object kind offers,
//! that a right-click selects first, and that a row runs the right command.
use super::super::{Layer, board_view};
use super::*;
use crate::context_menu::Run;
use egui::{Event, PointerButton, Pos2, Rect, pos2, vec2};
use pcbcraft_core::Point;
use pcbcraft_core::layout::RouteRequest;
use pcbcraft_core::layout::via::ViaRequest;
use solidcraft_ui_egui::fonts;

/// Operations compare by their Debug text (`Operation` has no `PartialEq`).
macro_rules! assert_op {
    ($left:expr, $right:expr $(,)?) => {
        assert_eq!(format!("{:?}", $left), format!("{:?}", $right))
    };
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<Event>,
) -> (egui::FullOutput, Option<Operation>) {
    let mut sent = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
            events,
            ..Default::default()
        },
        |ui| sent = editor.canvas(ui, engine),
    );
    output.textures_delta.clear();
    (output, sent)
}

fn click(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    at: Pos2,
    button: PointerButton,
) -> Vec<Operation> {
    let event = |pressed| Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut sent = Vec::new();
    for events in [
        vec![Event::PointerMoved(at)],
        vec![event(true)],
        vec![event(false)],
    ] {
        sent.extend(frame(editor, engine, ctx, events).1);
    }
    sent
}

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

/// `board` on the 900 × 700 canvas at the editor's view.
fn on_screen(editor: &PcbEditor, engine: &Engine, board: Point) -> Pos2 {
    let rect = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0));
    board_view(
        rect,
        engine.project().board_size,
        editor.zoom,
        editor.pan,
        false,
    )
    .screen(board)
}

/// Every row's icon exists (an unknown name draws the placeholder).
fn check_icons(rows: &Rows<Act>) {
    for entry in rows.iter().flatten() {
        assert!(
            !solidcraft_ui_egui::icon_svg::resolve(entry.icon).placeholder,
            "{} has no icon {}",
            entry.label,
            entry.icon
        );
    }
}

fn labels(rows: &Rows<Act>) -> Vec<String> {
    check_icons(rows);
    rows.iter()
        .map(|row| row.as_ref().map_or("—".to_owned(), |e| e.label.clone()))
        .collect()
}

fn enabled(rows: &Rows<Act>, label: &str) -> Option<bool> {
    rows.iter()
        .flatten()
        .find(|e| e.label == label)
        .map(|e| e.enabled)
}

fn text_at(output: &egui::FullOutput, label: &str) -> Result<Pos2, String> {
    crate::kit::text::tests::painted(output)
        .into_iter()
        .find(|(t, ..)| t == label)
        .map(|(_, rect, ..)| rect.center())
        .ok_or_else(|| format!("{label} is not painted"))
}

/// A demo engine with a routed track and a via on R1's second net.
fn routed() -> Result<(Engine, Id, Id, Id), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let net = engine
        .project()
        .net_of(pcbcraft_core::Endpoint {
            instance: engine.project().instances[0].id,
            terminal: pcbcraft_core::TerminalId(2),
        })
        .map(|n| n.id)
        .ok_or("demo net")?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Route {
            request: RouteRequest {
                net,
                start: [9.2, 8.0],
                end: [14.8, 8.0],
                width: 0.3,
                layer: Layer::Top.into(),
            },
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::AddVia {
            via: ViaRequest::through(net, [12.0, 11.0]),
        }),
    )?;
    let track = engine.project().tracks.first().ok_or("track")?.id;
    let via = engine.project().vias.first().ok_or("via")?.id;
    Ok((engine, net, track, via))
}

/// A part: Rotate, Flip side, Lock, Dimension to edge when a named edge
/// can hold it, Delete…; while locked the moves are dimmed and Lock says
/// Unlock.
#[test]
fn a_part_offers_its_placement_actions() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let r1 = engine.project().placements[0].instance;
    let target = Target::Part(r1);
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(
        labels(&rows),
        [
            "Rotate 90°",
            "Flip side",
            "—",
            "Lock",
            "Dimension to edge",
            "—",
            "Delete…"
        ]
    );
    assert_eq!(enabled(&rows, "Rotate 90°"), Some(true));
    let before = engine.project().placements[0].clone();
    assert_op!(
        editor.menu_act(&engine, Act::Rotate(r1)),
        Some(Operation::Layout(LayoutCommand::Move {
            instance: r1,
            position: before.position,
            rotation_deg: (before.rotation_deg + 90.0).rem_euclid(360.0),
            layer: before.layer,
        }))
    );
    assert_op!(
        editor.menu_act(&engine, Act::Flip(r1)),
        Some(Operation::Layout(LayoutCommand::Move {
            instance: r1,
            position: before.position,
            rotation_deg: before.rotation_deg,
            layer: if before.layer == Layer::Top {
                Layer::Bottom
            } else {
                Layer::Top
            },
        }))
    );
    let lock = editor
        .menu_act(&engine, Act::Lock(r1, true))
        .ok_or("lock")?;
    assert_op!(
        lock,
        Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: r1,
            locked: true,
        })
    );
    engine.apply(engine.revision(), lock)?;
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(enabled(&rows, "Unlock"), Some(true));
    assert_eq!(enabled(&rows, "Rotate 90°"), Some(false), "locked");
    assert_eq!(enabled(&rows, "Flip side"), Some(false), "locked");
    assert_eq!(enabled(&rows, "Delete…"), Some(true));

    // Delete… opens the same reviewed removal as the Schematic's Remove….
    assert!(editor.menu_act(&engine, Act::Remove(r1)).is_none());
    assert!(editor.update.is_open());
    let rows = editor.menu_rows(&ctx, &engine, target);
    assert_eq!(enabled(&rows, "Delete…"), Some(false), "a review is open");
    Ok(())
}

/// A track or a via: Delete with the registry's key, Select net and
/// Highlight net, the row saying what the toggle does now.
#[test]
fn a_track_or_via_offers_delete_and_its_net() -> TestResult {
    let (engine, net, track, via) = routed()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    // N8: Edit › Delete has something to delete only with a selection.
    assert!(!editor.can_delete(engine.project()));
    editor.via = Some(via);
    assert!(editor.can_delete(engine.project()));
    editor.via = None;
    let rows = editor.menu_rows(&ctx, &engine, Target::Track(track));
    assert_eq!(
        labels(&rows),
        ["Delete", "—", "Select net", "Highlight net"]
    );
    let delete = rows.iter().flatten().next().ok_or("Delete")?;
    assert_eq!(delete.key, crate::keys::menu_key(&ctx, "pcb.delete"));
    assert!(delete.key.as_ref().is_some_and(|k| !k.contains(", ")));
    assert_op!(
        editor.menu_act(&engine, Act::DeleteTrack(track)),
        Some(Operation::Layout(LayoutCommand::DeleteTrack { id: track }))
    );
    assert_op!(
        editor.menu_act(&engine, Act::DeleteVia(via)),
        Some(Operation::Layout(LayoutCommand::DeleteVia { id: via }))
    );
    // The toggle has no net to light until something selected one.
    assert_eq!(enabled(&rows, "Highlight net"), Some(false));
    assert!(editor.menu_act(&engine, Act::SelectNet(net)).is_none());
    assert_eq!(editor.highlighted_net(), Some(net));
    let rows = editor.menu_rows(&ctx, &engine, Target::Via(via));
    assert_eq!(enabled(&rows, "Clear net highlight"), Some(true));
    let highlight = rows
        .iter()
        .flatten()
        .find(|e| e.label == "Clear net highlight")
        .ok_or("the toggle")?;
    assert_eq!(highlight.run, Run::Command("pcb.highlight-net"));
    assert_eq!(highlight.key, crate::keys::shown(&ctx, "pcb.highlight-net"));
    Ok(())
}

/// A fill: Refill and Delete send what its card's footer sends; a locked
/// fill dims both.
#[test]
fn a_fill_offers_refill_and_delete() -> TestResult {
    let engine = Engine::showcase()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let id = engine.project().fills.first().ok_or("a fill")?.id;
    let rows = editor.menu_rows(&ctx, &engine, Target::Fill(id));
    assert_eq!(labels(&rows), ["Refill", "—", "Delete"]);
    assert_eq!(enabled(&rows, "Refill"), Some(true));
    assert_op!(
        editor.menu_act(&engine, Act::Refill(id)),
        Some(Operation::Fill(FillCommand::Refill {
            target: RefillTarget::One(id),
        }))
    );
    assert_op!(
        editor.menu_act(&engine, Act::DeleteFill(id)),
        Some(Operation::Fill(FillCommand::Delete { id }))
    );
    Ok(())
}

/// A right-click selects what a click there selects, and a row sends its
/// command; the empty board offers Route, Via and Fit view as registry
/// commands; no menu opens while a tool is armed.
#[test]
fn a_right_click_selects_and_a_row_runs_its_command() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = context();
    let mut editor = PcbEditor::default();
    frame(&mut editor, &engine, &ctx, vec![]);
    let placement = engine.project().placements[0].clone();
    let at = on_screen(&editor, &engine, placement.position);
    assert!(click(&mut editor, &engine, &ctx, at, PointerButton::Secondary).is_empty());
    assert_eq!(editor.selected_instance(), Some(placement.instance));
    assert_eq!(
        editor.menu.target(),
        Some(&Target::Part(placement.instance))
    );
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let lock = text_at(&output, "Lock")?;
    let sent = click(&mut editor, &engine, &ctx, lock, PointerButton::Primary);
    assert_op!(
        sent.first(),
        Some(&Operation::Layout(LayoutCommand::SetPlacementLock {
            instance: placement.instance,
            locked: true,
        }))
    );
    assert!(!editor.menu.is_open());

    let empty = pos2(880.0, 20.0);
    click(&mut editor, &engine, &ctx, empty, PointerButton::Secondary);
    assert_eq!(editor.menu.target(), Some(&Target::Board));
    assert_eq!(
        editor.selected_instance(),
        Some(placement.instance),
        "a right-click on nothing keeps the selection"
    );
    let rows = editor.menu_rows(&ctx, &engine, Target::Board);
    assert_eq!(labels(&rows), ["Route", "Via", "—", "Fit view"]);
    assert!(rows.iter().flatten().all(|e| e.enabled));
    let (output, _) = frame(&mut editor, &engine, &ctx, vec![]);
    let route = text_at(&output, "Route")?;
    click(&mut editor, &engine, &ctx, route, PointerButton::Primary);
    assert_eq!(editor.take_menu_command(), Some("pcb.route"));
    assert_eq!(editor.take_menu_command(), None, "once");

    // The Route tool keeps its right-click (it ends the route).
    assert!(editor.on_tool("route", &engine).is_none());
    click(&mut editor, &engine, &ctx, at, PointerButton::Secondary);
    assert!(!editor.menu.is_open());
    Ok(())
}
