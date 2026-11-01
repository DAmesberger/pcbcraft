//! The Pins tab (slice C5), driven through real egui frames where the
//! behaviour is visible, and through its pure helpers where it is data:
//! the ribbon offers it between Symbol and Footprint; chips select and add
//! functions; the card edits a function's mux code; the Unreviewed filter
//! and Confirm row clear the import review; peripherals list the
//! functions that offer each role; rules get fresh IDs and an explain
//! panel; Paste table maps columns explicitly and plans through
//! `library.pinout_preview` without changing the document; and the Symbol
//! tab's terminal card hands off to the Pins tab.
use super::super::pins_table::{self, ColumnMap, Delimiter, Filter, Mapping};
use super::super::rules_editor::{self, Check};
use super::*;
use egui::{Pos2, Rect};
use pcbcraft_core::FunctionKey;
use pcbcraft_core::functions::{FunctionConstraint, FunctionSelections};
use pcbcraft_core::library::pinout::ImportReview;
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Texts = Vec<(String, Pos2)>;

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
        editor.on_tab("pins");
        Ok(Self {
            ctx,
            engine,
            editor,
            time: 0.0,
        })
    }

    /// One frame: the canvas on the left 900 points, the card on the
    /// right 360; the texts drawn and their centres, and the operations.
    fn frame(&mut self, events: Vec<egui::Event>) -> (Texts, Option<Operation>) {
        self.time += 0.05;
        let mut operation = None;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1260.0, 1400.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                let canvas = Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 1400.0));
                let card = Rect::from_min_size(Pos2::new(900.0, 0.0), egui::vec2(360.0, 1400.0));
                let a = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(canvas), |ui| {
                        self.editor.canvas(ui, &self.engine)
                    })
                    .inner;
                let b = ui
                    .scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
                        self.editor.inspector(ui, &self.engine)
                    })
                    .inner;
                operation = a.or(b);
            },
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
        fn texts(shape: &egui::Shape, out: &mut Texts) {
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
        self.frame(vec![]).0.into_iter().map(|(t, _)| t).collect()
    }

    /// Click the first drawn text `label`; the operation the click made.
    fn click(&mut self, label: &str) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let (shown, _) = self.frame(vec![]);
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
        self.click_at(at)
    }

    fn click_at(&mut self, at: Pos2) -> Result<Option<Operation>, Box<dyn std::error::Error>> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(at)]);
        self.frame(vec![button(true)]);
        let (_, operation) = self.frame(vec![button(false)]);
        Ok(operation)
    }

    /// Send the editor's pending commit and reload; whether one was sent.
    fn commit(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        self.frame(vec![egui::Event::PointerGone]);
        match self.editor.commit(&self.ctx, &self.engine) {
            Some(op) => {
                self.engine.apply(self.engine.revision(), op)?;
                self.editor.commit(&self.ctx, &self.engine);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn component(&self) -> Result<&ComponentDefinition, Box<dyn std::error::Error>> {
        Ok(&self
            .editor
            .draft
            .as_ref()
            .ok_or("no working set")?
            .component)
    }

    fn component_mut(&mut self) -> Result<&mut ComponentDefinition, Box<dyn std::error::Error>> {
        Ok(&mut self
            .editor
            .draft
            .as_mut()
            .ok_or("no working set")?
            .component)
    }

    /// The engine's draft of the component being edited.
    fn engine_component(&self) -> Result<ComponentDefinition, Box<dyn std::error::Error>> {
        let set = self.editor.draft.as_ref().ok_or("no working set")?;
        let fresh = super::super::working_set::WorkingSet::find(
            self.engine.drafts(),
            Some(set.ids.component),
        )
        .ok_or("draft not loadable")?;
        Ok(fresh.component)
    }

    fn terminals(&self) -> Result<Vec<TerminalId>, Box<dyn std::error::Error>> {
        Ok(self.component()?.terminals.iter().map(|t| t.id).collect())
    }
}

/// The ribbon offers Pins between Symbol and Footprint, and the tab opens
/// on the pin table with one row per terminal.
#[test]
fn pins_tab_sits_between_symbol_and_footprint_and_lists_terminals() -> TestResult {
    let mut h = Harness::new()?;
    let ribbon = h.editor.ribbon(&h.engine);
    let labels: Vec<&str> = ribbon.tabs.iter().map(|t| t.label).collect();
    let pins = labels
        .iter()
        .position(|l| *l == "Pins")
        .ok_or("no Pins tab")?;
    assert_eq!(labels.get(pins - 1), Some(&"Symbol"));
    assert_eq!(labels.get(pins + 1), Some(&"Footprint"));
    assert!(ribbon.tabs.iter().any(|t| t.label == "Pins" && t.active));
    assert_eq!(h.editor.located().0, "pins");
    let shown = h.texts();
    for caption in ["NAME", "CLASS", "GPIO", "PADS"] {
        assert!(shown.iter().any(|t| t == caption), "{caption} in {shown:?}");
    }
    for terminal in &h.component()?.terminals.clone() {
        assert!(shown.contains(&terminal.name), "row {}", terminal.name);
    }
    Ok(())
}

/// "+" on a row adds a function to that terminal and selects it; the card
/// then edits it, and the edit reaches the engine as one commit.
#[test]
fn plus_chip_adds_a_function_and_the_card_edits_it() -> TestResult {
    let mut h = Harness::new()?;
    let first = *h.terminals()?.first().ok_or("no terminal")?;
    let before = h
        .component()?
        .terminal(first)
        .and_then(|t| t.functions())
        .map_or(0, |f| f.functions.len());
    h.click("+")?;
    let functions = h
        .component()?
        .terminal(first)
        .and_then(|t| t.functions())
        .ok_or("not electrical")?;
    assert_eq!(functions.functions.len(), before + 1);
    let added = functions.functions.last().ok_or("no function")?.id;
    assert_eq!(h.editor.pins.subject, Some(Subject::Function(first, added)));
    let shown = h.texts();
    for row in ["FUNCTION", "Direction", "Peripheral", "Mux code"] {
        assert!(shown.iter().any(|t| t == row), "{row} in the card");
    }
    assert!(h.commit()?);
    let stored = h.engine_component()?;
    assert_eq!(
        stored
            .terminal(first)
            .and_then(|t| t.functions())
            .map(|f| f.functions.len()),
        Some(before + 1)
    );
    Ok(())
}

/// Clicking a chip selects that function; the mux code typed in the card
/// is stored as opaque data, and an over-long one is never stored.
#[test]
fn chip_selects_a_function_and_mux_code_is_bounded() -> TestResult {
    let mut h = Harness::new()?;
    let first = *h.terminals()?.first().ok_or("no terminal")?;
    let label = h
        .component()?
        .terminal(first)
        .and_then(|t| t.functions())
        .and_then(|f| f.functions.first())
        .map(|f| (f.id, f.label.clone()))
        .ok_or("no function")?;
    h.click(&label.1)?;
    assert_eq!(
        h.editor.pins.subject,
        Some(Subject::Function(first, label.0))
    );
    // Focus the Mux code field and type.
    let (shown, _) = h.frame(vec![]);
    let row = shown
        .iter()
        .find(|(t, _)| t == "Mux code")
        .map(|(_, p)| *p)
        .ok_or("no Mux code row")?;
    let field = Pos2::new(1180.0, row.y);
    h.click_at(field)?;
    h.frame(vec![egui::Event::Text("AF7".into())]);
    let mux = |h: &Harness| -> Result<Option<String>, Box<dyn std::error::Error>> {
        Ok(h.component()?
            .terminal(first)
            .and_then(|t| t.functions())
            .and_then(|f| f.function(label.0))
            .and_then(|f| f.mux.as_ref().map(|m| m.0.clone())))
    };
    assert_eq!(mux(&h)?.as_deref(), Some("AF7"));
    h.frame(vec![egui::Event::Text("XXXXXXXXXXXXXXXXXX".into())]);
    assert_eq!(mux(&h)?.as_deref(), Some("AF7"), "17+ bytes are not stored");
    Ok(())
}

/// The Unreviewed filter shows only imported rows nobody confirmed;
/// Confirm row removes the row from the review, and the empty review is
/// dropped from the stored component.
#[test]
fn unreviewed_filter_and_confirm_row_clear_the_import_review() -> TestResult {
    let mut h = Harness::new()?;
    let ids = h.terminals()?;
    let (first, second) = match ids.as_slice() {
        [a, b, ..] => (*a, *b),
        _ => return Err("two terminals expected".into()),
    };
    h.component_mut()?.review = Some(Box::new(ImportReview {
        unreviewed: [first].into_iter().collect(),
        uncertain: Default::default(),
    }));
    assert!(h.commit()?);
    assert!(h.engine_component()?.review.is_some());
    h.click("Unreviewed 1")?;
    assert_eq!(h.editor.pins.table.filter, Filter::Unreviewed);
    let names: Vec<String> = h
        .component()?
        .terminals
        .iter()
        .map(|t| t.name.clone())
        .collect();
    let component = h.component()?.clone();
    let shown: Vec<bool> = component
        .terminals
        .iter()
        .map(|t| pins_table::matches(&component, t, Filter::Unreviewed, ""))
        .collect();
    assert_eq!(shown, vec![true, false], "{names:?}");
    h.editor.pins.subject = Some(Subject::Terminal(first));
    assert!(
        h.texts()
            .iter()
            .any(|t| t == "Imported and not reviewed yet")
    );
    h.click("Confirm row")?;
    assert!(h.component()?.review.is_none());
    assert!(h.commit()?);
    assert!(h.engine_component()?.review.is_none());
    let _ = second;
    Ok(())
}

/// A peripheral's roles list the functions that offer them, across
/// terminals; removing the peripheral clears every claim on it.
#[test]
fn peripherals_view_lists_offers_and_removal_clears_claims() -> TestResult {
    let mut h = Harness::new()?;
    let first = *h.terminals()?.first().ok_or("no terminal")?;
    let component = h.component_mut()?;
    let resource = add_peripheral(component)?;
    let role = component
        .resources
        .iter()
        .find(|r| r.id == resource)
        .and_then(|r| r.roles.first())
        .map(|r| r.id)
        .ok_or("no role")?;
    let function = add_function(component, first)?;
    if let Some(pcbcraft_core::library::TerminalClass::Electrical(f)) = component
        .terminals
        .iter_mut()
        .find(|t| t.id == first)
        .map(|t| &mut t.class)
        && let Some(f) = f.functions.iter_mut().find(|f| f.id == function)
    {
        f.resource_role = Some(ResourceRole { resource, role });
    }
    let offers = pins_table::offers(h.component()?);
    assert_eq!(
        offers.get(&(resource, role)),
        Some(&vec![(first, function)])
    );
    h.click("Peripherals")?;
    let shown = h.texts();
    assert!(shown.iter().any(|t| t == "peripheral 1"), "{shown:?}");
    assert!(shown.iter().any(|t| t.starts_with("role 1")), "{shown:?}");
    remove_peripheral(h.component_mut()?, resource, None);
    let cleared = h
        .component()?
        .terminal(first)
        .and_then(|t| t.functions())
        .and_then(|f| f.function(function))
        .map(|f| f.resource_role);
    assert_eq!(cleared, Some(None));
    Ok(())
}

/// Add rule gives each rule a fresh component-local ID and a unique label
/// with the starter script; the Rules view edits it in the canvas.
#[test]
fn add_rule_allocates_fresh_ids_and_unique_labels() -> TestResult {
    let mut h = Harness::new()?;
    h.click("Rules")?;
    h.click("Add rule")?;
    h.click("Add rule")?;
    let rules = &h.component()?.pin_rules;
    assert_eq!(rules.len(), 2);
    let labels: Vec<&str> = rules.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(labels, vec!["rule 1", "rule 2"]);
    assert!(
        rules
            .iter()
            .all(|r| r.script.source == rules_editor::NEW_RULE_SOURCE)
    );
    let ids: std::collections::BTreeSet<_> = rules.iter().map(|r| r.id).collect();
    assert_eq!(ids.len(), 2);
    let second = rules.get(1).map(|r| r.id).ok_or("no rule")?;
    assert_eq!(h.editor.pins.subject, Some(Subject::Rule(second)));
    let shown = h.texts();
    for text in ["SCRIPT", "EXPLAIN", "RULE"] {
        assert!(shown.iter().any(|t| t == text), "{text} in {shown:?}");
    }
    Ok(())
}

/// The syntax check never calls a broken or empty script compiled, and
/// bounds the source.
#[test]
fn syntax_check_rejects_empty_oversized_and_broken_scripts() {
    let rule = |source: &str| PinRule {
        id: RuleId(7),
        label: "r".into(),
        script: PinRuleScript {
            source: source.into(),
        },
        source: None,
    };
    assert!(matches!(rules_editor::check(&rule("  ")), Check::Error(_)));
    let long = "x".repeat(pcbcraft_core::functions::rules::MAX_RULE_SOURCE + 1);
    assert!(matches!(rules_editor::check(&rule(&long)), Check::Error(_)));
    assert_ne!(rules_editor::check(&rule("forbid(")), Check::Compiles);
}

/// The explain panel resolves a scratch selection without touching the
/// document: an `Excludes` pair active together is reported rejected, and
/// the rule view names active peripheral roles.
#[test]
fn explain_resolves_a_scratch_selection_and_builds_the_rule_view() -> TestResult {
    let mut h = Harness::new()?;
    let ids = h.terminals()?;
    let (a, b) = match ids.as_slice() {
        [a, b, ..] => (*a, *b),
        _ => return Err("two terminals expected".into()),
    };
    let component = h.component_mut()?;
    let fa = add_function(component, a)?;
    let fb = add_function(component, b)?;
    let resource = add_peripheral(component)?;
    let role = component
        .resources
        .first()
        .and_then(|r| r.roles.first())
        .map(|r| r.id)
        .ok_or("no role")?;
    if let Some(pcbcraft_core::library::TerminalClass::Electrical(f)) = component
        .terminals
        .iter_mut()
        .find(|t| t.id == a)
        .map(|t| &mut t.class)
        && let Some(f) = f.functions.iter_mut().find(|f| f.id == fa)
    {
        f.resource_role = Some(ResourceRole { resource, role });
    }
    component.constraints.push(FunctionConstraint::Excludes {
        a: FunctionKey {
            terminal: a,
            function: fa,
        },
        b: FunctionKey {
            terminal: b,
            function: fb,
        },
    });
    let component = h.component()?.clone();
    let scratch: FunctionSelections = [(a, fa), (b, fb)].into_iter().collect();
    let explained = rules_editor::explain(&component, &scratch);
    let resolution = explained.resolution.map_err(|e| e.to_string())?;
    assert_eq!(resolution.conflicts.len(), 1);
    let view = rules_editor::rule_view(&component, &resolution);
    let ta = view.terminals.iter().find(|t| t.terminal == a).ok_or("a")?;
    assert_eq!(ta.active.as_ref().map(|f| f.function), Some(fa));
    assert_eq!(
        view.resources.first().map(|r| r.active_roles.clone()),
        Some(vec!["role 1".to_owned()])
    );
    // A selection that names a function no longer declared is dropped.
    let stale: FunctionSelections = [(a, FunctionId(9999))].into_iter().collect();
    assert!(rules_editor::valid_scratch(&component, &stale).is_empty());
    Ok(())
}

/// Pasted text splits at the chosen delimiter with quotes honoured, and
/// is bounded.
#[test]
fn pasted_tables_split_with_quotes_and_bounds() -> TestResult {
    let rows = pins_table::split_table("1,\"PA9, alt\",X\n\n2,PA10,\"a\"\"b\"\n", Delimiter::Auto)?;
    assert_eq!(
        rows,
        vec![
            vec!["1".to_owned(), "PA9, alt".into(), "X".into()],
            vec!["2".to_owned(), "PA10".into(), "a\"b".into()],
        ]
    );
    let tabbed = pins_table::split_table("1\tPA9;x\n", Delimiter::Auto)?;
    assert_eq!(tabbed, vec![vec!["1".to_owned(), "PA9;x".into()]]);
    let wide = vec!["c"; pins_table::MAX_PASTE_COLUMNS + 1].join(",");
    assert!(pins_table::split_table(&wide, Delimiter::Comma).is_err());
    Ok(())
}

/// The document comes only from the mapping: no peripheral without the
/// person's role separator, peripherals and roles with it, row problems
/// reported, and no document without provenance.
#[test]
fn paste_document_follows_the_explicit_mapping() -> TestResult {
    let rows = pins_table::split_table(
        "Pin;Name;AF1;AF7\n1;PA9;TIM1_CH2;USART1_TX\n;NOPE;x;y\n1;DUP;a;b\n2;PA10;TIM1_CH3;USART1_RX\n",
        Delimiter::Auto,
    )?;
    let mut mapping = Mapping {
        columns: pins_table::initial_columns(4),
        source_title: "EXM datasheet".into(),
        page: "41".into(),
        ..Mapping::default()
    };
    let (document, problems) = pins_table::document(&rows, &mapping, "EXM", "QFN-48");
    let document = document.ok_or("no document")?;
    assert_eq!(document.pins.len(), 2);
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(document.peripherals.is_empty(), "nothing split by name");
    let first = document.pins.first().ok_or("no pin")?;
    assert_eq!(first.number.get("QFN-48").map(String::as_str), Some("1"));
    assert_eq!(first.functions.len(), 2);
    assert!(first.functions.iter().all(|f| f.peripheral.is_none()));
    assert_eq!(first.source.as_ref().and_then(|s| s.page), Some(41));
    assert_eq!(document.variants.first().map(|v| v.terminal_count), Some(2));

    mapping.role_separator = "_".into();
    let (document, _) = pins_table::document(&rows, &mapping, "EXM", "QFN-48");
    let document = document.ok_or("no document")?;
    let labels: Vec<&str> = document
        .peripherals
        .iter()
        .map(|p| p.label.as_str())
        .collect();
    assert_eq!(labels, vec!["TIM1", "USART1"]);
    let usart = document.peripherals.get(1).ok_or("no USART1")?;
    let roles: Vec<&str> = usart.roles.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(roles, vec!["TX", "RX"]);

    mapping.source_title.clear();
    assert!(
        pins_table::document(&rows, &mapping, "EXM", "QFN-48")
            .0
            .is_none()
    );
    mapping.source_title = "EXM".into();
    mapping.columns = vec![
        ColumnMap::Name,
        ColumnMap::Ignore,
        ColumnMap::Function,
        ColumnMap::Function,
    ];
    assert!(
        pins_table::document(&rows, &mapping, "EXM", "QFN-48")
            .0
            .is_none(),
        "no Number column"
    );
    Ok(())
}

/// Pin type, mux and GPIO columns: power rows import as power inputs
/// (with a function named after the pin when none is listed), mux codes
/// pair with functions in order or come from a split cell, and the GPIO
/// identity is taken only from its mapped columns.
#[test]
fn paste_document_maps_type_mux_and_gpio_columns() -> TestResult {
    use pcbcraft_core::functions::{Direction, SignalKind};
    use pcbcraft_core::library::pinout::PinoutClass;
    let rows = pins_table::split_table(
        "Pin;Name;Type;Port;Index;Functions;AF
         1;VDD;power;;;;
         2;PA9;I/O;A;9;GPIO,USART1_TX;,AF7
         3;NC1;nc;;;;
         4;PA10;I/O;A;;GPIO;
         5;PB0;bogus;;;;
",
        Delimiter::Semicolon,
    )?;
    let mapping = Mapping {
        columns: vec![
            ColumnMap::Number,
            ColumnMap::Name,
            ColumnMap::Type,
            ColumnMap::Port,
            ColumnMap::Index,
            ColumnMap::Function,
            ColumnMap::Mux,
        ],
        list_separator: ",".into(),
        source_title: "EXM datasheet".into(),
        ..Mapping::default()
    };
    let (document, problems) = pins_table::document(&rows, &mapping, "EXM", "QFN-48");
    let document = document.ok_or("no document")?;
    let messages: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
    assert_eq!(document.pins.len(), 3, "{messages:?}");
    assert!(
        messages
            .iter()
            .any(|m| m.contains("GPIO port or index without the other"))
    );
    assert!(messages.iter().any(|m| m.contains("pin type is not")));
    let vdd = &document.pins[0];
    assert_eq!(vdd.functions.len(), 1);
    assert_eq!(vdd.functions[0].label, "VDD");
    assert_eq!(vdd.functions[0].direction, Direction::PowerInput);
    assert_eq!(vdd.functions[0].signal, SignalKind::Power);
    let pa9 = &document.pins[1];
    assert_eq!(
        pa9.gpio.as_ref().map(|g| (g.port.as_str(), g.index)),
        Some(("A", 9))
    );
    let muxes: Vec<Option<&str>> = pa9.functions.iter().map(|f| f.mux.as_deref()).collect();
    assert_eq!(muxes, [None, Some("AF7")]);
    assert_eq!(pa9.functions[1].direction, Direction::Bidirectional);
    assert_eq!(document.pins[2].class, PinoutClass::DeviceNc);
    assert!(document.pins[2].functions.is_empty());

    // A mux code split out of the function cell instead.
    let rows = pins_table::split_table(
        "1;PA9;USART1_TX:AF7
",
        Delimiter::Semicolon,
    )?;
    let mapping = Mapping {
        columns: vec![ColumnMap::Number, ColumnMap::Name, ColumnMap::Function],
        header: false,
        mux_separator: ":".into(),
        source_title: "EXM datasheet".into(),
        ..Mapping::default()
    };
    let (document, _) = pins_table::document(&rows, &mapping, "EXM", "QFN-48");
    let function = document
        .as_ref()
        .and_then(|d| d.pins.first())
        .and_then(|p| p.functions.first())
        .ok_or("no function")?;
    assert_eq!(function.label, "USART1_TX");
    assert_eq!(function.mux.as_deref(), Some("AF7"));
    Ok(())
}

/// Unticked preview rows are applied as `PinoutAccept::Except`.
#[test]
fn unticked_paste_rows_are_left_out_of_the_apply() {
    use pcbcraft_core::library::pinout::PinoutAccept;
    let mut flow = pins_table::PasteFlow::new();
    assert_eq!(flow.accept(), PinoutAccept::All);
    flow.rejected.insert("2".into());
    assert_eq!(flow.accept(), PinoutAccept::Except(vec!["2".into()]));
}

/// Paste table → Preview goes through `library.pinout_preview`: it shows
/// the plan or the engine's refusal in the dialog and never changes the
/// document; Cancel closes it.
#[test]
fn paste_preview_plans_without_changing_the_document() -> TestResult {
    let mut h = Harness::new()?;
    h.click("Paste table…")?;
    let flow = h.editor.pins.table.paste.as_mut().ok_or("not open")?;
    flow.text = "Pin;Name;AF7\n1;PA9;USART1_TX\n2;PA10;USART1_RX\n".into();
    flow.mapping.source_title = "EXM datasheet".into();
    let revision = h.engine.revision();
    let operation = h.click("Preview")?;
    assert!(operation.is_none(), "a preview applies nothing");
    assert_eq!(h.engine.revision(), revision);
    let flow = h.editor.pins.table.paste.as_ref().ok_or("closed")?;
    assert!(
        flow.plan.is_some() || flow.refusal.is_some(),
        "the preview shows a plan or a refusal"
    );
    h.click("Cancel")?;
    assert!(h.editor.pins.table.paste.is_none());
    Ok(())
}

/// The Symbol tab's terminal card keeps name, class and a function
/// summary, and "Edit in Pins tab" opens that terminal in the Pins tab.
#[test]
fn symbol_terminal_card_hands_off_to_the_pins_tab() -> TestResult {
    let mut h = Harness::new()?;
    let first = *h.terminals()?.first().ok_or("no terminal")?;
    h.editor.on_tab("symbol");
    h.editor.selection.clear();
    h.editor.terminal_card = Some(first);
    let shown = h.texts();
    assert!(
        shown.iter().any(|t| t.starts_with("1 function")),
        "{shown:?}"
    );
    h.click("Edit in Pins tab")?;
    assert_eq!(h.editor.located().0, "pins");
    assert_eq!(h.editor.pins.subject, Some(Subject::Terminal(first)));
    Ok(())
}

/// Column widths never run past the width they are given.
#[test]
fn table_columns_fit_their_width() {
    for width in [0.0_f32, 200.0, 480.0, 700.0, 1400.0] {
        let total: f32 = pins_table::widths(width).iter().sum();
        assert!(total <= width + 0.01, "{total} > {width}");
    }
}

/// Options are data: pulls, speeds and flags round-trip through the card's
/// helpers.
#[test]
fn speeds_are_split_as_data() {
    assert_eq!(
        speed_values(" low, high ,,very_high"),
        vec!["low".to_owned(), "high".into(), "very_high".into()]
    );
}
