//! Drawn symbol content through public commands and queries: blocking and
//! advisory findings, retired graphic IDs, persistence bounds, the authoring
//! grid, placed units and the encoded-size budget.
use super::support::*;
use crate::completeness::CheckTarget;
use crate::geometry::{Bounds, Shape};
use crate::library::graphics::{
    self, MAX_GRAPHICS_PER_UNIT, MAX_SHAPE_POINTS, MAX_TEXT_LINES, MAX_VERTICES,
};
use crate::library::samples::{self, SampleRefs};
use crate::library::{
    Decorations, Fill, Graphic, Grid, HAlign, Item, Justify, Pin, PinDirection, Rotation,
    SymbolDefinition, SymbolHit, Text, TextContent, TextMetric, TextSize, Unit, VAlign,
};
use crate::schematic;
use crate::{
    BindingRef, DefinitionId, Engine, Error, FootprintRef, GraphicId, LibraryCommand, ObjectRef,
    OffGridPin, Operation, Outcome, PackageRef, Result, Revision, SymbolRef, TerminalId, UnitId,
    format,
};

fn two(name: &str) -> Spec<'_> {
    Spec {
        name,
        terminals: vec![(1, "A", passive()), (2, "B", passive())],
        units: vec![(1, "A", vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0])])],
        package: vec![(1, "1"), (2, "2")],
        mapping: vec![(1, 1), (2, 2)],
        physical: true,
    }
}

fn shape(id: u32, shape: Shape, width: f64, fill: Fill) -> Graphic {
    Graphic {
        id: GraphicId(id),
        item: Item::Shape { shape, width, fill },
    }
}

fn literal(id: u32, text: &str) -> Graphic {
    graphics::text(
        GraphicId(id),
        TextContent::Literal(text.into()),
        [0.0, 0.0],
        Justify::default(),
    )
}

/// Append a graphic to the first unit, allocating its ID.
fn add_graphic(engine: &mut Engine, symbol: DefinitionId, graphic: Graphic) -> Result<()> {
    with(engine, symbol, symbol_body, |s| {
        s.next_local = s.next_local.max(graphic.id.0 + 1);
        s.units[0].graphics.push(graphic);
    })
}

/// The findings of one draft with `code`.
fn findings_of(engine: &Engine, draft: DefinitionId, code: &str) -> Result<Vec<crate::Finding>> {
    Ok(check(engine, CheckTarget::Draft(draft))?
        .findings
        .into_iter()
        .filter(|f| f.code == code)
        .collect())
}

/// Publishing the fixture must fail and change nothing.
fn publish_refused(engine: &mut Engine, f: &Fixture, why: &str) -> Result<()> {
    let before = engine.document().clone();
    let revision = engine.revision();
    match publish(engine, f.all()) {
        Ok(published) => Err(Error::Invalid(format!("{why}: published {published:?}"))),
        Err(error) => {
            assert_eq!(error.code(), "conflict", "{why}: {error}");
            assert_eq!(engine.document(), &before, "{why}");
            assert_eq!(engine.revision(), revision, "{why}");
            Ok(())
        }
    }
}

#[test]
fn each_blocking_graphic_finding_names_its_graphic_and_blocks_publish() -> Result<()> {
    let bow_tie = vec![[0.0, 0.0], [2.0, 2.0], [2.0, 0.0], [0.0, 2.0]];
    let cases: Vec<(&str, Graphic)> = vec![
        (
            "graphic.degenerate",
            shape(
                10,
                Shape::Line {
                    a: [1.0, 1.0],
                    b: [1.0, 1.0],
                },
                0.25,
                Fill::None,
            ),
        ),
        (
            "graphic.degenerate",
            shape(
                10,
                Shape::Circle {
                    center: [0.0, 0.0],
                    radius: 0.005,
                },
                0.25,
                Fill::None,
            ),
        ),
        (
            "graphic.degenerate",
            shape(
                10,
                Shape::Arc {
                    center: [0.0, 0.0],
                    radius: 1.0,
                    start_deg: 0.0,
                    sweep_deg: 0.0,
                },
                0.25,
                Fill::None,
            ),
        ),
        (
            "graphic.polygon_self_intersecting",
            shape(10, Shape::polygon(bow_tie), 0.25, Fill::None),
        ),
        (
            "graphic.invisible",
            shape(10, Shape::rect([0.0, 0.0], [1.0, 1.0]), 0.0, Fill::None),
        ),
        (
            "graphic.fill_open",
            shape(
                10,
                Shape::Line {
                    a: [0.0, 0.0],
                    b: [1.0, 1.0],
                },
                0.25,
                Fill::Solid,
            ),
        ),
        ("text.empty", literal(10, "   ")),
        ("text.too_long", literal(10, &"x\n".repeat(MAX_TEXT_LINES))),
    ];
    for (code, graphic) in cases {
        let mut engine = Engine::default();
        let f = draft(&mut engine, &two("Pair"))?;
        assert!(check(&engine, CheckTarget::Draft(f.symbol))?.publishable);
        add_graphic(&mut engine, f.symbol, graphic)?;
        let report = check(&engine, CheckTarget::Draft(f.symbol))?;
        assert!(!report.publishable, "{code}");
        let found = findings_of(&engine, f.symbol, code)?;
        assert_eq!(found.len(), 1, "{code}: {:?}", report.findings);
        assert_eq!(
            found[0].objects,
            vec![ObjectRef::LibraryObject {
                definition: f.symbol,
                local: 10
            }],
            "{code}"
        );
        publish_refused(&mut engine, &f, code)?;
    }
    Ok(())
}

#[test]
fn pins_units_and_field_text_have_structural_rules() -> Result<()> {
    // A terminal with two pins.
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.symbol, symbol_body, |s| {
        let copy = s.units[0].pins[0].clone();
        s.units[0].pins.push(copy);
    })?;
    assert_eq!(
        findings_of(&engine, f.symbol, "symbol.pin_duplicate")?.len(),
        1
    );
    publish_refused(&mut engine, &f, "duplicate pin")?;

    // A unit that draws nothing.
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    with(&mut engine, f.symbol, symbol_body, |s| {
        s.units.push(Unit {
            id: UnitId(s.next_local),
            label: "B".into(),
            graphics: vec![],
            pins: vec![],
        });
        s.next_local += 1;
    })?;
    assert_eq!(
        findings_of(&engine, f.symbol, "symbol.unit_empty")?.len(),
        1
    );
    publish_refused(&mut engine, &f, "empty unit")?;

    // Field text the component does not select.
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    let unknown = DefinitionId(424_242);
    add_graphic(
        &mut engine,
        f.symbol,
        graphics::text(
            GraphicId(20),
            TextContent::Field(unknown),
            [0.0, 4.0],
            Justify::default(),
        ),
    )?;
    let found = findings_of(&engine, f.component, "component.text_field_unknown")?;
    assert_eq!(found.len(), 1);
    assert!(
        found[0]
            .objects
            .contains(&ObjectRef::Definition { id: unknown })
    );
    publish_refused(&mut engine, &f, "unknown field text")?;
    Ok(())
}

