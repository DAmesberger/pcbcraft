//! The Place palette's template rows: below the components, and choosing one
//! creates the component and arms placement. Driven through the engine.
use super::tests::{key, parts_engine, shown, workspace_frame};
use super::*;
use crate::template_gallery::TemplateGallery;
use egui::vec2;
use pcbcraft_core::Evaluators;
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn context() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

fn empty_engine() -> Engine {
    Engine::default()
}

/// The palette open with `text` typed; the texts it drew, top to bottom.
fn typed(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    text: &str,
) -> Vec<(String, Pos2)> {
    editor.on_tool("place", engine);
    workspace_frame(editor, engine, ctx, vec![]);
    let (output, _) = workspace_frame(editor, engine, ctx, vec![egui::Event::Text(text.into())]);
    let mut rows = shown(&output);
    rows.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));
    rows
}

fn apply_hosted(engine: &mut Engine, operation: Operation) -> TestResult {
    engine.apply_hosted(
        engine.revision(),
        operation,
        Evaluators {
            footprint: Some(&pcbcraft_solidcraft::footprint::SketchEvaluator),
            board: None,
        },
    )?;
    Ok(())
}

/// A search lists the project's components first, then a "From template"
/// group; a component search with no template match adds no group.
#[test]
fn templates_list_below_the_components() -> TestResult {
    let engine = parts_engine()?;
    let ctx = context();
    let mut editor = SchematicEditor::default();
    // Synthetic resistors are 0603 and so is a chip template row.
    let rows = typed(&mut editor, &engine, &ctx, "0603");
    let (_, components) = rows
        .iter()
        .find(|(t, _)| t.starts_with("IN THIS PROJECT"))
        .ok_or_else(|| format!("no component group in {rows:?}"))?;
    let (_, heading) = rows
        .iter()
        .find(|(t, _)| t.starts_with("FROM TEMPLATE"))
        .ok_or_else(|| format!("no template group in {rows:?}"))?;
    assert!(components.y < heading.y, "components first");
    assert!(
        rows.iter()
            .filter(|(t, _)| t.starts_with("EX-"))
            .all(|(_, p)| p.y < heading.y),
        "every component is above the group: {rows:?}"
    );
    assert!(
        rows.iter().any(|(t, p)| t == "Create" && p.y > heading.y),
        "template rows say Create: {rows:?}"
    );
    // The first row is a component; the highlight starts there.
    assert!(editor.palette.highlighted().is_some());

    let mut editor = SchematicEditor::default();
    let rows = typed(&mut editor, &engine, &ctx, "res>=10k");
    assert!(
        !rows.iter().any(|(t, _)| t.starts_with("FROM TEMPLATE")),
        "{rows:?}"
    );
    Ok(())
}

/// "usb" and "led", which found nothing in the tester's runs, find the new
/// templates; Place is offered in a project with no component yet.
#[test]
fn usb_and_led_find_the_templates_in_place() -> TestResult {
    let engine = empty_engine();
    let ctx = context();
    for (text, row) in [("usb", "USB-C receptacle"), ("led", "Chip LED 0603")] {
        let mut editor = SchematicEditor::default();
        let rows = typed(&mut editor, &engine, &ctx, text);
        assert!(
            rows.iter().any(|(t, _)| t.starts_with(row)),
            "{text} finds {row}: {rows:?}"
        );
        assert!(
            rows.iter().any(|(t, _)| t.starts_with("FROM TEMPLATE")),
            "{rows:?}"
        );
        assert!(!rows.iter().any(|(t, _)| t.starts_with("IN THIS PROJECT")));
    }
    let ribbon = SchematicEditor::default().ribbon(&engine);
    let place = ribbon
        .groups
        .iter()
        .flat_map(|g| &g.tools)
        .find(|t| t.id == "place")
        .ok_or("the Place tool")?;
    assert!(place.enabled);
    Ok(())
}

