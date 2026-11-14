//! Per-instance text placement and the production check of legend against
//! the board, through public commands and queries. Expected positions come
//! from the placement transform worked by hand.
use crate::fabrication::preflight::{self, PreflightItem};
use crate::fabrication::{self, FabricationOptions};
use crate::ids::{GraphicId, Id};
use crate::layout::{self, LayoutCommand, PlacedItem};
use crate::library::graphics::{Item, PlacedText, Rotation};
use crate::model::{Layer, Project, TextPlacement};
use crate::rules::{RuleKey, RuleValue, RulesCommand};
use crate::{Engine, Error, InstanceCommand, Operation, instances};
use std::collections::BTreeSet;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The showcase's first placed part with a silkscreen designator, moved to
/// the middle of the board at `rotation_deg` on `layer`.
fn part_at(engine: &mut Engine, rotation_deg: f64, layer: Layer) -> Fallible<Id> {
    let project = engine.project();
    let [w, h] = project.board_size;
    let instance = project
        .placements
        .iter()
        .map(|p| p.instance)
        .find(|id| instances::text::target(project, *id, None).is_ok())
        .ok_or("no part with a designator")?;
    engine.apply(
        engine.revision(),
        Operation::Layout(LayoutCommand::Move {
            instance,
            position: [w / 2.0, h / 2.0],
            rotation_deg,
            layer,
        }),
    )?;
    Ok(instance)
}

fn designator_text(project: &Project, instance: Id) -> Fallible<PlacedText> {
    let graphic = instances::text::target(project, instance, None)?;
    layout::placed_graphics(project)
        .into_iter()
        .filter(|g| g.instance == instance && g.graphic == graphic)
        .find_map(|g| match g.item {
            PlacedItem::Text(text) => Some(text),
            PlacedItem::Shape { .. } => None,
        })
        .ok_or_else(|| "no placed designator".into())
}

fn set(instance: Id, placement: Option<TextPlacement>) -> Operation {
    Operation::Instance(InstanceCommand::SetTextPlacement {
        instance,
        graphic: None,
        placement,
    })
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

#[test]
fn an_offset_and_turn_follow_the_part_on_each_rotation_and_side() -> Fallible {
    let moved = TextPlacement {
        offset: [1.0, 0.25],
        turn: Rotation::R90,
    };
    // The footprint's +x and +y on the board: at 90° they are +y and -x; on
    // the bottom, reflected across the x-axis, +x and -y.
    for (rotation, layer, delta) in [
        (0.0, Layer::Top, [1.0, 0.25]),
        (90.0, Layer::Top, [-0.25, 1.0]),
        (0.0, Layer::Bottom, [1.0, -0.25]),
    ] {
        let mut engine = Engine::showcase()?;
        let id = part_at(&mut engine, rotation, layer)?;
        let before = designator_text(engine.project(), id)?;
        engine.apply(engine.revision(), set(id, Some(moved)))?;
        let after = designator_text(engine.project(), id)?;
        assert!(
            close(after.at, [before.at[0] + delta[0], before.at[1] + delta[1]]),
            "{rotation} {layer:?}: {:?} from {:?}",
            after.at,
            before.at
        );
        // A quarter turn counter-clockwise on top, clockwise seen through
        // the board from the top on the bottom.
        let turned = if layer == Layer::Top { 90.0 } else { -90.0 };
        assert!(
            (after.rotation_deg - (before.rotation_deg + turned).rem_euclid(360.0)).abs() < 1e-9,
            "{rotation} {layer:?}: {} from {}",
            after.rotation_deg,
            before.rotation_deg
        );
        assert_eq!(after.text, before.text);
        assert_eq!(after.mirrored, before.mirrored);
    }
    Ok(())
}

#[test]
fn the_command_is_one_undo_step_and_restating_it_is_none() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 0.0, Layer::Top)?;
    let plain = serde_json::to_value(engine.project())?;
    let revision = engine.revision();
    let moved = TextPlacement {
        offset: [0.5, -0.5],
        turn: Rotation::R180,
    };
    engine.apply(revision, set(id, Some(moved)))?;
    assert_eq!(engine.revision(), revision + 1);
    // The same placement again changes nothing.
    engine.apply(revision + 1, set(id, Some(moved)))?;
    assert_eq!(engine.revision(), revision + 1);
    let held = designator_text(engine.project(), id)?;
    engine.undo()?;
    assert_eq!(serde_json::to_value(engine.project())?, plain);
    engine.redo()?;
    assert_eq!(designator_text(engine.project(), id)?, held);
    // Clearing, and a zero placement, leave the document as it was.
    engine.apply(engine.revision(), set(id, None))?;
    assert_eq!(serde_json::to_value(engine.project())?, plain);
    let cleared = engine.revision();
    engine.apply(cleared, set(id, Some(TextPlacement::default())))?;
    assert_eq!(engine.revision(), cleared);
    Ok(())
}

