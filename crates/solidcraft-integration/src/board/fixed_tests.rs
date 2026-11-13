//! Fixed geometry an enclosure link owns in the board sketch (slice B4,
//! `interface::housing`): a housing reference's trace enters the sketch as
//! a construction line between fixed points; a board edge dimensioned to
//! it follows when an update moves it; a trace that changed shape is lost
//! and keeps its last place; removing the reference removes the line and
//! the dimension to it.
use super::tests::{Fallible, apply, edit, sketch, sketched};
use pcbcraft_core::ids::ContentHash;
use pcbcraft_core::interface::housing::{HousingCommand, HousingRead, NewReference, ReferenceRead};
use pcbcraft_core::layout::outline::BoardEdit;
use pcbcraft_core::library::footprint_source::{
    DimKind, DimValue, SketchConstraintId, SketchEdit, SketchPointId, SketchRef,
};
use pcbcraft_core::model::dimension::{
    BodyName, FaceName, FeatureGeometry, HousingRefId, ReferenceStatus,
};
use pcbcraft_core::{Engine, Operation};

fn read(references: Vec<ReferenceRead>) -> HousingRead {
    HousingRead {
        enclosure_hint: "housing.solidcraft".into(),
        design_hint: "board.solidcraft".into(),
        component: "Board".into(),
        file_sha256: ContentHash([1; 32]),
        board_revision: None,
        references,
        bindings: Vec::new(),
        consumers: Vec::new(),
    }
}

fn line(y: f64, width: f64) -> FeatureGeometry {
    FeatureGeometry::Line {
        a: [-5.0, y],
        b: [width + 5.0, y],
    }
}

fn top(engine: &Engine) -> Fallible<f64> {
    let s = sketch(engine)?;
    let e = s
        .solved
        .edges
        .iter()
        .find(|e| e.name.0 == "top")
        .ok_or("top")?;
    Ok(e.a[1])
}

/// The sketch's ids that `pick` chooses from its canonical JSON.
fn find(
    engine: &Engine,
    list: &str,
    pick: impl Fn(&serde_json::Value) -> bool,
) -> Fallible<String> {
    let s = sketch(engine)?;
    let json: serde_json::Value = serde_json::from_str(&s.sketch.0)?;
    Ok(json[list]
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| pick(v))
        .and_then(|v| v["id"].as_str())
        .ok_or(list.to_owned())?
        .to_owned())
}

#[test]
fn a_board_edge_follows_the_fixed_line_a_reference_owns() -> Fallible<()> {
    let mut engine = sketched()?;
    let [width, height] = engine.project().board_size;
    apply(
        &mut engine,
        Operation::Housing(HousingCommand::RecordLink {
            read: read(Vec::new()),
        }),
    )?;
    apply(
        &mut engine,
        Operation::Housing(HousingCommand::RecordReference {
            reference: NewReference {
                body: BodyName("Far wall".into()),
                face: FaceName("F1:side:e1".into()),
                pick: [0.0, height + 1.0, 0.0],
                name: "Far wall · face".into(),
                geometry: line(height + 1.0, width),
            },
        }),
    )?;
    let owned = engine
        .project()
        .housing
        .as_ref()
        .and_then(|l| l.references.first())
        .map(|r| r.sketch.clone())
        .ok_or("the reference owns geometry")?;
    let SketchRef::Curve(curve) = owned.clone() else {
        return Err("a line".into());
    };
    // The fixed line cannot be dragged by the sketch's own edits.
    let point = find(&engine, "points", |p| {
        p["fixed"] == true && p["id"] != "origin"
    })?;
    assert!(
        edit(
            &mut engine,
            vec![BoardEdit::Sketch {
                edit: SketchEdit::Drag {
                    point: SketchPointId(point),
                    to: [0.0, 0.0],
                },
            }],
        )
        .is_err()
    );
    // The height gives way to 1 mm from the top-right corner to the line.
    let height_k = find(&engine, "constraints", |k| k["param"] == "height")?;
    let corner = find(&engine, "points", |p| {
        p["pos"][0].as_f64() == Some(width) && p["pos"][1].as_f64() == Some(height)
    })?;
    edit(
        &mut engine,
        vec![
            BoardEdit::Sketch {
                edit: SketchEdit::Delete {
                    ids: vec![SketchRef::Constraint(SketchConstraintId(height_k))],
                },
            },
            BoardEdit::Sketch {
                edit: SketchEdit::AddDimension {
                    kind: DimKind::Distance,
                    refs: vec![
                        SketchRef::Point(SketchPointId(corner)),
                        SketchRef::Curve(curve),
                    ],
                    value: DimValue::Fixed(1.0),
                },
            },
        ],
    )?;
    assert!((top(&engine)? - height).abs() <= 1e-9);

    // The wall moves 2 mm: the edge follows.
    let moved = ReferenceRead {
        id: HousingRefId(1),
        status: ReferenceStatus::Resolved,
        pick: Some([0.0, height + 3.0, 0.0]),
        geometry: Some(line(height + 3.0, width)),
        note: String::new(),
    };
    apply(
        &mut engine,
        Operation::Housing(HousingCommand::RecordUpdate {
            read: read(vec![moved]),
        }),
    )?;
    assert!((top(&engine)? - (height + 2.0)).abs() <= 1e-9);
    assert!((engine.project().board_size[1] - (height + 2.0)).abs() <= 1e-9);

    // A trace that changed shape is lost and keeps its last place.
    let changed = ReferenceRead {
        id: HousingRefId(1),
        status: ReferenceStatus::Resolved,
        pick: None,
        geometry: Some(FeatureGeometry::Point { at: [0.0, 0.0] }),
        note: String::new(),
    };
    apply(
        &mut engine,
        Operation::Housing(HousingCommand::RecordUpdate {
            read: read(vec![changed]),
        }),
    )?;
    let reference = engine
        .project()
        .housing
        .as_ref()
        .and_then(|l| l.references.first().cloned())
        .ok_or("kept")?;
    assert_eq!(reference.status, ReferenceStatus::Lost);
    assert_eq!(reference.geometry, line(height + 3.0, width));
    assert!((top(&engine)? - (height + 2.0)).abs() <= 1e-9);

    // Removed: the line and the dimension to it go; the board keeps its
    // shape.
    apply(
        &mut engine,
        Operation::Housing(HousingCommand::RemoveReference {
            id: HousingRefId(1),
        }),
    )?;
    let s = sketch(&engine)?;
    assert!(!s.sketch.0.contains(&format!(
        "\"{}\"",
        match &owned {
            SketchRef::Curve(c) => c.0.as_str(),
            _ => "",
        }
    )));
    assert!((top(&engine)? - (height + 2.0)).abs() <= 1e-9);
    Ok(())
}
