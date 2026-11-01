//! Footprint tab behaviour: drawing and picking typed pads, grid drags,
//! pad tools, layer toggles and the inspector rows, each edit committed
//! once through the public `library.edit` command.
use super::footprint_canvas;
use super::*;
use pcbcraft_core::library::{CornerMask, Hole, HoleShape, MaskRule, PadKind, PadShape, PasteRule};

const CANVAS: Rect = Rect {
    min: Pos2::ZERO,
    max: Pos2::new(600.0, 400.0),
};

fn button(pos: Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One canvas frame over the whole 600 × 400 screen; the drawn texts.
fn canvas_frame(
    editor: &mut PartEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Vec<String> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(CANVAS),
            events,
            ..Default::default()
        },
        |ui| {
            editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    let mut out = Vec::new();
    fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    for shape in &output.shapes {
        texts(&shape.shape, &mut out);
    }
    out
}

/// One inspector frame; the drawn texts.
fn inspector_frame(editor: &mut PartEditor, engine: &Engine, ctx: &egui::Context) -> Vec<String> {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 4000.0))),
            ..Default::default()
        },
        |ui| {
            editor.inspector(ui, engine);
        },
    );
    output.textures_delta.clear();
    let mut out = Vec::new();
    fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    for shape in &output.shapes {
        texts(&shape.shape, &mut out);
    }
    out
}

fn footprint_editor(
    ctx: &egui::Context,
    engine: &Engine,
) -> Result<PartEditor, Box<dyn std::error::Error>> {
    fonts::setup_bundled_fonts(ctx);
    let mut editor = PartEditor::default();
    editor.commit(ctx, engine);
    editor.on_tab("footprint");
    canvas_frame(&mut editor, engine, ctx, vec![]);
    Ok(editor)
}

fn pads(editor: &PartEditor) -> Vec<Pad> {
    editor
        .draft
        .as_ref()
        .map(|s| s.footprint.pads.clone())
        .unwrap_or_default()
}

fn click(editor: &mut PartEditor, engine: &Engine, ctx: &egui::Context, at: Pos2) {
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(at, true)],
        vec![button(at, false)],
    ] {
        canvas_frame(editor, engine, ctx, events);
    }
}

/// Commit the working copy through the engine; returns whether an edit
/// was sent.
fn commit(
    editor: &mut PartEditor,
    engine: &mut Engine,
    ctx: &egui::Context,
) -> pcbcraft_core::Result<bool> {
    let Some(operation) = editor.commit(ctx, engine) else {
        return Ok(false);
    };
    engine.apply(engine.revision(), operation)?;
    assert!(editor.commit(ctx, engine).is_none(), "sent once");
    Ok(true)
}

fn committed_pads(engine: &Engine, editor: &PartEditor) -> Option<Vec<Pad>> {
    let id = editor.draft.as_ref()?.ids.footprint;
    match &engine.drafts().get(id)?.definition {
        pcbcraft_core::library::Definition::Footprint(f) => Some(f.pads.clone()),
        _ => None,
    }
}

/// The view draws every pad's number and picks a pad by its land: a
/// rotated pad is picked inside its turned outline, not its unturned box.
#[test]
fn the_footprint_view_draws_and_picks_pads_by_their_land() -> Result<(), Box<dyn std::error::Error>>
{
    let ctx = egui::Context::default();
    let engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    // Turn pad 2 by 90° so its long side runs along x.
    let second = pads(&editor).get(1).cloned().ok_or("two pads")?;
    if let Some(set) = &mut editor.draft {
        let pad = &mut set.footprint.pads[1];
        pad.shape = PadShape::Rect { size: [0.6, 3.0] };
        pad.rotation_deg = 90.0;
    }
    editor.footprint_view = None;
    let shown = canvas_frame(&mut editor, &engine, &ctx, vec![]);
    for number in ["1", "2"] {
        assert!(shown.iter().any(|t| t == number), "{number} in {shown:?}");
    }
    let at = |e: &PartEditor, p: Point| footprint_canvas::screen(e, CANVAS, p).ok_or("not fitted");
    // Inside the turned land, outside the unturned one: picks pad 2.
    let inside = at(&editor, [second.position[0] + 1.2, second.position[1]])?;
    click(&mut editor, &engine, &ctx, inside);
    assert_eq!(editor.selected_pad, Some(second.id));
    assert_eq!(editor.selected, Some(TerminalId(2)));
    // Inside the unturned box only: nothing is picked.
    let outside = at(&editor, [second.position[0], second.position[1] + 1.2])?;
    click(&mut editor, &engine, &ctx, outside);
    assert_eq!(editor.selected_pad, None);
    Ok(())
}

