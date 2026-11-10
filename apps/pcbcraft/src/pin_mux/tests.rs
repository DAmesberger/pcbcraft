//! Pin-mux panel tests: the model and preview against a published
//! two-peripheral component (original fixture), the Apply batch and its
//! undo, rejected combinations, and the panel driven through headless
//! egui frames.
use super::model::{self, Candidate, Model, Staged};
use super::*;
use egui::{Event, Pos2, RawInput};
use pcbcraft_core::functions::{
    Conflict, Direction, Drive, Function, FunctionConstraint, FunctionEdit, Resolution, Resource,
    ResourceRole, Role, SignalKind, TerminalFunctions,
};
use pcbcraft_core::instances::NewInstance;
use pcbcraft_core::library::{Definition, DraftEdit, TerminalClass};
use pcbcraft_core::{
    ComponentRef, FunctionId, FunctionKey, LibraryCommand, Outcome, PhysicalChoice, Revision,
    RuleId,
};
use solidcraft_ui_egui::fonts;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The fixture's IDs, allocated from the component's local counter.
#[derive(Clone, Copy, Debug)]
struct Ids {
    u1: Id,
    passive: [FunctionId; 2],
    uart_tx: FunctionId,
    uart_rx: FunctionId,
    spi_sck: FunctionId,
    spi_mosi: FunctionId,
    usart1: ResourceId,
    tx: RoleId,
    rx: RoleId,
    spi1: ResourceId,
    sck: RoleId,
    mosi: RoleId,
}

const T1: TerminalId = TerminalId(1);
const T2: TerminalId = TerminalId(2);

fn function(
    id: FunctionId,
    label: &str,
    mux: Option<&str>,
    role: Option<(ResourceId, RoleId)>,
) -> Function {
    let (direction, drive) = if role.is_some() {
        (Direction::Bidirectional, Drive::PushPull)
    } else {
        (Direction::Passive, Drive::None)
    };
    Function {
        id,
        label: label.into(),
        direction,
        signal: if role.is_some() {
            SignalKind::Digital
        } else {
            SignalKind::Unspecified
        },
        drive,
        resource_role: role.map(|(resource, role)| ResourceRole { resource, role }),
        mux: mux.map(|m| pcbcraft_core::functions::MuxCode(m.into())),
        mux_table: None,
        source: None,
    }
}

