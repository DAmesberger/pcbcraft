//! Drawing through public commands: `library.edit_graphics` applies edits in
//! order and atomically, rounded corners and multi-line text are validated,
//! bounded, picked and diffed, and line breaks lay out in the core metric.
use super::support::*;
use crate::completeness::CheckTarget;
use crate::geometry::{Bounds, Shape};
use crate::library::graphics::{
    self, GraphicEdit, GraphicTarget, MAX_TEXT_LINES, TEXT_LINE_SPACING, text_lines,
};
use crate::library::{
    Definition, Fill, Graphic, HAlign, Item, Justify, Pin, PinDirection, Rotation, Text,
    TextContent, TextMetric, TextSize, VAlign,
};
use crate::{
    DefinitionId, Engine, GraphicId, LibraryCommand, Operation, Outcome, Result, TerminalId, UnitId,
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

fn edit_graphics(
    engine: &mut Engine,
    draft: DefinitionId,
    edits: Vec<GraphicEdit>,
) -> Result<Outcome> {
    apply(
        engine,
        Operation::Library(LibraryCommand::EditGraphics { draft, edits }),
    )
}

fn stroke(shape: Shape) -> Item {
    Item::Shape {
        shape,
        width: 0.25,
        fill: Fill::None,
    }
}

fn literal(text: &str, justify: Justify, line_spacing: f64) -> Item {
    Item::Text(Text {
        content: TextContent::Literal(text.into()),
        at: [0.0, 0.0],
        height: 1.0,
        rotation: Rotation::R0,
        justify,
        visible: true,
        line_spacing,
    })
}

fn symbol(engine: &Engine, id: DefinitionId) -> Result<crate::library::SymbolDefinition> {
    match body(engine, id)? {
        Definition::Symbol(s) => Ok(s),
        other => Err(crate::Error::Invalid(format!("not a symbol: {other:?}"))),
    }
}

/// Edits apply in order as one undo step and report the IDs they added.
#[test]
fn edit_graphics_applies_in_order_as_one_undo_step() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Op"))?;
    let before = symbol(&engine, f.symbol)?;
    let unit = GraphicTarget::Unit(UnitId(1));
    let outcome = edit_graphics(
        &mut engine,
        f.symbol,
        vec![
            GraphicEdit::Add {
                target: unit,
                item: stroke(Shape::polygon(vec![[-2.5, -2.5], [-2.5, 2.5], [2.5, 0.0]])),
            },
            GraphicEdit::Add {
                target: unit,
                item: stroke(Shape::Rect {
                    a: [-1.0, -1.0],
                    b: [1.0, 1.0],
                    radius: 0.5,
                }),
            },
            GraphicEdit::Translate {
                ids: vec![GraphicId(before.next_local)],
                delta: [2.5, 0.0],
            },
            GraphicEdit::Remove {
                id: GraphicId(before.next_local + 1),
            },
            GraphicEdit::SetPin {
                unit: UnitId(1),
                pin: Pin::new(TerminalId(2), [7.5, 0.0], PinDirection::Left, 2.5),
            },
            GraphicEdit::SetPinTextHeight { height: 1.0 },
        ],
    )?;
    let first = GraphicId(before.next_local);
    assert_eq!(
        outcome,
        Outcome::GraphicsEdited {
            added: vec![first, GraphicId(before.next_local + 1)]
        }
    );
    let after = symbol(&engine, f.symbol)?;
    assert_eq!(after.next_local, before.next_local + 2);
    assert_eq!(
        after.units[0].graphics.len(),
        1,
        "the rectangle was removed"
    );
    assert_eq!(
        after.units[0].graphics[0],
        Graphic {
            id: first,
            item: stroke(Shape::polygon(vec![[0.0, -2.5], [0.0, 2.5], [5.0, 0.0]])),
        }
    );
    assert_eq!(after.pin_text_height, 1.0);
    let pin = after.units[0].pin(TerminalId(2)).cloned();
    assert_eq!(
        pin.map(|p| (p.position, p.direction)),
        Some(([7.5, 0.0], PinDirection::Left))
    );
    engine.undo()?;
    assert_eq!(symbol(&engine, f.symbol)?, before, "one undo step");
    Ok(())
}