/// Enter on a ready row sends one command: a published component in one undo
/// step, placement armed on it, and a click places it.
#[test]
fn choosing_a_row_creates_the_component_and_arms_placement() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    typed(&mut editor, &engine, &ctx, "usb");
    assert!(editor.palette.highlighted().is_none(), "no component row");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    let [operation] = &operations[..] else {
        return Err(format!("one operation, got {operations:?}").into());
    };
    assert!(matches!(
        operation,
        Operation::Library(pcbcraft_core::LibraryCommand::PublishFromTemplate(_))
    ));
    assert!(!editor.palette.is_open());
    apply_hosted(&mut engine, operation.clone())?;

    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.tool, Tool::Place);
    let component = editor.component.ok_or("armed")?;
    let published = engine
        .project()
        .library
        .component(component)
        .ok_or("published")?;
    assert!(published.meta.name.starts_with("USB-C receptacle"));
    assert!(engine.drafts().items.is_empty(), "no draft left open");
    assert!(first_choices(engine.project(), component).is_some());

    // One undo step removes all of it.
    engine.undo()?;
    assert!(engine.project().library.components.is_empty());
    assert!(!engine.can_undo());
    Ok(())
}

/// VM check #4 (run 3 #10): on the placed USB-C receptacle no pin number's
/// box runs past its pin end, touches the end's mark, another number or a
/// pin name; stacked pads read in a short form; the long value wraps
/// within the body's width.
#[test]
fn usb_c_pin_numbers_sit_clear_and_the_value_wraps() -> TestResult {
    use pcbcraft_core::geometry::Bounds;
    use pcbcraft_core::schematic::{self, PIN_RING_MM};
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    typed(&mut editor, &engine, &ctx, "usb");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    apply_hosted(&mut engine, operations.into_iter().next().ok_or("one")?)?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    let component = editor.component.ok_or("armed")?;
    let (request, _) = first_choices(engine.project(), component).ok_or("a symbol")?;
    apply_hosted(
        &mut engine,
        Operation::Instance(InstanceCommand::Place(request)),
    )?;
    let project = engine.project();
    let occurrence = project.occurrences.first().ok_or("placed")?.id;
    let unit = schematic::placed_unit(project, occurrence)?;
    let m = TextMetric::default();
    let numbers: Vec<(&schematic::PlacedPin, Bounds)> = unit
        .pins
        .iter()
        .filter_map(|p| Some((p, p.number.as_ref()?.bounds(m))))
        .collect();
    assert_eq!(numbers.len(), 9, "every pin numbered");
    let names: Vec<Bounds> = unit
        .pins
        .iter()
        .filter_map(|p| Some(p.name.as_ref()?.bounds(m)))
        .collect();
    for (i, (pin, b)) in numbers.iter().enumerate() {
        let text = pin.number.as_ref().map_or("", |t| t.text.as_str());
        let ring = Bounds::point(pin.ring_centre(PIN_RING_MM)).inflated(PIN_RING_MM);
        assert!(!b.intersects(&ring), "{text}: on the pin end's mark");
        // Along the pin, the box stays between its end and the body.
        let o = pin.outward();
        let along = |p: [f64; 2]| p[0] * o[0] + p[1] * o[1];
        let (lo, hi) = (along(pin.body_end), along(pin.end));
        for corner in [b.min, b.max, [b.min[0], b.max[1]], [b.max[0], b.min[1]]] {
            let a = along(corner);
            assert!(a >= lo - 1e-9 && a <= hi + 1e-9, "{text}: past the pin end");
        }
        for (other, c) in &numbers[i + 1..] {
            let them = other.number.as_ref().map_or("", |t| t.text.as_str());
            assert!(!b.intersects(c), "{text} runs into {them}");
        }
        assert!(
            !names.iter().any(|n| b.intersects(n)),
            "{text} runs into a name"
        );
    }
    let shown: Vec<&str> = numbers
        .iter()
        .filter_map(|(p, _)| p.number.as_ref().map(|t| t.text.as_str()))
        .collect();
    for short in ["A4B9…", "A1B12…"] {
        assert!(shown.contains(&short), "{short} in {shown:?}");
    }
    let body = unit
        .shapes
        .iter()
        .map(|s| s.shape.bounds(s.width))
        .reduce(Bounds::union)
        .ok_or("a body")?;
    let value = unit
        .texts
        .iter()
        .find(|t| t.text.contains("USB4105"))
        .ok_or("the value")?;
    assert!(value.text.contains('\n'), "{:?}", value.text);
    let v = value.bounds(m);
    assert!(
        v.max[0] - v.min[0] <= body.max[0] - body.min[0] + 1e-9,
        "{v:?} within {body:?}"
    );
    Ok(())
}