/// Publish r2 of the demo resistor as a synthetic two-pin controller:
/// terminal 1 is `passive` (default), `uart_tx` (USART1 TX, AF7) or
/// `spi_sck` (SPI1 SCK); terminal 2 is `passive`, `uart_rx` (USART1 RX,
/// AF7) or `spi_mosi` (SPI1 MOSI). `uart_tx` excludes `spi_mosi`. Every
/// role is required once its peripheral is used. Placed as U1.
fn fixture(engine: &mut Engine) -> Result<Ids, Box<dyn std::error::Error>> {
    let r1 = engine.project().instances[0].component;
    let id = r1.id;
    if engine.drafts().get(id).is_some() {
        engine.apply(
            engine.revision(),
            Operation::Library(LibraryCommand::DiscardDraft { id }),
        )?;
    }
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::DraftFrom {
            revision: r1.erased(),
        }),
    )?;
    let draft = engine.drafts().get(id).cloned().ok_or("no draft")?;
    let mut definition = draft.definition;
    let Definition::Component(component) = &mut definition else {
        return Err("not a component draft".into());
    };
    let base = component.next_local;
    component.next_local = base + 12;
    let f = |n: u32| FunctionId(base + n);
    let ids = Ids {
        u1: Id::default(),
        passive: [f(0), f(1)],
        uart_tx: f(2),
        uart_rx: f(3),
        spi_sck: f(4),
        spi_mosi: f(5),
        usart1: ResourceId(base + 6),
        tx: RoleId(base + 7),
        rx: RoleId(base + 8),
        spi1: ResourceId(base + 9),
        sck: RoleId(base + 10),
        mosi: RoleId(base + 11),
    };
    let role = |id: RoleId, label: &str| Role {
        id,
        label: label.into(),
        capacity: 1,
        required_when_used: true,
        signal: None,
    };
    component.resources = vec![
        Resource {
            id: ids.usart1,
            label: "USART1".into(),
            kind: Some("uart".into()),
            roles: vec![role(ids.tx, "TX"), role(ids.rx, "RX")],
        },
        Resource {
            id: ids.spi1,
            label: "SPI1".into(),
            kind: Some("spi".into()),
            roles: vec![role(ids.sck, "SCK"), role(ids.mosi, "MOSI")],
        },
    ];
    component.constraints = vec![FunctionConstraint::Excludes {
        a: FunctionKey {
            terminal: T1,
            function: ids.uart_tx,
        },
        b: FunctionKey {
            terminal: T2,
            function: ids.spi_mosi,
        },
    }];
    for terminal in &mut component.terminals {
        let functions = if terminal.id == T1 {
            vec![
                function(ids.passive[0], "passive", None, None),
                function(
                    ids.uart_tx,
                    "uart_tx",
                    Some("AF7"),
                    Some((ids.usart1, ids.tx)),
                ),
                function(
                    ids.spi_sck,
                    "spi_sck",
                    Some("AF5"),
                    Some((ids.spi1, ids.sck)),
                ),
            ]
        } else if terminal.id == T2 {
            vec![
                function(ids.passive[1], "passive", None, None),
                function(
                    ids.uart_rx,
                    "uart_rx",
                    Some("AF7"),
                    Some((ids.usart1, ids.rx)),
                ),
                function(
                    ids.spi_mosi,
                    "spi_mosi",
                    Some("AF5"),
                    Some((ids.spi1, ids.mosi)),
                ),
            ]
        } else {
            continue;
        };
        let default = functions.first().map(|f| f.id);
        terminal.class = TerminalClass::Electrical(TerminalFunctions {
            functions,
            design_default: default,
            gpio: None,
            options: Vec::new(),
        });
    }
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Edit {
            edits: vec![DraftEdit {
                id,
                meta: draft.meta,
                definition,
            }],
        }),
    )?;
    engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Publish {
            drafts: vec![id],
            close: false,
        }),
    )?;
    let r2 = ComponentRef::new(id, Revision(2));
    let library = &engine.project().library;
    let published = library.component(r2).ok_or("no r2")?;
    let symbol = *published
        .definition
        .representations
        .first()
        .ok_or("no symbol")?;
    let unit = library
        .symbol(symbol)
        .and_then(|s| s.definition.units.first())
        .ok_or("no unit")?
        .id;
    let physical = published.definition.implementations.first().and_then(|b| {
        let binding = library.binding(*b)?;
        Some(PhysicalChoice {
            binding: b.id,
            footprint: binding.definition.footprints.first()?.id,
        })
    });
    let outcome = engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::Place(NewInstance {
            sheet: None,
            component: r2,
            designator: Some("U1".into()),
            properties: Vec::new(),
            representation: symbol.id,
            unit,
            position: crate::schematic_editor::pt([0.0, 20.0]),
            physical,
        })),
    )?;
    let Outcome::InstancePlaced { instance, .. } = outcome else {
        return Err(format!("unexpected outcome {outcome:?}").into());
    };
    Ok(Ids {
        u1: instance,
        ..ids
    })
}

fn setup() -> Result<(Engine, Ids), Box<dyn std::error::Error>> {
    let mut engine = Engine::demo()?;
    let ids = fixture(&mut engine)?;
    Ok((engine, ids))
}

fn instance(engine: &Engine, id: Id) -> Result<ComponentInstance, Box<dyn std::error::Error>> {
    Ok(engine.project().instance(id).ok_or("no instance")?.clone())
}

fn built(
    engine: &Engine,
    ids: Ids,
) -> Result<(ComponentInstance, Model), Box<dyn std::error::Error>> {
    let instance = instance(engine, ids.u1)?;
    let model = model::build(engine.project(), &instance)?;
    Ok((instance, model))
}

