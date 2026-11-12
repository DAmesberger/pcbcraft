//! The Component card by subject (slice W1a of docs/sidebar-ia.md), driven
//! through real egui frames: the Details tab holds the component-level
//! data and no geometry tab repeats it; the Symbol tab's card follows the
//! selection (SYMBOL, PIN, TERMINAL, GRAPHIC, TEXT) with object actions in
//! its footer; an armed tool shows its own card; issues locate; and
//! refusals are one-line toasts.
use super::drawing::Tool;
use super::*;
use crate::issues::Locate;
use crate::kit::card::Region;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Harness {
    ctx: egui::Context,
    engine: Engine,
    editor: PartEditor,
}

impl Harness {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        let engine = Engine::demo()?;
        let mut editor = PartEditor::default();
        editor.open_first(&engine);
        editor.commit(&ctx, &engine);
        Ok(Self {
            ctx,
            engine,
            editor,
        })
    }

    /// One card frame (the column hands over one body, as today) with
    /// `events`: the texts drawn and their centres, and the operation.
    fn card(&mut self, events: Vec<egui::Event>) -> (Vec<(String, Pos2)>, Option<Operation>) {
        let mut operation = None;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 2400.0))),
                events,
                ..Default::default()
            },
            |ui| operation = self.editor.inspector(ui, &self.engine),
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
        fn texts(shape: &egui::Shape, out: &mut Vec<(String, Pos2)>) {
            match shape {
                egui::Shape::Text(text) => out.push((
                    text.galley.job.text.clone(),
                    text.pos + text.galley.size() * 0.5,
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            texts(&shape.shape, &mut out);
        }
        (out, operation)
    }

    fn texts(&mut self) -> Vec<String> {
        self.card(vec![]).0.into_iter().map(|(t, _)| t).collect()
    }

    /// Click the drawn text `label`; the operation the click produced.
    fn click(&mut self, label: &str) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let (shown, _) = self.card(vec![]);
        let at = shown
            .iter()
            .find(|(t, _)| t == label)
            .map(|(_, p)| *p)
            .ok_or_else(|| {
                format!(
                    "{label} not in {:?}",
                    shown.iter().map(|(t, _)| t).collect::<Vec<_>>()
                )
            })?;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.card(vec![egui::Event::PointerMoved(at)]);
        self.card(vec![button(true)]);
        let (_, operation) = self.card(vec![button(false)]);
        Ok(operation)
    }

    /// Send the editor's pending commit and reload; whether one was sent.
    fn commit(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        match self.editor.commit(&self.ctx, &self.engine) {
            Some(op) => {
                self.engine.apply(self.engine.revision(), op)?;
                self.editor.commit(&self.ctx, &self.engine);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn set(&self) -> Result<&WorkingSet, Box<dyn std::error::Error>> {
        Ok(self.editor.draft.as_ref().ok_or("no working set")?)
    }

    fn posted(&mut self) -> Vec<kit::toast::Posted> {
        self.editor.flush(&self.ctx);
        kit::toast::take_posted(&self.ctx)
    }
}

const COMPONENT_SECTIONS: [&str; 5] = ["DEFINITION", "FIELDS", "PARTS", "LIBRARY", "PROVENANCE"];

/// The Details card offers the built-in fields until the project has them;
/// the click is one `library.import_standard_fields`.
#[test]
fn add_standard_fields_imports_them_once() -> TestResult {
    let mut h = Harness::new()?;
    let operation = h.click("Add standard fields")?;
    assert!(
        matches!(
            operation,
            Some(Operation::Library(LibraryCommand::ImportStandardFields {}))
        ),
        "{operation:?}"
    );
    let revision = h.engine.revision();
    if let Some(operation) = operation {
        h.engine.apply(revision, operation)?;
    }
    assert_eq!(h.engine.revision(), revision + 1, "one undo step");
    assert!(!h.texts().iter().any(|t| t == "Add standard fields"));
    Ok(())
}

/// The Symbol card's *Redraw as* lists the kinds that fit and sends one
/// `library.restyle_symbol` for the drawn symbol.
#[test]
fn redraw_as_sends_one_restyle_of_the_open_symbol() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    assert!(h.texts().iter().any(|t| t == "Redraw as"));
    h.click("Choose a kind")?;
    let symbol = h.set()?.ids.symbol;
    let pins = h.set()?.component.terminals.len();
    let operation = h.click("IC box")?;
    let Some(Operation::Library(LibraryCommand::RestyleSymbol {
        draft,
        kind,
        terminals,
    })) = operation.clone()
    else {
        return Err(format!("{operation:?}").into());
    };
    assert_eq!(draft, symbol);
    assert_eq!(
        kind,
        pcbcraft_core::library::symbol_style::SymbolKind::IcBox
    );
    assert_eq!(terminals, None);
    if let Some(operation) = operation {
        h.engine.apply(h.engine.revision(), operation)?;
    }
    h.editor.commit(&h.ctx, &h.engine);
    let drawn: usize = h.set()?.symbol.units.iter().map(|u| u.pins.len()).sum();
    assert_eq!(drawn, pins, "the working copy reloads the redrawn symbol");
    Ok(())
}

/// Details is the first tab and the one a component opens on; it holds
/// the component-level sections in order, and no geometry tab repeats any
/// of them.
#[test]
fn details_is_first_and_holds_the_component_level_sections() -> TestResult {
    let mut h = Harness::new()?;
    let ribbon = h.editor.ribbon(&h.engine);
    assert_eq!(ribbon.tabs.first().map(|t| t.label), Some("Details"));
    assert!(ribbon.tabs.first().is_some_and(|t| t.active));
    let shown = h.texts();
    let at: Vec<usize> = COMPONENT_SECTIONS
        .iter()
        .map(|s| shown.iter().position(|t| t == s).ok_or(*s))
        .collect::<Result<_, _>>()?;
    assert!(at.windows(2).all(|w| w[0] < w[1]), "in order: {at:?}");
    assert_eq!(h.editor.header(&h.engine).kind, "Component");
    for tab in ["symbol", "footprint", "mapping", "model"] {
        h.editor.on_tab(tab);
        let shown = h.texts();
        for section in COMPONENT_SECTIONS {
            assert!(!shown.iter().any(|t| t == section), "{section} on {tab}");
        }
    }
    Ok(())
}

/// The Symbol card with nothing selected is SYMBOL and its terminal list;
/// a row selects its pin, which names the card ("Pin 1"), and the PIN card
/// holds the electrical and the graphic halves. Esc-equivalent clearing
/// returns to SYMBOL.
#[test]
fn a_terminal_row_selects_its_pin_and_the_pin_card_merges_both_halves() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    let shown = h.texts();
    assert!(shown.iter().any(|t| t == "SYMBOL"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "TERMINALS"), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "PIN · ELECTRICAL"));
    let first = h.set()?.component.terminals[0].clone();
    h.click(&first.name)?;
    assert_eq!(h.editor.selected_pin(), Some(first.id));
    let header = h.editor.header(&h.engine);
    assert_eq!(header.kind, "Pin");
    assert!(header.title.starts_with("Pin "), "{}", header.title);
    let shown = h.texts();
    let electrical = shown.iter().position(|t| t == "PIN · ELECTRICAL");
    let graphic = shown.iter().position(|t| t == "PIN · GRAPHIC");
    assert!(electrical.is_some() && electrical < graphic, "{shown:?}");
    assert!(shown.iter().any(|t| t == "Remove pin"), "footer action");
    assert!(!shown.iter().any(|t| t == "TERMINALS"), "one subject");
    h.editor.cancel_tool();
    assert!(h.texts().iter().any(|t| t == "SYMBOL"));
    Ok(())
}