#[test]
fn a_placement_follows_its_footprint_lineage_and_only_its_texts() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 0.0, Layer::Top)?;
    let moved = TextPlacement {
        offset: [1.0, 0.0],
        turn: Rotation::R0,
    };
    engine.apply(engine.revision(), set(id, Some(moved)))?;
    let mut project = engine.project().clone();
    let footprint = project
        .instance(id)
        .and_then(|i| i.physical)
        .map(|p| p.footprint);
    let kept = instances::text::target(&project, id, None)?;
    // A new revision of the same footprint keeps the texts it still has.
    instances::instance_mut(&mut project, id)?
        .text_placements
        .insert(GraphicId(u32::MAX), moved);
    instances::text::follow_footprint(&mut project, id, footprint);
    let held = |p: &Project| -> Vec<GraphicId> {
        p.instance(id)
            .map(|i| i.text_placements.keys().copied().collect())
            .unwrap_or_default()
    };
    assert_eq!(held(&project), vec![kept]);
    // Another footprint's ids mean nothing here.
    instances::text::follow_footprint(&mut project, id, None);
    assert_eq!(held(&project), Vec::<GraphicId>::new());
    Ok(())
}

#[test]
fn refusals_leave_the_document_unchanged() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 0.0, Layer::Top)?;
    let revision = engine.revision();
    let before = serde_json::to_value(engine.project())?;
    let footprint_graphics: Vec<(GraphicId, bool)> = {
        let project = engine.project();
        let instance = project.instance(id).ok_or("instance")?;
        let physical = instances::resolve(&project.library, instance)?
            .physical
            .ok_or("physical")?;
        physical
            .footprint
            .definition
            .graphics
            .iter()
            .map(|g| (g.id, matches!(g.item, Item::Text(_))))
            .collect()
    };
    let at = |offset| {
        Some(TextPlacement {
            offset,
            turn: Rotation::R0,
        })
    };
    let mut refused = vec![
        set(id, at([f64::NAN, 0.0])),
        set(id, at([f64::INFINITY, 0.0])),
        set(id, at([1000.5, 0.0])),
        set(Id::MAX, at([1.0, 0.0])),
        Operation::Instance(InstanceCommand::SetTextPlacement {
            instance: id,
            graphic: Some(GraphicId(u32::MAX)),
            placement: at([1.0, 0.0]),
        }),
    ];
    if let Some((shape, _)) = footprint_graphics.iter().find(|(_, text)| !text) {
        refused.push(Operation::Instance(InstanceCommand::SetTextPlacement {
            instance: id,
            graphic: Some(*shape),
            placement: at([1.0, 0.0]),
        }));
    }
    for operation in refused {
        let text = format!("{operation:?}");
        assert!(engine.apply(revision, operation).is_err(), "{text}");
        assert_eq!(engine.revision(), revision, "{text}");
        assert_eq!(serde_json::to_value(engine.project())?, before, "{text}");
    }
    // An outdated revision.
    let stale = engine.apply(revision + 7, set(id, at([1.0, 0.0])));
    assert!(matches!(stale, Err(Error::Stale { .. })), "{stale:?}");
    Ok(())
}

