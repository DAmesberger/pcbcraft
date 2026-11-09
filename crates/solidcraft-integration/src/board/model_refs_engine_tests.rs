#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice S3 through the engine: a part held flush to the board's right edge
//! by a face of its part file follows a board-width change; a library
//! revision whose part file no longer has that face lists it first in the
//! update preview, makes it lost (blocking fabrication output) and leaves
//! the part where it was; undo restores everything.
use super::super::tests::{Fallible, apply, edit, hosted, set, sketch, sketched};
use super::tests::boxed;
use super::*;
use pcbcraft_core::Engine;
use pcbcraft_core::Layer;
use pcbcraft_core::Operation;
use pcbcraft_core::Outcome;
use pcbcraft_core::format::EditorFile;
use pcbcraft_core::ids::{ComponentRef, Id, ModelRef, Revision};
use pcbcraft_core::layout::LayoutCommand;
use pcbcraft_core::layout::dimensions::{DimensionCommand, REFERENCE_LOST, checks};
use pcbcraft_core::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use pcbcraft_core::library::footprint_source::{DimKind, DimValue, SketchRef};
use pcbcraft_core::library::mechanical::{PadUse, PartModelSource, PartSpec};
use pcbcraft_core::library::model3d::{AssetRef, ModelDefinition, ModelFormat, ModelProvenance};
use pcbcraft_core::library::package_model::PartDerived;
use pcbcraft_core::library::{PackageModel, Provenance};
use pcbcraft_core::model::dimension::{
    BoardRef, BodyName, FaceName, PartDimensionSpec, PartRelation,
};
use pcbcraft_core::updates::{Change, ChangeObject};

const MEDIA: &str = "application/x-solidcraft";

fn asset(bytes: &[u8]) -> PackedAsset {
    PackedAsset {
        hash: asset_hash(bytes),
        media_type: MEDIA.into(),
        data: AssetBytes::new(bytes.to_vec()),
    }
}

/// The box part as a mechanical part spec.
fn spec(bytes: &[u8]) -> PartSpec {
    let part = part_file::evaluate(bytes, &Default::default()).unwrap();
    let packed = asset(bytes);
    PartSpec {
        name: "Housing".into(),
        designator_prefix: "J".into(),
        source: "housing.solidcraft".into(),
        model: PartModelSource::File {
            name: "housing".into(),
            definition: Box::new(ModelDefinition {
                asset: AssetRef {
                    hash: packed.hash,
                    bytes: bytes.len() as u64,
                    media_type: MEDIA.into(),
                    file_name: "housing.solidcraft".into(),
                },
                format: ModelFormat::Solidcraft,
                bounds_mm: part.bounds_mm,
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
        derived: PartDerived {
            bounds_mm: part.bounds_mm,
            bodies: 1,
            surfaces: vec![],
        },
        lands: vec![],
        pads: PadUse::default(),
        bom: true,
        pick_and_place: None,
    }
}

fn x_of(engine: &Engine, part: Id) -> f64 {
    engine.project().placement_of(part).unwrap().position[0]
}

/// The board sketch's right edge.
fn right(engine: &Engine) -> Fallible<BoardRef> {
    let s = sketch(engine)?;
    let e = s
        .edges
        .iter()
        .find(|e| e.name.0 == "right")
        .ok_or("right")?;
    Ok(BoardRef::Board(SketchRef::Curve(e.curve.clone())))
}

/// A sketched demo board with the box housing placed at (30, 20), and the
/// housing's +x face (x = 2 in the part) named by `name`, picked at `pick`.
fn placed() -> Fallible<(Engine, Id, Vec<u8>, BoardRef)> {
    let mut engine = sketched()?;
    let (bytes, body) = boxed();
    let placed = apply(
        &mut engine,
        Operation::Layout(LayoutCommand::PlacePart {
            part: Box::new(spec(&bytes)),
            position: [30.0, 20.0],
            rotation_deg: 0.0,
            layer: Layer::Top,
            designator: None,
        }),
    )?;
    let Outcome::PartPlaced { instance, .. } = placed else {
        return Err(format!("{placed:?}").into());
    };
    let part = part_file::evaluate(&bytes, &Default::default())?;
    let b = part
        .state()
        .bodies
        .iter()
        .find(|b| b.name == body)
        .ok_or("body")?;
    let names = naming::face_names(b);
    let spots = naming::face_spots(b);
    let i = (0..names.len())
        .find(|i| {
            matches!(face_surface(&b.body, *i), Some(FaceSurface::Plane { normal, .. })
                if (normal.vec().x - 1.0).abs() < 1e-9)
        })
        .ok_or("+x face")?;
    let face = BoardRef::Part(FeatureRef::Face {
        placement: instance,
        face: ModelFaceRef {
            body: BodyName(body),
            face: FaceName(names[i].clone()),
            pick: arr(spots[i].ok_or("spot")?.point),
        },
    });
    Ok((engine, instance, bytes, face))
}

fn flush(face: BoardRef, edge: BoardRef) -> PartDimensionSpec {
    PartDimensionSpec {
        kind: PartRelation::Dim(DimKind::Distance),
        refs: vec![face, edge],
        value: Some(DimValue::Fixed(0.0)),
        reference: false,
    }
}

/// Held flush to the right edge by its +x face (2 mm right of its origin),
/// the housing's origin sits 2 mm inside the edge, and follows the edge
/// when the board grows 10 mm wider. Undo takes the dimension and its
/// trace away again.
#[test]
fn a_part_held_by_its_face_follows_the_board_width() -> Fallible<()> {
    let (mut engine, part, _, face) = placed()?;
    let width = engine.project().board_size[0];
    let before = engine.to_file();
    let edge = right(&engine)?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(face.clone(), edge),
        }),
    )?;
    assert!(
        (x_of(&engine, part) - (width - 2.0)).abs() < 1e-6,
        "{}",
        x_of(&engine, part)
    );
    let traces = &engine.project().model_traces;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].status, ReferenceStatus::Resolved);
    assert!(
        checks(engine.project()).is_empty(),
        "{:?}",
        checks(engine.project())
    );
    edit(
        &mut engine,
        vec![set("width", &format!("{} mm", width + 10.0))],
    )?;
    assert!(
        (x_of(&engine, part) - (width + 8.0)).abs() < 1e-6,
        "{}",
        x_of(&engine, part)
    );
    engine.undo()?;
    engine.undo()?;
    assert_eq!(engine.to_file(), before);
    assert!(engine.project().model_traces.is_empty());
    Ok(())
}