/// A drag moves the picked pad on the footprint grid and commits once,
/// as one undoable edit.
#[test]
fn dragging_a_pad_snaps_to_the_footprint_grid_and_commits_once()
-> Result<(), Box<dyn std::error::Error>> {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    let first = pads(&editor).first().cloned().ok_or("a pad")?;
    let at = |e: &PartEditor, p: Point| footprint_canvas::screen(e, CANVAS, p).ok_or("not fitted");
    let from = at(&editor, first.position)?;
    let to = at(
        &editor,
        [first.position[0] - 0.413, first.position[1] + 0.271],
    )?;
    for events in [
        vec![egui::Event::PointerMoved(from)],
        vec![button(from, true)],
        vec![egui::Event::PointerMoved(from + (to - from) * 0.5)],
        vec![egui::Event::PointerMoved(to)],
    ] {
        canvas_frame(&mut editor, &engine, &ctx, events);
    }
    assert!(
        editor.commit(&ctx, &engine).is_none(),
        "nothing commits mid-drag"
    );
    canvas_frame(&mut editor, &engine, &ctx, vec![button(to, false)]);
    let moved = pads(&editor).first().map(|p| p.position).ok_or("a pad")?;
    let on_grid = |v: f64| ((v / 0.05).round() * 0.05 - v).abs() < 1e-9;
    assert!(on_grid(moved[0]) && on_grid(moved[1]), "{moved:?}");
    assert!(
        (moved[0] - (first.position[0] - 0.4)).abs() < 0.051,
        "{moved:?}"
    );
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    let stored = committed_pads(&engine, &editor).ok_or("committed")?;
    assert_eq!(stored[0].position, moved);
    engine.undo()?;
    editor.commit(&ctx, &engine);
    assert_eq!(
        pads(&editor).first().map(|p| p.position),
        Some(first.position)
    );
    Ok(())
}

/// Every typed pad property the inspector edits commits through
/// `library.edit` and reloads unchanged: shape, rotation, kind, hole,
/// mask and paste.
#[test]
fn typed_pad_edits_round_trip_through_the_engine() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    if let Some(set) = &mut editor.draft {
        let pad = &mut set.footprint.pads[0];
        footprint_geometry::set_shape(
            pad,
            PadShape::RoundRect {
                size: [1.2, 1.6],
                radius_ratio: 0.25,
                max_radius: Some(0.2),
                corners: CornerMask {
                    top_left: true,
                    top_right: false,
                    bottom_right: false,
                    bottom_left: true,
                },
            },
        );
        pad.rotation_deg = pad.shape.canonical_rotation(450.0);
        pad.mask = MaskRule::Expansion(0.1);
        pad.paste = PasteRule::Reduction(0.05);
        let pad = &mut set.footprint.pads[1];
        footprint_geometry::set_kind(pad, PadKind::PlatedThrough, None);
        footprint_geometry::set_shape(pad, PadShape::Oblong { size: [1.6, 2.4] });
        pad.hole = Some(Hole {
            shape: HoleShape::Slot {
                width: 0.6,
                length: 1.2,
                angle_deg: 90.0,
            },
            plated: true,
            offset: [0.0, 0.1],
        });
        set.footprint.datum.rotation_deg = 90.0;
    }
    let edited = pads(&editor);
    assert_eq!(edited[0].rotation_deg, 90.0, "450° is canonical 90°");
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    assert_eq!(committed_pads(&engine, &editor), Some(edited.clone()));
    assert_eq!(
        pads(&editor),
        edited,
        "the working copy reloads what was stored"
    );
    // The inspector shows the typed rows of the selected pad.
    editor.selected_pad = Some(edited[1].id);
    let shown = inspector_frame(&mut editor, &engine, &ctx);
    for text in [
        "Kind",
        "Plated through-hole",
        "Shape",
        "Oblong",
        "Hole",
        "Slot",
        "Mask",
        "Paste",
        "Plated",
        "All copper layers",
    ] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    Ok(())
}