#[test]
fn the_request_parses_from_json_and_refuses_unknown_fields() -> Fallible {
    let parsed: InstanceCommand = serde_json::from_str(
        r#"{"command":"instance.set_text_placement",
            "params":{"instance":7,"placement":{"offset":[1.5,-2.0],"turn":"r90"}}}"#,
    )?;
    let InstanceCommand::SetTextPlacement {
        instance,
        graphic: None,
        placement: Some(placement),
    } = parsed
    else {
        return Err("wrong command".into());
    };
    assert_eq!(instance, 7);
    assert_eq!(placement.offset, [1.5, -2.0]);
    assert_eq!(placement.turn, Rotation::R90);
    for malformed in [
        r#"{"command":"instance.set_text_placement","params":{"instance":7,"x":1}}"#,
        r#"{"command":"instance.set_text_placement","params":{"placement":null}}"#,
        r#"{"command":"instance.set_text_placement",
            "params":{"instance":7,"placement":{"offset":[1.0]}}}"#,
        r#"{"command":"instance.set_text_placement",
            "params":{"instance":7,"placement":{"offset":[1.0,2.0],"turn":"r45"}}}"#,
    ] {
        assert!(
            serde_json::from_str::<InstanceCommand>(malformed).is_err(),
            "{malformed}"
        );
    }
    Ok(())
}

#[test]
fn a_placement_survives_the_file_and_a_bad_stored_one_is_refused() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 0.0, Layer::Top)?;
    engine.apply(
        engine.revision(),
        set(
            id,
            Some(TextPlacement {
                offset: [2.0, 3.0],
                turn: Rotation::R270,
            }),
        ),
    )?;
    let at = designator_text(engine.project(), id)?;
    let reopened = Engine::from_file(engine.to_file())?;
    assert_eq!(designator_text(reopened.project(), id)?, at);
    // A file holding an out-of-range offset does not open.
    let mut bad = engine.to_file();
    let project = &mut bad.project;
    let instance = instances::instance_mut(project, id)?;
    for placement in instance.text_placements.values_mut() {
        placement.offset = [5000.0, 0.0];
    }
    assert!(Engine::from_file(bad).is_err());
    Ok(())
}

/// Every drawn coordinate of a Gerber file, in nanometres.
fn gerber_points(text: &str) -> BTreeSet<(i64, i64)> {
    let mut points = BTreeSet::new();
    let (mut x, mut y) = (0i64, 0i64);
    for line in text.lines() {
        let Some(body) = line.strip_suffix('*') else {
            continue;
        };
        if !(body.ends_with("D01") || body.ends_with("D02")) {
            continue;
        }
        let number = |letter: char| -> Option<i64> {
            let from = body.find(letter)? + 1;
            let digits: String = body[from..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-')
                .collect();
            digits.parse().ok()
        };
        x = number('X').unwrap_or(x);
        y = number('Y').unwrap_or(y);
        points.insert((x, y));
    }
    points
}

#[test]
fn the_legend_gerber_prints_the_moved_text_where_the_command_put_it() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 90.0, Layer::Top)?;
    let legend = |engine: &Engine| -> Fallible<BTreeSet<(i64, i64)>> {
        let set = fabrication::generate(engine.project(), &FabricationOptions::default())?;
        let file = set
            .files
            .iter()
            .find(|f| f.name.ends_with("-legend-top.gbr"))
            .ok_or("no top legend")?;
        Ok(gerber_points(std::str::from_utf8(&file.bytes)?))
    };
    let before = legend(&engine)?;
    engine.apply(
        engine.revision(),
        set(
            id,
            Some(TextPlacement {
                offset: [1.0, 0.0],
                turn: Rotation::R0,
            }),
        ),
    )?;
    let after = legend(&engine)?;
    // The footprint's +1 mm x is +1 mm y on the board at 90°: every stroke
    // point of the text that moved moved by exactly that.
    let gone: Vec<_> = before.difference(&after).copied().collect();
    let come: BTreeSet<_> = after.difference(&before).copied().collect();
    assert!(!gone.is_empty());
    for (x, y) in gone {
        assert!(
            come.contains(&(x, y + 1_000_000)),
            "({x}, {y}) has no moved copy"
        );
    }
    Ok(())
}

fn findings(engine: &Engine) -> Fallible<Vec<PreflightItem>> {
    let report = preflight::report(
        engine.project(),
        engine.drafts(),
        None,
        engine.check_settings(),
    )?;
    Ok(report
        .items
        .into_iter()
        .filter(|i| i.code.starts_with("fabrication.legend_"))
        .collect())
}

