//! The Symbol view's drawing tools driven through real egui frames:
//! every tool is a ribbon button, each finished tool action is one commit
//! (one undo step), an unfinished polyline never reaches the document, Esc
//! and Cmd+Z act on pending points first, and rejected edits leave the
//! document unchanged.
use super::drawing::Tool;
use super::symbol_canvas::Arrange;
use super::*;
use pcbcraft_core::library::Definition;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Harness {
    ctx: egui::Context,
    engine: Engine,
    editor: PartEditor,
    time: f64,
}

impl Harness {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        editor.on_tab("symbol");
        let mut h = Self {
            ctx,
            engine,
            editor,
            time: 0.0,
        };
        h.frame(vec![]);
        // 10 px/mm: ±30 mm by ±20 mm is on the canvas.
        h.editor.symbol_view = Some(symbol_canvas::ViewState {
            pan: egui::Vec2::ZERO,
            scale: 10.0,
        });
        Ok(h)
    }

    /// One 600 × 400 canvas frame, a tenth of a second after the last.
    fn frame(&mut self, events: Vec<egui::Event>) {
        self.frame_with(events, egui::Modifiers::NONE);
    }

    fn frame_with(&mut self, mut events: Vec<egui::Event>, modifiers: egui::Modifiers) {
        self.time += 0.1;
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                self.editor.canvas(ui, &self.engine);
            },
        );
        output.textures_delta.clear();
    }

    /// Screen position of a point in millimetres.
    fn at(&self, p: Point) -> Pos2 {
        let view = self.editor.symbol_view.unwrap_or(symbol_canvas::ViewState {
            pan: egui::Vec2::ZERO,
            scale: 1.0,
        });
        Pos2::new(300.0, 200.0)
            + view.pan
            + egui::vec2(p[0] as f32 * view.scale, -p[1] as f32 * view.scale)
    }

    fn button(pos: Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    /// A click a second after the last, so clicks never read as double.
    fn click_with(&mut self, p: Point, modifiers: egui::Modifiers) {
        self.time += 1.0;
        let pos = self.at(p);
        self.frame_with(vec![egui::Event::PointerMoved(pos)], modifiers);
        self.frame_with(vec![Self::button(pos, true, modifiers)], modifiers);
        self.frame_with(vec![Self::button(pos, false, modifiers)], modifiers);
    }

    fn click(&mut self, p: Point) {
        self.click_with(p, egui::Modifiers::NONE);
    }

    fn drag(&mut self, from: Point, to: Point) {
        let (a, b) = (self.at(from), self.at(to));
        self.frame(vec![egui::Event::PointerMoved(a)]);
        self.frame(vec![Self::button(a, true, egui::Modifiers::NONE)]);
        self.frame(vec![egui::Event::PointerMoved(a + (b - a) * 0.5)]);
        self.frame(vec![egui::Event::PointerMoved(b)]);
        self.frame(vec![Self::button(b, false, egui::Modifiers::NONE)]);
    }

    fn key(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.frame_with(
            vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            modifiers,
        );
    }

    /// Send the editor's pending commit, if any. Returns whether one was
    /// sent.
    fn commit(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        match self.editor.commit(&self.ctx, &self.engine) {
            Some(op) => {
                self.engine.apply(self.engine.revision(), op)?;
                // Reload the working copy from the engine.
                self.editor.commit(&self.ctx, &self.engine);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn unit(&self) -> Result<Unit, Box<dyn std::error::Error>> {
        let id = self.editor.draft.as_ref().ok_or("draft")?.ids.symbol;
        match &self
            .engine
            .drafts()
            .get(id)
            .ok_or("symbol draft")?
            .definition
        {
            Definition::Symbol(s) => Ok(s.units[0].clone()),
            _ => Err("not a symbol".into()),
        }
    }

    /// Arm a tool the way a ribbon click does.
    fn arm(&mut self, tool: Tool) {
        let id = tool.id();
        self.editor.on_tool(id, &self.engine);
    }
}

/// The symbol canvas moves as the other canvases do: two fingers pan it
/// without zooming, a pinch zooms about the pointer.
#[test]
fn two_fingers_pan_the_symbol_and_a_pinch_zooms_it() -> TestResult {
    use crate::navigation::tests::swipe;
    let mut h = Harness::new()?;
    let at = h.at([4.0, 3.0]);
    h.frame(vec![egui::Event::PointerMoved(at)]);
    let before = h.editor.symbol_view.ok_or("no view")?;
    h.frame(swipe(egui::vec2(6.0, -2.0), egui::Modifiers::NONE, 2));
    let panned = h.editor.symbol_view.ok_or("no view")?;
    assert_eq!(panned.scale, before.scale, "two fingers must not zoom");
    assert!((panned.pan - before.pan - egui::vec2(12.0, -4.0)).length() < 1e-3);
    let mm = |h: &Harness| {
        let v = h.editor.symbol_view.unwrap_or(panned);
        let local = at - Pos2::new(300.0, 200.0) - v.pan;
        (local.x / v.scale, -local.y / v.scale)
    };
    let under = mm(&h);
    h.frame(vec![egui::Event::Zoom(2.0)]);
    let zoomed = h.editor.symbol_view.ok_or("no view")?;
    assert!(
        (zoomed.scale - 2.0 * panned.scale).abs() < 1e-4,
        "{}",
        zoomed.scale
    );
    let after = mm(&h);
    assert!((after.0 - under.0).abs() < 1e-4 && (after.1 - under.1).abs() < 1e-4);
    Ok(())
}

/// Every drawing tool is on the Symbol ribbon with an icon, in the Select,
/// Draw, Pins, Arrange and Grid groups. Variants fold into split buttons
/// (Line ▾ with Polyline, Rectangle ▾ with Rounded, Circle ▾ with Arc,
/// Text ▾ with Field), as
/// Solidcraft's sketch ribbon does; the button shows the last variant armed.
#[test]
fn the_symbol_ribbon_shows_every_drawing_tool() -> TestResult {
    let mut h = Harness::new()?;
    let ribbon = h.editor.ribbon(&h.engine);
    let groups: Vec<(&str, Vec<&str>)> = ribbon
        .groups
        .iter()
        .map(|g| (g.label, g.tools.iter().map(|t| t.label).collect()))
        .collect();
    let expect = [
        ("Select", vec!["Select"]),
        (
            "Draw",
            vec![
                "Line",
                "Rectangle",
                "Circle",
                "Polygon",
                "Text",
                "Import SVG…",
            ],
        ),
        ("Pins", vec!["Pin"]),
        ("Arrange", vec!["Rotate", "Mirror", "Duplicate", "Delete"]),
        ("Grid", vec!["Grid", "Snap"]),
    ];
    for (label, tools) in expect {
        assert!(
            groups.iter().any(|(l, t)| *l == label && *t == tools),
            "{label}: {groups:?}"
        );
    }
    let draw = ribbon
        .groups
        .iter()
        .find(|g| g.label == "Draw")
        .ok_or("draw group")?;
    let menus: Vec<Vec<&str>> = draw
        .tools
        .iter()
        .map(|t| t.menu.iter().map(|v| v.label).collect())
        .collect();
    assert_eq!(
        menus,
        vec![
            vec!["Line", "Polyline"],
            vec!["Rectangle", "Rounded"],
            vec!["Circle", "Arc"],
            vec![],
            vec!["Text", "Field"],
            vec![],
        ]
    );
    // Every drawing tool is reachable: a button or a split-button variant.
    let reachable: Vec<&str> = ribbon
        .groups
        .iter()
        .flat_map(|g| g.tools.iter())
        .flat_map(|t| std::iter::once(t.id).chain(t.menu.iter().map(|v| v.id)))
        .collect();
    for tool in Tool::ALL {
        assert!(reachable.contains(&tool.id()), "{tool:?}");
    }
    for group in &ribbon.groups {
        for tool in group
            .tools
            .iter()
            .chain(group.tools.iter().flat_map(|t| &t.menu))
        {
            assert!(
                !tool.icon.is_empty() && !tool.tip.is_empty(),
                "{}",
                tool.label
            );
        }
    }
    // A ribbon click arms the tool; its split button then shows and runs
    // it, selected, and keeps showing it after the tool ends.
    h.arm(Tool::Arc);
    let ribbon = h.editor.ribbon(&h.engine);
    let arc = ribbon
        .groups
        .iter()
        .flat_map(|g| g.tools.iter())
        .find(|t| t.id == Tool::Arc.id())
        .ok_or("arc tool")?;
    assert!(arc.selected && arc.enabled);
    assert_eq!((arc.label, arc.icon), ("Arc", Tool::Arc.icon()));
    assert!(
        arc.menu
            .iter()
            .any(|v| v.id == Tool::Arc.id() && v.selected)
    );
    h.arm(Tool::Select);
    let ribbon = h.editor.ribbon(&h.engine);
    assert!(
        ribbon
            .groups
            .iter()
            .flat_map(|g| g.tools.iter())
            .any(|t| t.id == Tool::Arc.id() && !t.selected)
    );
    Ok(())
}

/// Arming a drawing tool clears the selection, so Properties shows only
/// the tool's options.
#[test]
fn arming_a_drawing_tool_clears_the_selection() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.selection = [Selected::Pin(TerminalId(1))].into_iter().collect();
    h.arm(Tool::Line);
    assert!(h.editor.selection.is_empty());
    h.editor.selection = [Selected::Pin(TerminalId(1))].into_iter().collect();
    h.arm(Tool::Select);
    assert!(!h.editor.selection.is_empty(), "Select keeps it");
    Ok(())
}

/// Each tool, armed from the ribbon, adds exactly one graphic (or pin) in
/// exactly one commit, and one undo removes it.
#[test]
fn each_tool_commits_one_graphic_as_one_undo_step() -> TestResult {
    let mut h = Harness::new()?;
    type Gesture = Vec<Point>;
    let cases: Vec<(Tool, Gesture, bool)> = vec![
        (Tool::Line, vec![[-10.0, 10.0], [-5.0, 10.0]], false),
        (Tool::Rect, vec![[10.0, 10.0], [15.0, 12.5]], false),
        (Tool::RoundedRect, vec![[10.0, -10.0], [15.0, -5.0]], false),
        (Tool::Circle, vec![[-10.0, -10.0], [-7.5, -10.0]], false),
        (
            Tool::Arc,
            vec![[0.0, 10.0], [2.5, 10.0], [0.0, 12.5]],
            false,
        ),
        (Tool::Text, vec![[0.0, -10.0]], false),
        (Tool::Field, vec![[0.0, -12.5]], false),
        (Tool::Pin, vec![[-12.5, 0.0]], false),
        (
            Tool::Polyline,
            vec![[-15.0, 5.0], [-12.5, 5.0], [-12.5, 7.5]],
            true,
        ),
        (
            Tool::Polygon,
            vec![[15.0, 0.0], [17.5, 0.0], [17.5, 2.5]],
            true,
        ),
    ];
    for (tool, points, chain) in cases {
        let before = h.unit()?;
        let revision = h.engine.revision();
        h.arm(tool);
        for p in &points {
            h.click(*p);
        }
        if chain {
            assert!(!h.commit()?, "{tool:?}: nothing is sent mid-chain");
            assert_eq!(h.unit()?, before, "{tool:?}");
            h.key(egui::Key::Enter, egui::Modifiers::NONE);
        }
        assert!(h.commit()?, "{tool:?}: one commit");
        assert!(!h.commit()?, "{tool:?}: only one");
        assert_eq!(h.engine.revision(), revision + 1, "{tool:?}");
        let after = h.unit()?;
        let added =
            (after.graphics.len() + after.pins.len()) - (before.graphics.len() + before.pins.len());
        assert_eq!(added, 1, "{tool:?}");
        if tool == Tool::RoundedRect {
            assert!(after.graphics.iter().any(|g| matches!(
                &g.item,
                Item::Shape { shape: Shape::Rect { radius, .. }, .. } if *radius > 0.0
            )));
        }
        h.key(egui::Key::Escape, egui::Modifiers::NONE);
        h.key(egui::Key::Escape, egui::Modifiers::NONE);
        h.engine.undo()?;
        h.commit()?;
        assert_eq!(h.unit()?, before, "{tool:?}: one undo step");
    }
    Ok(())
}

/// A polyline mid-chain stays in the tool: Cmd+Z pops its points without
/// touching the document, Esc drops it, and a second Esc ends the tool.
#[test]
fn pending_points_never_reach_the_document() -> TestResult {
    let mut h = Harness::new()?;
    let document = h.engine.document().clone();
    h.arm(Tool::Polyline);
    for p in [[0.0, 10.0], [5.0, 10.0], [5.0, 15.0]] {
        h.click(p);
    }
    assert_eq!(h.editor.drawing.pending().len(), 3);
    assert!(!h.commit()?);
    h.key(egui::Key::Z, egui::Modifiers::COMMAND);
    assert_eq!(h.editor.drawing.pending().len(), 2, "Cmd+Z pops a point");
    assert_eq!(h.engine.document(), &document);
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!h.editor.drawing.busy(), "Esc drops the chain");
    assert_eq!(
        h.editor.drawing.tool(),
        Tool::Polyline,
        "the tool stays armed"
    );
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(h.editor.drawing.tool(), Tool::Select);
    assert!(!h.commit()?);
    assert_eq!(h.engine.document(), &document, "nothing was sent");
    // A right click ends a chain too.
    h.arm(Tool::Line);
    h.click([0.0, 10.0]);
    let pos = h.at([0.0, 10.0]);
    for pressed in [true, false] {
        h.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }]);
    }
    assert!(!h.editor.drawing.busy());
    assert_eq!(h.engine.document(), &document);
    Ok(())
}