#[test]
fn a_retired_graphic_id_is_never_reused() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    let outline = shape(5, Shape::rect([-2.5, -2.5], [2.5, 2.5]), 0.25, Fill::Body);
    add_graphic(&mut engine, f.symbol, outline.clone())?;
    publish(&mut engine, f.all())?;
    // r2 retires graphic 5.
    with(&mut engine, f.symbol, symbol_body, |s| {
        s.units[0].graphics.clear()
    })?;
    publish(&mut engine, vec![f.symbol])?;
    // r3 reuses it.
    add_graphic(&mut engine, f.symbol, outline)?;
    let found = findings_of(&engine, f.symbol, "definition.local_id_reused")?;
    assert_eq!(
        found.iter().map(|f| f.objects.clone()).collect::<Vec<_>>(),
        vec![vec![ObjectRef::LibraryObject {
            definition: f.symbol,
            local: 5
        }]]
    );
    let before = engine.document().clone();
    assert!(publish(&mut engine, vec![f.symbol]).is_err());
    assert_eq!(engine.document(), &before);
    // An ID at or above next_local is unallocated.
    with(&mut engine, f.symbol, symbol_body, |s| {
        s.units[0].graphics[0].id = GraphicId(s.next_local)
    })?;
    assert_eq!(
        findings_of(&engine, f.symbol, "definition.local_id_unallocated")?.len(),
        1
    );
    Ok(())
}

#[test]
fn drafts_beyond_the_persistence_bounds_are_refused_unchanged() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    let line = |i: u32| {
        shape(
            100 + i,
            Shape::Line {
                a: [0.0, 0.0],
                b: [1.0, f64::from(i)],
            },
            0.25,
            Fill::None,
        )
    };
    let polyline = |id: u32, n: usize| {
        shape(
            id,
            Shape::polyline((0..n).map(|i| [i as f64 * 0.01, 0.0]).collect()),
            0.25,
            Fill::None,
        )
    };
    type Breaker = Box<dyn Fn(&mut SymbolDefinition)>;
    let cases: Vec<(&str, &str, Breaker)> = vec![
        (
            "too many graphics in a unit",
            "limit_exceeded",
            Box::new(move |s| {
                s.next_local = 10_000;
                s.units[0].graphics = (0..=MAX_GRAPHICS_PER_UNIT as u32).map(line).collect();
            }),
        ),
        (
            "too many graphics in a definition",
            "limit_exceeded",
            Box::new(move |s| {
                s.next_local = 10_000;
                for u in 0..5 {
                    s.units.push(Unit {
                        id: UnitId(5000 + u),
                        label: format!("U{u}"),
                        graphics: (0..205).map(|i| line(u * 1000 + i)).collect(),
                        pins: vec![],
                    });
                }
            }),
        ),
        (
            "too many points in a polyline",
            "limit_exceeded",
            Box::new(move |s| {
                s.next_local = 10_000;
                s.units[0].graphics = vec![polyline(10, MAX_SHAPE_POINTS + 1)];
            }),
        ),
        (
            "too many vertices in a definition",
            "limit_exceeded",
            Box::new(move |s| {
                s.next_local = 10_000;
                s.units[0].graphics = (0..9).map(|i| polyline(10 + i, 1000)).collect();
            }),
        ),
        (
            "literal text too long",
            "limit_exceeded",
            Box::new(|s| s.units[0].graphics = vec![literal(10, &"x".repeat(257))]),
        ),
        (
            "stroke too wide",
            "invalid_input",
            Box::new(move |s| {
                let mut g = line(1);
                if let Item::Shape { width, .. } = &mut g.item {
                    *width = 5.5;
                }
                s.units[0].graphics = vec![g];
            }),
        ),
        (
            "text too small",
            "invalid_input",
            Box::new(|s| {
                let mut g = literal(10, "R");
                if let Item::Text(t) = &mut g.item {
                    t.height = 0.05;
                }
                s.units[0].graphics = vec![g];
            }),
        ),
        (
            "arc beyond one turn",
            "invalid_input",
            Box::new(|s| {
                s.units[0].graphics = vec![shape(
                    10,
                    Shape::Arc {
                        center: [0.0, 0.0],
                        radius: 1.0,
                        start_deg: 0.0,
                        sweep_deg: 400.0,
                    },
                    0.25,
                    Fill::None,
                )]
            }),
        ),
        (
            "non-finite point",
            "invalid_input",
            Box::new(|s| s.units[0].pins[0].position = [f64::NAN, 0.0]),
        ),
        (
            "pin too long",
            "invalid_input",
            Box::new(|s| s.units[0].pins[0].length = 60.0),
        ),
        (
            "pin text far away",
            "invalid_input",
            Box::new(|s| s.units[0].pins[0].name.offset = -11.0),
        ),
        (
            "pin text height",
            "invalid_input",
            Box::new(|s| s.pin_text_height = 0.0),
        ),
    ];
    for (why, code, breaker) in cases {
        let before = engine.document().clone();
        let revision = engine.revision();
        let error = match with(&mut engine, f.symbol, symbol_body, |s| breaker(s)) {
            Ok(()) => return Err(Error::Invalid(format!("{why}: accepted"))),
            Err(error) => error,
        };
        assert_eq!(error.code(), code, "{why}: {error}");
        assert_eq!(engine.document(), &before, "{why}");
        assert_eq!(engine.revision(), revision, "{why}");
    }
    // At the limits themselves the draft is accepted.
    with(&mut engine, f.symbol, symbol_body, |s| {
        s.next_local = 10_000;
        s.units[0].graphics = (0..MAX_VERTICES / MAX_SHAPE_POINTS)
            .map(|i| polyline(10 + i as u32, MAX_SHAPE_POINTS))
            .collect();
    })?;
    Ok(())
}

