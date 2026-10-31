//! Footprint sketch tools (slice F4): each gesture is one
//! `library.edit_footprint` commit through the real evaluator; Esc and a
//! refused gesture leave the footprint unchanged; the ribbon, keys and
//! canvas reach the same state machine.
use super::footprint_tools::{
    ActiveLayer, ArrayTool, FootprintTool, Gesture, Pick, Scene, ToolState,
};
use super::*;
use pcbcraft_core::ids::PackageTerminalId;
use pcbcraft_core::library::footprint_edit;
use pcbcraft_core::library::footprint_source::{FootprintEdit, PadTemplate};
use pcbcraft_core::library::{
    DrawingLayer, FootprintDefinition, HoleShape, PackageDefinition, PackageTerminal, Pad, PadKind,
    PadShape,
};
use pcbcraft_solidcraft::footprint::SketchEvaluator;
use pcbcraft_solidcraft::footprint::view::{SketchView, sketch_view};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn package(terminals: u32) -> PackageDefinition {
    PackageDefinition {
        body_size: [4.9, 3.9, 1.5],
        terminals: (1..=terminals)
            .map(|n| PackageTerminal {
                id: PackageTerminalId(n + 100),
                designation: n.to_string(),
            })
            .collect(),
        ..PackageDefinition::default()
    }
}

/// A footprint draft and the tool state drawing on it; commits run the
/// core's `library.edit_footprint` with the Solidcraft evaluator.
struct Bench {
    fp: FootprintDefinition,
    pkg: PackageDefinition,
    tools: ToolState,
}

impl Bench {
    fn new(terminals: u32) -> Self {
        Self {
            fp: FootprintDefinition {
                next_local: 1,
                ..FootprintDefinition::default()
            },
            pkg: package(terminals),
            tools: ToolState::default(),
        }
    }
    fn view(&self) -> SketchView {
        self.fp
            .source
            .as_deref()
            .and_then(|s| sketch_view(s).ok())
            .unwrap_or_default()
    }
    fn scene<'a>(&'a self, view: &'a SketchView) -> Scene<'a> {
        Scene {
            footprint: &self.fp,
            package: &self.pkg,
            view,
            evaluator: &SketchEvaluator,
            tolerance: 0.05,
            chord: 0.001,
        }
    }
    /// Click at `at`; the committed edits when the gesture finished.
    fn press(&mut self, at: Point) -> pcbcraft_core::Result<Option<Vec<FootprintEdit>>> {
        let view = self.view();
        let mut tools = self.tools.clone();
        let gesture = tools.press(at, &self.scene(&view));
        self.tools = tools;
        match gesture? {
            Gesture::Commit(edits) => {
                self.apply(&edits)?;
                Ok(Some(edits))
            }
            Gesture::None => Ok(None),
        }
    }
    fn apply(&mut self, edits: &[FootprintEdit]) -> pcbcraft_core::Result<()> {
        let (fp, _) = footprint_edit::edit(&self.fp, &self.pkg, edits, &SketchEvaluator)?;
        self.fp = fp;
        Ok(())
    }
    fn pad(&self, number: &str) -> Result<&Pad, String> {
        self.fp
            .pads
            .iter()
            .find(|p| p.number == number)
            .ok_or_else(|| format!("no pad {number}"))
    }
    fn params(&self) -> Vec<String> {
        self.fp
            .source
            .as_deref()
            .map(|s| s.parameters.iter().map(|p| p.name.0.clone()).collect())
            .unwrap_or_default()
    }
}