/// An edit the engine refuses (a text height beyond the bounds) leaves the
/// document unchanged and the working copy is reloaded from it.
#[test]
fn a_refused_edit_leaves_the_document_unchanged() -> TestResult {
    let mut h = Harness::new()?;
    let document = h.engine.document().clone();
    let revision = h.engine.revision();
    if let Some(Item::Text(text)) = h
        .editor
        .draft
        .as_mut()
        .and_then(|s| {
            s.symbol.units[0]
                .graphics
                .iter_mut()
                .find(|g| matches!(g.item, Item::Text(_)))
        })
        .map(|g| &mut g.item)
    {
        text.height = 500.0;
    }
    let operation = h
        .editor
        .commit(&h.ctx, &h.engine)
        .ok_or("an edit is sent")?;
    assert!(h.engine.apply(revision, operation).is_err());
    assert_eq!(h.engine.document(), &document);
    assert_eq!(h.engine.revision(), revision);
    h.editor.commit(&h.ctx, &h.engine);
    assert_eq!(h.editor.draft, h.editor.baseline);
    assert!(
        h.editor
            .draft
            .as_ref()
            .is_some_and(
                |s| s.symbol.units[0].graphics.iter().all(|g| match &g.item {
                    Item::Text(t) => t.height < 50.0,
                    _ => true,
                })
            )
    );
    Ok(())
}

