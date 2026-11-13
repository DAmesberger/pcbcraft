//! Slice A3's gate: in the live view's model every part is its package
//! revision's own design, placed by the document's transform, and lies
//! where the exact build's solved joints hold it (every vertex within
//! 1 µm), on both sides and at every turn, for every kind of part model:
//! generated, envelope (with a turned datum and model placement), library
//! model and part file. No design of the board is made.
use super::*;
use crate::project::tests::{envelope_demo, instances, library_demo, set_body};
use crate::project::{Flat, Rebuilt, build, build_flat, update};
use pcbcraft_core::{Engine, Id, Layer, Project};
use std::collections::BTreeSet;

/// The turns every kind of part model is placed at, on each side.
const TURNS: [f64; 6] = [0.0, 90.0, 180.0, 270.0, 33.0, -127.5];

fn demo() -> Result<Project> {
    Ok(Engine::demo()
        .map_err(|error| Error::Verification(error.to_string()))?
        .project()
        .clone())
}

fn meshed(project: &Project) -> Result<NativeModel> {
    build_flat(project, &SourceCache::default(), None, Flat::Meshed)
}

/// `project` with R1 on the top turned by `turn` and R2 on the bottom
/// turned the other way, so each turn is seen on both sides.
fn turned(project: &Project, turn: f64) -> Result<Project> {
    let (r1, r2) = instances(project)?;
    let mut project = project.clone();
    for placement in &mut project.placements {
        if placement.instance == r1 {
            placement.layer = Layer::Top;
            placement.rotation_deg = turn;
        } else if placement.instance == r2 {
            placement.layer = Layer::Bottom;
            placement.rotation_deg = -turn;
        }
    }
    Ok(project)
}

/// The exact build and the meshed model of `project` place its parts
/// alike; returns how closely.
fn placed_alike(project: &Project, what: &str) -> Result<Parity> {
    let exact = build(project)?;
    let live = meshed(project)?;
    let parity = parity(&exact, &live)?;
    assert!(
        parity.differences.is_empty(),
        "{what}: {:#?}",
        parity.differences
    );
    assert!(parity.bodies > 0, "{what}: no part body compared");
    Ok(parity)
}

/// Every turn of `project`'s parts on both sides, against the exact build.
fn every_turn(project: &Project, what: &str) -> Result<()> {
    let mut worst = 0.0_f64;
    let mut vertices = 0;
    for turn in TURNS {
        let parity = placed_alike(&turned(project, turn)?, &format!("{what} at {turn}°"))?;
        worst = worst.max(parity.worst_mm);
        vertices += parity.vertices;
    }
    eprintln!("{what}: {vertices} vertices, at most {worst:.2e} mm from the exact build's");
    Ok(())
}

/// Generated package models (the demo's chips).
#[test]
fn generated_models_sit_where_the_exact_build_holds_them() -> Result<()> {
    every_turn(&demo()?, "generated chips")
}

/// Envelope boxes, their package placement turned about two axes and their
/// footprint datum moved and turned.
#[test]
fn envelopes_sit_where_the_exact_build_holds_them() -> Result<()> {
    let mut project = envelope_demo()?;
    let package = &mut project.library.packages[0].definition;
    set_body(package, None);
    package.placement = crate::test_units::placement([0.25, -0.4, 0.1], [90.0, 0.0, 90.0]);
    project.library.footprints[0].definition.datum = pcbcraft_core::library::Datum {
        offset: crate::test_units::pt([0.3, -0.4]),
        rotation_mdeg: crate::test_units::deg(90.0),
    };
    every_turn(&project, "envelopes")
}

/// A library model (the original glTF fixture).
#[test]
fn library_models_sit_where_the_exact_build_holds_them() -> Result<()> {
    every_turn(&library_demo()?, "library models")
}

/// A part file (the ESP32 example's USB4105), placed through
/// `layout.place_part` on both sides at every turn.
#[test]
fn part_files_sit_where_the_exact_build_holds_them() -> crate::board::tests::Fallible<()> {
    use crate::board::package_datum_tests::{USB4105, spec};
    use crate::board::tests::apply;
    use pcbcraft_core::layout::LayoutCommand;
    use pcbcraft_core::{Operation, Outcome};
    let mut engine = Engine::demo()?;
    let spec = spec(USB4105)?;
    let mut x = -30.0;
    for layer in [Layer::Top, Layer::Bottom] {
        for turn in TURNS {
            let outcome = apply(
                &mut engine,
                Operation::Layout(LayoutCommand::PlacePart {
                    part: Box::new(spec.clone()),
                    position: [x, if layer == Layer::Top { 8.0 } else { -8.0 }],
                    rotation_deg: turn,
                    layer,
                    designator: None,
                }),
            )?;
            if !matches!(outcome, Outcome::PartPlaced { .. }) {
                return Err(format!("{outcome:?}").into());
            }
            x += 12.0;
        }
    }
    let parity = placed_alike(engine.project(), "part files")?;
    eprintln!(
        "part files: {} bodies, {} vertices, at most {:.2e} mm from the exact build's",
        parity.bodies, parity.vertices, parity.worst_mm
    );
    Ok(())
}