/// A failing edit anywhere in the list leaves the draft and the revision
/// counter unchanged; nothing is inferred.
#[test]
fn failed_graphic_edits_leave_the_draft_unchanged() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Op"))?;
    let unit = GraphicTarget::Unit(UnitId(1));
    let add = GraphicEdit::Add {
        target: unit,
        item: stroke(Shape::Line {
            a: [0.0, 0.0],
            b: [1.0, 0.0],
        }),
    };
    let failing: Vec<(&str, Vec<GraphicEdit>)> = vec![
        ("no edits", vec![]),
        (
            "unknown graphic",
            vec![add.clone(), GraphicEdit::Remove { id: GraphicId(999) }],
        ),
        (
            "unknown unit",
            vec![GraphicEdit::Add {
                target: GraphicTarget::Unit(UnitId(77)),
                item: stroke(Shape::Line {
                    a: [0.0, 0.0],
                    b: [1.0, 0.0],
                }),
            }],
        ),
        (
            "terminal not on the component",
            vec![
                add.clone(),
                GraphicEdit::SetPin {
                    unit: UnitId(1),
                    pin: Pin::new(TerminalId(9), [0.0, 0.0], PinDirection::Right, 2.5),
                },
            ],
        ),
        (
            "translated twice",
            vec![GraphicEdit::Translate {
                ids: vec![],
                delta: [1.0, 0.0],
            }],
        ),
        (
            "radii not one per point",
            vec![GraphicEdit::Add {
                target: unit,
                item: stroke(Shape::Polyline {
                    points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
                    radii: vec![0.5],
                }),
            }],
        ),
        (
            "negative radius",
            vec![GraphicEdit::Add {
                target: unit,
                item: stroke(Shape::Rect {
                    a: [0.0, 0.0],
                    b: [1.0, 1.0],
                    radius: -0.1,
                }),
            }],
        ),
        (
            "line spacing out of range",
            vec![GraphicEdit::Add {
                target: unit,
                item: literal("a\nb", Justify::default(), 9.0),
            }],
        ),
        (
            "pin text too small",
            vec![add.clone(), GraphicEdit::SetPinTextHeight { height: 0.0 }],
        ),
        (
            "pin of a terminal without one",
            vec![
                GraphicEdit::RemovePin {
                    terminal: TerminalId(1),
                },
                GraphicEdit::RemovePin {
                    terminal: TerminalId(1),
                },
            ],
        ),
    ];
    for (why, edits) in failing {
        let document = engine.document().clone();
        let revision = engine.revision();
        let result = edit_graphics(&mut engine, f.symbol, edits);
        assert!(result.is_err(), "{why}: {result:?}");
        assert_eq!(engine.document(), &document, "{why}");
        assert_eq!(engine.revision(), revision, "{why}");
    }
    // Footprints take no graphic edits yet.
    let result = edit_graphics(&mut engine, f.footprint, vec![add]);
    assert!(result.is_err());
    Ok(())
}