/// Esc in the palette on a template row, and Esc once placement is armed,
/// leave the project as it was; the app undoes the create in the second case
/// (the revision it is valid at comes back).
#[test]
fn cancelling_changes_nothing() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let before = engine.project().clone();
    typed(&mut editor, &engine, &ctx, "led");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(operations.is_empty());
    assert!(!editor.palette.is_open());
    assert_eq!(editor.tool, Tool::Select);
    assert_eq!(*engine.project(), before);

    typed(&mut editor, &engine, &ctx, "led");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    let [operation] = &operations[..] else {
        return Err(format!("one operation, got {operations:?}").into());
    };
    apply_hosted(&mut engine, operation.clone())?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.tool, Tool::Place);
    workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert_eq!(editor.tool, Tool::Select);
    assert_eq!(editor.take_template_undo(), Some(engine.revision()));
    engine.undo()?;
    assert_eq!(*engine.project(), before);
    assert!(engine.drafts().items.is_empty());
    Ok(())
}

/// Once something else changed, Esc only leaves the tool: the component
/// stays with what was placed from it.
#[test]
fn esc_after_a_placement_keeps_the_component() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    typed(&mut editor, &engine, &ctx, "usb");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    apply_hosted(&mut engine, operations.into_iter().next().ok_or("one")?)?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    let component = editor.component.ok_or("armed")?;
    let (request, _) = first_choices(engine.project(), component).ok_or("a symbol")?;
    apply_hosted(
        &mut engine,
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: Some(editor.sheet),
            ..request
        })),
    )?;
    workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert_eq!(editor.tool, Tool::Select);
    assert_eq!(editor.take_template_undo(), None);
    Ok(())
}

/// A refused create reopens the palette with the reason and arms nothing.
#[test]
fn a_refusal_comes_back_to_the_palette() -> TestResult {
    let engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    typed(&mut editor, &engine, &ctx, "usb");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert_eq!(operations.len(), 1);
    refusal::hand_back(
        &ctx,
        refusal::Refused {
            origin: Origin::Dialog(egui::Id::unique(PLACE_TEMPLATE)),
            error: pcbcraft_core::Error::Invalid("the project is full of drafts".into()),
        },
        false,
    );
    // Handed back inside a frame by the app: the frame after claims it.
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    let (output, _) = workspace_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(editor.palette.is_open());
    let rows = shown(&output);
    assert!(
        rows.iter().any(|(t, _)| t.contains("full of drafts")),
        "the reason shows: {rows:?}"
    );
    assert_eq!(editor.component, None);
    assert_eq!(editor.template_armed, None);
    Ok(())
}