/// A sloped face is refused when the dimension is added, with nothing
/// changed: the housing tilted 30° about x by its model placement has no
/// square ±y faces.
#[test]
fn a_dimension_to_a_sloped_face_is_refused() -> Fallible<()> {
    let (mut engine, part, bytes, _) = placed()?;
    let mut project = engine.project().clone();
    for p in project.library.packages.iter_mut() {
        if matches!(p.definition.model, PackageModel::PartFile(_)) {
            p.definition.placement.rotation_deg = [30.0, 0.0, 0.0];
        }
    }
    let mut file = engine.to_file();
    file.project = project;
    engine = Engine::from_file(file)?;
    let part_eval = part_file::evaluate(&bytes, &Default::default())?;
    let b = &part_eval.state().bodies[0];
    let names = naming::face_names(b);
    let spots = naming::face_spots(b);
    let i = (0..names.len())
        .find(|i| {
            matches!(face_surface(&b.body, *i), Some(FaceSurface::Plane { normal, .. })
                if (normal.vec().y + 1.0).abs() < 1e-9)
        })
        .ok_or("-y face")?;
    let front = BoardRef::Part(FeatureRef::Face {
        placement: part,
        face: ModelFaceRef {
            body: BodyName(b.name.clone()),
            face: FaceName(names[i].clone()),
            pick: arr(spots[i].ok_or("spot")?.point),
        },
    });
    let before = engine.to_file();
    let rev = engine.revision();
    let edge = right(&engine)?;
    let refused = apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(front, edge),
        }),
    );
    let Err(pcbcraft_core::Error::Unsupported(why)) = refused else {
        panic!("{refused:?}")
    };
    assert!(why.contains("pick an edge"), "{why}");
    assert_eq!(engine.revision(), rev);
    assert_eq!(engine.to_file(), before);
    Ok(())
}

