#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice B2's gate through the engine and the real part file: the
//! ESP32 dev board example's USB4105 part file declares its `opening`
//! (a construction plane on the mating face, docs/architecture/
//! board-interface.md), the part-file reader makes it a package datum,
//! `layout.place_part` stores it on the package, and
//! `interface::package` places it in the board frame on both sides and at
//! every rotation exactly where the 3D board, built by Solidcraft from the
//! same document, holds the part.
use super::tests::{Fallible, apply};
use crate::models::part_file;
use crate::project::build;
use sparksmith_core::ids::Id;
use sparksmith_core::interface::package::{instance_datums, model_to_board};
use sparksmith_core::interface::{DatumGeometry, Frame};
use sparksmith_core::layout::LayoutCommand;
use sparksmith_core::library::Provenance;
use sparksmith_core::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use sparksmith_core::library::datum::{DatumProfile, PackageDatumGeometry, PackageDatumOrigin};
use sparksmith_core::library::mechanical::{PadUse, PartModelSource, PartSpec};
use sparksmith_core::library::model3d::{AssetRef, ModelDefinition, ModelFormat, ModelProvenance};
use sparksmith_core::units::Nm;
use sparksmith_core::{Engine, Layer, Operation, Outcome};

/// The example's original USB4105 part file (`author_parts`).
pub(crate) const USB4105: &[u8] =
    include_bytes!("../../../../examples/esp32-s31-devboard/parts/usb4105.solidcraft");
const MEDIA: &str = "application/x-solidcraft";