#[test]
fn lint_is_advisory_draft_only_and_follows_the_grid() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    let report = check(&engine, CheckTarget::Draft(f.symbol))?;
    assert!(report.publishable);
    let missing = findings_of(&engine, f.symbol, "symbol.field_missing")?;
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].blocks, None);
    assert!(findings_of(&engine, f.symbol, "symbol.pin_off_grid")?.is_empty());

    // Off the 2.5 mm grid: advisory, still publishable.
    with(&mut engine, f.symbol, symbol_body, |s| {
        s.units[0].pins[1].position = [5.0, 1.0]
    })?;
    add_graphic(
        &mut engine,
        f.symbol,
        graphics::text(
            GraphicId(9),
            TextContent::Designator,
            [0.0, 4.0],
            Justify::default(),
        ),
    )?;
    let report = check(&engine, CheckTarget::Draft(f.symbol))?;
    assert!(report.publishable);
    let off = findings_of(&engine, f.symbol, "symbol.pin_off_grid")?;
    assert_eq!(off.len(), 1);
    assert_eq!(off[0].blocks, None);
    assert!(off[0].objects.contains(&ObjectRef::LibraryObject {
        definition: f.component,
        local: 2
    }));
    assert!(findings_of(&engine, f.symbol, "symbol.field_missing")?.is_empty());

    // Published revisions carry no lint.
    let published = publish(&mut engine, f.all())?;
    let symbol = published
        .iter()
        .find(|r| r.id == f.symbol)
        .copied()
        .ok_or_else(|| Error::NotFound("symbol revision".into()))?;
    let stored = check(&engine, CheckTarget::Revision(symbol))?;
    assert!(stored.findings.iter().all(|f| f.blocks.is_some()));

    // A 2.54 mm grid does not divide the 5 mm sheet grid.
    set_grid(&mut engine, 2.54)?;
    assert_eq!(
        findings_of(&engine, f.symbol, "symbol.grid_sheet")?.len(),
        1
    );
    assert_eq!(
        findings_of(&engine, f.symbol, "symbol.pin_off_grid")?.len(),
        2
    );
    Ok(())
}

fn set_grid(engine: &mut Engine, symbol_mm: f64) -> Result<Outcome> {
    apply(
        engine,
        Operation::Library(LibraryCommand::SetGrid {
            grid: Grid {
                symbol_mm,
                footprint_mm: 0.05,
            },
        }),
    )
}

#[test]
fn set_grid_is_undoable_saved_and_never_moves_geometry() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Pair"))?;
    assert_eq!(engine.drafts().grid, Grid::default());
    let before = body(&engine, f.symbol)?;
    let outcome = set_grid(&mut engine, 2.0)?;
    assert_eq!(
        outcome,
        Outcome::GridSet {
            off_grid: vec![
                OffGridPin {
                    draft: f.symbol,
                    terminal: TerminalId(1)
                },
                OffGridPin {
                    draft: f.symbol,
                    terminal: TerminalId(2)
                },
            ]
        }
    );
    assert_eq!(body(&engine, f.symbol)?, before);
    assert_eq!(engine.drafts().grid.symbol_mm, 2.0);
    let reopened = Engine::from_file(format::decode(&format::encode(&engine.to_file())?)?)?;
    assert_eq!(reopened.drafts().grid.symbol_mm, 2.0);
    engine.undo()?;
    assert_eq!(engine.drafts().grid, Grid::default());
    engine.redo()?;
    assert_eq!(
        set_grid(&mut engine, 1.25)?,
        Outcome::GridSet { off_grid: vec![] }
    );
    for bad in [0.0, 0.005, 10.5, f64::NAN, f64::INFINITY] {
        let document = engine.document().clone();
        let revision = engine.revision();
        let error = set_grid(&mut engine, bad).err();
        assert_eq!(error.map(|e| e.code()), Some("invalid_input"), "{bad}");
        assert_eq!(engine.document(), &document);
        assert_eq!(engine.revision(), revision);
    }
    Ok(())
}

#[test]
fn sample_symbols_are_drawn_on_grid_and_publish_without_lint() -> Result<()> {
    let refs = SampleRefs {
        component: DefinitionId(1),
        symbol: SymbolRef::new(DefinitionId(2), Revision::FIRST),
        package: PackageRef::new(DefinitionId(3), Revision::FIRST),
        footprint: FootprintRef::new(DefinitionId(4), Revision::FIRST),
        binding: BindingRef::new(DefinitionId(5), Revision::FIRST),
    };
    for edits in [
        samples::two_terminal("R", samples::Kind::RESISTOR, refs),
        samples::two_terminal_header("J", samples::Kind::CONNECTOR, refs),
    ] {
        let Some(symbol) = edits.iter().find_map(|e| match &e.definition {
            crate::library::Definition::Symbol(s) => Some(s.clone()),
            _ => None,
        }) else {
            return Err(Error::NotFound("sample symbol".into()));
        };
        let unit = &symbol.units[0];
        // Text reads smaller than the body: designator and name at
        // 1.25 mm, pin numbers smaller still; terminal names repeat the
        // pin numbers, so they are hidden.
        let body = unit.graphics[0].item.bounds(TextMetric::default());
        let body_height = body.max[1] - body.min[1];
        for graphic in &unit.graphics[1..] {
            let Item::Text(text) = &graphic.item else {
                return Err(Error::Invalid(format!("{graphic:?}")));
            };
            assert_eq!(text.height, 1.25, "{graphic:?}");
            assert!(text.height < body_height);
        }
        assert_eq!(symbol.pin_text_height, 1.0);
        // A two-terminal passive hides its numbers too; the connector,
        // pins all on its left, shows them.
        let connector = unit
            .pins
            .iter()
            .all(|p| p.direction == crate::library::PinDirection::Right);
        assert!(
            unit.pins
                .iter()
                .all(|p| !p.name.visible && p.number.visible == connector)
        );
        assert!(matches!(
            unit.graphics[0].item,
            Item::Shape {
                shape: Shape::Rect { .. },
                fill: Fill::Body,
                ..
            }
        ));
        for pin in &unit.pins {
            assert!(graphics::on_grid(pin.position, 2.5));
            // Each pin ends on the body edge.
            let body = unit.graphics[0].item.bounds(TextMetric::default());
            let edge = pin.body_end();
            assert!(
                (edge[0] - body.min[0] - 0.125).abs() < 1e-9
                    || (edge[0] - body.max[0] + 0.125).abs() < 1e-9,
                "{pin:?}"
            );
        }
    }
    // The demo publishes the resistor sample; its drafts lint clean.
    let engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let report = check(&engine, CheckTarget::Draft(f.symbol))?;
    assert!(report.publishable);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    Ok(())
}

