#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice B3's exported design on the demo board, with an external opening
//! on its package: the slots name faces Solidcraft really made, a slot's
//! names do not depend on what came before it, the tiers share section 1,
//! the metadata carries the sidecar, and a headless enclosure that links
//! the design follows a part moved by `layout.move`. The ESP32 example's
//! acceptance tests run through the automation session.
use super::{SLAB, build, read_metadata};
use crate::enclosure::{Enclosure, Operation};
use pcbcraft_core::ids::{Id, InterfaceSourceId};
use pcbcraft_core::interface::publish::{candidate, record};
use pcbcraft_core::interface::{InterfaceCommand, Sidecar};
use pcbcraft_core::layout::LayoutCommand;
use pcbcraft_core::library::datum::{
    DatumFrame, DatumName, DatumProfile, PackageDatum, PackageDatumGeometry, PackageDatumOrigin,
};
use pcbcraft_core::library::package_model::{PackageEdit, edit_package};
use pcbcraft_core::model::interface::Tier;
use pcbcraft_core::units::Nm;
use pcbcraft_core::{Engine, Operation as Op};
use serde_json::json;
use std::collections::BTreeMap;

type Fallible<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

const SOURCE: InterfaceSourceId = InterfaceSourceId(0xb3);

fn nm(mm: f64) -> Nm {
    Nm::from_mm_f64_rounded(mm).unwrap_or(Nm::ZERO)
}

/// The demo, its package carrying the USB4105's opening (8.34 × 2.56 mm,
/// round ends, 1.85 mm deep) as an authored external datum.
fn engine() -> Fallible<Engine> {
    let demo = Engine::demo()?;
    let mut project = demo.project().clone();
    let entry = project.library.packages.first_mut().ok_or("package")?;
    let opening = PackageDatum {
        name: DatumName("opening".into()),
        geometry: PackageDatumGeometry::Volume {
            frame: DatumFrame {
                origin_nm: [Nm::ZERO, nm(-3.675), nm(1.73)],
                x: [1.0, 0.0, 0.0],
                y: [0.0, 0.0, 1.0],
            },
            profile: DatumProfile::Rect {
                size_nm: [nm(8.34), nm(2.56)],
                corner_radius_nm: nm(1.28),
            },
            height_nm: nm(1.85),
        },
        clearance_nm: None,
        external: true,
        origin: PackageDatumOrigin::Authored,
    };
    entry.definition = edit_package(&entry.definition, &[PackageEdit::AddDatum(opening)])?;
    project.interface = Default::default();
    let mut file = demo.to_file();
    file.project = project;
    Ok(Engine::from_file(file)?)
}

fn sidecar(engine: &Engine, tier: Tier) -> Fallible<Sidecar> {
    Ok(candidate(engine.project(), tier, SOURCE)?)
}

/// Publish as the host does, overwriting `path`; returns the sidecar.
fn publish(engine: &mut Engine, tier: Tier, path: &std::path::Path) -> Fallible<Sidecar> {
    let exported = build(engine.project(), sidecar(engine, tier)?)?;
    let _ = std::fs::remove_file(path);
    exported.save_new(path.to_str().ok_or("path")?)?;
    let record = record(engine.project(), &exported.sidecar)?;
    let revision = engine.revision();
    engine.apply(
        revision,
        Op::Interface(InterfaceCommand::RecordPublish {
            source_id: SOURCE,
            record,
        }),
    )?;
    Ok(exported.sidecar)
}

fn placed(engine: &Engine, k: usize) -> Fallible<Id> {
    Ok(engine
        .project()
        .placements
        .get(k)
        .ok_or("placement")?
        .instance)
}

fn move_by(engine: &mut Engine, instance: Id, by: [f64; 2]) -> Fallible {
    let p = engine
        .project()
        .placement_of(instance)
        .ok_or("placement")?
        .clone();
    let revision = engine.revision();
    engine.apply(
        revision,
        Op::Layout(LayoutCommand::Move {
            instance,
            position: [p.position[0] + by[0], p.position[1] + by[1]],
            rotation_deg: p.rotation_deg,
            layer: p.layer,
        }),
    )?;
    Ok(())
}

/// Every face name the design has, by body.
fn face_names(native: &mut crate::native::Native, body: &str) -> Fallible<Vec<String>> {
    let reply = native.call("model.faces", json!({"body": body}))?;
    Ok(reply["faces"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["name"].as_str().map(str::to_owned))
        .collect())
}