pub(crate) fn spec(bytes: &[u8]) -> Fallible<PartSpec> {
    let part = part_file::evaluate(bytes, &Default::default())?;
    let packed = PackedAsset {
        hash: asset_hash(bytes),
        media_type: MEDIA.into(),
        data: AssetBytes::new(bytes.to_vec()),
    };
    Ok(PartSpec {
        name: "USB4105".into(),
        designator_prefix: "J".into(),
        source: "usb4105.solidcraft".into(),
        model: PartModelSource::File {
            name: "usb4105".into(),
            definition: Box::new(ModelDefinition {
                asset: AssetRef {
                    hash: packed.hash,
                    bytes: bytes.len() as u64,
                    media_type: MEDIA.into(),
                    file_name: "usb4105.solidcraft".into(),
                },
                format: ModelFormat::Solidcraft,
                bounds_nm: crate::test_units::bounds(part.bounds_mm),
                provenance: ModelProvenance {
                    record: Provenance {
                        licence: Some("MIT OR Apache-2.0".into()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..ModelDefinition::default()
            }),
            asset: packed,
        },
        values: Default::default(),
        derived: part_file::derived(&part)?,
        lands: vec![],
        pads: PadUse::default(),
        bom: true,
        pick_and_place: None,
    })
}

const EPS: f64 = 1e-6;

fn near(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() <= EPS, "{what}: {a} against {b}");
}

/// The part file's convention: the `opening` plane, sized by its
/// parameters, becomes the external datum `opening`.
#[test]
fn the_usb4105_part_file_declares_its_opening() -> Fallible<()> {
    let part = part_file::evaluate(USB4105, &Default::default())?;
    let report = part.report();
    assert!(report.ok, "{:?}", report.findings);
    let [opening] = part.datums.as_slice() else {
        return Err(format!("{} datums", part.datums.len()).into());
    };
    assert_eq!(opening.name.0, "opening");
    assert!(opening.external);
    assert_eq!(opening.origin, PackageDatumOrigin::PartFile);
    let PackageDatumGeometry::Volume {
        frame,
        profile,
        height_nm,
    } = opening.geometry
    else {
        return Err("a volume".into());
    };
    let mm = |v: Nm| v.to_mm_f64();
    assert_eq!(frame.origin(), [0.0, -3.675, 1.73]);
    assert_eq!(frame.x, [1.0, 0.0, 0.0]);
    assert_eq!(frame.y, [0.0, 0.0, 1.0]);
    // Out of the receptacle, along the plug's way in.
    assert_eq!(frame.z().map(|v| v + 0.0), [0.0, -1.0, 0.0]);
    let DatumProfile::Rect {
        size_nm,
        corner_radius_nm,
    } = profile;
    assert_eq!(size_nm.map(mm), [8.34, 2.56]);
    assert_eq!(mm(corner_radius_nm), 1.28);
    assert_eq!(mm(height_nm), 1.85);
    // The report carries it in millimetres.
    let json = serde_json::to_value(&report.datums)?;
    assert_eq!(json[0]["geometry"]["height"], serde_json::json!(1.85));
    Ok(())
}

/// The gate: placed through `layout.place_part` on both sides at every
/// quarter turn and at 33°, the opening lands where the 3D board holds the
/// part. Every vertex of the part's bodies in the built board, taken back
/// through the core's model → board transform, falls in the part file's
/// own bounds; at the quarter turns the opening's origin lies on the
/// front face of the part's box, centred across it and 1.73 mm off the
/// seating plane, and its z points out of that face.
#[test]
fn a_usb_c_opening_reaches_every_placement_in_the_board_frame() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let spec = spec(USB4105)?;
    let part = part_file::evaluate(USB4105, &Default::default())?;
    let mut placed: Vec<(Id, Layer, f64)> = Vec::new();
    let mut x = -24.0;
    for layer in [Layer::Top, Layer::Bottom] {
        for rotation in [0.0, 90.0, 180.0, 270.0, 33.0] {
            let outcome = apply(
                &mut engine,
                Operation::Layout(LayoutCommand::PlacePart {
                    part: Box::new(spec.clone()),
                    position: [x, if layer == Layer::Top { 8.0 } else { -8.0 }],
                    rotation_deg: rotation,
                    layer,
                    designator: None,
                }),
            )?;
            let Outcome::PartPlaced { instance, .. } = outcome else {
                return Err(format!("{outcome:?}").into());
            };
            placed.push((instance, layer, rotation));
            x += 12.0;
            if x > 24.0 {
                x = -24.0;
            }
        }
    }
    let project = engine.project().clone();
    let model = build(&project)?;
    let [lo, hi] = part.bounds_mm;
    for (instance, layer, rotation) in placed {
        let what = format!("{layer:?} at {rotation}°");
        let placement = project.placement_of(instance).ok_or("placement")?;
        let package = project
            .library
            .packages
            .iter()
            .find(|p| p.definition.datum("opening").is_some())
            .ok_or("the package carries the opening")?;
        let to_board = model_to_board(placement, &package.definition, &project.stackup)?;
        // The engine's pose: every vertex of the instance's bodies.
        let picks: Vec<u64> = model
            .sources
            .iter()
            .filter(|s| s.instance == Some(instance) && s.pad.is_none())
            .map(|s| s.pick_id)
            .collect();
        let mut back_lo = [f64::MAX; 3];
        let mut back_hi = [f64::MIN; 3];
        let mut box_lo = [f64::MAX; 3];
        let mut box_hi = [f64::MIN; 3];
        let r = to_board.rotation;
        let t = to_board.translation;
        for g in model
            .geometry
            .iter()
            .filter(|g| g.item.is_some_and(|i| picks.contains(&i)) && !g.kind.is_appearance())
        {
            for p in &g.world_mesh().positions {
                let w = [p.x, p.y, p.z];
                let d = [w[0] - t[0], w[1] - t[1], w[2] - t[2]];
                // The rigid inverse: the rotation's transpose.
                let m: [f64; 3] =
                    std::array::from_fn(|j| r[0][j] * d[0] + r[1][j] * d[1] + r[2][j] * d[2]);
                for axis in 0..3 {
                    back_lo[axis] = back_lo[axis].min(m[axis]);
                    back_hi[axis] = back_hi[axis].max(m[axis]);
                    box_lo[axis] = box_lo[axis].min(w[axis]);
                    box_hi[axis] = box_hi[axis].max(w[axis]);
                }
            }
        }
        assert!(back_lo[0] < back_hi[0], "{what}: the part has no bodies");
        for axis in 0..3 {
            near(back_lo[axis], lo[axis], &format!("{what}: min {axis}"));
            near(back_hi[axis], hi[axis], &format!("{what}: max {axis}"));
        }
        // The core's datum is the model datum through that transform.
        let datums = instance_datums(&project, instance)?;
        let [opening] = datums.as_slice() else {
            return Err(format!("{what}: {} datums", datums.len()).into());
        };
        assert!(opening.external, "{what}");
        let DatumGeometry::Volume {
            frame: Frame { origin, x, y },
            height,
            ..
        } = &opening.geometry
        else {
            return Err(format!("{what}: a volume").into());
        };
        near(*height, 1.85, &what);
        let o = origin.0;
        let expected = to_board.apply([0.0, -3.675, 1.73]);
        for axis in 0..3 {
            near(o[axis], expected[axis], &format!("{what}: origin {axis}"));
        }
        let out = [
            x.0[1] * y.0[2] - x.0[2] * y.0[1],
            x.0[2] * y.0[0] - x.0[0] * y.0[2],
            x.0[0] * y.0[1] - x.0[1] * y.0[0],
        ];
        near(
            out[2],
            0.0,
            &format!("{what}: out stays in the board plane"),
        );
        // On the bottom the opening hangs below the board.
        let rise = 1.73;
        match layer {
            Layer::Top => near(o[2] - box_lo[2], rise, &format!("{what}: rise")),
            Layer::Bottom => near(box_hi[2] - o[2], rise, &format!("{what}: drop")),
        }
        if rotation % 90.0 != 0.0 {
            continue;
        }
        // Quarter turns: the box is the part's; the opening is on the face
        // it points out of, centred across it.
        let k = if out[0].abs() > 0.5 { 0 } else { 1 };
        let across = 1 - k;
        if out[k] > 0.0 {
            near(o[k], box_hi[k], &format!("{what}: on the front face"));
        } else {
            near(o[k], box_lo[k], &format!("{what}: on the front face"));
        }
        near(
            o[across],
            (box_lo[across] + box_hi[across]) / 2.0,
            &format!("{what}: centred across"),
        );
    }
    Ok(())
}