/// Remove pin in the footer removes the pin from the unit and keeps its
/// terminal, as one commit; the card then shows the terminal's TERMINAL
/// card from the list, with Remove terminal.
#[test]
fn remove_pin_keeps_the_terminal_and_its_card_remains_reachable() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    let terminal = h.set()?.component.terminals[0].clone();
    h.editor.show_terminal(terminal.id);
    assert_eq!(h.editor.selected_pin(), Some(terminal.id));
    h.click("Remove pin")?;
    assert!(h.commit()?, "one commit");
    let set = h.set()?;
    assert!(set.symbol.pin(terminal.id).is_none(), "the pin is gone");
    assert!(
        set.component.terminal(terminal.id).is_some(),
        "the terminal stays"
    );
    h.click(&terminal.name)?;
    assert_eq!(h.editor.header(&h.engine).kind, "Terminal");
    let shown = h.texts();
    assert!(shown.iter().any(|t| t == "TERMINAL"), "{shown:?}");
    assert!(shown.iter().any(|t| t == "Remove terminal"), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "PIN · GRAPHIC"));
    Ok(())
}

/// A selected shape shows GRAPHIC under a header naming the shape; a text
/// shows TEXT. Remove in the footer deletes it in one commit.
#[test]
fn a_selected_graphic_names_the_card_and_remove_deletes_it() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    let unit = h.set()?.symbol.units[0].clone();
    let rect = unit
        .graphics
        .iter()
        .find(|g| matches!(g.item, Item::Shape { .. }))
        .ok_or("a shape")?
        .id;
    let text = unit
        .graphics
        .iter()
        .find(|g| matches!(g.item, Item::Text(_)))
        .ok_or("a text")?
        .id;
    h.editor
        .select([Selected::Graphic(text)].into_iter().collect());
    assert_eq!(h.editor.header(&h.engine).kind, "Text");
    assert!(h.texts().iter().any(|t| t == "TEXT"));
    h.editor
        .select([Selected::Graphic(rect)].into_iter().collect());
    let header = h.editor.header(&h.engine);
    assert_eq!(
        (header.kind, header.title.as_str()),
        ("Graphic", "Rectangle")
    );
    assert!(h.texts().iter().any(|t| t == "GRAPHIC"));
    h.click("Remove")?;
    assert!(h.commit()?);
    assert!(h.set()?.symbol.units[0].graphic(rect).is_none());
    Ok(())
}

