//! The Fill and Route tools' Net pickers name a net nobody named by the
//! pins it joins ("N2 · J1 CC1, R3 2"), a named net by its name alone, and
//! a choice too long for the menu ends in "…" and shows whole on hover
//! (run 4, finding 13).
use super::*;
use crate::kit::text::tests::{cuts, painted, shows_in_full, tooltips_at_once};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// One frame of the subject slot `width` wide, a quarter second after the
/// last (egui shows no tooltip until the pointer moved after a click).
fn frame(
    editor: &mut PcbEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
    width: f32,
) -> egui::FullOutput {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, 900.0))),
            events,
            time: Some(ctx.input(|i| i.time) + 0.25),
            ..Default::default()
        },
        |ui| {
            editor.subject_slot(ui, engine);
        },
    );
    output.textures_delta.clear();
    output
}

fn texts(output: &egui::FullOutput) -> Vec<String> {
    painted(output).into_iter().map(|(t, ..)| t).collect()
}

/// Arm `tool`, open its Net picker (showing `none`) in a slot `width`
/// wide and return the menu's frame with the editor and context to hover
/// it.
fn open_picker(
    engine: Engine,
    tool: &str,
    none: &str,
    width: f32,
) -> Result<(PcbEditor, Engine, egui::Context, egui::FullOutput), Box<dyn std::error::Error>> {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    let mut editor = PcbEditor::default();
    editor.on_tool(tool, &engine);
    assert!(editor.subject() == Subject::Tool);
    frame(&mut editor, &engine, &ctx, vec![], width);
    let shown = frame(&mut editor, &engine, &ctx, vec![], width);
    let at = painted(&shown)
        .into_iter()
        .find(|(t, ..)| t == none)
        .map(|(_, rect, ..)| rect.center())
        .ok_or_else(|| format!("no {none} in {:?}", texts(&shown)))?;
    frame(&mut editor, &engine, &ctx, via_tests::click(at), width);
    frame(&mut editor, &engine, &ctx, vec![], width);
    let menu = frame(&mut editor, &engine, &ctx, vec![], width);
    Ok((editor, engine, ctx, menu))
}

fn lists_nets_with_their_pins(tool: &str, none: &str) -> TestResult {
    let card = Tokens::get().form_card_width;
    // A named net is listed by its name alone (the showcase names its
    // nets).
    let (_, _, _, menu) = open_picker(Engine::showcase()?, tool, none, card)?;
    let shown = texts(&menu);
    assert!(shown.iter().any(|t| t == "GND"), "{shown:?}");
    // Two connected resistors: their net has no name of its own. A
    // narrow slot cuts its choice.
    let narrow = 150.0;
    let (mut editor, engine, ctx, menu) = open_picker(Engine::demo()?, tool, none, narrow)?;
    let project = engine.project();
    let shown = texts(&menu);
    let auto = project
        .nets
        .iter()
        .find(|n| sparksmith_core::nets::auto_named(n) && n.terminals.len() >= 2)
        .ok_or("the demo has an unnamed net")?;
    let described = sparksmith_core::nets::described(project, auto);
    assert!(
        described.starts_with(&format!("{} · ", auto.name)),
        "{described}"
    );
    let designator = project
        .instance(auto.terminals[0].instance)
        .map(|i| i.designator.clone())
        .ok_or("instance")?;
    assert!(described.contains(&designator), "{described}");
    assert!(shown.contains(&described), "{described} not in {shown:?}");
    assert!(
        !shown.contains(&auto.name),
        "{} listed bare: {shown:?}",
        auto.name
    );
    // The choice, cut short, shows whole on hover, the menu staying open;
    // chosen, it shows cut in the closed picker and whole on hover there.
    tooltips_at_once(&ctx);
    let mut run = |events| frame(&mut editor, &engine, &ctx, events, narrow);
    run(vec![egui::Event::PointerGone]);
    let rest = run(vec![]);
    let choice = cuts(&rest)
        .into_iter()
        .find(|c| c.text == described)
        .ok_or("the narrow menu cuts the choice")?;
    run(vec![egui::Event::PointerMoved(choice.at)]);
    run(vec![]);
    assert!(shows_in_full(&run(vec![]), &described), "{described}");
    run(via_tests::click(choice.at));
    run(vec![egui::Event::PointerGone]);
    let rest = run(vec![]);
    let shown = cuts(&rest)
        .into_iter()
        .find(|c| c.text == described)
        .ok_or("the closed picker shows the choice cut")?;
    run(vec![egui::Event::PointerMoved(shown.at)]);
    run(vec![]);
    assert!(shows_in_full(&run(vec![]), &described), "{described}");
    Ok(())
}

#[test]
fn the_fill_tool_lists_unnamed_nets_with_their_pins() -> TestResult {
    lists_nets_with_their_pins("fill", "No net")
}

#[test]
fn the_route_card_lists_unnamed_nets_with_their_pins() -> TestResult {
    lists_nets_with_their_pins("route", "From the first pad")
}

/// The pin list of a net is sorted, names a pin by its terminal name and
/// stops after six pins with how many more there are.
#[test]
fn a_described_net_names_its_pins_by_designator_and_pin() -> TestResult {
    for engine in [Engine::showcase()?, Engine::demo()?] {
        let project = engine.project();
        for net in &project.nets {
            let described = sparksmith_core::nets::described(project, net);
            if !sparksmith_core::nets::auto_named(net) {
                assert_eq!(described, net.name);
                continue;
            }
            let pins = described
                .split_once(" · ")
                .map(|(_, pins)| pins)
                .ok_or_else(|| format!("no pins: {described}"))?;
            let listed: Vec<&str> = pins.split(", ").filter(|p| !p.starts_with('+')).collect();
            assert_eq!(listed.len(), net.terminals.len().min(6), "{described}");
            let mut sorted = listed.clone();
            sorted.sort_unstable();
            assert_eq!(listed, sorted, "{described}");
            if net.terminals.len() > 6 {
                assert!(
                    described.ends_with(&format!("+{} more", net.terminals.len() - 6)),
                    "{described}"
                );
            }
        }
    }
    Ok(())
}