#[test]
fn candidates_come_from_declared_roles_and_pads_from_the_footprint() -> TestResult {
    let (engine, ids) = setup()?;
    let (_, model) = built(&engine, ids)?;
    assert_eq!(model.designator, "U1");
    let labels: Vec<&str> = model.peripherals.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["USART1", "SPI1"]);
    let usart = &model.peripherals[0];
    assert_eq!(
        usart.roles[0].candidates,
        [Candidate {
            terminal: T1,
            function: ids.uart_tx
        }]
    );
    assert_eq!(
        usart.roles[1].candidates,
        [Candidate {
            terminal: T2,
            function: ids.uart_rx
        }]
    );
    assert_eq!(
        model::candidate_label(&model, &usart.roles[0].candidates[0]),
        format!("{} · uart_tx · AF7", model.pins[0].name)
    );
    // The demo footprint lands both terminals; each pad keeps its terminal.
    assert!(model.physical);
    let landed: Vec<Option<TerminalId>> = model.pads.iter().map(|p| p.terminal).collect();
    assert!(
        landed.contains(&Some(T1)) && landed.contains(&Some(T2)),
        "{landed:?}"
    );
    // Nothing is in use: both terminals take their passive default.
    assert!(model.current.conflicts.is_empty());
    Ok(())
}

#[test]
fn placing_both_roles_previews_two_changes_and_applies_as_one_undo_step() -> TestResult {
    let (mut engine, ids) = setup()?;
    let (instance, model) = built(&engine, ids)?;
    let mut staged = Staged::new();
    let empty = model::preview(engine.project(), &instance, &model, &staged)?;
    assert_eq!(
        empty.gate(),
        Some("Nothing to apply: choose peripherals or pins first")
    );
    let tx = model.peripherals[0].roles[0].candidates[0].clone();
    model::place(&model, &empty, ids.usart1, ids.tx, Some(&tx), &mut staged);
    let half = model::preview(engine.project(), &instance, &model, &staged)?;
    // TX without RX is incomplete, never rejected: Apply is allowed and
    // the requirement is explained.
    assert_eq!(half.changed, [T1]);
    assert!(half.gate().is_none());
    assert!(
        half.lines
            .iter()
            .any(|l| l.level == Level::Warning && l.terminals.contains(&T1)),
        "{:?}",
        half.lines
    );
    let rx = model.peripherals[0].roles[1].candidates[0].clone();
    model::place(&model, &half, ids.usart1, ids.rx, Some(&rx), &mut staged);
    let full = model::preview(engine.project(), &instance, &model, &staged)?;
    assert_eq!(full.changed, [T1, T2]);
    assert!(full.lines.is_empty(), "{:?}", full.lines);
    assert_eq!(
        full.placements(&model, ids.usart1),
        [(ids.tx, T1), (ids.rx, T2)]
    );
    assert_eq!(
        full.edits,
        [
            FunctionEdit {
                terminal: T1,
                function: Some(ids.uart_tx)
            },
            FunctionEdit {
                terminal: T2,
                function: Some(ids.uart_rx)
            },
        ]
    );

    // Apply through the panel's handler: one select_functions batch.
    let mut state = State::default();
    let operation = handle(&mut state, &engine, &instance, &model, &full, Action::Apply)
        .ok_or("Apply sent nothing")?;
    let Operation::Instance(InstanceCommand::SelectFunctions {
        instance: target,
        edits,
    }) = &operation
    else {
        return Err(format!("expected instance.select_functions, got {operation:?}").into());
    };
    assert_eq!(*target, ids.u1);
    assert_eq!(edits, &full.edits);
    let before = engine.project().clone();
    engine.apply(engine.revision(), operation)?;
    let after = instance_of(&engine, ids.u1)?;
    assert_eq!(after.functions.get(&T1), Some(&ids.uart_tx));
    assert_eq!(after.functions.get(&T2), Some(&ids.uart_rx));
    // Applied staging settles: nothing is left to apply.
    model::settle(&after, &mut staged);
    assert!(staged.is_empty());
    engine.undo()?;
    assert_eq!(engine.project(), &before);
    Ok(())
}