#[test]
fn unit_bounds_and_hits_use_hand_derived_extents() -> Result<()> {
    let engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let crate::library::Definition::Symbol(symbol) = body(&engine, f.symbol)? else {
        return Err(Error::Invalid("not a symbol".into()));
    };
    let unit = &symbol.units[0];
    let m = TextMetric::default();
    // Pins reach ±5 mm; the designator (1.25 mm high, bottom at 1.5 mm)
    // and name (top at −1.5 mm) are 4 placeholder characters × 0.6 ×
    // 1.25 mm.
    assert_eq!(
        unit.bounds(m),
        Some(Bounds {
            min: [-5.0, -2.75],
            max: [5.0, 2.75]
        })
    );
    // Picking is one rule, on the drawing core resolves: the designator
    // reads "R?" and the name "Resistor" there.
    let drawn = drawn_unit(&engine, &f, unit.id)?;
    assert_eq!(
        drawn.hit([-4.0, 0.1], 0.2, m),
        Some(SymbolHit::Pin(TerminalId(1)))
    );
    assert_eq!(
        drawn.hit([0.0, 0.0], 0.2, m),
        Some(SymbolHit::Graphic(GraphicId(2)))
    );
    assert_eq!(
        drawn.hit([0.5, 2.5], 0.2, m),
        Some(SymbolHit::Graphic(GraphicId(3)))
    );
    assert_eq!(
        drawn.hit([0.0, -2.5], 0.2, m),
        Some(SymbolHit::Graphic(GraphicId(4)))
    );
    assert_eq!(drawn.hit([8.0, 8.0], 0.2, m), None);
    Ok(())
}

#[test]
fn pin_decorations_shorten_the_stub_and_sit_on_the_body() {
    let mut pin = Pin::new(TerminalId(1), [-5.0, 0.0], PinDirection::Right, 2.5);
    assert_eq!(pin.line(), [[-5.0, 0.0], [-2.5, 0.0]]);
    assert!(pin.decoration_shapes().is_empty());
    pin.decorations = Decorations {
        inverted: true,
        clock: true,
    };
    // A 1 mm bubble (0.4 × the 2.5 mm module) ends at the body edge.
    assert_eq!(pin.line(), [[-5.0, 0.0], [-3.5, 0.0]]);
    assert_eq!(
        pin.decoration_shapes(),
        vec![
            Shape::Circle {
                center: [-3.0, 0.0],
                radius: 0.5
            },
            Shape::polyline(vec![[-2.5, 0.5], [-1.5, 0.0], [-2.5, -0.5]]),
        ]
    );
}

#[test]
fn placed_units_resolve_text_pins_and_numbers_in_sheet_coordinates() -> Result<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let r1 = by_designator(&engine, "R1")?;
    let occurrence = |engine: &Engine| {
        engine
            .project()
            .occurrences_of(r1)
            .next()
            .map(|o| o.id)
            .ok_or_else(|| Error::NotFound("occurrence".into()))
    };
    let placed = schematic::placed_unit(engine.project(), occurrence(&engine)?)?;
    // R1 sits at (−12, 0).
    assert_eq!(placed.shapes.len(), 1);
    assert_eq!(
        placed.shapes[0].shape,
        Shape::rect([-14.5, -1.0], [-9.5, 1.0])
    );
    assert_eq!(placed.shapes[0].fill, Fill::Body);
    let texts: Vec<(&str, [f64; 2])> = placed
        .texts
        .iter()
        .map(|t| (t.text.as_str(), t.at))
        .collect();
    assert_eq!(texts, vec![("R1", [-12.0, 1.5]), ("10 kΩ", [-12.0, -1.5])]);
    let pin = &placed.pins[0];
    assert_eq!(pin.terminal, TerminalId(1));
    assert_eq!(pin.end, [-17.0, 0.0]);
    assert_eq!(pin.body_end, [-14.5, 0.0]);
    assert_eq!(
        schematic::pin_position(engine.project(), ep(r1, 1)),
        Some(pin.end)
    );
    // The sample hides terminal names, which only repeat the numbers, and
    // a two-terminal passive's numbers.
    assert!(pin.name.is_none());
    assert!(placed.pins.iter().all(|p| p.number.is_none()));

    // Field text shows the instance's effective value; hidden text and
    // hidden pin numbers are not placed.
    let field = engine.project().library.fields[0].id;
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.symbol, symbol_body, |s| {
            s.next_local = 6;
            s.units[0].graphics.push(Graphic {
                id: GraphicId(5),
                item: Item::Text(Text {
                    content: TextContent::Field(field),
                    at: [0.0, -4.0],
                    height: 1.5,
                    rotation: Rotation::R0,
                    justify: Justify {
                        h: HAlign::Center,
                        v: VAlign::Top,
                    },
                    visible: true,
                    line_spacing: graphics::TEXT_LINE_SPACING,
                }),
            });
            if let Item::Text(t) = &mut s.units[0].graphics[2].item {
                t.visible = false;
            }
            s.units[0].pins[0].number.visible = true;
        })
    })?;
    engine.apply_update(
        engine.revision(),
        crate::updates::Change::ComponentRevision {
            instances: vec![r1],
            target,
            resolutions: Default::default(),
        },
        false,
    )?;
    let value = inspect(&engine, r1)?.properties[0]
        .text
        .clone()
        .ok_or_else(|| Error::NotFound("field text".into()))?;
    let placed = schematic::placed_unit(engine.project(), occurrence(&engine)?)?;
    let texts: Vec<&str> = placed.texts.iter().map(|t| t.text.as_str()).collect();
    assert_eq!(texts, vec!["R1", value.as_str()]);
    // Pin 1's number, shown, is centred on the stub 0.5 mm above it; pin
    // 2's stays hidden and is not placed.
    let number = placed.pins[0]
        .number
        .as_ref()
        .ok_or_else(|| Error::NotFound("pin number".into()))?;
    assert_eq!(number.text, "1");
    assert_eq!(number.at, [-15.75, 0.5]);
    assert!(placed.pins[1].number.is_none());

    // Without a physical choice numbers are hidden, never guessed.
    let mut engine = Engine::default();
    let schematic_only = component(
        &mut engine,
        &Spec {
            physical: false,
            ..two("Probe")
        },
    )?;
    let (_, occurrence) = place(&mut engine, &schematic_only, "U1", 1, [0.0, 0.0])?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    assert!(placed.pins.iter().all(|p| p.number.is_none()));
    assert!(placed.pins.iter().all(|p| p.name.is_some()));
    assert!(schematic::placed_unit(engine.project(), 999_999).is_err());
    Ok(())
}