/// An armed drawing tool replaces Properties with its tool card: the
/// header names the tool, its options are rows (no "… tool" section), and
/// Done returns to Select and the previous card.
#[test]
fn an_armed_tool_shows_its_card_and_done_returns_to_select() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    h.editor.on_tool(Tool::Rect.id(), &h.engine);
    let header = h.editor.header(&h.engine);
    assert_eq!((header.kind, header.title.as_str()), ("Tool", "Rectangle"));
    assert_eq!(h.editor.subject_kind(), kit::card::Subject::Tool);
    let shown = h.texts();
    for row in ["Stroke", "Fill", "Done"] {
        assert!(shown.iter().any(|t| t == row), "{row} in {shown:?}");
    }
    assert!(!shown.iter().any(|t| t.ends_with(" TOOL")), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "SYMBOL"), "one subject");
    h.click("Done")?;
    assert_eq!(h.editor.drawing.tool(), Tool::Select);
    assert_eq!(h.editor.subject_kind(), kit::card::Subject::Properties);
    // A chain tool's card has Finish, disabled until enough points.
    h.editor.on_tool(Tool::Polygon.id(), &h.engine);
    let shown = h.texts();
    assert!(shown.iter().any(|t| t == "Finish") && shown.iter().any(|t| t == "Cancel"));
    Ok(())
}

/// Each subject has its own identity, so the card's scroll resets when the
/// subject changes and is kept while it stays.
#[test]
fn each_subject_has_its_own_scroll_identity() -> TestResult {
    let mut h = Harness::new()?;
    let details = h.editor.inspector_card(&h.engine).subject;
    assert_eq!(details, h.editor.inspector_card(&h.engine).subject);
    h.editor.on_tab("symbol");
    let symbol = h.editor.inspector_card(&h.engine).subject;
    let terminal = h.set()?.component.terminals[0].id;
    h.editor.show_terminal(terminal);
    let pin = h.editor.inspector_card(&h.engine).subject;
    assert!(details != symbol && symbol != pin && details != pin);
    Ok(())
}