/// The wall before and after a board change: the box's own faces (named
/// `box_faces`) keep their plane and area (a face the cutout pierces keeps
/// its area, though its centroid moves with the hole); every face the cut
/// made moved by 2 mm ± 1 µm along X. Returns how many moved.
fn compare_wall(
    box_faces: &[String],
    before: &[crate::enclosure::Face],
    after: &[crate::enclosure::Face],
) -> Fallible<usize> {
    let mut followed = 0;
    assert_eq!(before.len(), after.len(), "the wall keeps its faces");
    for a in before {
        let b = after
            .iter()
            .find(|b| b.name == a.name)
            .ok_or_else(|| format!("face {} is gone", a.name))?;
        if box_faces.contains(&a.name) {
            let (n, m) = (a.normal.ok_or("a plane")?, b.normal.ok_or("a plane")?);
            let plane = |n: [f64; 3], c: [f64; 3]| n[0] * c[0] + n[1] * c[1] + n[2] * c[2];
            assert!(
                (0..3).all(|k| (n[k] - m[k]).abs() <= 1e-9),
                "{} turned",
                a.name
            );
            assert!(
                (plane(n, a.centroid) - plane(m, b.centroid)).abs() <= 1e-6,
                "{} moved off its plane",
                a.name
            );
            assert!(
                (a.area - b.area).abs() <= 1e-6,
                "{} changed its area",
                a.name
            );
            continue;
        }
        let d: Vec<f64> = (0..3).map(|k| b.centroid[k] - a.centroid[k]).collect();
        assert!(
            (d[0] - 2.0).abs() <= 1e-3 && d[1].abs() <= 1e-6 && d[2].abs() <= 1e-6,
            "cutout face {} moved by {d:?}, not 2 mm along X",
            a.name
        );
        followed += 1;
    }
    Ok(followed)
}

/// Each datum's faces are faces the design really has, on the body that
/// carries it (the slab, or the body named by its token); a frame,
/// parameter or plane datum names none.
#[test]
fn every_named_face_is_in_the_design() -> Fallible {
    let engine = engine()?;
    let mut exported = build(engine.project(), sidecar(&engine, Tier::Interface)?)?;
    let sidecar = exported.sidecar.clone();
    let native = exported.slots_mut().ok_or("the interface tier")?;
    let opening = format!("i{}.opening", placed(&engine, 0)?);
    let mut seen = 0;
    for datum in &sidecar.datums {
        let slab = matches!(
            datum.role.as_str(),
            "board.outline" | "board.top" | "board.bottom" | "hole" | "board.cutout"
        );
        let body = if slab { SLAB } else { datum.token.as_str() };
        match datum.kind {
            pcbcraft_core::library::datum::DatumKind::Frame
            | pcbcraft_core::library::datum::DatumKind::Parameter
            | pcbcraft_core::library::datum::DatumKind::Plane
                if !slab =>
            {
                assert!(datum.faces.is_empty(), "{}", datum.token);
                continue;
            }
            _ => {}
        }
        if datum.faces.is_empty() {
            continue;
        }
        let names = face_names(native, body)?;
        for (role, name) in &datum.faces {
            assert!(
                names.contains(name),
                "{} {role}: {name} is not a face of {body}: {names:?}",
                datum.token
            );
        }
        seen += 1;
    }
    let front = sidecar
        .datums
        .iter()
        .find(|d| d.token == opening)
        .and_then(|d| d.faces.get("front"))
        .ok_or("the opening's front")?;
    assert!(front.ends_with(":start"), "{front}");
    assert!(
        seen >= 4,
        "board top, bottom, outline and the openings: {seen}"
    );
    Ok(())
}

/// A slot's feature ids follow from its position alone: deleting the
/// first part, whose opening's slot comes before the second's, retires
/// that slot, and the second opening keeps every face name.
#[test]
fn a_slot_keeps_its_names_whatever_comes_before_it() -> Fallible {
    let dir = tempfile::tempdir()?;
    let mut engine = engine()?;
    let first = placed(&engine, 0)?;
    let second = format!("i{}.opening", placed(&engine, 1)?);
    // Published, so the document holds its slots from now on.
    let before = publish(
        &mut engine,
        Tier::Interface,
        &dir.path().join("b.solidcraft"),
    )?;
    let revision = engine.revision();
    engine.apply_update(
        revision,
        pcbcraft_core::updates::Change::Instance {
            edit: pcbcraft_core::updates::InstanceChange::Remove { instance: first },
        },
        false,
    )?;
    let after = build(engine.project(), sidecar(&engine, Tier::Interface)?)?.sidecar;
    let faces = |s: &Sidecar| -> Option<BTreeMap<String, String>> {
        s.datums
            .iter()
            .find(|d| d.token == second)
            .map(|d| d.faces.clone())
    };
    assert!(faces(&before).is_some_and(|f| !f.is_empty()));
    assert_eq!(faces(&before), faces(&after));
    assert_eq!(after.retired_slots.len(), before.retired_slots.len() + 1);
    Ok(())
}