#[test]
fn a_pin_name_equal_to_its_number_is_drawn_once() -> Result<()> {
    let spec = |physical| Spec {
        terminals: vec![(1, "1", passive()), (2, "B", passive())],
        physical,
        ..two("Pair")
    };
    let mut engine = Engine::default();
    let fixture = component(&mut engine, &spec(true))?;
    let (_, occurrence) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    let text = |t: &Option<crate::schematic::PlacedText>| t.as_ref().map(|t| t.text.clone());
    let pins: Vec<_> = placed
        .pins
        .iter()
        .map(|p| (text(&p.name), text(&p.number)))
        .collect();
    // Pin 1's name "1" repeats its number and is dropped; pin 2's name
    // "B" differs from its number "2" and both are drawn.
    assert_eq!(
        pins,
        vec![
            (None, Some("1".into())),
            (Some("B".into()), Some("2".into()))
        ]
    );
    // Picking and bounds follow the drawing: the dropped name's place
    // inside the body hits the body, not the pin.
    let m = TextMetric::default();
    let bounds = placed
        .bounds(m)
        .ok_or_else(|| Error::NotFound("bounds".into()))?;
    for pin in &placed.pins {
        for t in pin.name.iter().chain(&pin.number) {
            let b = t.bounds(m);
            assert!(bounds.contains(b.min) && bounds.contains(b.max), "{t:?}");
        }
    }

    // Without a shown number there is nothing to repeat: the name stays.
    let mut engine = Engine::default();
    let fixture = component(&mut engine, &spec(false))?;
    let (_, occurrence) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    assert_eq!(
        placed.pins[0].name.as_ref().map(|t| t.text.as_str()),
        Some("1")
    );
    assert!(placed.pins[0].number.is_none());
    Ok(())
}

/// e2e UX finding 4: the slot under the body shows the instance's value at
/// the value text size, never the component name, and the value is not
/// drawn a second time below it.
#[test]
fn the_value_slot_shows_the_value_never_the_component_name() -> Result<()> {
    let engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let occurrence = engine
        .project()
        .occurrences_of(r1)
        .next()
        .map(|o| o.id)
        .ok_or_else(|| Error::NotFound("occurrence".into()))?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    let texts: Vec<(&str, f64)> = placed
        .texts
        .iter()
        .map(|t| (t.text.as_str(), t.height))
        .collect();
    assert_eq!(
        texts,
        vec![
            ("R1", graphics::SYMBOL_TEXT_HEIGHT_MM),
            ("10 kΩ", graphics::VALUE_TEXT_HEIGHT_MM)
        ]
    );
    assert!(placed.fields.is_empty(), "{:?}", placed.fields);
    let instance = engine
        .project()
        .instance(r1)
        .ok_or_else(|| Error::NotFound("R1".into()))?;
    assert_eq!(
        crate::instances::value_text(&engine.project().library, instance)?,
        Some("10 kΩ".into())
    );
    Ok(())
}

/// Draft `spec` with a component-name text under its body, and publish.
fn named_part(engine: &mut Engine, spec: &Spec<'_>) -> Result<Fixture> {
    let fixture = draft(engine, spec)?;
    with(engine, fixture.symbol, symbol_body, |s| {
        let id = GraphicId(s.next_local);
        s.next_local += 1;
        s.units[0].graphics.push(graphics::text(
            id,
            TextContent::ComponentName,
            [0.0, -2.0],
            Justify::default(),
        ));
    })?;
    publish(engine, fixture.all())?;
    Ok(fixture)
}

/// A two-terminal part with no value shows no value text; one of more
/// terminals (an IC) is named by its component.
#[test]
fn without_a_value_only_a_multi_terminal_part_shows_its_name() -> Result<()> {
    let mut engine = Engine::default();
    let fixture = named_part(&mut engine, &two("Chip capacitor 0603 (1608 metric)"))?;
    let (_, occurrence) = place(&mut engine, &fixture, "C1", 1, [0.0, 0.0])?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    assert!(
        placed.texts.iter().all(|t| !t.text.contains("capacitor")),
        "{:?}",
        placed.texts
    );

    let mut engine = Engine::default();
    let spec = Spec {
        terminals: vec![
            (1, "A", passive()),
            (2, "B", passive()),
            (3, "C", passive()),
        ],
        units: vec![(
            1,
            "A",
            vec![(1, [-5.0, 0.0]), (2, [5.0, 0.0]), (3, [5.0, 2.5])],
        )],
        package: vec![(1, "1"), (2, "2"), (3, "3")],
        mapping: vec![(1, 1), (2, 2), (3, 3)],
        ..two("AP2112K-3.3")
    };
    let fixture = named_part(&mut engine, &spec)?;
    let (_, occurrence) = place(&mut engine, &fixture, "U1", 1, [0.0, 0.0])?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    let name = placed
        .texts
        .iter()
        .find(|t| t.text == "AP2112K-3.3")
        .ok_or_else(|| Error::NotFound(format!("{:?}", placed.texts)))?;
    assert_eq!(name.height, graphics::VALUE_TEXT_HEIGHT_MM);
    Ok(())
}