/// A corner rounded beyond its edges, or an open end, blocks publishing
/// with `graphic.radius_too_large`; a fitting radius publishes.
#[test]
fn oversized_corner_radii_block_publishing() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Op"))?;
    let unit = GraphicTarget::Unit(UnitId(1));
    let bad = [
        Shape::Rect {
            a: [-2.5, -2.5],
            b: [2.5, 0.0],
            radius: 1.3,
        },
        Shape::Polyline {
            points: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
            radii: vec![0.2, 0.0, 0.0],
        },
        Shape::Polygon {
            points: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            radii: vec![2.0, 2.0, 2.0],
        },
    ];
    for shape in bad {
        let outcome = edit_graphics(
            &mut engine,
            f.symbol,
            vec![GraphicEdit::Add {
                target: unit,
                item: stroke(shape.clone()),
            }],
        )?;
        let Outcome::GraphicsEdited { added } = outcome else {
            return Err(crate::Error::Invalid("no outcome".into()));
        };
        let report = check(&engine, CheckTarget::Draft(f.symbol))?;
        let blocking: Vec<_> = report
            .findings
            .iter()
            .filter(|fi| fi.code == "graphic.radius_too_large")
            .collect();
        assert_eq!(blocking.len(), 1, "{shape:?}: {:?}", report.findings);
        assert!(blocking[0].blocks.is_some());
        edit_graphics(
            &mut engine,
            f.symbol,
            vec![GraphicEdit::Remove { id: added[0] }],
        )?;
    }
    edit_graphics(
        &mut engine,
        f.symbol,
        vec![GraphicEdit::Add {
            target: unit,
            item: stroke(Shape::Rect {
                a: [-2.5, -2.5],
                b: [2.5, 2.5],
                radius: 2.5,
            }),
        }],
    )?;
    publish(&mut engine, f.all())?;
    Ok(())
}

/// Line breaks lay out in the core metric: the block is as wide as its
/// longest line and `1 + (lines − 1) × spacing` cap heights tall, and its
/// justification places the whole block.
#[test]
fn multi_line_text_bounds_follow_lines_spacing_and_justification() {
    let m = TextMetric::default();
    let left_top = Justify {
        h: HAlign::Left,
        v: VAlign::Top,
    };
    let size = TextSize::of("A\nBBB", 2.0);
    assert_eq!((size.columns, size.lines), (3, 2));
    // 3 × 0.6 wide, 1 + 1 × 2 tall, hanging below the anchor.
    let b = m.bounds([0.0, 0.0], 1.0, 0.0, left_top, size);
    assert!(close(b, [0.0, -3.0], [1.8, 0.0]), "{b:?}");
    let centred = Justify {
        h: HAlign::Center,
        v: VAlign::Middle,
    };
    let b = m.bounds([10.0, 10.0], 1.0, 0.0, centred, size);
    assert!(close(b, [9.1, 8.5], [10.9, 11.5]), "{b:?}");
    // A quarter turn stands the block up.
    let b = m.bounds([0.0, 0.0], 1.0, 90.0, left_top, size);
    assert!(close(b, [0.0, 0.0], [3.0, 1.8]), "{b:?}");
    // Each line centres on its own cap box inside the block.
    let lines = text_lines("A\nBBB", [0.0, 0.0], 1.0, 0.0, left_top, 2.0);
    let at: Vec<_> = lines.iter().map(|l| (l.text, l.at)).collect();
    assert_eq!(at, vec![("A", [0.0, -0.5]), ("BBB", [0.0, -2.5])]);
    let bottom = Justify {
        h: HAlign::Right,
        v: VAlign::Bottom,
    };
    let lines = text_lines("A\nB\nC", [0.0, 0.0], 2.0, 90.0, bottom, 1.5);
    let at: Vec<_> = lines.iter().map(|l| l.at).collect();
    // Block 2 × (1 + 2 × 1.5) = 8 mm; lines 3 mm apart, turned 90°.
    assert!(
        near(at[0], [-7.0, 0.0]) && near(at[1], [-4.0, 0.0]) && near(at[2], [-1.0, 0.0]),
        "{at:?}"
    );
    // One line goes where a one-line text goes.
    let one = text_lines("R1", [1.0, 2.0], 1.5, 0.0, centred, TEXT_LINE_SPACING);
    assert_eq!(one[0].at, [1.0, 2.0]);
    assert_eq!(TextSize::of("R1", 1.6), TextSize::line(2));
}