/// A family's custom entry has values to enter: choosing it asks for the
/// same form, which sends nothing until its primary. Esc there changes
/// nothing; its primary publishes and hands the component over to arm.
#[test]
fn a_row_with_choices_takes_the_same_form_first() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let rows = typed(&mut editor, &engine, &ctx, "soic custom");
    assert!(
        rows.iter().any(|(t, _)| t.starts_with("FROM TEMPLATE")),
        "{rows:?}"
    );
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert!(
        operations.is_empty(),
        "the form comes first: {operations:?}"
    );
    let (hit, query) = editor.take_template_form().ok_or("the form")?;
    assert!(hit.is_custom());

    let before = engine.project().clone();
    let run = |gallery: &mut TemplateGallery, engine: &Engine, events| {
        let mut sent = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))),
                events,
                ..Default::default()
            },
            |ui| sent = gallery.show(ui.ctx(), engine),
        );
        output.textures_delta.clear();
        sent
    };
    let mut gallery = TemplateGallery::default();
    gallery.open_on(&hit, query.clone(), true, None);
    assert!(run(&mut gallery, &engine, vec![]).is_none());
    let esc = vec![key(egui::Key::Escape, egui::Modifiers::NONE)];
    assert!(run(&mut gallery, &engine, esc).is_none());
    assert!(!gallery.is_open());
    assert_eq!(*engine.project(), before);

    gallery.open_on(&hit, query, true, None);
    assert!(run(&mut gallery, &engine, vec![]).is_none());
    let enter = vec![key(egui::Key::Enter, egui::Modifiers::NONE)];
    let sent = run(&mut gallery, &engine, enter).ok_or("the primary sends")?;
    assert!(matches!(
        sent,
        Operation::Library(pcbcraft_core::LibraryCommand::PublishFromTemplate(_))
    ));
    apply_hosted(&mut engine, sent)?;
    assert!(run(&mut gallery, &engine, vec![]).is_none());
    let component = gallery.take_to_place().ok_or("handed over")?;

    editor.arm_created(&ctx, &engine, component);
    assert_eq!(editor.tool, Tool::Place);
    assert_eq!(editor.component, Some(component));
    Ok(())
}

/// The founder's bar for this list: both groups in one neat sheet, and a
/// cut row shows its full text on hover.
#[test]
fn the_palette_rows_show_in_full_on_hover() -> TestResult {
    let engine = parts_engine()?;
    let mut editor = SchematicEditor::default();
    let screen = crate::kit::text::tests::Screen::new(vec2(1000.0, 700.0));
    let run = |editor: &mut SchematicEditor, events: Vec<egui::Event>| {
        screen.run(events, |ui| {
            editor.canvas(ui, &engine);
        })
    };
    editor.on_tool("place", &engine);
    run(&mut editor, vec![]);
    run(&mut editor, vec![egui::Event::Text("0603".into())]);
    let drawn = run(&mut editor, vec![]);
    screen.picture("place-palette-templates", &drawn, 1.0)?;

    let mut usb = SchematicEditor::default();
    usb.on_tool("place", &engine);
    run(&mut usb, vec![]);
    run(&mut usb, vec![egui::Event::Text("usb".into())]);
    let drawn = run(&mut usb, vec![]);
    screen.picture("place-palette-usb", &drawn, 1.0)?;
    let (_, missing) =
        crate::kit::text::tests::cuts_without_hover(&screen.ctx, |events| run(&mut usb, events));
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
    Ok(())
}

/// "header 1x4" is the part itself: "not yet created", read as written, the
/// default marked, listed once. Enter creates it in one undo step and arms it.
#[test]
fn a_generic_part_creates_without_a_form_and_arms_placement() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let rows = typed(&mut editor, &engine, &ctx, "header 1x4");
    let at = |label: &str| rows.iter().find(|(t, _)| t == label).map(|(_, p)| p.y);
    for label in ["not yet created", "rows × columns", "2.54 mm (default)"] {
        assert!(at(label).is_some(), "{label}: {rows:?}");
    }
    let same = rows
        .iter()
        .filter(|(t, p)| t == "Pin header 1×4 2.54 mm" && p.x < 800.0);
    assert_eq!(
        same.count(),
        1,
        "one row, not also a template row: {rows:?}"
    );
    let (_, row) = rows
        .iter()
        .find(|(t, p)| t == "Pin header 1×4 2.54 mm" && p.x < 800.0)
        .ok_or("the row")?;
    if let Some((_, heading)) = rows.iter().find(|(t, _)| t.starts_with("FROM TEMPLATE")) {
        assert!(row.y < heading.y, "the part above the template rows");
    }

    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    let [operation] = &operations[..] else {
        return Err(format!("one operation, got {operations:?}").into());
    };
    assert!(matches!(
        operation,
        Operation::Library(pcbcraft_core::LibraryCommand::PublishFromTemplate(_))
    ));
    apply_hosted(&mut engine, operation.clone())?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.tool, Tool::Place);
    let component = editor.component.ok_or("armed")?;
    let published = engine
        .project()
        .library
        .component(component)
        .ok_or("published")?;
    assert_eq!(published.meta.name, "Pin header 1×4 2.54 mm");
    assert!(published.definition.generic.is_some());
    assert!(engine.drafts().items.is_empty());

    engine.undo()?;
    assert!(engine.project().library.components.is_empty());
    assert!(!engine.can_undo());
    Ok(())
}