/// The Hole and Fiducial tools add pads that land no terminal, and a
/// copper pad for an already landed terminal lands none either; the pad
/// selector reaches them and Remove pad takes only the selected pad.
#[test]
fn pad_tools_add_terminal_less_pads_and_remove_the_selected_one()
-> Result<(), Box<dyn std::error::Error>> {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    let before = pads(&editor);
    editor.on_tool("pad-npth", &engine);
    editor.on_tool("pad-fiducial", &engine);
    editor.on_tool("pad-pth", &engine);
    let after = pads(&editor);
    assert_eq!(after.len(), before.len() + 3);
    let added = &after[before.len()..];
    assert_eq!(
        added
            .iter()
            .map(|p| (p.kind, p.terminal.is_some()))
            .collect::<Vec<_>>(),
        vec![
            (PadKind::NonPlatedHole, false),
            (PadKind::Fiducial, false),
            // The selected terminal already has a pad: the new copper pad
            // lands none rather than a second, inferred one.
            (PadKind::PlatedThrough, false),
        ]
    );
    assert_eq!(added[0].number, "MH1");
    assert_eq!(added[1].number, "FID1");
    assert!(
        before.iter().all(|p| p.number != added[2].number),
        "a new pad's number repeats no existing pad's"
    );
    // New pads land clear of the others.
    for pair in after.windows(2) {
        assert!(!pair[0].bounds().intersects(&pair[1].bounds()) || pair[0].id == pair[1].id);
    }
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    assert_eq!(committed_pads(&engine, &editor), Some(after.clone()));
    // The hole is selectable from the inspector's pad list.
    editor.selected_pad = Some(added[0].id);
    let shown = inspector_frame(&mut editor, &engine, &ctx);
    assert!(
        shown.iter().any(|t| t == "MH1 · Non-plated hole"),
        "{shown:?}"
    );
    editor.on_tool("remove-pad", &engine);
    let left = pads(&editor);
    assert_eq!(left.len(), after.len() - 1);
    assert!(left.iter().all(|p| p.id != added[0].id));
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    Ok(())
}

/// The ribbon's layer toggles show and hide the display-only overlays and
/// drawings; nothing in the document changes.
#[test]
fn layer_toggles_change_the_view_only() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = egui::Context::default();
    let engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    let selected = |editor: &mut PartEditor| {
        editor
            .ribbon(&engine)
            .groups
            .iter()
            .flat_map(|g| g.tools.iter())
            .find(|t| t.id == "fp-mask")
            .map(|t| t.selected)
    };
    assert_eq!(selected(&mut editor), Some(true));
    let draft = editor.draft.clone();
    editor.on_tool("fp-mask", &engine);
    assert_eq!(selected(&mut editor), Some(false));
    assert!(!editor.footprint_layers.mask);
    assert_eq!(editor.draft, draft);
    assert!(editor.commit(&ctx, &engine).is_none());
    Ok(())
}

/// Blocking findings come before lint and name pads by number; the
/// footprint's issue slot counts the blocker, and locating the issue
/// selects the pad it names on the Footprint tab.
#[test]
fn findings_are_grouped_and_locating_one_selects_the_pad() -> Result<(), Box<dyn std::error::Error>>
{
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    let pad = {
        let set = editor.draft.as_mut().ok_or("draft")?;
        let pad = set.footprint.pads.first_mut().ok_or("a pad")?;
        pad.terminal = None;
        pad.number = "7".into();
        pad.id
    };
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    let listed = editor.listed_findings(&engine);
    let blocking = listed.iter().take_while(|f| f.blocking).count();
    assert!(
        listed.iter().skip(blocking).all(|f| !f.blocking),
        "errors first"
    );
    let finding = listed
        .iter()
        .find(|f| f.blocking && f.text.starts_with("Pad 7 "))
        .ok_or("the pad's finding, by number")?
        .clone();
    let slot = editor
        .inspector_card(&engine)
        .issues
        .ok_or("an issue slot")?;
    assert!(slot.counts.error >= 1, "{slot:?}");
    editor.on_tab("symbol");
    editor.selected_pad = None;
    assert!(crate::issues::Locate::locate(
        &mut editor,
        &engine,
        &finding.objects
    ));
    assert_eq!(editor.selected_pad, Some(pad));
    assert!(editor.view == View::Footprint, "the Footprint tab");
    Ok(())
}