/// The issue slot counts the inspected object's issues through
/// `project.issues`: a pin off the symbol grid is an issue of that pin,
/// not of another pin, and locating it from Details selects the pin.
#[test]
fn the_issue_slot_counts_the_pin_and_locating_selects_it() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    let [a, b] = [
        h.set()?.component.terminals[0].id,
        h.set()?.component.terminals[1].id,
    ];
    if let Some(set) = &mut h.editor.draft
        && let Some(pin) = set.symbol.units[0]
            .pins
            .iter_mut()
            .find(|p| p.terminal == a)
    {
        pin.position.x_nm = crate::schematic_editor::nm(pin.position_mm()[0] + 0.3);
    }
    assert!(h.commit()?);
    h.editor.show_terminal(a);
    let slot = h.editor.inspector_card(&h.engine).issues.ok_or("slot")?;
    assert!(slot.counts.warning + slot.counts.error >= 1, "{slot:?}");
    assert_eq!(slot.label, "on this pin");
    h.editor.show_terminal(b);
    let other = h.editor.inspector_card(&h.engine).issues.ok_or("slot")?;
    assert_eq!(other.counts.warning + other.counts.error, 0, "{other:?}");
    let component = h.set()?.ids.component;
    h.editor.on_tab("details");
    h.editor.selection.clear();
    let pin = ObjectRef::LibraryObject {
        definition: component,
        local: a.0,
    };
    assert!(h.editor.locate(&h.engine, &[pin]));
    assert!(h.editor.view == View::Symbol);
    assert_eq!(h.editor.selected_pin(), Some(a));
    Ok(())
}

/// Locating a definition opens its tab; an object this workspace does not
/// show is not claimed; a terminal without a pin opens its TERMINAL card;
/// locating never changes the document.
#[test]
fn locate_opens_tabs_and_cards_without_editing() -> TestResult {
    let mut h = Harness::new()?;
    let ids = h.set()?.ids;
    let revision = h.engine.revision();
    for (id, view) in [
        (ids.symbol, View::Symbol),
        (ids.footprint, View::Footprint),
        (ids.binding, View::Mapping),
        (ids.package, View::Model),
        (ids.component, View::Details),
    ] {
        assert!(h.editor.locate(&h.engine, &[ObjectRef::Definition { id }]));
        assert!(h.editor.view == view, "{view:?}");
    }
    let board = ObjectRef::Instance { id: 999_999 };
    assert!(!h.editor.locate(&h.engine, &[board]));
    // A terminal whose pin is removed shows as a TERMINAL card.
    let terminal = h.set()?.component.terminals[0].id;
    if let Some(set) = &mut h.editor.draft {
        for unit in &mut set.symbol.units {
            unit.pins.retain(|p| p.terminal != terminal);
        }
    }
    assert!(h.commit()?);
    let revision_after = h.engine.revision();
    let object = ObjectRef::LibraryObject {
        definition: ids.component,
        local: terminal.0,
    };
    assert!(h.editor.locate(&h.engine, &[object]));
    assert_eq!(h.editor.header(&h.engine).kind, "Terminal");
    assert_eq!(h.engine.revision(), revision_after);
    assert!(revision_after > revision);
    assert!(
        h.editor.commit(&h.ctx, &h.engine).is_none(),
        "nothing to commit"
    );
    Ok(())
}

/// Refused gestures are one-line toasts with an explicit severity that
/// follow the message style; the card has no notice text at all.
#[test]
fn refusals_are_styled_toasts() -> TestResult {
    let mut h = Harness::new()?;
    if let Some(set) = &mut h.editor.draft {
        set.component.terminals.truncate(1);
    }
    h.editor.selected = h.set()?.component.terminals.first().map(|t| t.id);
    h.editor.remove_selected();
    h.editor.tell(Level::Error, NO_MORE_TERMINALS);
    let posted = h.posted();
    assert_eq!(posted.len(), 2, "{posted:?}");
    assert!(posted.iter().all(|p| p.level == Level::Error));
    for post in &posted {
        let problems = pcbcraft_core::issues::style::check(&post.text);
        assert!(problems.is_empty(), "{:?}: {problems:?}", post.text);
    }
    Ok(())
}