fn instance_of(engine: &Engine, id: Id) -> Result<ComponentInstance, Box<dyn std::error::Error>> {
    instance(engine, id)
}

#[test]
fn a_rejected_combination_is_explained_inline_and_never_applied() -> TestResult {
    let (engine, ids) = setup()?;
    let (instance, model) = built(&engine, ids)?;
    let staged: Staged = [(T1, Some(ids.uart_tx)), (T2, Some(ids.spi_mosi))]
        .into_iter()
        .collect();
    let preview = model::preview(engine.project(), &instance, &model, &staged)?;
    assert_eq!(preview.resolution.conflicts.len(), 1);
    let first = preview.lines.first().ok_or("no line")?;
    assert_eq!(first.level, Level::Error);
    assert_eq!(first.terminals, [T1, T2]);
    assert!(!first.text.is_empty() && first.text.chars().count() <= 80);
    assert_eq!(
        preview.gate(),
        Some("Resolve the rejected combination above to apply")
    );
    let mut state = State::default();
    assert!(
        handle(
            &mut state,
            &engine,
            &instance,
            &model,
            &preview,
            Action::Apply
        )
        .is_none()
    );
    Ok(())
}

#[test]
fn unticking_a_peripheral_releases_its_pins() -> TestResult {
    let (mut engine, ids) = setup()?;
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SelectFunctions {
            instance: ids.u1,
            edits: vec![
                FunctionEdit {
                    terminal: T1,
                    function: Some(ids.uart_tx),
                },
                FunctionEdit {
                    terminal: T2,
                    function: Some(ids.uart_rx),
                },
            ],
        }),
    )?;
    let (instance, model) = built(&engine, ids)?;
    let mut staged = Staged::new();
    let current = model::preview(engine.project(), &instance, &model, &staged)?;
    assert_eq!(current.placements(&model, ids.usart1).len(), 2);
    let stuck = model::disable(&model, &current, ids.usart1, &mut staged);
    assert!(stuck.is_empty());
    let released = model::preview(engine.project(), &instance, &model, &staged)?;
    assert!(released.placements(&model, ids.usart1).is_empty());
    assert_eq!(released.changed, [T1, T2]);
    // Clearing returns each terminal to its passive design default.
    assert_eq!(
        released.edits,
        [
            FunctionEdit {
                terminal: T1,
                function: None
            },
            FunctionEdit {
                terminal: T2,
                function: None
            },
        ]
    );
    assert!(released.gate().is_none());
    Ok(())
}

#[test]
fn moving_a_role_releases_its_old_pin() -> TestResult {
    let (engine, ids) = setup()?;
    let (instance, model) = built(&engine, ids)?;
    // SPI1 SCK on terminal 1, then the role cleared: terminal 1 is back to
    // passive and nothing is staged.
    let mut staged = Staged::new();
    let empty = model::preview(engine.project(), &instance, &model, &staged)?;
    let sck = Candidate {
        terminal: T1,
        function: ids.spi_sck,
    };
    model::place(&model, &empty, ids.spi1, ids.sck, Some(&sck), &mut staged);
    let placed = model::preview(engine.project(), &instance, &model, &staged)?;
    assert_eq!(placed.placements(&model, ids.spi1), [(ids.sck, T1)]);
    model::place(&model, &placed, ids.spi1, ids.sck, None, &mut staged);
    let cleared = model::preview(engine.project(), &instance, &model, &staged)?;
    assert!(cleared.placements(&model, ids.spi1).is_empty());
    assert!(cleared.edits.is_empty());
    Ok(())
}