/// Click and Shift-click select; a window box takes what is inside; a drag
/// moves the selection on the grid and commits once on release; Rotate,
/// Mirror, Duplicate and Delete each commit once and keep identity.
#[test]
fn selection_moves_and_arranges_in_one_commit_each() -> TestResult {
    let mut h = Harness::new()?;
    // Draw a rectangle away from the resistor.
    h.arm(Tool::Rect);
    h.click([10.0, 10.0]);
    h.click([15.0, 12.5]);
    h.commit()?;
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    let rect = h.unit()?.graphics.last().map(|g| g.id).ok_or("rect")?;
    // Click its edge selects it; Shift-click on a pin adds the pin.
    h.click([12.5, 10.0]);
    assert_eq!(
        h.editor.selection,
        [Selected::Graphic(rect)].into_iter().collect()
    );
    h.click_with([5.0, 0.0], egui::Modifiers::SHIFT);
    assert_eq!(h.editor.selection.len(), 2);
    // A window box from empty space around the rectangle selects it alone.
    h.drag([9.0, 9.0], [16.0, 14.0]);
    assert_eq!(
        h.editor.selection,
        [Selected::Graphic(rect)].into_iter().collect()
    );
    // Drag the rectangle by about (2.4, −2.6) mm: it moves (2.5, −2.5).
    let revision = h.engine.revision();
    h.drag([15.0, 11.0], [17.4, 8.4]);
    assert!(h.commit()?);
    assert!(!h.commit()?);
    assert_eq!(h.engine.revision(), revision + 1, "one commit on release");
    let moved = h.unit()?;
    let shape = moved.graphic(rect).map(|g| g.item.clone());
    assert!(
        matches!(&shape, Some(Item::Shape { shape: Shape::Rect { a, b, .. }, .. }) if *a == [12.5, 7.5] && *b == [17.5, 10.0]),
        "{shape:?}"
    );
    for action in Arrange::ALL {
        let before = h.unit()?;
        let revision = h.engine.revision();
        h.editor.on_tool(action.id(), &h.engine);
        assert!(h.commit()?, "{action:?}");
        assert_eq!(h.engine.revision(), revision + 1, "{action:?}");
        let after = h.unit()?;
        match action {
            Arrange::Duplicate => {
                assert_eq!(after.graphics.len(), before.graphics.len() + 1);
            }
            Arrange::Delete => {
                assert_eq!(after.graphics.len(), before.graphics.len() - 1);
            }
            _ => assert_eq!(
                after.graphics.iter().map(|g| g.id).collect::<Vec<_>>(),
                before.graphics.iter().map(|g| g.id).collect::<Vec<_>>(),
                "{action:?} keeps identity"
            ),
        }
    }
    // With nothing selected, arranging changes nothing.
    let document = h.engine.document().clone();
    h.editor.selection.clear();
    h.editor.on_tool(Arrange::Rotate.id(), &h.engine);
    assert!(!h.commit()?);
    assert_eq!(h.engine.document(), &document);
    Ok(())
}

