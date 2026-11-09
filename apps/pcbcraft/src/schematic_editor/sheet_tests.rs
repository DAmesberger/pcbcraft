//! Schematic tests of sheets, labels, power ports and sheet symbols: the
//! tools issue one typed command per click, the canvas shows only the
//! active sheet, the registry arms the tools, and a label card jumps to
//! the net's places on other sheets.
use super::*;
use egui::vec2;
use pcbcraft_core::{LabelKind, Outcome, PowerStyle, SheetId};
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const CANVAS: Rect = Rect::from_min_max(Pos2::ZERO, pos2(800.0, 800.0));

fn canvas_frame(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (Vec<egui::Shape>, Option<Operation>) {
    let mut operation = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(CANVAS),
            events,
            ..Default::default()
        },
        |ui| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(CANVAS), |ui| {
                operation = editor.canvas(ui, engine);
            });
        },
    );
    output.textures_delta.clear();
    let mut shapes = Vec::new();
    fn flatten(shape: egui::Shape, out: &mut Vec<egui::Shape>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.into_iter().for_each(|s| flatten(s, out)),
            other => out.push(other),
        }
    }
    for clipped in output.shapes {
        flatten(clipped.shape, &mut shapes);
    }
    (shapes, operation)
}

fn texts(shapes: &[egui::Shape]) -> Vec<String> {
    shapes
        .iter()
        .filter_map(|s| match s {
            egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
}

/// Press and release at `p`; the operation the canvas returns.
fn click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    p: Pos2,
) -> Option<Operation> {
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut out = None;
    for events in [
        vec![egui::Event::PointerMoved(p)],
        vec![button(true)],
        vec![button(false)],
    ] {
        if let (_, Some(op)) = canvas_frame(editor, engine, ctx, events) {
            out = Some(op);
        }
    }
    out
}

fn ep(instance: Id, terminal: u32) -> Endpoint {
    Endpoint {
        instance,
        terminal: pcbcraft_core::TerminalId(terminal),
    }
}

/// An editor whose view centres `p` at `scale` pixels per millimetre.
fn centred(p: Point, scale: f32) -> SchematicEditor {
    SchematicEditor {
        scale,
        pan: vec2(-(p[0] as f32) * scale, p[1] as f32 * scale),
        ..Default::default()
    }
}

fn ctx() -> egui::Context {
    let ctx = egui::Context::default();
    fonts::setup_bundled_fonts(&ctx);
    ctx
}

/// Place one more resistor like the demo's first, on `sheet`.
fn place(
    engine: &mut Engine,
    name: &str,
    at: Point,
    sheet: SheetId,
) -> Result<Id, Box<dyn std::error::Error>> {
    let first = engine.project().instances[0].clone();
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: Some(sheet),
            component: first.component,
            designator: Some(name.into()),
            properties: vec![],
            representation: first.representation,
            unit: pcbcraft_core::UnitId(1),
            position: at,
            physical: None,
        })),
    )?;
    Ok(engine
        .project()
        .instance_by_designator(name)
        .map(|i| i.id)
        .ok_or("placed")?)
}

fn add_sheet(engine: &mut Engine) -> Result<SheetId, Box<dyn std::error::Error>> {
    match engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::AddSheet {
            name: None,
            paper: None,
            orientation: None,
        }),
    )? {
        Outcome::SheetAdded { sheet } => Ok(sheet),
        other => Err(format!("{other:?}").into()),
    }
}

#[test]
fn the_label_tool_attaches_a_label_to_a_wire_in_one_step() -> TestResult {
    let mut engine = Engine::demo()?;
    let a = place(&mut engine, "R90", [0.0, -400.0], SheetId::FIRST)?;
    let b = place(&mut engine, "R91", [40.0, -400.0], SheetId::FIRST)?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::Connect {
            from: ep(a, 2).into(),
            to: ep(b, 1).into(),
        }),
    )?;
    let wire = engine.project().wires_at(ep(a, 2)).next().ok_or("wire")?.id;
    let (from, to) =
        schematic::wire_ends(engine.project(), engine.project().wire(wire).ok_or("w")?)
            .ok_or("ends")?;
    let mid = schematic::nearest_on_wire(
        engine.project(),
        wire,
        [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0],
    )
    .ok_or("mid")?;
    let ctx = ctx();
    let mut editor = centred(mid, 12.0);
    editor.on_tool("label", &engine);
    editor.drafts.label = "SDA".into();
    let op = click(&mut editor, &engine, &ctx, CANVAS.center()).ok_or("no label placed")?;
    let Operation::Schematic(SchematicCommand::PlaceLabel {
        name,
        kind,
        attach,
        sheet,
        ..
    }) = &op
    else {
        return Err(format!("{op:?}").into());
    };
    assert_eq!(name, "SDA");
    assert_eq!(*kind, LabelKind::Local);
    assert_eq!(*sheet, Some(SheetId::FIRST));
    assert!(
        matches!(attach, Some(WireTarget::OnWire(p)) if p.wire == wire),
        "{attach:?}"
    );
    let before = engine.project().clone();
    let revision = engine.revision();
    engine.apply(engine.revision(), op)?;
    assert_eq!(engine.revision(), revision + 1);
    let net = engine.project().net_of(ep(a, 2)).ok_or("net")?;
    assert_eq!(net.name, "SDA");
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