/// The same text again finds the part: no second component, nothing sent.
#[test]
fn a_part_the_project_holds_is_what_the_text_finds() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    typed(&mut editor, &engine, &ctx, "resistor 0603");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    apply_hosted(&mut engine, operations.into_iter().next().ok_or("one")?)?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    let component = editor.component.ok_or("armed")?;

    let mut editor = SchematicEditor::default();
    let rows = typed(&mut editor, &engine, &ctx, "resistor 0603");
    assert!(
        !rows.iter().any(|(t, _)| t == "not yet created"),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(|(t, _)| t.starts_with("IN THIS PROJECT")),
        "{rows:?}"
    );
    let named = "Chip resistor 0603 (1608 metric)";
    let listed = rows.iter().filter(|(t, p)| t == named && p.x < 800.0);
    assert_eq!(listed.count(), 1, "once, not also by name: {rows:?}");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert!(operations.is_empty(), "{operations:?}");
    assert_eq!(editor.tool, Tool::Place);
    assert_eq!(editor.component, Some(component));
    // Esc keeps a part that was already there.
    workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert_eq!(editor.take_template_undo(), None);
    assert_eq!(engine.project().library.latest_components().len(), 1);
    Ok(())
}

/// Esc in the palette and Esc once armed leave the project as it was.
#[test]
fn cancelling_a_generic_part_changes_nothing() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let before = engine.project().clone();
    typed(&mut editor, &engine, &ctx, "led 0603");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert!(operations.is_empty());
    assert_eq!(*engine.project(), before);

    typed(&mut editor, &engine, &ctx, "led 0603");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    apply_hosted(&mut engine, operations.into_iter().next().ok_or("one")?)?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(editor.tool, Tool::Place);
    workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
    );
    assert_eq!(editor.tool, Tool::Select);
    assert_eq!(editor.take_template_undo(), Some(engine.revision()));
    engine.undo()?;
    assert_eq!(*engine.project(), before);
    Ok(())
}

/// A value the text named reaches each instance placed: the LED's colour
/// field exists today. A passive's value takes the same path once its
/// template selects the field; until then it is left out.
#[test]
fn the_named_colour_reaches_the_placed_instance() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let rows = typed(&mut editor, &engine, &ctx, "led 0603 red");
    assert!(rows.iter().any(|(t, _)| t == "Red"), "{rows:?}");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    apply_hosted(&mut engine, operations.into_iter().next().ok_or("one")?)?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    let at = pos2(120.0, 680.0);
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut placed = Vec::new();
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(true)],
        vec![button(false)],
    ] {
        placed.extend(workspace_frame(&mut editor, &engine, &ctx, events).1);
    }
    let [Operation::Instance(InstanceCommand::Place(request))] = &placed[..] else {
        return Err(format!("one component.place, got {placed:?}").into());
    };
    assert_eq!(request.properties.len(), 1, "{request:?}");
    apply_hosted(&mut engine, placed[0].clone())?;
    let instance = engine.project().instances.first().ok_or("placed")?;
    assert!(
        instance
            .properties
            .contains_key(&request.properties[0].field),
        "{instance:?}"
    );
    Ok(())
}