#[test]
fn a_rule_verdict_is_explained_with_its_rule_label() -> TestResult {
    let (engine, ids) = setup()?;
    let (_, mut model) = built(&engine, ids)?;
    model.rules.insert(RuleId(7), "swd_free".into());
    let resolution = Resolution {
        conflicts: vec![Conflict::Rule {
            rule: RuleId(7),
            terminals: vec![T2],
            message: "SWD pins stay free while debugging".into(),
        }],
        notes: vec![pcbcraft_core::functions::RuleNote {
            rule: RuleId(8),
            message: "Using this pin as GPIO disables debugging".into(),
            terminals: vec![T1],
        }],
        ..Resolution::default()
    };
    let lines = model::explain(&model, &resolution);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].level, Level::Error);
    assert_eq!(
        lines[0].text,
        "swd_free: SWD pins stay free while debugging"
    );
    assert_eq!(lines[0].terminals, [T2]);
    // A rule without a label (unknown here) keeps the core's message.
    assert_eq!(lines[1].level, Level::Info);
    assert_eq!(lines[1].text, "Using this pin as GPIO disables debugging");
    Ok(())
}

#[test]
fn cycling_steps_through_alternatives_over_the_base_and_wraps() -> TestResult {
    let (engine, ids) = setup()?;
    let (instance, model) = built(&engine, ids)?;
    let preview = model::preview(engine.project(), &instance, &model, &Staged::new())?;
    let base: Staged = [(T2, Some(ids.uart_rx))].into_iter().collect();
    let suggestion = |function| Suggestion {
        edits: vec![FunctionEdit {
            terminal: T1,
            function: Some(function),
        }],
        changed: vec![T1],
        optional: vec![],
        unplaced: vec![],
    };
    let mut state = State {
        staged: model::stage_suggestion(&base, &suggestion(ids.uart_tx).edits),
        alternatives: Some(Alternatives {
            suggestions: vec![suggestion(ids.uart_tx), suggestion(ids.spi_sck)],
            index: 0,
            base: base.clone(),
            exhausted: false,
        }),
        ..State::default()
    };
    handle(
        &mut state,
        &engine,
        &instance,
        &model,
        &preview,
        Action::Cycle(1),
    );
    assert_eq!(state.staged.get(&T1), Some(&Some(ids.spi_sck)));
    assert_eq!(state.staged.get(&T2), Some(&Some(ids.uart_rx)));
    handle(
        &mut state,
        &engine,
        &instance,
        &model,
        &preview,
        Action::Cycle(1),
    );
    assert_eq!(state.staged.get(&T1), Some(&Some(ids.uart_tx)));
    handle(
        &mut state,
        &engine,
        &instance,
        &model,
        &preview,
        Action::Cycle(-1),
    );
    assert_eq!(state.staged.get(&T1), Some(&Some(ids.spi_sck)));
    Ok(())
}

#[test]
fn ticking_asks_the_engine_for_a_suggestion() -> TestResult {
    let (engine, ids) = setup()?;
    let (instance, model) = built(&engine, ids)?;
    let preview = model::preview(engine.project(), &instance, &model, &Staged::new())?;
    let mut state = State::default();
    handle(
        &mut state,
        &engine,
        &instance,
        &model,
        &preview,
        Action::Tick(ids.usart1, true),
    );
    // The search runs on a worker thread; wait for it.
    assert!(
        state.pending.is_some(),
        "the suggestion runs off the UI thread"
    );
    poll(&mut state, true);
    assert!(state.pending.is_none());
    assert!(state.expanded.contains(&ids.usart1));
    let request = SuggestRequest {
        enable: vec![PeripheralRequest {
            peripheral: ids.usart1,
            roles: None,
        }],
        keep_existing: true,
        avoid: Vec::new(),
        limit: Some(MAX_SUGGESTIONS),
    };
    match engine.query(Query::SuggestPins {
        instance: ids.u1,
        request,
    }) {
        // The solver's first suggestion is staged and the rest cycle.
        Ok(QueryResult::SuggestPins(report)) if !report.suggestions.is_empty() => {
            let alt = state.alternatives.as_ref().ok_or("no alternatives")?;
            assert_eq!(alt.suggestions, report.suggestions);
            assert_eq!(
                model::edits(&instance, &state.staged),
                model::edits(
                    &instance,
                    &model::stage_suggestion(&Staged::new(), &report.suggestions[0].edits)
                )
            );
        }
        other => return Err(format!("USART1 has a suggestion: {other:?}").into()),
    }
    Ok(())
}