#[test]
fn the_power_tool_places_a_port_in_the_chosen_style() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = ctx();
    let mut editor = centred([200.0, -300.0], 12.0);
    editor.on_tool("power", &engine);
    assert_eq!(
        editor.subject(engine.project()),
        Subject::LabelTool(LabelTool::Power)
    );
    editor.drafts.power = "+3V3".into();
    editor.drafts.power_style = PowerStyle::Bar;
    let op = click(&mut editor, &engine, &ctx, CANVAS.center()).ok_or("no port")?;
    let Operation::Schematic(SchematicCommand::PlaceLabel {
        name, kind, attach, ..
    }) = &op
    else {
        return Err(format!("{op:?}").into());
    };
    assert_eq!(name, "+3V3");
    assert_eq!(
        *kind,
        LabelKind::Power {
            style: PowerStyle::Bar
        }
    );
    assert!(attach.is_none());
    engine.apply(engine.revision(), op)?;
    let label = engine.project().labels.first().ok_or("label")?;
    assert_eq!(
        label.kind,
        LabelKind::Power {
            style: PowerStyle::Bar
        }
    );
    // The canvas draws the port's name.
    let (shapes, _) = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert!(texts(&shapes).iter().any(|t| t == "+3V3"));
    Ok(())
}

#[test]
fn the_sheet_symbol_tool_creates_a_child_sheet() -> TestResult {
    let mut engine = Engine::demo()?;
    let ctx = ctx();
    let mut editor = centred([150.0, -150.0], 6.0);
    editor.on_tool("sheet-symbol", &engine);
    editor.drafts.sheet_name = "Power".into();
    let op = click(&mut editor, &engine, &ctx, CANVAS.center()).ok_or("no symbol")?;
    let Outcome::SheetSymbolPlaced { symbol, child } = engine.apply(engine.revision(), op)? else {
        return Err("not placed".into());
    };
    let project = engine.project();
    assert_eq!(project.sheets.list.len(), 2);
    assert_eq!(project.sheet(child).map(|s| s.name.as_str()), Some("Power"));
    assert_eq!(
        project.sheet_symbol(symbol).map(|s| s.sheet),
        Some(SheetId::FIRST)
    );
    // Double-clicking the symbol opens its sheet (a fresh context, so the
    // placing click does not make it a triple click).
    let ctx = self::ctx();
    let corner = project.sheet_symbol(symbol).ok_or("symbol")?.position;
    editor.on_tool("select", &engine);
    let inside = schematic_view(&editor).screen([corner[0] + 10.0, corner[1] - 10.0]);
    let button = |pressed| egui::Event::PointerButton {
        pos: inside,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    for events in [
        vec![egui::Event::PointerMoved(inside)],
        vec![button(true)],
        vec![button(false)],
        vec![button(true)],
        vec![button(false)],
    ] {
        canvas_frame(&mut editor, &engine, &ctx, events);
    }
    assert_eq!(editor.active_sheet(), child);
    Ok(())
}

fn schematic_view(editor: &SchematicEditor) -> symbol_view::View {
    symbol_view::View {
        origin: CANVAS.center() + editor.pan,
        scale: editor.scale,
    }
}

/// Run 3, finding 23: Fit view keeps the page, title block included,
/// above the floating navigation bar, so the Fit button never covers it.
#[test]
fn fit_keeps_the_page_clear_of_the_navigation_bar() -> TestResult {
    let engine = Engine::demo()?;
    let ctx = ctx();
    let mut editor = SchematicEditor::default();
    editor.fit();
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    let page = editor
        .sheet_extent(engine.project())
        .ok_or("no sheet extent")?;
    let view = schematic_view(&editor);
    let foot = view.screen(page.min).y;
    let head = view.screen(page.max).y;
    let bar = CANVAS.bottom() - crate::navigation::bar_band();
    assert!(foot <= bar + 0.5, "page foot {foot} under the bar at {bar}");
    assert!(head >= CANVAS.top() - 0.5, "page head {head} cut");
    Ok(())
}

#[test]
fn a_shown_sheet_draws_only_its_own_symbols() -> TestResult {
    let mut engine = Engine::demo()?;
    let second = add_sheet(&mut engine)?;
    place(&mut engine, "R99", [60.0, -60.0], second)?;
    let ctx = ctx();
    let mut editor = SchematicEditor::default();
    editor.fit();
    let shown = |editor: &mut SchematicEditor| -> Vec<String> {
        canvas_frame(editor, &engine, &ctx, vec![]);
        texts(&canvas_frame(editor, &engine, &ctx, vec![]).0)
    };
    let first = shown(&mut editor);
    assert!(first.iter().any(|t| t == "R1"), "{first:?}");
    assert!(!first.iter().any(|t| t == "R99"));
    // What the Browser's Sheets row does.
    editor.show_sheet(second, engine.project());
    let other = shown(&mut editor);
    assert!(other.iter().any(|t| t == "R99"), "{other:?}");
    assert!(!other.iter().any(|t| t == "R1"));
    // The title block names the page.
    assert!(other.iter().any(|t| t == "2 of 2"), "{other:?}");
    Ok(())
}

#[test]
fn the_registry_arms_the_label_tools() -> TestResult {
    use crate::commands::{Action, Chord, Registry};
    use crate::shell::{RibbonEvent, Workspace};
    let registry = Registry::standard();
    for (tool, chord) in [
        ("label", Chord::key(egui::Key::L)),
        ("global-label", Chord::key(egui::Key::L).with_shift()),
        ("hierarchical-label", Chord::key(egui::Key::H)),
        ("power", Chord::key(egui::Key::P).with_shift()),
        ("sheet-symbol", Chord::key(egui::Key::S).with_shift()),
        ("note", Chord::key(egui::Key::T)),
    ] {
        let command = registry.for_tool(Workspace::Schematic, tool).ok_or(tool)?;
        assert_eq!(registry.chord(command), Some(chord), "{tool}");
        let Action::Ribbon(Workspace::Schematic, RibbonEvent::Tool(id)) = command.action else {
            return Err(tool.into());
        };
        // The dispatcher sends the ribbon event: the editor arms the tool,
        // and its ribbon offers it.
        let engine = Engine::demo()?;
        let mut editor = SchematicEditor::default();
        editor.on_tool(id, &engine);
        assert_ne!(editor.tool, Tool::Select, "{tool}");
        let ribbon = editor.ribbon(&engine);
        assert!(
            ribbon
                .groups
                .iter()
                .flat_map(|g| g
                    .tools
                    .iter()
                    .chain(g.tools.iter().flat_map(|t| t.menu.iter())))
                .any(|t| t.id == id && t.enabled),
            "{tool}"
        );
    }
    let engine = Engine::demo()?;
    let mut editor = SchematicEditor::default();
    editor.on_tool("label", &engine);
    assert_eq!(editor.tool, Tool::Label(LabelTool::Local));
    Ok(())
}

/// Label ▾ is a split button: its ▾ part lists Net, Global and
/// Hierarchical label, and its upper part arms the kind used last.
#[test]
fn label_arms_the_last_label_kind_and_its_menu_lists_every_kind() -> TestResult {
    let engine = Engine::demo()?;
    let mut editor = SchematicEditor::default();
    let label = |editor: &mut SchematicEditor| -> Result<(String, Vec<String>), String> {
        let ribbon = editor.ribbon(&engine);
        let tool = ribbon
            .groups
            .iter()
            .flat_map(|g| g.tools.iter())
            .find(|t| t.label == "Label")
            .ok_or("no Label button")?;
        Ok((
            tool.id.to_owned(),
            tool.menu.iter().map(|v| v.label.to_owned()).collect(),
        ))
    };
    let (shown, menu) = label(&mut editor)?;
    assert_eq!(shown, "label", "a net label at first");
    assert_eq!(menu, ["Net label", "Global label", "Hierarchical label"]);
    editor.on_tool("global-label", &engine);
    editor.on_tool("select", &engine);
    assert_eq!(label(&mut editor)?.0, "global-label");
    // The upper part runs the shown id: the global label tool again.
    let shown = label(&mut editor)?.0;
    editor.on_tool(&shown, &engine);
    assert_eq!(editor.tool, Tool::Label(LabelTool::Global));
    // A power port is no label kind of this button.
    editor.on_tool("power", &engine);
    assert_eq!(label(&mut editor)?.0, "global-label");
    Ok(())
}

/// Render the Properties card and click the text `label` in it.
fn card_click(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    label: &str,
) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
    let frame = |editor: &mut SchematicEditor, events| {
        let mut command = None;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(700.0, 1400.0))),
                events,
                ..Default::default()
            },
            |ui| command = kit::form(ui, |ui| editor.inspector(ui, engine)),
        );
        output.textures_delta.clear();
        (output, command)
    };
    fn find(shape: &egui::Shape, label: &str) -> Option<Pos2> {
        match shape {
            egui::Shape::Text(t) if t.galley.job.text == label => {
                Some(t.pos + t.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|s| find(s, label)),
            _ => None,
        }
    }
    let (output, _) = frame(editor, vec![]);
    let at = output
        .shapes
        .iter()
        .find_map(|s| find(&s.shape, label))
        .ok_or_else(|| format!("no {label}"))?;
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let mut out = None;
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(true)],
        vec![button(false)],
    ] {
        if let (_, Some(op)) = frame(editor, events) {
            out = Some(op);
        }
    }
    Ok(out)
}