/// How far the designator's ink reaches past its centre in board x, from
/// the stroke font's own output.
fn designator_extent(project: &Project, instance: Id) -> Fallible<(f64, f64)> {
    let text = designator_text(project, instance)?;
    let strokes = layout::legend::text_strokes(&text, &project.rules.effective().silkscreen);
    let half = strokes.width / 2.0;
    let xs = strokes.strokes.iter().flatten().map(|p| p[0]);
    let min = xs.clone().fold(f64::INFINITY, f64::min) - half;
    let max = xs.fold(f64::NEG_INFINITY, f64::max) + half;
    Ok((min, max))
}

/// An offset in the footprint's +x (rotation 0, top) that puts the ink's
/// left end `gap` mm from x = 0: negative is past the edge.
fn offset_to_left_edge(project: &Project, instance: Id, gap: f64) -> Fallible<f64> {
    Ok(gap - designator_extent(project, instance)?.0)
}

#[test]
fn legend_past_the_outline_is_a_warning_naming_the_item_until_it_is_moved_back() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 0.0, Layer::Top)?;
    engine.apply(engine.revision(), set(id, None))?;
    let name = engine
        .project()
        .instance(id)
        .map(|i| i.designator.clone())
        .ok_or("instance")?;
    let on_board = |items: &[PreflightItem]| {
        !items.iter().any(|i| {
            i.objects
                .iter()
                .any(|o| format!("{o:?}").contains(&format!("id: {id}")))
        })
    };
    assert!(on_board(&findings(&engine)?), "{:#?}", findings(&engine)?);
    // Half a millimetre past x = 0.
    let dx = offset_to_left_edge(engine.project(), id, -0.5)?;
    engine.apply(
        engine.revision(),
        set(
            id,
            Some(TextPlacement {
                offset: [dx, 0.0],
                turn: Rotation::R0,
            }),
        ),
    )?;
    let found = findings(&engine)?;
    let finding = found
        .iter()
        .find(|i| i.code == "fabrication.legend_outside")
        .ok_or("no finding")?;
    assert_eq!(
        finding.title,
        format!("Designator {name} runs outside the board")
    );
    assert_eq!(finding.severity, crate::issues::Severity::Warning);
    assert!(crate::issues::style::check(&finding.title).is_empty());
    assert!(crate::issues::style::check(finding.fix.as_deref().unwrap_or("x")).is_empty());
    // It warns; it does not block export by itself.
    assert!(!finding.blocks());
    // One micrometre on the board is clear while no clearance is asked.
    let dx = offset_to_left_edge(engine.project(), id, 0.001)? + dx;
    engine.apply(
        engine.revision(),
        set(
            id,
            Some(TextPlacement {
                offset: [dx, 0.0],
                turn: Rotation::R0,
            }),
        ),
    )?;
    assert!(on_board(&findings(&engine)?), "{:#?}", findings(&engine)?);
    // The rule asks for 0.2 mm: 1 µm is now too close, and says so.
    engine.apply(
        engine.revision(),
        Operation::Rules(RulesCommand::SetOverride {
            key: RuleKey::SilkEdgeClearance,
            value: RuleValue::Length(0.2),
        }),
    )?;
    let close = findings(&engine)?;
    let near = close
        .iter()
        .find(|i| i.code == "fabrication.legend_edge")
        .ok_or("no clearance finding")?;
    assert_eq!(
        near.title,
        format!("Designator {name} is 0.00 mm from the board edge; needs 0.2 mm")
    );
    Ok(())
}

#[test]
fn a_side_that_is_not_printed_has_no_legend_finding() -> Fallible {
    let mut engine = Engine::showcase()?;
    let id = part_at(&mut engine, 0.0, Layer::Top)?;
    let dx = offset_to_left_edge(engine.project(), id, -0.5)?;
    engine.apply(
        engine.revision(),
        set(
            id,
            Some(TextPlacement {
                offset: [dx, 0.0],
                turn: Rotation::R0,
            }),
        ),
    )?;
    assert!(!findings(&engine)?.is_empty());
    let mut project = engine.project().clone();
    project.stackup.silkscreen_top = false;
    let report = preflight::report(&project, engine.drafts(), None, &Default::default())?;
    assert!(
        report
            .items
            .iter()
            .all(|i| !i.code.starts_with("fabrication.legend_")
                || !i
                    .objects
                    .iter()
                    .any(|o| format!("{o:?}").contains(&format!("id: {id}")))),
        "an unprinted legend cannot run off the board"
    );
    Ok(())
}

