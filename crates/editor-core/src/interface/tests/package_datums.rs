//! Slice B2: package datums placed on the board (`interface::package`),
//! the `library.edit_package` datum edits and the part file's and the
//! form's declared datums.
use super::Fallible;
use crate::Engine;
use crate::geometry::Transform3;
use crate::interface::package::{external_datums, instance_datums, model_to_board};
use crate::interface::{DatumGeometry, Frame};
use crate::library::datum::{
    DatumFrame, DatumName, DatumProfile, PackageDatum, PackageDatumGeometry, PackageDatumOrigin,
};
use crate::library::package_model::{PackageEdit, PartDerived, PartFileModel, edit_package};
use crate::library::{
    Datum, ModelPlacement, PackageDefinition, PackageForm, PackageModel, UsbCForm, model_frame,
};
use crate::model::{Layer, Project};
use crate::units::Nm;

const EPS: f64 = 1e-9;

fn nm(mm: f64) -> Nm {
    Nm::from_mm_f64_rounded(mm).unwrap_or(Nm::ZERO)
}

/// The opening the USB4105 part file declares (its `opening` plane at the
/// shell's mating face, 8.34 × 2.56 mm with round ends, 1.85 mm deep), as
/// the host's part-file reader stores it in the package's derived data.
fn usb4105_opening() -> PackageDatum {
    PackageDatum {
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
        origin: PackageDatumOrigin::PartFile,
    }
}

fn demo() -> Fallible<Project> {
    let project = Engine::demo()?.project().clone();
    if project.placements.is_empty() {
        return Err("the demo has no placements".into());
    }
    Ok(project)
}

/// The demo with its one package using a part file that declares the
/// opening (through `library.edit_package`'s own edit), a footprint datum
/// and a model placement that both turn, so the chain is not the identity.
fn with_part_file_opening() -> Fallible<Project> {
    let mut project = demo()?;
    let package = project
        .library
        .packages
        .first_mut()
        .ok_or("the demo package")?;
    let part = PackageModel::PartFile(Box::new(PartFileModel {
        model: Default::default(),
        values: Default::default(),
        derived: PartDerived {
            bounds_nm: [
                [nm(-4.47), nm(-3.675), Nm::ZERO],
                [nm(4.47), nm(3.675), nm(3.31)],
            ],
            bodies: 1,
            surfaces: vec![],
            datums: vec![PackageDatum {
                origin: PackageDatumOrigin::Authored,
                ..usb4105_opening()
            }],
        },
    }));
    let placement = ModelPlacement::from_mm([0.25, -0.4, 0.1], [0.0, 0.0, 90.0])?;
    package.definition = edit_package(
        &package.definition,
        &[
            PackageEdit::SetModel(part),
            PackageEdit::SetPlacement(placement),
        ],
    )?;
    let footprint = project
        .library
        .footprints
        .first_mut()
        .ok_or("the demo footprint")?;
    footprint.definition.datum = Datum {
        offset: crate::units::PointNm::from_mm_f64_rounded([0.3, -0.4])?,
        rotation_mdeg: crate::units::Angle::from_degrees_f64_rounded(90.0)?,
    };
    Ok(project)
}