fn near(a: Point, b: Point) -> bool {
    (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
}

fn rect(w: f64, h: f64) -> PadShape {
    PadShape::Rect { size: [w, h] }
}

/// A SOIC-8 drawn as a dual row: pad 1, then the row direction. One
/// commit makes eight pads numbered counter-clockwise, sized by named
/// parameters, pad 1 landing package terminal 1.
#[test]
fn a_dual_row_gesture_draws_a_soic8_in_one_commit() -> TestResult {
    let mut b = Bench::new(8);
    b.tools.arm(FootprintTool::Array(ArrayTool::DualRow));
    assert_eq!(b.press([-2.7, 1.905])?, None, "pad 1 alone is no commit");
    assert!(b.fp.source.is_none(), "nothing applied mid-gesture");
    let edits = b
        .press([-2.7, 0.0])?
        .ok_or("the direction finishes the gesture")?;
    assert_eq!(
        edits
            .iter()
            .filter(|e| matches!(e, FootprintEdit::AddArray { .. }))
            .count(),
        1
    );
    assert_eq!(b.fp.pads.len(), 8);
    for (n, at) in [
        ("1", [-2.7, 1.905]),
        ("4", [-2.7, -1.905]),
        ("5", [2.7, -1.905]),
        ("8", [2.7, 1.905]),
    ] {
        let pad = b.pad(n)?;
        assert!(near(pad.position, at), "pad {n} at {:?}", pad.position);
        assert_eq!(pad.shape, rect(1.55, 0.6), "pad {n}");
    }
    assert_eq!(b.pad("1")?.terminal, Some(PackageTerminalId(101)));
    for name in ["pitch", "row_span", "pad_l", "pad_w"] {
        assert!(
            b.params().iter().any(|p| p == name),
            "{name} in {:?}",
            b.params()
        );
    }
    assert_eq!(
        b.tools.tool,
        Some(FootprintTool::Array(ArrayTool::DualRow)),
        "stays armed"
    );
    Ok(())
}

/// A pad tool places one pad per click, reusing the size parameters;
/// Space turns the next pad a quarter.
#[test]
fn pad_tools_place_one_pad_per_click_and_space_turns_it() -> TestResult {
    let mut b = Bench::new(4);
    b.tools.arm(FootprintTool::Pad(PadTemplate::Rect));
    b.press([1.0, 1.0])?.ok_or("one click places a pad")?;
    b.press([-1.0, 1.0])?.ok_or("and another")?;
    assert_eq!(b.fp.pads.len(), 2);
    assert!(near(b.pad("1")?.position, [1.0, 1.0]));
    assert_eq!(b.pad("2")?.terminal, Some(PackageTerminalId(102)));
    assert!(b.tools.turn(), "Space turns a pad tool");
    b.press([0.0, -2.0])?.ok_or("a turned pad")?;
    let turned = b.pad("3")?;
    let [w, h] = turned.shape.extent();
    let along_y = (w - 0.6).abs() < 1e-9 && (h - 1.55).abs() < 1e-9;
    let rotated =
        (turned.rotation_deg - 90.0).abs() < 1e-9 || (turned.rotation_deg - 270.0).abs() < 1e-9;
    assert!(
        along_y || rotated,
        "{:?} {}",
        turned.shape,
        turned.rotation_deg
    );
    let mut names = b.params();
    names.sort();
    assert_eq!(names, ["pad_l", "pad_w"], "sizes are shared parameters");
    b.tools.arm(FootprintTool::Pad(PadTemplate::Circle));
    b.press([3.0, 0.0])?.ok_or("a round pad")?;
    assert_eq!(b.pad("4")?.shape, PadShape::Circle { diameter: 1.0 });
    Ok(())
}

/// Lines draw on the active layer as one graphic role per segment and
/// chain from their end; Esc ends the chain, then the tool, and changes
/// nothing.
#[test]
fn drawing_tools_draw_on_the_active_layer_and_esc_changes_nothing() -> TestResult {
    let mut b = Bench::new(0);
    b.tools.arm(FootprintTool::Line);
    b.tools.layer = ActiveLayer::Silkscreen;
    assert_eq!(b.press([-1.0, 2.0])?, None);
    b.press([1.0, 2.0])?.ok_or("a segment")?;
    assert_eq!(b.tools.pending, vec![[1.0, 2.0]], "chains from its end");
    b.press([1.0, -2.0])?.ok_or("a second segment")?;
    let silk: Vec<_> =
        b.fp.graphics
            .iter()
            .filter(|g| g.layer == DrawingLayer::Silkscreen)
            .collect();
    assert_eq!(silk.len(), 2);
    let source = b.fp.source.as_deref().ok_or("a sketch")?;
    assert_eq!(source.graphics.len(), 2);
    // The second segment starts on the first one's end: coincident, and
    // each axis-aligned line is constrained H or V.
    let view = b.view();
    let kinds: Vec<&str> = view.constraints.iter().map(|k| k.kind).collect();
    assert!(kinds.contains(&"Coincident"), "{kinds:?}");
    assert!(
        kinds.contains(&"Horizontal") && kinds.contains(&"Vertical"),
        "{kinds:?}"
    );
    let before = b.fp.clone();
    b.tools.cancel();
    assert!(b.tools.pending.is_empty());
    assert_eq!(
        b.tools.tool,
        Some(FootprintTool::Line),
        "Esc ends the chain first"
    );
    b.tools.cancel();
    assert_eq!(b.tools.tool, None, "then the tool");
    assert_eq!(b.fp, before, "Esc changes nothing");
    // Construction geometry has no role and no output.
    b.tools.layer = ActiveLayer::Construction;
    b.tools.arm(FootprintTool::Rect);
    b.press([-3.0, -3.0])?;
    b.press([3.0, 3.0])?.ok_or("a construction rectangle")?;
    assert_eq!(b.fp.graphics.len(), 2, "construction is never output");
    // Courtyard circle and assembly arc.
    b.tools.layer = ActiveLayer::Courtyard;
    b.tools.arm(FootprintTool::Circle);
    b.press([0.0, 0.0])?;
    b.press([3.5, 0.0])?.ok_or("a courtyard circle")?;
    b.tools.layer = ActiveLayer::Assembly;
    b.tools.arm(FootprintTool::Arc);
    b.press([0.0, 0.0])?;
    b.press([1.0, 0.0])?;
    b.tools.hover([0.7, 0.7]);
    b.tools.hover([0.0, 1.0]);
    b.press([0.0, 1.0])?.ok_or("an assembly arc")?;
    let layers: Vec<DrawingLayer> = b.fp.graphics.iter().map(|g| g.layer).collect();
    assert!(
        layers.contains(&DrawingLayer::Courtyard) && layers.contains(&DrawingLayer::Assembly),
        "{layers:?}"
    );
    Ok(())
}

/// Drill on a pad adds a plated hole bound to `drill`; a bare click makes
/// an NPTH pad; a slot on a pad is a stadium hole.
#[test]
fn drill_and_slot_make_holes_on_pads_and_bare_npth_holes() -> TestResult {
    let mut b = Bench::new(2);
    b.tools.options.pad.size = [1.8, 1.8];
    b.tools.arm(FootprintTool::Pad(PadTemplate::Rect));
    b.press([0.0, 0.0])?.ok_or("a pad")?;
    b.press([5.0, 0.0])?.ok_or("a second pad")?;
    b.tools.arm(FootprintTool::Drill);
    b.press([0.1, 0.1])?.ok_or("a click on the pad drills it")?;
    let one = b.pad("1")?;
    assert_eq!(one.kind, PadKind::PlatedThrough);
    let hole = one.hole.as_ref().ok_or("a hole")?;
    assert!(hole.plated);
    assert_eq!(hole.shape, HoleShape::Round { diameter: 0.8 });
    assert!(b.params().iter().any(|p| p == "drill"));
    b.press([0.0, 4.0])?.ok_or("a bare click")?;
    let npth =
        b.fp.pads
            .iter()
            .find(|p| p.kind == PadKind::NonPlatedHole)
            .ok_or("an NPTH pad")?;
    assert!(near(npth.position, [0.0, 4.0]));
    assert!(npth.hole.as_ref().is_some_and(|h| !h.plated));
    assert!(npth.terminal.is_none() && npth.number.is_empty());
    // Drilling a drilled pad is refused and leaves the state unchanged.
    let before = b.fp.clone();
    assert!(b.press([0.0, 0.0]).is_err());
    assert_eq!(b.fp, before);
    b.tools.arm(FootprintTool::Slot);
    b.press([4.7, 0.0])?;
    b.press([5.3, 0.0])?.ok_or("both slot ends")?;
    let two = b.pad("2")?;
    match two.hole.as_ref().map(|h| h.shape) {
        Some(HoleShape::Slot { width, length, .. }) => {
            assert!(
                (width - 0.8).abs() < 1e-6 && (length - 1.4).abs() < 1e-6,
                "{width} {length}"
            );
        }
        other => return Err(format!("a slot, not {other:?}").into()),
    }
    Ok(())
}

/// Quad and grid arrays: per-side counts, a grid with a centre void, and
/// deleting an instance skips it in its array.
#[test]
fn quad_and_grid_arrays_with_skips() -> TestResult {
    let mut b = Bench::new(20);
    b.tools.options.pad.size = [0.8, 0.25];
    b.tools.options.array.nx = 3;
    b.tools.options.array.ny = 5;
    b.tools.options.array.pitch = 0.5;
    b.tools.options.array.span = [4.8, 3.8];
    b.tools.arm(FootprintTool::Array(ArrayTool::Quad));
    b.press([0.0, 0.0])?.ok_or("one click places a quad")?;
    assert_eq!(b.fp.pads.len(), 16);
    let one = b.pad("1")?;
    assert!(near(one.position, [-2.4, 1.0]), "{:?}", one.position);
    assert_eq!(one.shape, rect(0.8, 0.25));
    let mut g = Bench::new(0);
    g.tools.options.pad.diameter = 1.0;
    g.tools.options.array.template = PadTemplate::Circle;
    g.tools.options.array.rows = 3;
    g.tools.options.array.cols = 3;
    g.tools.options.array.void = [1, 1];
    g.tools.options.array.alpha = true;
    g.tools.arm(FootprintTool::Array(ArrayTool::Grid));
    g.press([-2.54, 2.54])?.ok_or("a grid")?;
    assert_eq!(g.fp.pads.len(), 8, "the centre is void");
    assert!(g.pad("A1").is_ok() && g.pad("C3").is_ok());
    // Select an instance and delete it: skipped, not removed by number.
    let c3 = g.pad("C3")?.clone();
    g.tools.arm(FootprintTool::Select);
    let view = g.view();
    let mut tools = g.tools.clone();
    tools.select(c3.position, &g.scene(&view), false);
    g.tools = tools;
    assert_eq!(g.tools.selection, vec![Pick::Pad(c3.id)]);
    let Gesture::Commit(edits) = g.tools.delete_edits(&g.scene(&view))? else {
        return Err("delete commits".into());
    };
    g.apply(&edits)?;
    assert_eq!(g.fp.pads.len(), 7);
    assert!(g.pad("C3").is_err());
    let a1 = g.pad("A1")?.id;
    assert!(
        g.fp.pads.iter().any(|p| p.id == a1),
        "other pads keep their identity"
    );
    Ok(())
}

/// Select picks a pad by its land; a drag moves its sketch entities by
/// the snapped offset in one `Move`; an array moves through its seed.
#[test]
fn select_and_move_pads_and_arrays() -> TestResult {
    let mut b = Bench::new(8);
    b.tools.arm(FootprintTool::Array(ArrayTool::DualRow));
    b.press([-2.7, 1.905])?;
    b.press([-2.7, 0.0])?.ok_or("a SOIC")?;
    let ids: Vec<_> = b.fp.pads.iter().map(|p| (p.id, p.position)).collect();
    b.tools.arm(FootprintTool::Select);
    let five = b.pad("5")?.position;
    let view = b.view();
    let mut tools = b.tools.clone();
    tools.select(five, &b.scene(&view), false);
    b.tools = tools;
    let Gesture::Commit(edits) = b.tools.move_edits([0.5, 0.0], &b.scene(&view))? else {
        return Err("a move commits".into());
    };
    assert_eq!(edits.len(), 1, "one Move");
    b.apply(&edits)?;
    for (id, at) in ids {
        let pad = b.fp.pads.iter().find(|p| p.id == id).ok_or("same pads")?;
        assert!(
            near(pad.position, [at[0] + 0.5, at[1]]),
            "{id}: {:?}",
            pad.position
        );
    }
    Ok(())
}

/// The Custom tool frees a template pad's outline, and starts a profile
/// pad on empty board in one commit.
#[test]
fn the_custom_tool_converts_or_starts_profile_pads() -> TestResult {
    let mut b = Bench::new(2);
    b.tools.arm(FootprintTool::Pad(PadTemplate::Rect));
    b.press([0.0, 0.0])?;
    b.tools.arm(FootprintTool::CustomPad);
    b.press([0.0, 0.0])?.ok_or("convert")?;
    b.press([4.0, 0.0])?.ok_or("start a profile pad")?;
    let source = b.fp.source.as_deref().ok_or("a sketch")?;
    assert_eq!(source.pads.len(), 2);
    assert!(source.pads.iter().all(|r| matches!(
        r.geometry,
        pcbcraft_core::library::footprint_source::PadGeometry::Profile
    )));
    assert_eq!(b.fp.pads.len(), 2, "both still solve to pads");
    Ok(())
}

/// Sketch tools refuse a footprint with explicit pads, changing nothing.
#[test]
fn sketch_tools_refuse_explicit_pads() -> TestResult {
    let mut b = Bench::new(1);
    b.fp.pads.push(Pad::smd_rect(
        pcbcraft_core::PadId(1),
        PackageTerminalId(101),
        "1",
        [0.0, 0.0],
        [1.0, 1.0],
    ));
    let before = b.fp.clone();
    b.tools.arm(FootprintTool::Pad(PadTemplate::Rect));
    assert!(b.press([2.0, 0.0]).is_err());
    assert_eq!(b.fp, before);
    assert!(b.tools.pending.is_empty(), "the gesture is dropped");
    Ok(())
}

/// Ids and keys: every tool round-trips its id, keys are unique and
/// leave Solidcraft's model keys and the Symbol tab's chords alone.
#[test]
fn tool_ids_and_keys_are_unique() {
    let mut ids = std::collections::BTreeSet::new();
    let mut keys = std::collections::BTreeSet::new();
    for tool in FootprintTool::ALL {
        assert_eq!(FootprintTool::from_id(tool.id()), Some(tool));
        assert!(ids.insert(tool.id()));
        if let Some(k) = tool.key() {
            assert!(keys.insert(format!("{:?}{}", k.key, k.shift)), "{tool:?}");
        }
    }
    for layer in ActiveLayer::ALL {
        assert_eq!(ActiveLayer::from_id(layer.id()), Some(layer));
        assert!(ids.insert(layer.id()));
    }
    // X stays Solidcraft's Construction toggle, Space the quarter turn.
    for taken in ["X", "Space"] {
        assert!(
            !keys
                .iter()
                .any(|k| k.starts_with(taken) && k.ends_with("false")),
            "{taken}"
        );
    }
}

// --- The tools in the Footprint tab -----------------------------------

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

fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One canvas frame; the operation it sent.
fn frame(
    editor: &mut PartEditor,
    engine: &Engine,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Option<Operation> {
    let mut sent = None;
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(CANVAS),
            events,
            ..Default::default()
        },
        |ui| {
            sent = editor.canvas(ui, engine);
        },
    );
    output.textures_delta.clear();
    sent
}