#[test]
fn shown_field_values_without_field_text_stack_below_the_unit() -> Result<()> {
    let mut engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let r1 = by_designator(&engine, "R1")?;
    // A symbol with no value slot: the component-name text is hidden.
    let target = cascade(&mut engine, &f, |e| {
        with(e, f.symbol, symbol_body, |s| {
            if let Item::Text(t) = &mut s.units[0].graphics[2].item {
                t.visible = false;
            }
        })
    })?;
    engine.apply_update(
        engine.revision(),
        crate::updates::Change::ComponentRevision {
            instances: vec![r1],
            target,
            resolutions: Default::default(),
        },
        false,
    )?;
    let occurrence = engine
        .project()
        .occurrences_of(r1)
        .next()
        .map(|o| o.id)
        .ok_or_else(|| Error::NotFound("occurrence".into()))?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    assert_eq!(placed.occurrence, Some(occurrence));
    // The demo shows resistance on the symbol and now draws no text for
    // it: the value sits centred 0.5 mm below the body's stroke at
    // −1.125 mm, at the value text size.
    let [value] = placed.fields.as_slice() else {
        return Err(Error::Invalid(format!("{:?}", placed.fields)));
    };
    assert_eq!(value.text, "10 kΩ");
    assert!((value.at[0] + 12.0).abs() < 1e-9, "{:?}", value.at);
    assert!((value.at[1] + 1.625).abs() < 1e-9, "{:?}", value.at);
    assert_eq!(value.height, graphics::VALUE_TEXT_HEIGHT_MM);
    assert_eq!(
        value.justify,
        Justify {
            h: HAlign::Center,
            v: VAlign::Top
        }
    );
    // Bounds cover every drawn item, including the stacked value.
    let m = TextMetric::default();
    let bounds = placed
        .bounds(m)
        .ok_or_else(|| Error::NotFound("bounds".into()))?;
    for text in placed.texts.iter().chain(&placed.fields) {
        let b = text.bounds(m);
        assert!(bounds.contains(b.min) && bounds.contains(b.max), "{text:?}");
    }
    assert!((bounds.min[1] + 1.625 + graphics::VALUE_TEXT_HEIGHT_MM).abs() < 1e-9);
    assert_eq!([bounds.min[0], bounds.max[0]], [-17.0, -7.0]);
    Ok(())
}

#[test]
fn placed_units_pick_what_they_draw() -> Result<()> {
    let engine = Engine::demo()?;
    let r1 = by_designator(&engine, "R1")?;
    let occurrence = engine
        .project()
        .occurrences_of(r1)
        .next()
        .map(|o| o.id)
        .ok_or_else(|| Error::NotFound("occurrence".into()))?;
    let placed = schematic::placed_unit(engine.project(), occurrence)?;
    let m = TextMetric::default();
    let tol = 0.2;
    // The end of the resolved value, wider than the four characters a
    // placeholder is assumed to have in the definition.
    let name = placed
        .texts
        .iter()
        .find(|t| t.text == "10 kΩ")
        .ok_or_else(|| Error::NotFound("name".into()))?;
    let b = name.bounds(m);
    let end = [b.max[0] - 0.05, (b.min[1] + b.max[1]) / 2.0];
    assert!(
        b.max[0] - b.min[0]
            > m.bounds([0.0; 2], name.height, 0.0, name.justify, TextSize::line(4))
                .max[0]
                * 2.0
    );
    assert!(name.graphic.is_some());
    assert_eq!(
        placed.hit(end, tol, m),
        name.graphic.map(SymbolHit::Graphic)
    );
    // The value is drawn once, in its slot: nothing is stacked.
    assert!(placed.fields.is_empty());
    let v = name.bounds(m);
    let inside = [(v.min[0] + v.max[0]) / 2.0, (v.min[1] + v.max[1]) / 2.0];
    // Pins and the body, in sheet coordinates; empty sheet misses.
    assert_eq!(
        placed.hit([-16.0, 0.1], tol, m),
        Some(SymbolHit::Pin(TerminalId(1)))
    );
    assert_eq!(
        placed.hit([-12.0, 0.9], tol, m),
        Some(SymbolHit::Graphic(placed.shapes[0].graphic))
    );
    assert_eq!(placed.hit([-12.0, 20.0], tol, m), None);
    // Every hit lies within the drawn bounds the views fit to.
    let bounds = placed
        .bounds(m)
        .ok_or_else(|| Error::NotFound("bounds".into()))?;
    assert!(bounds.contains(end) && bounds.contains(inside));
    Ok(())
}