/// Keys arm tools without the ribbon; Shift chords are distinct from plain
/// keys; Shift+R repeats the last tool.
#[test]
fn keys_arm_tools_and_shift_r_repeats() -> TestResult {
    let mut h = Harness::new()?;
    h.key(egui::Key::L, egui::Modifiers::SHIFT);
    assert_eq!(h.editor.drawing.tool(), Tool::Polyline);
    h.key(egui::Key::L, egui::Modifiers::NONE);
    assert_eq!(h.editor.drawing.tool(), Tool::Line);
    h.key(egui::Key::R, egui::Modifiers::NONE);
    assert_eq!(h.editor.drawing.tool(), Tool::Rect);
    h.key(egui::Key::Escape, egui::Modifiers::NONE);
    assert_eq!(h.editor.drawing.tool(), Tool::Select);
    h.key(egui::Key::R, egui::Modifiers::SHIFT);
    assert_eq!(h.editor.drawing.tool(), Tool::Rect, "repeat");
    for tool in Tool::ALL {
        if let Some(k) = tool.key() {
            let modifiers = if k.shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            };
            h.key(egui::Key::Escape, egui::Modifiers::NONE);
            h.key(k.key, modifiers);
            assert_eq!(h.editor.drawing.tool(), tool, "{k:?}");
        }
    }
    Ok(())
}