fn click(
    editor: &mut PartEditor,
    engine: &mut Engine,
    ctx: &egui::Context,
    at: Pos2,
) -> pcbcraft_core::Result<bool> {
    let mut sent = false;
    for events in [
        vec![egui::Event::PointerMoved(at)],
        vec![button(at, true)],
        vec![button(at, false)],
        vec![],
    ] {
        if let Some(op) = frame(editor, engine, ctx, events) {
            engine.apply_with(engine.revision(), op, &SketchEvaluator)?;
            sent = true;
        }
    }
    Ok(sent)
}

/// The demo resistor with its explicit pads removed: the Footprint tab
/// offers the sketch tools.
fn empty_footprint(
    ctx: &egui::Context,
    engine: &mut Engine,
) -> Result<PartEditor, Box<dyn std::error::Error>> {
    fonts::setup_bundled_fonts(ctx);
    let mut editor = PartEditor::default();
    editor.commit(ctx, engine);
    editor.on_tab("footprint");
    if let Some(set) = &mut editor.draft {
        set.footprint.pads.clear();
    }
    if let Some(op) = editor.commit(ctx, engine) {
        engine.apply(engine.revision(), op)?;
    }
    editor.commit(ctx, engine);
    frame(&mut editor, engine, ctx, vec![]);
    Ok(editor)
}