/// One inspector frame with input; the drawn texts with their rects, and
/// the clipboard text the frame produced.
fn inspector_input(
    editor: &mut PartEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (Vec<(String, Rect)>, Vec<String>) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 4000.0))),
            events,
            ..Default::default()
        },
        |ui| {
            editor.inspector(ui, engine);
        },
    );
    output.textures_delta.clear();
    fn texts(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => out.push((
                text.galley.job.text.clone(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for shape in &output.shapes {
        texts(&shape.shape, &mut out);
    }
    let copied = output
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    (out, copied)
}

/// One home for a problem: the card shows a pad's blocking finding only as
/// its issue slot's count, never as text, and locating the finding (what
/// an Issues row does, its rows being buttons) selects the pad on the
/// Footprint tab from any tab.
#[test]
fn a_finding_shows_as_the_slot_count_and_locates_its_pad() -> Result<(), Box<dyn std::error::Error>>
{
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    let pad = {
        let set = editor.draft.as_mut().ok_or("draft")?;
        let pad = set.footprint.pads.first_mut().ok_or("a pad")?;
        pad.terminal = None;
        pad.number = "7".into();
        pad.id
    };
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    let (shown, _) = inspector_input(&mut editor, &engine, &ctx, vec![]);
    assert!(
        !shown.iter().any(|(t, _)| t.starts_with("Pad 7 ")),
        "no finding text in the card: {shown:?}"
    );
    let objects = editor
        .listed_findings(&engine)
        .into_iter()
        .find(|f| f.blocking && f.text.starts_with("Pad 7 "))
        .ok_or("the pad's finding")?
        .objects;
    let slot = editor
        .inspector_card(&engine)
        .issues
        .ok_or("an issue slot")?;
    assert!(slot.counts.error >= 1, "{slot:?}");
    for tab in ["details", "symbol", "mapping", "model"] {
        editor.on_tab(tab);
        editor.selected_pad = None;
        assert!(crate::issues::Locate::locate(
            &mut editor,
            &engine,
            &objects
        ));
        assert_eq!(editor.selected_pad, Some(pad), "from {tab}");
        assert!(editor.view == View::Footprint, "the Footprint tab");
    }
    Ok(())
}

/// B-f: the Terminals list and the Pad section show one selection. An
/// unmapped pad lights no terminal, a clicked terminal shows its own pad,
/// and a terminal without a pad shows no other terminal's pad.
#[test]
fn the_terminals_list_and_the_pad_section_agree() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    // Pad 2 lands no terminal; terminal 2 has no pad.
    let (first, second) = {
        let set = editor.draft.as_mut().ok_or("draft")?;
        let [a, b] = set.footprint.pads.as_mut_slice() else {
            return Err("two pads".into());
        };
        b.terminal = None;
        (a.id, b.id)
    };
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    let set = editor.draft.as_ref().ok_or("draft")?;
    let names: Vec<(TerminalId, String)> = set
        .component
        .terminals
        .iter()
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let [(one, one_name), (two, two_name)] = names.as_slice() else {
        return Err("two terminals".into());
    };
    let lit = |editor: &PartEditor| {
        let set = editor.draft.as_ref()?;
        let pad = footprint_props::current_pad(
            &set.footprint,
            &set.binding,
            editor.selected_pad,
            editor.selected,
        )?;
        Some(pad)
    };
    // The unmapped pad selected on the canvas, terminal 1 still selected.
    editor.selected = Some(*one);
    editor.selected_pad = Some(second);
    let (shown, _) = inspector_input(&mut editor, &engine, &ctx, vec![]);
    assert_eq!(lit(&editor), Some(second));
    // A terminal's row: its name under the Terminals caption.
    let label = |shown: &[(String, Rect)], name: &str| {
        let below = shown.iter().find(|(t, _)| t == "TERMINALS")?.1.bottom();
        shown
            .iter()
            .find(|(t, r)| t == name && r.top() > below)
            .map(|(_, r)| r.center())
    };
    let at = label(&shown, one_name).ok_or("terminal 1's row")?;
    // Clicking terminal 1 shows its pad, even though it was selected.
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(at, true)],
        vec![button(at, false)],
    ] {
        inspector_input(&mut editor, &engine, &ctx, events);
    }
    assert_eq!(editor.selected, Some(*one));
    assert_eq!(lit(&editor), Some(first));
    // Terminal 2 has no pad: the Pad section shows none, not pad 1.
    let (shown, _) = inspector_input(&mut editor, &engine, &ctx, vec![]);
    let at = label(&shown, two_name).ok_or("terminal 2's row")?;
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(at, true)],
        vec![button(at, false)],
    ] {
        inspector_input(&mut editor, &engine, &ctx, events);
    }
    assert_eq!(editor.selected, Some(*two));
    assert_eq!(lit(&editor), None);
    let (shown, _) = inspector_input(&mut editor, &engine, &ctx, vec![]);
    let note = format!("No pad lands terminal ‘{two_name}’. Click a pad on the canvas to show it.");
    assert!(shown.iter().any(|(t, _)| *t == note), "{shown:?}");
    Ok(())
}