/// The Details footer offers Update placed instances… only when nothing
/// is unpublished and placed instances use an older revision; it opens the
/// instance review.
#[test]
fn update_placed_instances_shows_only_for_older_instances() -> TestResult {
    let mut h = Harness::new()?;
    assert!(!h.editor.inspector_card(&h.engine).footer, "all current");
    // Publish r2: the demo's placed instances stay on r1.
    if let Some(set) = &mut h.editor.draft {
        set.footprint.pads[0].position = [-1.5, 0.0];
    }
    assert!(h.commit()?);
    assert!(
        !h.editor.inspector_card(&h.engine).footer,
        "not while unpublished"
    );
    let publish = h.editor.publish(&h.engine).ok_or("publish")?;
    h.engine.apply(h.engine.revision(), publish)?;
    h.editor.commit(&h.ctx, &h.engine);
    if h.editor.update_candidates(&h.engine).is_empty() {
        return Err("the demo places instances of the component".into());
    }
    assert!(h.editor.inspector_card(&h.engine).footer);
    h.click("Update placed instances…")?;
    assert!(h.editor.update.is_open(), "the review opens");
    assert_eq!(h.editor.header(&h.engine).kind, "Review");
    Ok(())
}

/// The Details tab's canvas previews the symbol: a click there selects
/// nothing and keys do nothing.
#[test]
fn the_details_preview_takes_no_input() -> TestResult {
    let mut h = Harness::new()?;
    let canvas = Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0));
    let frame = |h: &mut Harness, events: Vec<egui::Event>| {
        let mut output = h.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(canvas),
                events,
                ..Default::default()
            },
            |ui| {
                h.editor.canvas(ui, &h.engine);
            },
        );
        output.textures_delta.clear();
    };
    frame(&mut h, vec![]);
    let at = canvas.center();
    for pressed in [true, false] {
        frame(
            &mut h,
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }
    frame(
        &mut h,
        vec![egui::Event::Key {
            key: egui::Key::R,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert!(h.editor.selection.is_empty());
    assert_eq!(h.editor.drawing.tool(), Tool::Select, "R did not arm");
    assert!(h.editor.view == View::Details);
    Ok(())
}

/// The card regions a column draws separately give the same body and
/// footer as the one-body fallback.
#[test]
fn the_regions_split_body_and_footer() -> TestResult {
    let mut h = Harness::new()?;
    h.editor.on_tab("symbol");
    let terminal = h.set()?.component.terminals[0].id;
    h.editor.show_terminal(terminal);
    let mut body = Vec::new();
    let mut footer = Vec::new();
    for (region, out) in [(Region::Body, &mut body), (Region::Footer, &mut footer)] {
        let mut output = h.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(360.0, 2400.0))),
                ..Default::default()
            },
            |ui| {
                h.editor.inspector_region(ui, &h.engine, region);
            },
        );
        output.textures_delta.clear();
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.job.text.clone()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            texts(&shape.shape, out);
        }
    }
    assert!(body.iter().any(|t| t == "PIN · ELECTRICAL"));
    assert!(!body.iter().any(|t| t == "Remove pin"));
    assert_eq!(footer, ["Remove pin"]);
    Ok(())
}

/// An empty Component workspace's card (the one-hour run's 260-point
/// column): the hint wraps whole, and any text cut short shows in full on
/// hover. With `PCBCRAFT_TEXT_PICTURES=<dir>` it writes the card's picture.
#[test]
fn the_empty_card_hint_wraps_whole() -> Result<(), Box<dyn std::error::Error>> {
    let engine = Engine::demo()?;
    let mut editor = PartEditor::default();
    let screen = crate::kit::text::tests::Screen::new(egui::vec2(260.0, 400.0));
    let run = |editor: &mut PartEditor, events: Vec<egui::Event>| {
        screen.run(events, |ui| {
            editor.inspector(ui, &engine);
        })
    };
    run(&mut editor, vec![]);
    let rest = run(&mut editor, vec![]);
    let hint = "Open a component, or start a new one, on the start page";
    assert!(
        crate::kit::text::tests::shows_in_full(&rest, hint),
        "the hint is whole: {:?}",
        crate::kit::text::tests::cuts(&rest)
    );
    screen.picture("component-empty-card", &rest, 2.0)?;
    let (_, missing) =
        crate::kit::text::tests::cuts_without_hover(&screen.ctx, |events| run(&mut editor, events));
    assert!(missing.is_empty(), "cut without a hover: {missing:?}");
    Ok(())
}