fn screen(editor: &PartEditor, p: Point) -> Result<Pos2, &'static str> {
    super::footprint_canvas::screen(editor, CANVAS, p).ok_or("not fitted")
}

fn committed(engine: &Engine, editor: &PartEditor) -> Option<FootprintDefinition> {
    let id = editor.draft.as_ref()?.ids.footprint;
    match &engine.drafts().get(id)?.definition {
        pcbcraft_core::library::Definition::Footprint(f) => Some(f.clone()),
        _ => None,
    }
}

/// The ribbon offers the Sketch, Pads and Layer groups once the
/// footprint has no explicit pads; its buttons arm the tools.
#[test]
fn the_ribbon_offers_sketch_pads_and_layer_groups() -> TestResult {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = PartEditor::default();
    editor.commit(&ctx, &engine);
    editor.on_tab("footprint");
    let labels = |r: &crate::shell::Ribbon| r.groups.iter().map(|g| g.label).collect::<Vec<_>>();
    let explicit = editor.ribbon(&engine);
    assert!(
        !labels(&explicit).contains(&"Sketch"),
        "explicit pads keep the pad tools"
    );
    let mut editor = empty_footprint(&ctx, &mut engine)?;
    let ribbon = editor.ribbon(&engine);
    for group in ["Sketch", "Pads", "Layer"] {
        assert!(labels(&ribbon).contains(&group), "{:?}", labels(&ribbon));
    }
    let ids: Vec<&str> = ribbon
        .groups
        .iter()
        .flat_map(|g| g.tools.iter())
        .flat_map(|t| std::iter::once(t.id).chain(t.menu.iter().map(|m| m.id)))
        .collect();
    for id in [
        "fp-select",
        "fp-line",
        "fp-pad-rect",
        "fp-array-dual",
        "fp-drill",
        "fp-slot",
        "fp-on-silk",
    ] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    editor.on_tool("fp-array-quad", &engine);
    assert_eq!(
        editor.footprint_tool.tool,
        Some(FootprintTool::Array(ArrayTool::Quad))
    );
    editor.on_tool("fp-on-courtyard", &engine);
    assert_eq!(editor.footprint_tool.layer, ActiveLayer::Courtyard);
    Ok(())
}