#[test]
fn package_pads_fit_the_view_and_keep_their_aspect() {
    let pads = vec![
        model::PadView {
            number: "1".into(),
            center: [-1.0, 0.0],
            size: [0.5, 1.0],
            round: false,
            terminal: Some(T1),
        },
        model::PadView {
            number: "2".into(),
            center: [1.0, 0.0],
            size: [0.5, 1.0],
            round: false,
            terminal: Some(T2),
        },
    ];
    let bounds = package::bounds(&pads);
    assert_eq!(bounds, Some(([-1.25, -0.5], [1.25, 0.5])));
    let Some(bounds) = bounds else { return };
    let area = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
    let left = package::pad_rect(area, bounds, &pads[0]);
    let right = package::pad_rect(area, bounds, &pads[1]);
    assert!(area.contains_rect(left) && area.contains_rect(right));
    // 2.5 mm across fills the 100 px width: 40 px per mm.
    assert!((left.width() - 20.0).abs() < 1e-3 && (left.height() - 40.0).abs() < 1e-3);
    assert!(left.center().x < right.center().x);
    assert!(package::bounds(&[]).is_none());
}

// ---- The panel in headless egui frames ----

/// One frame's painted texts and the operation the panel sent.
type Frame = (Vec<(String, Rect)>, Option<Operation>);

/// A headless frame loop that records painted text.
struct Harness {
    ctx: egui::Context,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        fonts::setup_bundled_fonts(&ctx);
        Self { ctx, time: 0.0 }
    }

    /// One frame drawing `instance`'s card rows (`row`, then `entry`);
    /// returns the painted texts and the operation the panel sent.
    fn frame(
        &mut self,
        engine: &Engine,
        id: Id,
        events: Vec<Event>,
        rows: bool,
    ) -> Result<Frame, Box<dyn std::error::Error>> {
        self.time += 0.4;
        let instance = instance(engine, id)?;
        let mut sent = None;
        let output = self.ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1600.0, 1000.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                if rows {
                    kit::form(ui, |ui| {
                        row(ui, engine, &instance);
                        sent = entry(ui, engine, &instance);
                    });
                }
            },
        );
        let mut output = output;
        output.textures_delta.clear();
        let mut texts = Vec::new();
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
            match shape {
                egui::Shape::Text(t) => out.push((
                    t.galley.text().to_owned(),
                    t.galley.rect.translate(t.pos.to_vec2()),
                )),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        for shape in &output.shapes {
            walk(&shape.shape, &mut texts);
        }
        Ok((texts, sent))
    }

    /// Settle, then click the first painted `text`; returns what the click
    /// frames sent.
    fn click(
        &mut self,
        engine: &Engine,
        id: Id,
        text: &str,
    ) -> Result<Vec<Operation>, Box<dyn std::error::Error>> {
        // Windows settle their size over a few frames.
        for _ in 0..3 {
            self.frame(engine, id, vec![], true)?;
        }
        let (texts, _) = self.frame(engine, id, vec![], true)?;
        let at = texts
            .iter()
            .find(|(t, _)| t == text)
            .map(|(_, r)| r.center())
            .ok_or_else(|| format!("{text:?} is not painted: {texts:?}"))?;
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let mut sent = Vec::new();
        for events in [
            vec![Event::PointerMoved(at)],
            vec![press(true)],
            vec![press(false)],
        ] {
            sent.extend(self.frame(engine, id, events, true)?.1);
        }
        Ok(sent)
    }
}

fn names(texts: &[(String, Rect)]) -> Vec<&str> {
    texts.iter().map(|(t, _)| t.as_str()).collect()
}