/// B-i: the Mapping canvas links mapped rows only; an unmapped row reads
/// "Unmapped" with no link drawn.
#[test]
fn the_mapping_canvas_draws_no_link_for_an_unmapped_row() -> Result<(), Box<dyn std::error::Error>>
{
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    editor.on_tab("mapping");
    let links = |editor: &mut PartEditor, engine: &Engine| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(CANVAS),
                ..Default::default()
            },
            |ui| {
                editor.canvas(ui, engine);
            },
        );
        output.textures_delta.clear();
        fn count(shape: &egui::Shape) -> usize {
            match shape {
                egui::Shape::LineSegment { .. } => 1,
                egui::Shape::Vec(shapes) => shapes.iter().map(count).sum(),
                _ => 0,
            }
        }
        output.shapes.iter().map(|s| count(&s.shape)).sum::<usize>()
    };
    let mapped = links(&mut editor, &engine);
    let set = editor.draft.as_mut().ok_or("draft")?;
    let terminals = set.package.terminals.len();
    let first = set
        .package
        .terminals
        .first()
        .ok_or("a package terminal")?
        .id;
    set.binding.mappings.retain(|m| m.package_terminal != first);
    assert!(commit(&mut editor, &mut engine, &ctx)?);
    let unmapped = links(&mut editor, &engine);
    assert!(mapped >= terminals, "{mapped} links for {terminals} rows");
    assert_eq!(unmapped, mapped - 1, "one link fewer");
    Ok(())
}

/// The picker previews what Build makes, warns when its pad count
/// differs from the package, and clears a sourced footprint completely.
#[test]
fn the_template_picker_previews_warns_and_clears() -> Result<(), Box<dyn std::error::Error>> {
    use super::footprint_template_picker::{accepts, cleared, count_warning};
    use pcbcraft_core::library::templates::footprint_family::TemplateKind;
    let ctx = egui::Context::default();
    let engine = Engine::demo()?;
    let mut editor = footprint_editor(&ctx, &engine)?;
    let package = editor.draft.as_ref().ok_or("draft")?.package.clone();
    editor.template_picker.open(TemplateKind::Qfn, &package);
    let pads = editor
        .template_picker
        .preview(&package)
        .ok_or("a preview")?
        .clone()?;
    assert!(pads.len() > package.terminals.len());
    assert!(
        editor
            .template_picker
            .ghost()
            .is_some_and(|g| g.len() == pads.len())
    );
    assert!(count_warning(&pads, &package).is_some_and(|w| w.contains("terminals")));
    // A sourced footprint (the built QFN) clears to an empty explicit one.
    let empty = pcbcraft_core::library::FootprintDefinition {
        next_local: 1,
        ..Default::default()
    };
    let template = editor.template_picker.form.clone().ok_or("form")?;
    let (built, _) = pcbcraft_core::library::templates::footprint_family::build(
        &empty,
        &package,
        &template,
        &pcbcraft_solidcraft::footprint::SketchEvaluator,
    )?;
    assert!(!accepts(&built));
    let reset = cleared(&built);
    assert!(accepts(&reset), "a template builds into it again");
    assert!(reset.source.is_none() && reset.pads.is_empty() && reset.polarity.is_none());
    Ok(())
}