/// The mechanical tier is the interface tier's slots, named alike, then
/// the board; the metadata of both carries the sidecar.
#[test]
fn the_tiers_share_their_slots_and_carry_the_sidecar() -> Fallible {
    let engine = engine()?;
    let slots = build(engine.project(), sidecar(&engine, Tier::Interface)?)?;
    let board = build(engine.project(), sidecar(&engine, Tier::Mechanical)?)?;
    let faces = |s: &Sidecar| -> Vec<_> { s.datums.iter().map(|d| d.faces.clone()).collect() };
    assert_eq!(faces(&slots.sidecar), faces(&board.sidecar));
    assert_eq!(read_metadata(slots.native())?, Some(slots.sidecar.clone()));
    assert_eq!(read_metadata(board.native())?, Some(board.sidecar.clone()));
    let world = board.native().world_state()?;
    assert!(
        world.bodies.len() > slots.native().world_state()?.bodies.len(),
        "section 2 holds the board's bodies"
    );
    Ok(())
}

/// The enclosure follows: a wall in front of the board with a cutout
/// projected from the first part's opening; the part moved 2 mm along X by
/// `layout.move`, the interface published again and the links updated,
/// the cutout's faces moved 2 mm ± 1 µm along X and no other face of the
/// wall moved.
#[test]
fn a_linked_enclosure_follows_a_moved_part() -> Fallible {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("board.solidcraft");
    let mut engine = engine()?;
    let part = placed(&engine, 0)?;
    let first = publish(&mut engine, Tier::Interface, &path)?;
    let token = format!("i{part}.opening");
    let opening = first
        .datums
        .iter()
        .find(|d| d.token == token)
        .ok_or("opening")?;
    let pcbcraft_core::interface::DatumGeometry::Volume { frame, .. } = &opening.geometry else {
        return Err("a volume".into());
    };
    let o = frame.origin.0;
    let mut enclosure = Enclosure::new("Housing")?;
    let bodies = enclosure.link(path.to_str().ok_or("path")?, "Board")?;
    assert!(bodies.contains(&token), "{bodies:?}");
    let front = enclosure
        .faces(&token)?
        .into_iter()
        .find(|f| f.normal.is_some_and(|n| n[1] < -0.99))
        .ok_or("the opening's front face")?;
    // A 2 mm wall across the opening's way out, 1 mm in front of it.
    let wall_y = o[1] - 1.0;
    enclosure.solid_box(
        "Wall",
        [o[0] - 20.0, wall_y - 2.0, o[2] - 6.0],
        [40.0, 2.0, 12.0],
    )?;
    let box_faces: Vec<String> = enclosure
        .faces("Wall")?
        .into_iter()
        .map(|f| f.name)
        .collect();
    enclosure.projected(
        "USB opening",
        [[0.0, wall_y, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        &token,
        front.point,
        2.0,
        Operation::Cut,
    )?;
    assert!(
        enclosure.feature_errors().is_empty(),
        "{:?}",
        enclosure.feature_errors()
    );
    let wall = enclosure.faces("Wall")?;
    assert!(
        wall.len() > 6,
        "the cutout went through the wall: {} faces",
        wall.len()
    );

    move_by(&mut engine, part, [2.0, 0.0])?;
    publish(&mut engine, Tier::Interface, &path)?;
    let update = enclosure.update_links()?;
    assert!(update.failed.is_empty(), "{update:?}");
    assert!(
        enclosure.feature_errors().is_empty(),
        "{:?}",
        enclosure.feature_errors()
    );
    let moved = enclosure.faces("Wall")?;
    let followed = compare_wall(&box_faces, &wall, &moved)?;
    assert!(followed >= 4, "the cutout's faces moved: {followed}");
    Ok(())
}