fn near(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

fn close(b: Bounds, min: [f64; 2], max: [f64; 2]) -> bool {
    near(b.min, min) && near(b.max, max)
}

/// Multi-line literal text is printable, places through `placed_unit` with
/// its spacing, picks by its whole block and is bounded to 16 lines.
#[test]
fn multi_line_text_publishes_places_and_picks_as_one_block() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Op"))?;
    let unit = GraphicTarget::Unit(UnitId(1));
    let left_top = Justify {
        h: HAlign::Left,
        v: VAlign::Top,
    };
    let outcome = edit_graphics(
        &mut engine,
        f.symbol,
        vec![GraphicEdit::Add {
            target: unit,
            item: literal("GAIN\nx10", left_top, 2.0),
        }],
    )?;
    let Outcome::GraphicsEdited { added } = outcome else {
        return Err(crate::Error::Invalid("no outcome".into()));
    };
    let s = symbol(&engine, f.symbol)?;
    // The second line (2 mm below the first) picks the same graphic.
    assert_eq!(
        drawn_unit(&engine, &f, UnitId(1))?.hit([0.5, -2.5], 0.1, TextMetric::default()),
        Some(crate::library::SymbolHit::Graphic(added[0]))
    );
    let bounds = s.units[0].bounds(TextMetric::default());
    assert!(bounds.is_some_and(|b| b.min[1] <= -3.0), "{bounds:?}");
    publish(&mut engine, f.all())?;
    // Control characters other than line breaks, and more than 16 lines,
    // are refused as text.
    assert!(graphics::printable_text("a\nb"));
    assert!(!graphics::printable_text("a\tb"));
    assert!(!graphics::printable_text("a\r\nb"));
    assert!(!graphics::printable_text(&"x\n".repeat(MAX_TEXT_LINES)));
    assert!(graphics::printable_text(&"x\n".repeat(MAX_TEXT_LINES - 1)));
    Ok(())
}

/// The update preview reports a changed corner radius as a presentation
/// change of that graphic.
#[test]
fn a_changed_corner_radius_is_a_presentation_diff() -> Result<()> {
    let mut engine = Engine::default();
    let f = draft(&mut engine, &two("Op"))?;
    let unit = GraphicTarget::Unit(UnitId(1));
    let rect = |radius| {
        stroke(Shape::Rect {
            a: [-2.5, -2.5],
            b: [2.5, 2.5],
            radius,
        })
    };
    let Outcome::GraphicsEdited { added } = edit_graphics(
        &mut engine,
        f.symbol,
        vec![GraphicEdit::Add {
            target: unit,
            item: rect(0.0),
        }],
    )?
    else {
        return Err(crate::Error::Invalid("no outcome".into()));
    };
    let first = publish(&mut engine, f.all())?;
    let symbol_ref = first
        .iter()
        .find(|r| r.id == f.symbol)
        .copied()
        .ok_or_else(|| crate::Error::Invalid("symbol not published".into()))?;
    // Publishing keeps the draft, now based on the new revision.
    edit_graphics(
        &mut engine,
        f.symbol,
        vec![GraphicEdit::Replace {
            id: added[0],
            item: rect(1.0),
        }],
    )?;
    let (Definition::Symbol(old), Definition::Symbol(new)) =
        (body_of(&engine, symbol_ref)?, body(&engine, f.symbol)?)
    else {
        return Err(crate::Error::Invalid("not symbols".into()));
    };
    assert_eq!(
        crate::updates::graphic_diff(&old.units[0].graphics, &new.units[0].graphics),
        (0, 0, 1),
        "added, removed, changed"
    );
    Ok(())
}

fn body_of(engine: &Engine, r: crate::DefinitionRef) -> Result<Definition> {
    engine
        .project()
        .library
        .definition(r)
        .map(|(_, d)| d)
        .ok_or_else(|| crate::Error::NotFound("revision".into()))
}