#[test]
fn pin_assignment_opens_the_panel_and_apply_sends_one_batch() -> TestResult {
    let (mut engine, ids) = setup()?;
    let mut harness = Harness::new();
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    assert!(
        names(&texts).contains(&"No peripheral in use"),
        "{:?}",
        names(&texts)
    );
    assert!(!names(&texts).contains(&"U1 · Pin assignment"));
    assert!(
        harness
            .click(&engine, ids.u1, "Pin assignment…")?
            .is_empty()
    );
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    let shown = names(&texts);
    for expected in [
        "U1 · Pin assignment",
        "PERIPHERALS",
        "PINS",
        "PACKAGE",
        "USART1",
        "SPI1",
        "No changes staged",
    ] {
        assert!(
            shown.contains(&expected),
            "{expected:?} missing from {shown:?}"
        );
    }
    assert!(is_open(&harness.ctx, ids.u1));

    // Stage USART1 on both pins (as role choices do), then Apply.
    let mut state = load(&harness.ctx);
    state.staged = [(T1, Some(ids.uart_tx)), (T2, Some(ids.uart_rx))]
        .into_iter()
        .collect();
    store(&harness.ctx, state);
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    let shown = names(&texts);
    assert!(
        shown.contains(&"2 pins change function; nets, pads and copper unchanged"),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|t| t.starts_with("USART1 · ")),
        "{shown:?}"
    );
    let sent = harness.click(&engine, ids.u1, "Apply")?;
    let [operation] = &sent[..] else {
        return Err(format!("expected one operation, got {sent:?}").into());
    };
    let Operation::Instance(InstanceCommand::SelectFunctions { instance, edits }) = operation
    else {
        return Err(format!("expected instance.select_functions, got {operation:?}").into());
    };
    assert_eq!(*instance, ids.u1);
    assert_eq!(edits.len(), 2);
    engine.apply(engine.revision(), operation.clone())?;
    // After the Apply lands the panel stays open with nothing staged, and
    // the card row names the peripheral in use.
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    let shown = names(&texts);
    assert!(shown.contains(&"No changes staged"), "{shown:?}");
    assert!(shown.contains(&"USART1 in use"), "{shown:?}");
    Ok(())
}

#[test]
fn a_rejected_preview_shows_its_reason_and_apply_stays_disabled() -> TestResult {
    let (engine, ids) = setup()?;
    let mut harness = Harness::new();
    // Revealed between frames, as the Issues locate does.
    harness.frame(&engine, ids.u1, vec![], false)?;
    reveal(&harness.ctx, ids.u1, Some(T2));
    // A new window sizes itself invisibly on its first frame.
    harness.frame(&engine, ids.u1, vec![], true)?;
    let mut state = load(&harness.ctx);
    state.staged = [(T1, Some(ids.uart_tx)), (T2, Some(ids.spi_mosi))]
        .into_iter()
        .collect();
    store(&harness.ctx, state);
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    let instance = instance(&engine, ids.u1)?;
    let model = model::build(engine.project(), &instance)?;
    let preview = model::preview(
        engine.project(),
        &instance,
        &model,
        &load(&harness.ctx).staged,
    )?;
    let reason = &preview.lines.first().ok_or("no line")?.text;
    assert!(
        names(&texts).contains(&reason.as_str()),
        "{:?}",
        names(&texts)
    );
    assert!(harness.click(&engine, ids.u1, "Apply")?.is_empty());
    // The revealed pin is the focus pin.
    assert_eq!(load(&harness.ctx).focus, Some(T2));
    Ok(())
}

#[test]
fn the_panel_closes_when_its_instance_is_no_longer_shown() -> TestResult {
    let (engine, ids) = setup()?;
    let mut harness = Harness::new();
    // Revealed between frames, as the Issues locate does.
    harness.frame(&engine, ids.u1, vec![], false)?;
    reveal(&harness.ctx, ids.u1, None);
    // A new window sizes itself invisibly on its first frame.
    harness.frame(&engine, ids.u1, vec![], true)?;
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    assert!(names(&texts).contains(&"U1 · Pin assignment"));
    // A frame without the instance card (another subject), then the card
    // again: the panel stays closed.
    harness.frame(&engine, ids.u1, vec![], false)?;
    harness.frame(&engine, ids.u1, vec![], false)?;
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    assert!(
        !names(&texts).contains(&"U1 · Pin assignment"),
        "{:?}",
        names(&texts)
    );
    assert!(!is_open(&harness.ctx, ids.u1));
    Ok(())
}