/// A Space key still "down" in egui after typing (its release went to a
/// text field) does not leave the canvas panning: tool clicks still draw.
#[test]
fn a_space_left_down_by_typing_does_not_block_drawing() -> TestResult {
    let mut h = Harness::new()?;
    // Space pressed with no release seen by the canvas, as after typing.
    h.time += 1.0;
    h.ctx
        .memory_mut(|m| m.request_focus(egui::Id::new("elsewhere")));
    h.frame(vec![egui::Event::Key {
        key: egui::Key::Space,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }]);
    h.ctx
        .memory_mut(|m| m.surrender_focus(egui::Id::new("elsewhere")));
    h.arm(Tool::Rect);
    h.click([10.0, 10.0]);
    h.click([15.0, 12.5]);
    assert!(h.commit()?, "the rectangle was drawn");
    Ok(())
}

/// A pin placed outside the drawn body ends at the outline, not at the
/// stroke's outer edge, so grid pins keep grid lengths.
#[test]
fn pins_end_on_the_outline_with_grid_lengths() -> TestResult {
    let mut h = Harness::new()?;
    // The demo body is the rectangle (−2.5, −1)–(2.5, 1) with a 0.25 stroke.
    h.arm(Tool::Pin);
    h.click([0.0, 5.0]);
    assert!(h.commit()?);
    let unit = h.unit()?;
    let pin = unit.pins.last().ok_or("pin")?;
    assert_eq!(pin.direction, PinDirection::Down);
    assert_eq!(pin.length, 4.0);
    Ok(())
}