#[test]
fn definition_units_draw_placeholders_and_working_binding_numbers() -> Result<()> {
    let engine = Engine::demo()?;
    let f = demo_fixture(&engine)?;
    let definition = |id| body(&engine, id);
    let (
        crate::library::Definition::Symbol(symbol),
        crate::library::Definition::Component(component),
        crate::library::Definition::Binding(binding),
        crate::library::Definition::Package(package),
    ) = (
        definition(f.symbol)?,
        definition(f.component)?,
        definition(f.binding)?,
        definition(f.package)?,
    )
    else {
        return Err(Error::Invalid("unexpected kinds".into()));
    };
    let library = &engine.project().library;
    // The sample hides its pin numbers; show them to see the working
    // binding's.
    let mut symbol = symbol;
    for pin in &mut symbol.units[0].pins {
        pin.number.visible = true;
    }
    let view = schematic::DefinitionUnit {
        symbol: &symbol,
        unit: UnitId(1),
        component: &component,
        component_name: "Resistor",
        physical: Some((&binding, &package)),
    };
    let drawn = schematic::definition_unit(library, view)?;
    assert_eq!(drawn.occurrence, None);
    // Drawn in the definition frame, untransformed.
    assert_eq!(drawn.shapes[0].shape, Shape::rect([-2.5, -1.0], [2.5, 1.0]));
    let texts: Vec<&str> = drawn.texts.iter().map(|t| t.text.as_str()).collect();
    // The value slot shows the shown field: its default, else its name.
    assert_eq!(texts, vec!["R?", "Resistance"]);
    let numbers: Vec<Option<&str>> = drawn
        .pins
        .iter()
        .map(|p| p.number.as_ref().map(|t| t.text.as_str()))
        .collect();
    assert_eq!(numbers, vec![Some("1"), Some("2")]);
    assert_eq!(drawn.pins[0].end, [-5.0, 0.0]);
    // The demo component sets no default, so the shown field reads its
    // name, once, in the slot.
    assert!(drawn.fields.is_empty());

    // Without a working binding numbers are hidden, never guessed; an
    // unknown unit is refused.
    let unbound = schematic::definition_unit(
        library,
        schematic::DefinitionUnit {
            physical: None,
            ..view
        },
    )?;
    assert!(unbound.pins.iter().all(|p| p.number.is_none()));
    assert!(
        schematic::definition_unit(
            library,
            schematic::DefinitionUnit {
                unit: UnitId(99),
                ..view
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn upright_text_never_reads_downward_or_leftward() {
    let j = |h, v| Justify { h, v };
    let left_bottom = j(HAlign::Left, VAlign::Bottom);
    let right_top = j(HAlign::Right, VAlign::Top);
    // Right and up read as authored.
    assert_eq!(graphics::upright(0.0, left_bottom), (0.0, left_bottom));
    assert_eq!(graphics::upright(90.0, left_bottom), (90.0, left_bottom));
    assert_eq!(graphics::upright(-270.0, left_bottom), (90.0, left_bottom));
    // Left and down are turned half a turn with the anchor swapped, so
    // they cover the same box.
    assert_eq!(graphics::upright(180.0, left_bottom), (0.0, right_top));
    assert_eq!(graphics::upright(270.0, left_bottom), (90.0, right_top));
    assert_eq!(graphics::upright(-90.0, right_top), (90.0, left_bottom));
    let centred = j(HAlign::Center, VAlign::Middle);
    assert_eq!(graphics::upright(540.0, centred), (0.0, centred));
    let m = TextMetric::default();
    for rotation in [180.0, 270.0] {
        let (turned, justify) = graphics::upright(rotation, left_bottom);
        let a = m.bounds([3.0, 4.0], 1.5, rotation, left_bottom, TextSize::line(5));
        let b = m.bounds([3.0, 4.0], 1.5, turned, justify, TextSize::line(5));
        for (p, q) in [(a.min, b.min), (a.max, b.max)] {
            assert!(crate::geometry::distance(p, q) < 1e-9, "{a:?} vs {b:?}");
        }
    }
}

#[test]
fn symbols_round_trip_and_reject_unknown_shape_fields() -> Result<()> {
    let symbol = SymbolDefinition {
        component: DefinitionId(7),
        next_local: 9,
        pin_text_height: 1.25,
        units: vec![Unit {
            id: UnitId(1),
            label: "A".into(),
            graphics: vec![
                shape(
                    2,
                    Shape::polyline(vec![[0.0, 0.0], [1.0, 1.0], [2.0, 0.0]]),
                    0.25,
                    Fill::None,
                ),
                shape(
                    3,
                    Shape::Arc {
                        center: [0.0, 0.0],
                        radius: 1.0,
                        start_deg: 90.0,
                        sweep_deg: -180.0,
                    },
                    0.1,
                    Fill::None,
                ),
                shape(
                    4,
                    Shape::polygon(vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]),
                    0.0,
                    Fill::Solid,
                ),
                graphics::text(
                    GraphicId(5),
                    TextContent::UnitLabel,
                    [0.0, 0.0],
                    Justify::default(),
                ),
            ],
            pins: vec![Pin::new(
                TerminalId(1),
                [-5.0, 0.0],
                PinDirection::Right,
                2.5,
            )],
        }],
    };
    let json = serde_json::to_value(&symbol)?;
    assert_eq!(
        serde_json::from_value::<SymbolDefinition>(json.clone())?,
        symbol
    );
    let mut extra = json;
    extra["units"][0]["graphics"][0]["item"]["shape"]["shape"]["polyline"]["closed"] =
        serde_json::json!(true);
    assert!(serde_json::from_value::<SymbolDefinition>(extra).is_err());
    Ok(())
}

/// G1 exit measurement: the encoded size of one symbol at the full graphics
/// limits and of 100 typical revisions, against the 16 MiB file budget.
/// A symbol at every graphics limit, its numbers drawn from `coordinate`.
fn full_limit_symbol(coordinate: impl Fn(usize) -> f64) -> SymbolDefinition {
    let mut graphics_left = graphics::MAX_GRAPHICS;
    let mut units = Vec::new();
    let mut id = 100u32;
    let polylines = 8usize;
    let per_polyline = (MAX_VERTICES - 2 * (graphics::MAX_GRAPHICS - polylines)) / polylines;
    for u in 0..4u32 {
        let mut graphics = Vec::new();
        while graphics.len() < MAX_GRAPHICS_PER_UNIT && graphics_left > 0 {
            id += 1;
            // Polylines carry one corner radius per point and one
            // rectangle is rounded, the largest encoding of each.
            let shape = if graphics_left <= polylines {
                Shape::Polyline {
                    points: (0..per_polyline)
                        .map(|i| [coordinate(i), coordinate(i + 1)])
                        .collect(),
                    radii: (0..per_polyline).map(|i| coordinate(i).abs()).collect(),
                }
            } else if graphics_left == polylines + 1 {
                Shape::Rect {
                    a: [coordinate(id as usize), coordinate(1)],
                    b: [coordinate(2), coordinate(id as usize)],
                    radius: coordinate(3).abs(),
                }
            } else {
                Shape::Line {
                    a: [coordinate(id as usize), coordinate(1)],
                    b: [coordinate(2), coordinate(id as usize)],
                }
            };
            graphics.push(Graphic {
                id: GraphicId(id),
                item: Item::Shape {
                    shape,
                    width: 0.25,
                    fill: Fill::None,
                },
            });
            graphics_left -= 1;
        }
        units.push(Unit {
            id: UnitId(u + 1),
            label: format!("Unit {u}"),
            graphics,
            pins: (0..64)
                .map(|t| {
                    Pin::new(
                        TerminalId(u * 64 + t + 1),
                        [coordinate(t as usize), coordinate(1)],
                        PinDirection::Right,
                        2.5,
                    )
                })
                .collect(),
        });
    }
    SymbolDefinition {
        component: DefinitionId(1),
        next_local: id + 1,
        pin_text_height: 1.25,
        units,
    }
}

#[test]
fn encoded_symbol_sizes_fit_the_file_budget() -> Result<()> {
    // Worst case: every graphic and vertex limit reached with long numbers.
    let full = full_limit_symbol(|i| -1_234.567_891_234_5 + i as f64 * 0.000_123_456_789);
    assert!(
        crate::library::check_bounds(
            &meta("Full"),
            &crate::library::Definition::Symbol(full.clone())
        )
        .is_ok()
    );
    let full_bytes = format::encode_bounded(&full)?.len();
    // The same limits with on-grid coordinates, as an author would draw them.
    let gridded = full_limit_symbol(|i| (i % 160) as f64 * 1.25 - 100.0);
    let gridded_bytes = format::encode_bounded(&gridded)?.len();

    let refs = SampleRefs {
        component: DefinitionId(1),
        symbol: SymbolRef::new(DefinitionId(2), Revision::FIRST),
        package: PackageRef::new(DefinitionId(3), Revision::FIRST),
        footprint: FootprintRef::new(DefinitionId(4), Revision::FIRST),
        binding: BindingRef::new(DefinitionId(5), Revision::FIRST),
    };
    let typical = samples::two_terminal("Resistor", samples::Kind::RESISTOR, refs)
        .into_iter()
        .find_map(|e| match e.definition {
            crate::library::Definition::Symbol(s) => Some(s),
            _ => None,
        })
        .ok_or_else(|| Error::NotFound("sample symbol".into()))?;
    let hundred = vec![typical; 100];
    let typical_bytes = format::encode_bounded(&hundred)?.len();
    eprintln!(
        "encoded size (pretty JSON): full-limit symbol {full_bytes} B (on-grid coordinates {gridded_bytes} B); 100 typical symbols {typical_bytes} B; budget {} B",
        format::MAX_FILE_BYTES
    );
    // Measured on this checkout (G2: per-point corner radii, a rounded
    // rectangle, text line spacing): 1 670 744 B (1 370 150 B on the grid)
    // and 267 802 B (2.7 kB each), so ten full-limit revisions fit one file.
    // Require at least eight; typical revisions are negligible. The thin
    // headroom is an open risk recorded in docs/component-model.md.
    assert!(full_bytes * 8 < format::MAX_FILE_BYTES, "{full_bytes}");
    assert!(
        typical_bytes * 50 < format::MAX_FILE_BYTES,
        "{typical_bytes}"
    );
    Ok(())
}

/// The SOIC-14 MCU of the e2e UX run (finding 5): left pins 1–7 and right
/// pins 8–14 face each other in pairs (pin 6 RESET opposite pin 9 USB_DM).
const MCU_PINS: [&str; 14] = [
    "PA05", "PA08", "PA09", "PA14", "PA15", "RESET", "SWCLK", "SWDIO", "USB_DM", "USB_DP", "GND",
    "VDD", "SDA", "SCL",
];

/// On every row the left and the right pin names, measured as printed,
/// lie inside the body and keep at least a millimetre between them.
fn assert_names_fit(symbol: &SymbolDefinition, names: &[&str]) -> Result<()> {
    let m = crate::schematic::scene::LABEL_METRIC;
    let unit = &symbol.units[0];
    let Some(body) = unit.graphics.iter().find_map(|g| match &g.item {
        Item::Shape {
            shape: Shape::Rect { a, b, .. },
            ..
        } => Bounds::of([*a, *b]),
        _ => None,
    }) else {
        return Err(Error::NotFound("no body".into()));
    };
    let boxes: Vec<(f64, Bounds)> = unit
        .pins
        .iter()
        .map(|pin| {
            let name = names[pin.terminal.0 as usize - 1];
            let (at, rotation, justify) = pin.name_anchor();
            let b = m.bounds(
                at,
                symbol.pin_text_height,
                rotation,
                justify,
                TextSize::of(name, 1.6),
            );
            (pin.position[1], b)
        })
        .collect();
    for (y, b) in &boxes {
        assert!(
            b.min[0] >= body.min[0] && b.max[0] <= body.max[0],
            "{b:?} {body:?}"
        );
        for (y2, other) in &boxes {
            if (y - y2).abs() < 1e-9 && b.min[0] < other.min[0] {
                assert!(other.min[0] - b.max[0] >= 1.0, "{b:?} {other:?}");
            }
        }
    }
    for pin in &unit.pins {
        for c in pin.position {
            let k = c / graphics::SYMBOL_MODULE_MM;
            assert!((k - k.round()).abs() < 1e-9, "off grid {c}");
        }
    }
    Ok(())
}

#[test]
fn a_generated_ic_box_is_as_wide_as_its_pin_names() -> Result<()> {
    let terminals: Vec<(TerminalId, &str)> = MCU_PINS
        .iter()
        .enumerate()
        .map(|(k, name)| (TerminalId(k as u32 + 1), *name))
        .collect();
    let symbol = samples::named_block_symbol(DefinitionId(7), &terminals);
    assert_names_fit(&symbol, &MCU_PINS)
}

#[test]
fn an_existing_narrow_box_widens_when_its_pins_are_renamed() -> Result<()> {
    // Drawn while the pins were named by number, as a package template
    // draws it, then renamed by a pin table.
    let ids: Vec<TerminalId> = (1..=14).map(TerminalId).collect();
    let mut symbol = samples::block_symbol(DefinitionId(7), &ids);
    let before = symbol.clone();
    let name = |t: TerminalId| MCU_PINS.get(t.0 as usize - 1).map(|n| (*n).to_owned());
    assert!(samples::fit_pin_names(&mut symbol, name));
    assert_names_fit(&symbol, &MCU_PINS)?;
    // Rows, pin lengths and graphics IDs are kept: only the width changed.
    for (a, b) in symbol.units[0].pins.iter().zip(&before.units[0].pins) {
        assert_eq!(
            (a.terminal, a.position[1], a.length),
            (b.terminal, b.position[1], b.length)
        );
    }
    // Fitting again changes nothing.
    assert!(!samples::fit_pin_names(&mut symbol, name));
    Ok(())
}

/// Run 3, finding 10: a pin number longer than its stub ("A4B9,B4A9" on a
/// USB-C VBUS pin) starts at the body and runs outward on every side, so it
/// never reaches the name inside the body; a short one stays centred.
#[test]
fn a_long_pin_number_stays_clear_of_the_body() {
    let m = TextMetric::default();
    let height = 1.0;
    let text = "A4B9,B4A9";
    let width = text.len() as f64 * m.advance * height;
    let body = [0.0, 0.0];
    for (direction, position) in [
        (PinDirection::Right, [-2.54, 0.0]),
        (PinDirection::Left, [2.54, 0.0]),
        (PinDirection::Up, [0.0, -2.54]),
        (PinDirection::Down, [0.0, 2.54]),
    ] {
        let pin = Pin::new(TerminalId(1), position, direction, 2.54);
        assert_eq!(pin.body_end(), body);
        let (at, rotation, justify) = pin.number_anchor(width);
        let d = direction.vector();
        // Along the pin's axis the text's box runs from the body outward.
        let along = |p: [f64; 2]| p[0] * d[0] + p[1] * d[1];
        assert!(along(at).abs() < 1e-9, "{direction:?} starts at the body");
        let runs_out = match (rotation == 0.0, justify.h) {
            (true, HAlign::Right) | (false, HAlign::Right) => {
                matches!(direction, PinDirection::Right | PinDirection::Up)
            }
            (_, HAlign::Left) => {
                matches!(direction, PinDirection::Left | PinDirection::Down)
            }
            _ => false,
        };
        assert!(runs_out, "{direction:?}: {justify:?}");
        let (_, _, short) = pin.number_anchor(0.6);
        assert_eq!(short.h, HAlign::Center);
    }
}