/// The next revision of the housing's whole library chain (model, package,
/// footprint, binding, component), its part file `bytes`, appended as a
/// library import appends one.
fn revise(engine: &Engine, part: Id, bytes: &[u8]) -> Fallible<(Engine, ComponentRef)> {
    let mut file: EditorFile = engine.to_file();
    let p = &mut file.project;
    let instance = p.instance(part).ok_or("instance")?.clone();
    let lib = &mut p.library;
    let next = |r: Revision| Revision(r.0 + 1);
    let component = lib
        .component(instance.component)
        .ok_or("component")?
        .clone();
    let binding = lib
        .bindings
        .iter()
        .find(|b| Some(b.id) == instance.physical.map(|ph| ph.binding))
        .ok_or("binding")?
        .clone();
    let package = lib
        .package(binding.definition.package)
        .ok_or("package")?
        .clone();
    let footprint = lib
        .footprints
        .iter()
        .find(|f| f.definition.package.id == package.id)
        .ok_or("footprint")?
        .clone();
    let PackageModel::PartFile(use_) = &package.definition.model else {
        return Err("not a part file".into());
    };
    let mut model = lib.model(use_.model).ok_or("model")?.clone();
    let packed = asset(bytes);
    model.revision = next(model.revision);
    model.definition.asset.hash = packed.hash;
    model.definition.asset.bytes = bytes.len() as u64;
    lib.assets.push(packed);
    let mut package2 = package.clone();
    package2.revision = next(package.revision);
    if let PackageModel::PartFile(u) = &mut package2.definition.model {
        u.model = ModelRef::new(model.id, model.revision);
    }
    let mut footprint2 = footprint.clone();
    footprint2.revision = next(footprint.revision);
    footprint2.definition.package.revision = package2.revision;
    let mut binding2 = binding.clone();
    binding2.revision = next(binding.revision);
    binding2.definition.package.revision = package2.revision;
    for f in &mut binding2.definition.footprints {
        f.revision = footprint2.revision;
    }
    let mut component2 = component.clone();
    component2.revision = next(component.revision);
    for b in &mut component2.definition.implementations {
        b.revision = binding2.revision;
    }
    let target = ComponentRef::new(component2.id, component2.revision);
    lib.models.push(model);
    lib.packages.push(package2);
    lib.footprints.push(footprint2);
    lib.bindings.push(binding2);
    lib.components.push(component2);
    Ok((Engine::from_file(file)?, target))
}

/// The box built in another order (the hole first), so its +x face has
/// another persistent name: the face the dimension names is gone.
fn reordered() -> Vec<u8> {
    let mut s = solidcraft_engine::Session::default();
    s.execute("file.new", &serde_json::json!({"name": "part"}))
        .unwrap();
    s.execute(
        "solid.cylinder",
        &serde_json::json!({"diameter": 1, "height": 1, "base": [0.5, 0, 0], "name": "Peg"}),
    )
    .unwrap();
    let made = s
        .execute(
            "solid.box",
            &serde_json::json!({"corner": [-2, -1, 0], "length": 4, "width": 2, "height": 1,
                "name": "Housing", "operation": "new_body"}),
        )
        .unwrap();
    let body = made["created_bodies"][0].as_str().unwrap().to_owned();
    s.execute(
        "appearance.assign",
        &serde_json::json!({"bodies": [body], "name": "lcp", "color": "#202020", "opacity": 1.0}),
    )
    .unwrap();
    solidcraft_engine::io::write_design(&s.doc)
}

/// A library revision whose part file lost the face: the preview lists it
/// first; applied, the reference is lost, the housing stays where it was
/// and fabrication output is blocked; undo restores everything.
#[test]
fn a_revision_that_loses_the_face_is_previewed_first_and_blocks_fab() -> Fallible<()> {
    let (mut engine, part, _, face) = placed()?;
    let edge = right(&engine)?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(face, edge),
        }),
    )?;
    let held_at = x_of(&engine, part);
    let (mut engine, target) = revise(&engine, part, &reordered())?;
    let before = engine.to_file();
    let change = Change::ComponentRevision {
        instances: vec![part],
        target,
        resolutions: Default::default(),
    };
    let plan = engine.preview_update_hosted(change.clone(), hosted());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let first = plan.items.first().ok_or("no rows")?;
    assert_eq!(first.object, ChangeObject::Dimension, "{:?}", plan.items);
    assert!(first.line.contains("is lost"), "{}", first.line);
    assert_eq!(plan.changes.first(), Some(&first.detail));
    engine.apply_hosted(
        plan.revision,
        Operation::Update(Box::new(pcbcraft_core::updates::ApplyUpdate {
            expected_revision: plan.revision,
            change,
            remove_conflicting_tracks: false,
            merge_named_nets: false,
        })),
        hosted(),
    )?;
    let trace = &engine.project().model_traces[0];
    assert_eq!(trace.status, ReferenceStatus::Lost);
    assert!(trace.geometry.is_some(), "the last geometry is kept");
    assert_eq!(x_of(&engine, part), held_at);
    let findings = checks(engine.project());
    assert_eq!(
        findings.iter().filter(|f| f.code == REFERENCE_LOST).count(),
        1,
        "{findings:?}"
    );
    let report = pcbcraft_core::fabrication::preflight::report(
        engine.project(),
        &engine.document().drafts,
        None,
    )?;
    assert!(
        report
            .items
            .iter()
            .any(|i| i.code == REFERENCE_LOST && i.blocks())
    );
    engine.undo()?;
    assert_eq!(engine.to_file(), before);
    assert_eq!(
        engine.project().model_traces[0].status,
        ReferenceStatus::Resolved
    );
    Ok(())
}