/// The footprint and package drafts a starter row builds.
fn starter(
    name: &str,
) -> Fallible<(
    crate::library::FootprintDefinition,
    crate::library::PackageDefinition,
)> {
    use crate::library::templates::catalog::{NewFromTemplate, RowSource};
    let mut engine = Engine::profiled()?;
    let row = crate::library::templates::starter::row(name)?;
    let crate::Outcome::DraftsCreated { ids } = engine.apply(
        engine.revision(),
        Operation::Library(crate::LibraryCommand::NewFromTemplate(Box::new(
            NewFromTemplate {
                row: RowSource::Starter { name: name.into() },
                values: None,
                skeleton: row.definition.skeleton,
                name: name.into(),
                designator_prefix: None,
                symbol: None,
                without_fields: false,
            },
        ))),
    )?
    else {
        return Err("no drafts".into());
    };
    let (mut footprint, mut package) = (None, None);
    for id in ids {
        match engine.drafts().get(id).map(|d| &d.definition) {
            Some(crate::library::Definition::Footprint(f)) => footprint = Some(f.clone()),
            Some(crate::library::Definition::Package(p)) => package = Some(p.clone()),
            _ => {}
        }
    }
    Ok((footprint.ok_or("footprint")?, package.ok_or("package")?))
}

/// Run 4, finding 21: a part's default silkscreen designator lands clear of
/// its body on every turn and on either side; an explicit offset in the
/// Designator card is kept as given, even over the body.
#[test]
fn a_default_designator_stays_clear_of_the_body() -> Fallible {
    use crate::geometry::Bounds;
    use crate::library::graphics::TextMetric;
    for name in ["Chip resistor 0603 (1608 metric)", "SOT-23-3"] {
        let (footprint, package) = starter(name)?;
        let envelope = footprint.envelope_outline(&package);
        let mut engine = Engine::showcase()?;
        let id = part_at(&mut engine, 0.0, Layer::Top)?;
        let mut project = engine.project().clone();
        let footprint_id =
            instances::resolve(&project.library, project.instance(id).ok_or("part")?)?
                .physical
                .map(|p| p.footprint.id)
                .ok_or("physical")?;
        for published in project.library.footprints.iter_mut() {
            if published.id == footprint_id {
                published.definition = footprint.clone();
            }
        }
        for layer in [Layer::Top, Layer::Bottom] {
            for rotation in [0.0, 90.0, 180.0, 270.0] {
                let placement = project
                    .placements
                    .iter_mut()
                    .find(|p| p.instance == id)
                    .ok_or("placement")?;
                placement.rotation_deg = rotation;
                placement.layer = layer;
                let t = layout::placement_transform(placement, &footprint.datum);
                let body = Bounds::of(
                    [
                        envelope.min,
                        [envelope.max[0], envelope.min[1]],
                        envelope.max,
                        [envelope.min[0], envelope.max[1]],
                    ]
                    .map(|c| t.apply(c)),
                )
                .ok_or("body")?;
                let text = designator_text(&project, id)?;
                assert!(
                    !text.bounds(TextMetric::default()).intersects(&body),
                    "{name} at {rotation}° on {layer:?}: {:?} over {body:?}",
                    text.bounds(TextMetric::default())
                );
            }
        }
        // Moved by hand onto the body (on the top, unturned, the
        // footprint's axes are the board's): kept there.
        let placement = project
            .placements
            .iter_mut()
            .find(|p| p.instance == id)
            .ok_or("placement")?;
        placement.rotation_deg = 0.0;
        placement.layer = Layer::Top;
        let centre = placement.position;
        let before = designator_text(&project, id)?;
        let graphic = instances::text::target(&project, id, None)?;
        let offset = [centre[0] - before.at[0], centre[1] - before.at[1]];
        let instance = project
            .instances
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or("instance")?;
        instance.text_placements.insert(
            graphic,
            TextPlacement {
                offset,
                turn: Rotation::R0,
            },
        );
        let after = designator_text(&project, id)?;
        assert!(
            (after.at[0] - centre[0]).abs() < 1e-6 && (after.at[1] - centre[1]).abs() < 1e-6,
            "{name}: {:?} vs {centre:?}",
            after.at
        );
    }
    Ok(())
}