#[test]
fn next_in_the_label_card_shows_the_other_sheet() -> TestResult {
    let mut engine = Engine::demo()?;
    let second = add_sheet(&mut engine)?;
    let r1 = engine.project().instances[0].id;
    let r99 = place(&mut engine, "R99", [60.0, -60.0], second)?;
    let mut labels = Vec::new();
    for (sheet, endpoint) in [(SheetId::FIRST, ep(r1, 1)), (second, ep(r99, 1))] {
        let outcome = engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::PlaceLabel {
                sheet: Some(sheet),
                name: "SIG".into(),
                kind: LabelKind::Global {
                    shape: pcbcraft_core::PortShape::Passive,
                },
                position: [0.0, 20.0],
                rotation: Rotation::R0,
                attach: Some(endpoint.into()),
            }),
        )?;
        let Outcome::LabelPlaced { label } = outcome else {
            return Err("label".into());
        };
        labels.push(label);
    }
    // One net across both sheets.
    assert_eq!(
        engine.project().net_of(ep(r1, 1)).map(|n| n.id),
        engine.project().net_of(ep(r99, 1)).map(|n| n.id)
    );
    let ctx = ctx();
    let mut editor = SchematicEditor::default();
    editor.refresh_connectivity(&engine);
    editor.select_item(Some(Item::Label(labels[0])), engine.project());
    card_click(&mut editor, &engine, &ctx, "Next")?;
    assert_eq!(editor.active_sheet(), second);
    assert_eq!(editor.item, Some(Item::Label(labels[1])));
    // Issues locate a label on another sheet the same way.
    editor.show_sheet(SheetId::FIRST, engine.project());
    assert!(crate::issues::Locate::locate(
        &mut editor,
        &engine,
        &[ObjectRef::Label { id: labels[1] }]
    ));
    assert_eq!(editor.active_sheet(), second);
    Ok(())
}