/// The check sees a part drawn 2 µm off, one turned over, and one missing.
#[test]
fn the_placement_check_finds_a_moved_flipped_or_missing_part() -> Result<()> {
    let project = turned(&demo()?, 33.0)?;
    let exact = build(&project)?;
    let live = meshed(&project)?;
    let (r1, _) = instances(&project)?;
    let first = live
        .geometry
        .iter()
        .position(|g| part_bodies(&live, r1).iter().any(|b| std::ptr::eq(*b, g)))
        .ok_or_else(|| Error::Verification("R1 has no body".into()))?;
    let edit = |change: &dyn Fn(&mut Place)| -> Result<Vec<String>> {
        let mut off = meshed(&project)?;
        if let Some(place) = off.geometry.get_mut(first).and_then(|g| g.place.as_mut()) {
            change(place);
        }
        Ok(parity(&exact, &off)?.differences)
    };
    let moved = edit(&|p| p.0[0][3] += 0.002)?;
    assert_eq!(moved.len(), 1, "{moved:#?}");
    assert!(moved[0].contains("from the exact build's"));
    let flipped = edit(&|p| {
        *p = then(p, &place_of(&Transform3::rot_x(180.0)));
    })?;
    assert_eq!(flipped.len(), 1, "{flipped:#?}");
    let mut missing = meshed(&project)?;
    missing.geometry.remove(first);
    let differences = parity(&exact, &missing)?.differences;
    assert_eq!(differences.len(), 1, "{differences:#?}");
    assert!(differences[0].contains("bodies exactly"));
    Ok(())
}

/// The part bodies `model` draws for `instance`.
fn part_bodies(model: &NativeModel, instance: Id) -> Vec<&Geometry> {
    let picks: BTreeSet<u64> = model
        .sources
        .iter()
        .filter(|s| s.instance == Some(instance) && s.pad.is_none())
        .map(|s| s.pick_id)
        .collect();
    model
        .geometry
        .iter()
        .filter(|g| !g.kind.is_appearance() && g.item.is_some_and(|i| picks.contains(&i)))
        .collect()
}

/// The live model has no design of the board. Each package revision is
/// one design of its own, evaluated once: the demo's two resistors, of
/// one package revision, draw the same meshes, each placed where it lies.
#[test]
fn the_live_model_places_one_design_per_package_revision_and_no_board() -> Result<()> {
    let project = demo()?;
    let live = meshed(&project)?;
    assert!(live.native().is_none(), "no native design of the board");
    assert!(live.world().is_none());
    let revisions: BTreeSet<_> = project
        .placements
        .iter()
        .filter_map(|p| project.instance(p.instance))
        .filter_map(|i| pcbcraft_core::instances::resolve(&project.library, i).ok())
        .filter_map(|r| r.physical)
        .map(|p| (p.package.id, p.package.revision))
        .collect();
    assert_eq!(revisions.len(), 1, "the demo's resistors share a package");
    assert_eq!(live.part_designs(), revisions.len());
    assert_eq!(live.models.len(), 1, "one definition per package revision");
    let (r1, r2) = instances(&project)?;
    let (a, b) = (part_bodies(&live, r1), part_bodies(&live, r2));
    assert!(!a.is_empty() && a.len() == b.len());
    for (a, b) in a.iter().zip(&b) {
        assert!(Arc::ptr_eq(&a.mesh, &b.mesh), "one mesh per design body");
        assert!(a.place.is_some() && a.place != b.place);
    }
    Ok(())
}

/// Moving and turning a part (and widening a track) evaluates no part
/// design again: the next live model takes every design and mesh it
/// kept, runs no native command, and the moved part lies where the exact
/// build of the edited board holds it.
#[test]
fn an_edit_evaluates_no_part_design_again() -> Result<()> {
    let mut project = demo()?;
    let library = SourceCache::default();
    let live = build_flat(&project, &library, None, Flat::Meshed)?;
    let (r1, _) = instances(&project)?;
    let meshes: Vec<usize> = part_bodies(&live, r1)
        .iter()
        .map(|g| Arc::as_ptr(&g.mesh) as usize)
        .collect();
    for placement in &mut project.placements {
        if placement.instance == r1 {
            placement.position[0] += 1.5;
            placement.rotation_deg += 90.0;
        }
    }
    if let Some(track) = project.tracks.first_mut() {
        track.width += 0.05;
    }
    let started = std::time::Instant::now();
    let (moved, how) = update(live, &project, &library, None)?;
    let took = started.elapsed();
    assert_eq!(how, Rebuilt::InPlace);
    assert_eq!(moved.flat(), Flat::Meshed);
    let again: Vec<usize> = part_bodies(&moved, r1)
        .iter()
        .map(|g| Arc::as_ptr(&g.mesh) as usize)
        .collect();
    assert_eq!(again, meshes, "the part's design was not made again");
    let parity = parity(&build(&project)?, &moved)?;
    assert!(parity.differences.is_empty(), "{:#?}", parity.differences);
    eprintln!("demo part move and track edit: live model in {took:?}");
    Ok(())
}