/// VM check #5: a generic part from search, created and then placed, is one
/// undo step. The first placement joins the create (as the app does with
/// `take_template_join`); a second placement is a step of its own.
#[test]
fn creating_and_placing_a_generic_part_is_one_undo_step() -> TestResult {
    let mut engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let before = engine.project().clone();
    typed(&mut editor, &engine, &ctx, "10k 0603");
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    apply_hosted(&mut engine, operations.into_iter().next().ok_or("one")?)?;
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    workspace_frame(&mut editor, &engine, &ctx, vec![]);
    let click = |editor: &mut SchematicEditor, engine: &Engine, at| {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut sent = Vec::new();
        for events in [
            vec![egui::Event::PointerMoved(at)],
            vec![button(true)],
            vec![button(false)],
        ] {
            sent.extend(workspace_frame(editor, engine, &ctx, events).1);
        }
        sent
    };
    let place = |editor: &mut SchematicEditor, engine: &mut Engine, at| -> TestResult {
        let sent = click(editor, engine, at);
        let [operation @ Operation::Instance(InstanceCommand::Place(_))] = &sent[..] else {
            return Err(format!("one component.place, got {sent:?}").into());
        };
        apply_hosted(engine, operation.clone())?;
        if let Some(revision) = editor.take_template_join() {
            engine.join_last_step(revision)?;
        }
        Ok(())
    };
    place(&mut editor, &mut engine, pos2(120.0, 680.0))?;
    place(&mut editor, &mut engine, pos2(320.0, 680.0))?;
    assert_eq!(engine.project().instances.len(), 2);
    engine.undo()?;
    assert_eq!(engine.project().instances.len(), 1, "the second on its own");
    assert_eq!(engine.project().library.latest_components().len(), 1);
    engine.undo()?;
    assert!(engine.project().instances.is_empty());
    assert!(
        engine.project().library.components.is_empty(),
        "the create went with the first placement"
    );
    assert_eq!(*engine.project(), before);
    assert!(!engine.can_undo());
    Ok(())
}

/// A part the family refuses lists why; Enter says so and sends nothing.
#[test]
fn a_refused_part_says_why_and_sends_nothing() -> TestResult {
    let engine = empty_engine();
    let ctx = context();
    let mut editor = SchematicEditor::default();
    let rows = typed(&mut editor, &engine, &ctx, "header 1x9999");
    assert!(rows.iter().any(|(t, _)| t == "cannot be made"), "{rows:?}");
    assert!(
        rows.iter().any(|(t, _)| t.contains("at most 512 pins")),
        "{rows:?}"
    );
    let (_, operations) = workspace_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![key(egui::Key::Enter, egui::Modifiers::NONE)],
    );
    assert!(operations.is_empty());
    assert!(editor.palette.is_open());
    assert_eq!(editor.component, None);
    Ok(())
}

/// The badges fit the row, with every full text and meaning on hover.
#[test]
fn generic_rows_show_their_badges_in_full_on_hover() -> TestResult {
    let engine = parts_engine()?;
    let mut editor = SchematicEditor::default();
    let screen = crate::kit::text::tests::Screen::new(vec2(1000.0, 700.0));
    let run = |editor: &mut SchematicEditor, events: Vec<egui::Event>| {
        screen.run(events, |ui| {
            editor.canvas(ui, &engine);
        })
    };
    editor.on_tool("place", &engine);
    run(&mut editor, vec![]);
    run(&mut editor, vec![egui::Event::Text("100n 0603 16V".into())]);
    let drawn = run(&mut editor, vec![]);
    screen.picture("place-palette-generic-values", &drawn, 1.0)?;
    let (_, missing) =
        crate::kit::text::tests::cuts_without_hover(&screen.ctx, |events| run(&mut editor, events));
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");

    let mut header = SchematicEditor::default();
    header.on_tool("place", &engine);
    run(&mut header, vec![]);
    run(&mut header, vec![egui::Event::Text("header 5x2".into())]);
    let drawn = run(&mut header, vec![]);
    screen.picture("place-palette-generic-header", &drawn, 1.0)?;
    let (_, missing) =
        crate::kit::text::tests::cuts_without_hover(&screen.ctx, |events| run(&mut header, events));
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
    Ok(())
}