#[test]
fn delete_removes_the_selected_label_in_one_step() -> TestResult {
    let mut engine = Engine::demo()?;
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: "GND".into(),
            kind: LabelKind::Power {
                style: PowerStyle::Ground,
            },
            position: [100.0, -100.0],
            rotation: Rotation::R0,
            attach: None,
        }),
    )?;
    let label = engine.project().labels[0].id;
    let ctx = ctx();
    let mut editor = centred([100.0, -100.0], 12.0);
    editor.refresh_connectivity(&engine);
    editor.select_item(Some(Item::Label(label)), engine.project());
    let (_, op) = canvas_frame(
        &mut editor,
        &engine,
        &ctx,
        vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    let op = op.ok_or("no delete")?;
    assert!(matches!(
        op,
        Operation::Schematic(SchematicCommand::RemoveLabel { label: l }) if l == label
    ));
    engine.apply(engine.revision(), op)?;
    assert!(engine.project().labels.is_empty());
    Ok(())
}

/// The ESP32 dev board example, opened over the demo the app starts with
/// as the app opens a file.
fn esp32_board() -> Result<Engine, Box<dyn std::error::Error>> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/esp32-s31-devboard/esp32-s31-devboard.pcbcraft"
    );
    let mut engine = Engine::demo()?;
    engine.replace_document(pcbcraft_io::read_project(std::path::Path::new(path))?)?;
    Ok(engine)
}