/// Drawing a SOIC-like footprint on the canvas: the dual-row tool from
/// the ribbon, two clicks, one commit through the engine and one undo.
#[test]
fn a_soic_drawn_on_the_canvas_is_one_undoable_commit() -> TestResult {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = empty_footprint(&ctx, &mut engine)?;
    editor.on_tool("fp-array-dual", &engine);
    let revision = engine.revision();
    let first = screen(&editor, [-2.7, 1.9])?;
    assert!(!click(&mut editor, &mut engine, &ctx, first)?);
    assert_eq!(engine.revision(), revision, "pad 1 alone commits nothing");
    let toward = screen(&editor, [-2.7, -1.0])?;
    assert!(click(&mut editor, &mut engine, &ctx, toward)?);
    let fp = committed(&engine, &editor).ok_or("a footprint draft")?;
    assert_eq!(fp.pads.len(), 8);
    let one = fp.pads.iter().find(|p| p.number == "1").ok_or("pad 1")?;
    assert!(near(one.position, [-2.7, 1.9]), "{:?}", one.position);
    assert_eq!(engine.revision(), revision + 1, "one commit");
    engine.undo()?;
    let back = committed(&engine, &editor).ok_or("a footprint draft")?;
    assert!(
        back.pads.is_empty() && back.source.is_none(),
        "one undo step"
    );
    Ok(())
}