#[test]
fn a_component_without_peripherals_offers_no_pin_assignment() -> TestResult {
    let engine = Engine::demo()?;
    let r1 = engine.project().instances[0].id;
    let mut harness = Harness::new();
    let (texts, _) = harness.frame(&engine, r1, vec![], true)?;
    assert!(
        !names(&texts).contains(&"Pin assignment…"),
        "{:?}",
        names(&texts)
    );
    Ok(())
}

/// A blocker's way out stages its edits over the base it was found for.
#[test]
fn staging_the_way_out_applies_its_edits_over_the_base() -> TestResult {
    let (engine, ids) = setup()?;
    let (instance, model) = built(&engine, ids)?;
    let preview = model::preview(engine.project(), &instance, &model, &Staged::new())?;
    let base: Staged = [(T2, Some(ids.uart_rx))].into_iter().collect();
    let way_out = Suggestion {
        edits: vec![FunctionEdit {
            terminal: T1,
            function: Some(ids.uart_tx),
        }],
        changed: vec![T1],
        optional: vec![],
        unplaced: vec![],
    };
    let mut state = State {
        blockers: vec!["blocked".into()],
        way_out: Some(("Way out: move it".into(), way_out, base)),
        ..State::default()
    };
    handle(
        &mut state,
        &engine,
        &instance,
        &model,
        &preview,
        Action::StageWayOut,
    );
    assert_eq!(state.staged.get(&T1), Some(&Some(ids.uart_tx)));
    assert_eq!(state.staged.get(&T2), Some(&Some(ids.uart_rx)));
    assert!(state.blockers.is_empty() && state.way_out.is_none());
    Ok(())
}

/// Locating a `config.*` issue in the Issues surface selects the instance
/// on the sheet and opens its pin-mux panel at the issue's pin.
#[test]
fn locating_a_config_issue_opens_the_panel_at_its_pin() -> TestResult {
    use crate::issues::{PinMuxTarget, pin_mux_target};
    use pcbcraft_core::ObjectRef;
    use pcbcraft_core::issues::IssueScope;
    let (mut engine, ids) = setup()?;
    // USART1 TX without its required RX: a `config.requirement_unmet`.
    engine.apply(
        engine.revision(),
        Operation::Instance(InstanceCommand::SelectFunctions {
            instance: ids.u1,
            edits: vec![FunctionEdit {
                terminal: T1,
                function: Some(ids.uart_tx),
            }],
        }),
    )?;
    let QueryResult::Issues(report) = engine.query(Query::Issues {
        scope: IssueScope::All,
    })?
    else {
        return Err("issues".into());
    };
    let row = report
        .rows
        .iter()
        .find(|r| r.finding.code.starts_with("config."))
        .ok_or("a config.* issue")?;
    let refs: Vec<ObjectRef> = row.objects().copied().collect();
    let target = pin_mux_target(&row.finding.code, &refs).ok_or("a pin-mux target")?;
    assert_eq!(target.instance, ids.u1);
    assert!(target.terminal.is_some(), "the issue names its pin");
    // Other codes never open it.
    assert_eq!(pin_mux_target("erc.unconnected", &refs), None);
    assert_eq!(
        pin_mux_target("config.rule_unmet", &[ObjectRef::Instance { id: ids.u1 }]),
        Some(PinMuxTarget {
            instance: ids.u1,
            terminal: None
        })
    );

    let mut sheet = crate::schematic_editor::SchematicEditor::default();
    assert!(crate::issues::Locate::locate(&mut sheet, &engine, &refs));
    let mut harness = Harness::new();
    harness.frame(&engine, ids.u1, vec![], false)?;
    assert!(!is_open(&harness.ctx, ids.u1));
    reveal(&harness.ctx, target.instance, target.terminal);
    // A new window sizes itself invisibly on its first frame.
    harness.frame(&engine, ids.u1, vec![], true)?;
    let (texts, _) = harness.frame(&engine, ids.u1, vec![], true)?;
    assert!(names(&texts).contains(&"U1 · Pin assignment"));
    assert!(is_open(&harness.ctx, ids.u1));
    assert_eq!(load(&harness.ctx).focus, target.terminal);
    Ok(())
}