/// Every symbol of the active sheet lies on the canvas and shows its
/// designator, and the view is the framed page: its width plus a 5 mm
/// margin on each side across 800 points. Returns how many symbols.
fn assert_framed(
    editor: &mut SchematicEditor,
    engine: &Engine,
    ctx: &egui::Context,
    page_width_mm: f32,
) -> Result<usize, Box<dyn std::error::Error>> {
    let shown = texts(&canvas_frame(editor, engine, ctx, vec![]).0);
    let project = engine.project();
    let view = schematic_view(editor);
    let mut symbols = 0;
    for occurrence in project
        .occurrences
        .iter()
        .filter(|o| o.sheet == editor.sheet)
    {
        let bounds = editor
            .placed_bounds
            .get(&occurrence.id)
            .ok_or("a resolved symbol")?;
        for corner in [bounds.min, bounds.max] {
            assert!(
                CANVAS.contains(view.screen(corner)),
                "{bounds:?} at scale {} and pan {:?}",
                editor.scale,
                editor.pan
            );
        }
        let designator = &project
            .instance(occurrence.instance)
            .ok_or("its instance")?
            .designator;
        assert!(shown.contains(designator), "{designator} in {shown:?}");
        symbols += 1;
    }
    let scale = 800.0 / (page_width_mm + 10.0);
    assert!((editor.scale - scale).abs() < 1e-3, "{}", editor.scale);
    Ok(symbols)
}

/// The founder's test drive: the ESP32 schematic opened zoomed into an
/// empty corner of its sheet, showing no circuit until Fit.
#[test]
fn an_opened_schematic_and_a_switched_to_sheet_show_their_content_framed() -> TestResult {
    let engine = esp32_board()?;
    let project = engine.project();
    let ctx = ctx();
    let mut editor = SchematicEditor::default();
    // A view left over from the document before.
    editor.show_sheet(project.sheets.list[1].id, project);
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    editor.scale = 40.0;
    editor.pan = vec2(900.0, -700.0);
    // As the app does on open, before Schematic has drawn a frame.
    editor.document_opened(project);
    assert_eq!(editor.active_sheet(), SheetId::FIRST);
    // Sheet 1 is A4 landscape, sheet 2 A3 landscape.
    let first = assert_framed(&mut editor, &engine, &ctx, 297.0)?;
    assert!(first > 0, "sheet 1 has symbols");

    let second = project.sheets.list[1].id;
    editor.scale = 40.0;
    editor.pan = vec2(-900.0, 700.0);
    editor.show_sheet(second, project);
    let symbols = assert_framed(&mut editor, &engine, &ctx, 420.0)?;
    assert!(symbols > 0, "sheet 2 has symbols");
    assert_eq!(first + symbols, project.occurrences.len());
    Ok(())
}

/// Framing happens once. The user's view stays through later frames and
/// edits, and across a visit to another workspace, which draws no canvas.
#[test]
fn a_framed_sheet_keeps_the_users_view_afterwards() -> TestResult {
    let mut engine = esp32_board()?;
    let ctx = ctx();
    let mut editor = SchematicEditor::default();
    editor.document_opened(engine.project());
    assert_framed(&mut editor, &engine, &ctx, 297.0)?;
    editor.scale = 9.0;
    editor.pan = vec2(120.0, -80.0);
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    engine.apply(
        engine.revision(),
        Operation::Schematic(SchematicCommand::RenameProject {
            name: "ESP32 board".into(),
        }),
    )?;
    canvas_frame(&mut editor, &engine, &ctx, vec![]);
    assert_eq!((editor.scale, editor.pan), (9.0, vec2(120.0, -80.0)));
    Ok(())
}