/// Keys arm the tools (N pad, L line, H drill) and Esc ends the tool;
/// Space turns the next pad.
#[test]
fn keys_arm_footprint_tools() -> TestResult {
    let ctx = egui::Context::default();
    let mut engine = Engine::demo()?;
    let mut editor = empty_footprint(&ctx, &mut engine)?;
    let inside = Pos2::new(300.0, 200.0);
    for (k, tool) in [
        (egui::Key::N, FootprintTool::Pad(PadTemplate::Rect)),
        (egui::Key::L, FootprintTool::Line),
        (egui::Key::H, FootprintTool::Drill),
        (egui::Key::R, FootprintTool::Rect),
    ] {
        frame(
            &mut editor,
            &engine,
            &ctx,
            vec![egui::Event::PointerMoved(inside), key(k)],
        );
        assert_eq!(editor.footprint_tool.tool, Some(tool), "{k:?}");
    }
    frame(&mut editor, &engine, &ctx, vec![key(egui::Key::X)]);
    assert_eq!(
        editor.footprint_tool.layer,
        ActiveLayer::Silkscreen,
        "X leaves construction"
    );
    frame(&mut editor, &engine, &ctx, vec![key(egui::Key::N)]);
    frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Space)]);
    assert_eq!(editor.footprint_tool.quarter_turns, 1);
    frame(&mut editor, &engine, &ctx, vec![key(egui::Key::Escape)]);
    assert_eq!(editor.footprint_tool.tool, None);
    Ok(())
}