fn close(a: [f64; 3], b: [f64; 3], what: &str) -> Fallible {
    if (0..3).all(|i| (a[i] - b[i]).abs() <= EPS) {
        Ok(())
    } else {
        Err(format!("{what}: {a:?} against {b:?}").into())
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The footprint-frame chain the 2D drawing and the 3D board use, written
/// out: the model frame into the footprint frame (`model_frame`), onto the
/// board in the plane (`layout::placement_transform`), and up from the
/// side's land copper (on the bottom, down from it).
fn by_the_footprint_chain(
    project: &Project,
    package: &PackageDefinition,
    datum: &Datum,
    placement: &crate::model::Placement,
    p: [f64; 3],
) -> Fallible<[f64; 3]> {
    let f = model_frame(datum, &package.placement).apply(p);
    let [x, y] = crate::layout::placement_transform(placement, datum).apply([f[0], f[1]]);
    let stackup = &project.stackup;
    let z = match placement.layer {
        Layer::Top => {
            stackup
                .copper_span(crate::model::CopperLayer::Top)
                .ok_or("top")?
                .1
                + f[2]
        }
        Layer::Bottom => {
            stackup
                .copper_span(crate::model::CopperLayer::Bottom)
                .ok_or("bottom")?
                .0
                - f[2]
        }
    };
    Ok([x, y, z])
}

pub(super) fn a_part_file_opening_reaches_the_board_frame() -> Fallible {
    let mut project = with_part_file_opening()?;
    let package = project
        .library
        .packages
        .first()
        .ok_or("package")?
        .definition
        .clone();
    // The part file's datum became the package's, marked as the part
    // file's.
    let declared = package.datum("opening").ok_or("the package's opening")?;
    assert_eq!(declared.origin, PackageDatumOrigin::PartFile);
    assert!(declared.external);
    let datum = project
        .library
        .footprints
        .first()
        .ok_or("footprint")?
        .definition
        .datum;
    let instance = project.placements.first().ok_or("placement")?.instance;
    let PackageDatumGeometry::Volume { frame: model, .. } = declared.geometry else {
        return Err("the opening is a volume".into());
    };
    let mut seen = 0;
    for layer in [Layer::Top, Layer::Bottom] {
        for rotation in [0.0, 90.0, 180.0, 270.0, 33.0] {
            let placement = project
                .placements
                .iter_mut()
                .find(|p| p.instance == instance)
                .ok_or("placement")?;
            placement.layer = layer;
            placement.rotation_deg = rotation;
            placement.position = [17.5, -6.25];
            let placement = placement.clone();
            let datums = instance_datums(&project, instance)?;
            let [placed] = datums.as_slice() else {
                return Err(format!("{} datums", datums.len()).into());
            };
            let what = format!("{layer:?} at {rotation}°");
            assert_eq!(placed.name.0, "opening");
            assert!(placed.external);
            let DatumGeometry::Volume {
                frame: Frame { origin, x, y },
                loops,
                height,
            } = &placed.geometry
            else {
                return Err(format!("{what}: not a volume").into());
            };
            // The origin and the axes, through the footprint chain: a point
            // a unit along each model axis lands a unit along the board's.
            let at =
                |p: [f64; 3]| by_the_footprint_chain(&project, &package, &datum, &placement, p);
            let o = model.origin();
            let expect_origin = at(o)?;
            close(origin.0, expect_origin, &format!("{what}: origin"))?;
            let along = |axis: [f64; 3]| -> Fallible<[f64; 3]> {
                let tip = at([o[0] + axis[0], o[1] + axis[1], o[2] + axis[2]])?;
                Ok(std::array::from_fn(|i| tip[i] - expect_origin[i]))
            };
            close(x.0, along(model.x)?, &format!("{what}: x"))?;
            close(y.0, along(model.y)?, &format!("{what}: y"))?;
            // The volume rises out of the receptacle, away from the board's
            // laminate on neither side: z stays in the board plane.
            let out = cross(x.0, y.0);
            close(out, along(model.z())?, &format!("{what}: out"))?;
            assert!(out[2].abs() <= EPS, "{what}: {out:?}");
            assert_eq!(*height, 1.85, "{what}");
            // The profile is drawn in the datum's own frame: unchanged.
            let [outline] = loops.as_slice() else {
                return Err(format!("{what}: one loop").into());
            };
            let b = outline.vertex_bounds();
            close(
                [b.min[0], b.min[1], 0.0],
                [-4.17, -1.28, 0.0],
                &format!("{what}: profile"),
            )?;
            close([b.max[0], b.max[1], 0.0], [4.17, 1.28, 0.0], &what)?;
            // The opening sits 1.73 mm off the seating plane, above the
            // board on the top and below it on the bottom.
            let seat = by_the_footprint_chain(&project, &package, &datum, &placement, [0.0; 3])?;
            let rise = origin.0[2] - seat[2];
            let sign = if layer == Layer::Top { 1.0 } else { -1.0 };
            assert!((rise - sign * 1.73).abs() <= EPS, "{what}: {rise}");
            seen += 1;
        }
    }
    assert_eq!(seen, 10);
    // The transform is rigid on both sides (no mirror in 3D).
    for layer in [Layer::Top, Layer::Bottom] {
        let mut placement = project.placements.first().ok_or("placement")?.clone();
        placement.layer = layer;
        let t: Transform3 = model_to_board(&placement, &package, &project.stackup)?;
        let r = t.rotation;
        let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
            - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
            + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
        assert!((det - 1.0).abs() <= EPS, "{layer:?}: {det}");
    }
    Ok(())
}

pub(super) fn no_opening_is_inferred_from_a_name() -> Fallible {
    let mut project = demo()?;
    let entry = project
        .library
        .packages
        .first_mut()
        .ok_or("the demo package")?;
    // Named and shaped like a USB-C receptacle, but declaring nothing.
    entry.meta.name = "USB-C receptacle (GCT USB4105)".into();
    entry.definition.envelope_nm = [nm(8.94), nm(7.35), nm(3.31)];
    if let Some(component) = project.library.components.first_mut() {
        component.meta.name = "USB-C".into();
    }
    let instance = project.placements.first().ok_or("placement")?.instance;
    assert_eq!(instance_datums(&project, instance)?, vec![]);
    assert_eq!(external_datums(&project), vec![]);
    // Declared by the typed USB-C form (its drawing's values), it is there.
    let entry = project
        .library
        .packages
        .first_mut()
        .ok_or("the demo package")?;
    entry.definition = edit_package(
        &entry.definition,
        &[PackageEdit::SetForm(PackageForm::UsbC(Box::new(
            UsbCForm::typical(),
        )))],
    )?;
    let opening = instance_datums(&project, instance)?;
    assert_eq!(
        opening
            .iter()
            .map(|d| d.name.0.as_str())
            .collect::<Vec<_>>(),
        ["opening"]
    );
    assert_eq!(
        external_datums(&project).len(),
        project.placements.len(),
        "one opening per placement of the package"
    );
    Ok(())
}

/// Every placement of a package with an external datum lists it; a datum
/// not marked external is placed but not in the rule's input.
#[test]
fn external_datums_list_every_placement_and_only_external_ones() -> Fallible {
    let mut project = with_part_file_opening()?;
    let axis = PackageDatum {
        name: DatumName("axis_led".into()),
        geometry: PackageDatumGeometry::Axis {
            point_nm: [Nm::ZERO, Nm::ZERO, nm(1.0)],
            direction: [0.0, 0.0, 1.0],
            diameter_nm: Some(nm(3.0)),
        },
        clearance_nm: None,
        external: false,
        origin: PackageDatumOrigin::Authored,
    };
    let entry = project.library.packages.first_mut().ok_or("package")?;
    entry.definition = edit_package(&entry.definition, &[PackageEdit::AddDatum(axis)])?;
    let external = external_datums(&project);
    let placed: Vec<_> = project.placements.iter().map(|p| p.instance).collect();
    assert!(placed.len() >= 2, "the demo places two parts");
    assert_eq!(
        external.iter().map(|d| d.instance).collect::<Vec<_>>(),
        placed
    );
    assert!(external.iter().all(|d| d.name.0 == "opening"));
    for instance in placed {
        let all = instance_datums(&project, instance)?;
        assert_eq!(all.len(), 2);
        let led = all.iter().find(|d| d.name.0 == "axis_led").ok_or("axis")?;
        let DatumGeometry::Axis { diameter, .. } = led.geometry else {
            return Err("an axis".into());
        };
        assert_eq!(diameter, Some(3.0));
    }
    Ok(())
}

/// The datum edits: add, set and remove by name, each refused unchanged
/// when it would repeat a name, find none, or carry bad geometry; set
/// makes a part file's datum authored, so using the part file again keeps
/// it.
#[test]
fn datum_edits_are_checked_and_authored_datums_are_kept() -> Fallible {
    let project = with_part_file_opening()?;
    let package = &project
        .library
        .packages
        .first()
        .ok_or("package")?
        .definition;
    let opening = package.datum("opening").ok_or("opening")?.clone();
    // A repeated name, an unknown one, bad axes, a reserved name.
    assert!(edit_package(package, &[PackageEdit::AddDatum(opening.clone())]).is_err());
    let unknown = PackageDatum {
        name: DatumName("opening_rear".into()),
        ..opening.clone()
    };
    assert!(edit_package(package, &[PackageEdit::SetDatum(unknown.clone())]).is_err());
    assert!(
        edit_package(
            package,
            &[PackageEdit::RemoveDatum(DatumName("opening_rear".into()))]
        )
        .is_err()
    );
    let skew = PackageDatum {
        geometry: PackageDatumGeometry::Plane {
            origin_nm: [Nm::ZERO; 3],
            normal: [1.0, 1.0, 0.0],
        },
        ..unknown.clone()
    };
    assert!(edit_package(package, &[PackageEdit::AddDatum(skew)]).is_err());
    let reserved = PackageDatum {
        name: DatumName("frame".into()),
        ..unknown.clone()
    };
    assert!(edit_package(package, &[PackageEdit::AddDatum(reserved)]).is_err());
    let fat = PackageDatum {
        geometry: PackageDatumGeometry::Profile {
            frame: DatumFrame {
                origin_nm: [Nm::ZERO; 3],
                x: [1.0, 0.0, 0.0],
                y: [0.0, 1.0, 0.0],
            },
            profile: DatumProfile::Rect {
                size_nm: [nm(2.0), nm(1.0)],
                corner_radius_nm: nm(0.6),
            },
        },
        ..unknown.clone()
    };
    assert!(edit_package(package, &[PackageEdit::AddDatum(fat)]).is_err());
    // Add, then remove, the second opening.
    let added = edit_package(package, &[PackageEdit::AddDatum(unknown.clone())])?;
    assert_eq!(added.datums.len(), 2);
    assert_eq!(
        added.datum("opening_rear").map(|d| d.origin),
        Some(PackageDatumOrigin::Authored)
    );
    let removed = edit_package(
        &added,
        &[PackageEdit::RemoveDatum(DatumName("opening_rear".into()))],
    )?;
    assert_eq!(removed.datums, package.datums);
    // Using the part file again replaces its datums; once edited the
    // opening is authored and stays as edited.
    let PackageModel::PartFile(part) = &package.model else {
        return Err("a part file".into());
    };
    let again = PackageEdit::SetModel(PackageModel::PartFile(part.clone()));
    let reused = edit_package(package, &[again.clone()])?;
    assert_eq!(reused.datums, package.datums);
    let narrower = PackageDatum {
        external: false,
        ..opening.clone()
    };
    let edited = edit_package(package, &[PackageEdit::SetDatum(narrower)])?;
    let kept = edit_package(&edited, &[again])?;
    let datum = kept.datum("opening").ok_or("opening")?;
    assert_eq!(datum.origin, PackageDatumOrigin::Authored);
    assert!(!datum.external, "the edit is kept");
    // Another model takes the part file's datums with it, not authored
    // ones.
    let envelope = edit_package(&added, &[PackageEdit::SetModel(PackageModel::default())])?;
    assert_eq!(
        envelope
            .datums
            .iter()
            .map(|d| d.name.0.as_str())
            .collect::<Vec<_>>(),
        ["opening_rear"]
    );
    Ok(())
}

/// A datum edit takes millimetres on the wire, as every package edit does,
/// and stores whole nanometres.
#[test]
fn a_datum_edit_carries_millimetres() -> Fallible {
    let edit = serde_json::json!({"edit": "add_datum", "value": {
        "name": "opening",
        "geometry": {"kind": "volume",
            "frame": {"origin": [0.0, -3.675, 1.73], "x": [1.0, 0.0, 0.0], "y": [0.0, 0.0, 1.0]},
            "profile": {"shape": "rect", "size": [8.34, 2.56], "corner_radius": 1.28},
            "height": 1.85},
        "external": true, "origin": "authored"}});
    let PackageEdit::AddDatum(datum) = serde_json::from_value::<PackageEdit>(edit.clone())? else {
        return Err("an add".into());
    };
    let expected = PackageDatum {
        origin: PackageDatumOrigin::Authored,
        ..usb4105_opening()
    };
    assert_eq!(datum, expected);
    assert_eq!(serde_json::to_value(PackageEdit::AddDatum(datum))?, edit);
    Ok(())
}