/// A pin placed when every terminal has one adds a terminal named after
/// its pin number (the next free one), and on a chip body the editor says
/// where the resulting finding is fixed, and the finding locates there.
#[test]
fn a_new_pin_numbers_its_terminal_and_warns_on_a_chip_body() -> TestResult {
    let mut h = Harness::new()?;
    h.arm(Tool::Pin);
    h.click([-12.5, 0.0]);
    assert!(h.commit()?);
    let set = h.editor.draft.as_ref().ok_or("draft")?;
    let terminal = set.component.terminals.last().ok_or("terminal")?;
    assert_eq!(terminal.name, "3");
    let physical = set
        .binding
        .mappings
        .iter()
        .find(|m| m.terminal == terminal.id)
        .and_then(|m| set.package.terminal(m.package_terminal))
        .ok_or("package terminal")?;
    assert_eq!(physical.designation, "3", "name and number agree");
    h.editor.flush(&h.ctx);
    let posted = crate::kit::toast::take_posted(&h.ctx);
    assert!(
        posted
            .iter()
            .any(|p| p.level == Level::Warning && p.text.contains("3D tab")),
        "{posted:?}"
    );
    // The publish finding locates where it is fixed: the 3D tab.
    let finding = h
        .editor
        .listed_findings(&h.engine)
        .into_iter()
        .find(|f| f.blocking && f.text.contains("two terminals"))
        .ok_or("the body's terminal-count finding")?;
    h.editor.on_tab("symbol");
    assert!(crate::issues::Locate::locate(
        &mut h.editor,
        &h.engine,
        &finding.objects
    ));
    assert!(h.editor.view == View::Model, "the 3D tab");
    Ok(())
}

/// Adding a terminal grows the template's body only, tracked by ID: once
/// it is deleted, a body-filled rectangle the author drew is left as drawn.
#[test]
fn adding_a_terminal_never_stretches_an_authored_rectangle() -> TestResult {
    let mut h = Harness::new()?;
    let body = h.editor.body.ok_or("template body tracked")?;
    // Delete the template body and draw a body-filled rectangle.
    h.editor.selection = [Selected::Graphic(body.1)].into_iter().collect();
    h.editor.on_tool(Arrange::Delete.id(), &h.engine);
    h.commit()?;
    h.editor.fill = Fill::Body;
    h.arm(Tool::Rect);
    h.click([-2.5, -2.5]);
    h.click([2.5, 2.5]);
    h.commit()?;
    let drawn = h.unit()?.graphics.last().cloned().ok_or("rectangle")?;
    h.editor.add_terminal();
    let unit = &h.editor.draft.as_ref().ok_or("draft")?.symbol.units[0];
    assert_eq!(unit.graphic(drawn.id), Some(&drawn), "left as drawn");
    assert!(unit.pins.len() == 3, "the pin is still added");
    Ok(())
}