/// Pictures of the ESP32 board's labels on the canvas, for evidence: run
/// with `PCBCRAFT_PICTURES=<dir>`. Each is the canvas's own egui output,
/// filled by the CPU rasterizer of the layout tests.
#[test]
fn write_label_pictures() -> TestResult {
    let Some(dir) = std::env::var_os("PCBCRAFT_PICTURES") else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)?;
    let engine = esp32_board()?;
    let project = engine.project();
    let first = project.sheets.list[0].id;
    let second = project.sheets.list[1].id;
    let views: [(&str, SheetId, Point, f32); 6] = [
        ("labels-sheet1-usb", first, [40.0, -55.0], 9.0),
        ("labels-sheet1-ldo", first, [150.0, -55.0], 9.0),
        ("labels-sheet2-module", second, [110.0, -135.0], 4.0),
        ("labels-sheet2-module-gnd", second, [128.0, -125.0], 12.0),
        ("labels-sheet2-module-io", second, [102.0, -125.0], 12.0),
        ("labels-sheet2-headers", second, [315.0, -80.0], 7.0),
    ];
    for (name, sheet, at, scale) in views {
        let mut editor = centred(at, scale);
        editor.switch_sheet(Some(sheet), project);
        let picture = crate::pcb_editor::raster_tests::render(|ui| {
            editor.canvas(ui, &engine);
        });
        let png = picture.png().ok_or("png")?;
        std::fs::write(dir.join(format!("{name}.png")), png)?;
    }
    Ok(())
}

/// The demo with a VBUS port on R1's pin 2, whose wire runs to R2's pin 1,
/// and a GND port on R2's pin 2: the VBUS wire and its middle.
fn vbus_and_gnd(
    engine: &mut Engine,
) -> Result<(Id, pcbcraft_core::geometry::Point), Box<dyn std::error::Error>> {
    let project = engine.project();
    let (r1, r2) = (project.instances[0].id, project.instances[1].id);
    let (from, to) = (WireEnd::from(ep(r1, 2)), WireEnd::from(ep(r2, 1)));
    let wire = project
        .wires
        .iter()
        .find(|w| (w.from, w.to) == (from, to) || (w.from, w.to) == (to, from))
        .map(|w| w.id)
        .ok_or("the demo's wire")?;
    let a = schematic::pin_position(project, ep(r1, 2)).ok_or("pin")?;
    let b = schematic::pin_position(project, ep(r2, 1)).ok_or("pin")?;
    for (name, style, at) in [
        ("VBUS", PowerStyle::Bar, ep(r1, 2)),
        ("GND", PowerStyle::Ground, ep(r2, 2)),
    ] {
        engine.apply(
            engine.revision(),
            Operation::Schematic(SchematicCommand::PlaceLabel {
                sheet: None,
                name: name.into(),
                kind: LabelKind::Power { style },
                position: [0.0, 0.0],
                rotation: Default::default(),
                attach: Some(WireTarget::Terminal(at)),
            }),
        )?;
    }
    Ok((wire, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]))
}

/// Z4: the Power tool, still on its default GND, clicked onto the VBUS
/// wire. Core refuses the port, so the board cannot be shorted silently;
/// the refusal opens the review, which names both nets.
#[test]
fn a_gnd_port_clicked_onto_a_vbus_wire_opens_the_merge_review() -> TestResult {
    let mut engine = Engine::demo()?;
    let (wire, middle) = vbus_and_gnd(&mut engine)?;
    let ctx = ctx();
    let mut editor = centred(middle, 12.0);
    editor.on_tool("power", &engine);
    assert_eq!(editor.drafts.power, "GND", "the tool's default");
    let op = click(&mut editor, &engine, &ctx, CANVAS.center()).ok_or("no port")?;
    let Operation::Schematic(command @ SchematicCommand::PlaceLabel { name, attach, .. }) = &op
    else {
        return Err(format!("{op:?}").into());
    };
    assert_eq!(name, "GND");
    assert!(
        matches!(attach, Some(WireTarget::OnWire(p)) if p.wire == wire),
        "{attach:?}"
    );
    let before = engine.document().clone();
    let refused = engine.apply(engine.revision(), op.clone());
    assert!(
        matches!(&refused, Err(pcbcraft_core::Error::Conflict(m)) if m.contains("VBUS and GND")),
        "{refused:?}"
    );
    assert_eq!(engine.document(), &before);
    assert!(editor.review_named_merge(&engine, command));
    assert!(editor.update.is_open(), "no review");
    let Some(Change::Schematic { command: reviewed }) = editor.update.change() else {
        return Err("the review is not of the port".into());
    };
    assert_eq!(format!("{reviewed:?}"), format!("{command:?}"));
    let plan = engine.preview_update(Change::Schematic {
        command: command.clone(),
    });
    let content = crate::update_preview::content(&plan, engine.project(), None, &[]);
    assert!(
        content
            .lines
            .iter()
            .any(|l| l.title == "Joins VBUS and GND into one net, GND"),
        "{:?}",
        content.lines
    );
    Ok(())
}
