#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Slice S4 through the engine: faces of a generated model and of a STEP
//! model as dimension references, and the glTF refusal.
//!
//! - The demo's R1 chip resistor (a generated model) is held flush to the
//!   board's right edge by the end face of its second termination, and
//!   follows the edge when the board grows.
//! - A STEP model's face survives a library revision that keeps its file
//!   as `Resolved`; a revision with another file re-finds the face by its
//!   name and makes it `Inexact` until it is picked again.
//! - A glTF model's face is refused with a finding that names STEP and part
//!   files.
use super::super::tests::{Fallible, apply, edit, hosted, set, sketch, sketched};
use super::*;
use crate::models::imported::tests::{fixture_glb, fixture_step, lead_key, model, use_lead};
use crate::models::inspect::inspect;
use crate::models::named;
use sparksmith_core::Engine;
use sparksmith_core::Operation;
use sparksmith_core::format::EditorFile;
use sparksmith_core::ids::{ComponentRef, DefinitionId, Id, ModelRef, Revision};
use sparksmith_core::layout::dimensions::{DimensionCommand, REFERENCE_INEXACT, checks};
use sparksmith_core::library::bundle::{AssetBytes, PackedAsset};
use sparksmith_core::library::footprint_source::{DimKind, DimValue, SketchRef};
use sparksmith_core::library::model3d::{LeadTag, ModelFormat};
use sparksmith_core::library::{
    DefinitionMeta, LibraryModel, PackageDefinition, PackageModel, Published,
};
use sparksmith_core::model::dimension::{
    BoardRef, BodyName, FaceName, PartDimension, PartDimensionSpec, PartRelation,
};
use sparksmith_core::updates::{ApplyUpdate, Change};

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

fn flush(face: BoardRef, edge: BoardRef) -> PartDimensionSpec {
    PartDimensionSpec {
        kind: PartRelation::Dim(DimKind::Distance),
        refs: vec![face, edge],
        value: Some(DimValue::Fixed(0.0)),
        reference: false,
    }
}

fn r1(engine: &Engine) -> Fallible<Id> {
    Ok(engine
        .project()
        .instance_by_designator("R1")
        .ok_or("R1")?
        .id)
}

fn x_of(engine: &Engine, part: Id) -> f64 {
    engine.project().placement_of(part).unwrap().position[0]
}

/// The package R1 uses.
fn package_of(project: &sparksmith_core::Project, part: Id) -> Fallible<PackageDefinition> {
    let instance = project.instance(part).ok_or("instance")?;
    let physical = sparksmith_core::instances::resolve(&project.library, instance)?
        .physical
        .ok_or("physical")?;
    Ok(physical.package.definition.clone())
}

/// The face of body `body` in `state` whose outward normal is +x, as a
/// reference of `part` picked at its spot.
fn plus_x_face(state: &ModelState, body: &str, part: Id) -> Fallible<FeatureRef> {
    let b = state.bodies.iter().find(|b| b.name == body).ok_or("body")?;
    let names = naming::face_names(b);
    let spots = naming::face_spots(b);
    let i = (0..names.len())
        .filter(|i| {
            matches!(face_surface(&b.body, *i), Some(FaceSurface::Plane { normal, .. })
                if (normal.vec().x - 1.0).abs() < 1e-9)
        })
        .max_by(|a, b| {
            let x = |i: &usize| spots[*i].map_or(f64::MIN, |s| s.point.x);
            x(a).total_cmp(&x(b))
        })
        .ok_or("+x face")?;
    Ok(FeatureRef::Face {
        placement: part,
        face: ModelFaceRef {
            body: BodyName(body.into()),
            face: FaceName(names[i].clone()),
            pick: arr(spots[i].ok_or("spot")?.point),
        },
    })
}

/// The traced line's x in the part frame (a face square to x).
fn trace_x(engine: &Engine) -> Fallible<f64> {
    let t = engine.project().model_traces.first().ok_or("no trace")?;
    match t.geometry {
        Some(FeatureGeometry::Line { a, b }) if (a[0] - b[0]).abs() < 1e-9 => Ok(a[0]),
        other => Err(format!("{other:?}").into()),
    }
}

/// R1's end face (its second termination's +x face in the generated
/// model's own design) held flush to the right edge: R1 moves so the face
/// lies on the edge, and follows the edge when the board grows 10 mm.
#[test]
fn a_generated_models_end_face_holds_the_part_to_the_edge() -> Fallible<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let mut engine = sketched()?;
    let part = r1(&engine)?;
    let placement = engine
        .project()
        .placement_of(part)
        .ok_or("placement")?
        .clone();
    assert_eq!(
        placement.rotation_deg, 0.0,
        "the test reads x in the part frame"
    );
    let package = package_of(engine.project(), part)?;
    assert!(crate::models::is_generated(&package), "R1 is generated");
    let design = named::generated(&package)?;
    let face = plus_x_face(design.state(), "termination 2", part)?;
    let width = engine.project().board_size[0];
    let edge = right(&engine)?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(BoardRef::Part(face), edge),
        }),
    )?;
    let traces = &engine.project().model_traces;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].status, ReferenceStatus::Resolved, "{traces:?}");
    // The end face lies half the resistor's length right of its origin.
    let end = trace_x(&engine)?;
    assert!(end > 0.0, "{end}");
    assert!((x_of(&engine, part) + end - width).abs() < 1e-6);
    assert!(checks(engine.project()).is_empty());
    edit(
        &mut engine,
        vec![set("width", &format!("{} mm", width + 10.0))],
    )?;
    assert!(
        (x_of(&engine, part) + end - (width + 10.0)).abs() < 1e-6,
        "{}",
        x_of(&engine, part)
    );
    Ok(())
}

/// The demo with R1's and R2's package given the library model
/// `definition` (with its `bytes`) in place of its generated model.
fn with_library_model(format: ModelFormat, bytes: &[u8]) -> Fallible<(Engine, ModelRef)> {
    let engine = sketched()?;
    let mut file: EditorFile = engine.to_file();
    let project = &mut file.project;
    let seen = inspect(bytes, format)?;
    let mut definition = model(format, bytes, &seen);
    definition.asset.media_type = format.media_type().into();
    definition.asset.file_name = match format {
        ModelFormat::Step => "fixture.step".into(),
        _ => "fixture.glb".into(),
    };
    definition.leads = vec![LeadTag {
        key: lead_key(&seen)?,
        designation: "1".into(),
        contact_nm: crate::test_units::nm3([0.0; 3]),
    }];
    definition.leads[0].contact_nm =
        crate::test_units::nm3(crate::models::imported::lead_contacts(&definition, bytes)?[0]);
    definition.provenance.record.licence = Some("CC0-1.0".into());
    let id = DefinitionId(project.allocate()?);
    let reference = ModelRef::new(id, Revision(1));
    project.library.assets.push(PackedAsset {
        hash: definition.asset.hash,
        media_type: definition.asset.media_type.clone(),
        data: AssetBytes::new(bytes.to_vec()),
    });
    project.library.models.push(Published {
        id,
        revision: Revision(1),
        meta: DefinitionMeta {
            name: "Original fixture".into(),
            ..DefinitionMeta::default()
        },
        definition,
    });
    let part = r1(&engine)?;
    let instance = project.instance(part).ok_or("instance")?.clone();
    let package_id = sparksmith_core::instances::resolve(&project.library, &instance)?
        .physical
        .ok_or("physical")?
        .package
        .id;
    let package = project
        .library
        .packages
        .iter_mut()
        .find(|p| p.id == package_id)
        .ok_or("package")?;
    let terminal = package.definition.terminals.first().ok_or("terminal")?.id.0;
    package.definition.model = PackageModel::Library(LibraryModel {
        model: reference,
        ..use_lead(terminal)
    });
    Ok((Engine::from_file(file)?, reference))
}

/// The next revision of R1's whole library chain (model, package,
/// footprint, binding, component), the model's file `bytes` (the same
/// bytes for a revision that keeps the file).
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
        .filter(|b| Some(b.id) == instance.physical.map(|ph| ph.binding))
        .max_by_key(|b| b.revision)
        .ok_or("binding")?
        .clone();
    let package = lib
        .package(binding.definition.package)
        .ok_or("package")?
        .clone();
    let footprint = lib
        .footprints
        .iter()
        .filter(|f| f.definition.package.id == package.id)
        .max_by_key(|f| f.revision)
        .ok_or("footprint")?
        .clone();
    let PackageModel::Library(use_) = &package.definition.model else {
        return Err("not a library model".into());
    };
    let mut model = lib.model(use_.model).ok_or("model")?.clone();
    model.revision = next(model.revision);
    let hash = sparksmith_core::library::bundle::asset_hash(bytes);
    if hash != model.definition.asset.hash {
        model.definition.asset.hash = hash;
        model.definition.asset.bytes = bytes.len() as u64;
        lib.assets.push(PackedAsset {
            hash,
            media_type: model.definition.asset.media_type.clone(),
            data: AssetBytes::new(bytes.to_vec()),
        });
    }
    let mut package2 = package.clone();
    package2.revision = next(package.revision);
    if let PackageModel::Library(u) = &mut package2.definition.model {
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

/// Apply the revision `target` to `part` as a library update does.
fn update(engine: &mut Engine, part: Id, target: ComponentRef) -> Fallible<()> {
    let change = Change::ComponentRevision {
        instances: vec![part],
        target,
        resolutions: Default::default(),
    };
    let plan = engine.preview_update_hosted(change.clone(), hosted());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    engine.apply_hosted(
        plan.revision,
        Operation::Update(Box::new(ApplyUpdate {
            expected_revision: plan.revision,
            change,
            remove_conflicting_tracks: false,
            merge_named_nets: false,
        })),
        hosted(),
    )?;
    Ok(())
}

/// The STEP fixture written again under another name: the same bodies,
/// other bytes.
fn other_step_file() -> Fallible<Vec<u8>> {
    let bytes = fixture_step()?;
    let text = String::from_utf8(bytes)?;
    let other = text.replacen("fixture", "fixture-b", 1);
    assert_ne!(other, text, "the file names itself");
    Ok(other.into_bytes())
}

/// A STEP model's face is traced on the design that imports it. A
/// revision that keeps the file keeps the reference `Resolved` (the import
/// is the same for the same bytes); a revision with another file re-finds
/// the face by its name but, its names worked out from geometry, only as
/// `Inexact`, which stops it driving and blocks fabrication output, until
/// a new pick at the point the note gives makes it `Resolved` again.
#[test]
fn a_step_face_is_inexact_after_a_new_file_until_picked_again() -> Fallible<()> {
    let bytes = fixture_step()?;
    let (mut engine, _) = with_library_model(ModelFormat::Step, &bytes)?;
    let part = r1(&engine)?;
    let package = package_of(engine.project(), part)?;
    let PackageModel::Library(use_) = &package.model else {
        return Err("not a library model".into());
    };
    let definition = engine
        .project()
        .library
        .model(use_.model)
        .ok_or("model")?
        .definition
        .clone();
    let design = named::step(&definition, &bytes)?;
    let body = design
        .state()
        .bodies
        .iter()
        .find(|b| b.name.contains("Body"))
        .ok_or("Body")?
        .name
        .clone();
    let face = plus_x_face(design.state(), &body, part)?;
    let edge = right(&engine)?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(BoardRef::Part(face.clone()), edge),
        }),
    )?;
    let first = engine.project().model_traces[0].clone();
    assert_eq!(first.status, ReferenceStatus::Resolved, "{first:?}");
    // A revision that keeps the file.
    let (mut engine, target) = revise(&engine, part, &bytes)?;
    update(&mut engine, part, target)?;
    let kept = engine.project().model_traces[0].clone();
    assert_ne!(kept.package, first.package, "traced in the new revision");
    assert_eq!(kept.status, ReferenceStatus::Resolved, "{kept:?}");
    assert_eq!(kept.geometry, first.geometry);
    // A revision with another file.
    let held_at = x_of(&engine, part);
    let (mut engine, target) = revise(&engine, part, &other_step_file()?)?;
    update(&mut engine, part, target)?;
    let refound = engine.project().model_traces[0].clone();
    assert_eq!(refound.status, ReferenceStatus::Inexact, "{refound:?}");
    assert!(
        refound.note.contains("worked out from the"),
        "{}",
        refound.note
    );
    assert_eq!(
        x_of(&engine, part),
        held_at,
        "an inexact reference stops driving"
    );
    let findings = checks(engine.project());
    assert!(
        findings.iter().any(|f| f.code == REFERENCE_INEXACT),
        "{findings:?}"
    );
    // Picked again at the point the note gives.
    let at = point_in(&refound.note).ok_or("the note gives a point")?;
    let FeatureRef::Face {
        face: mut picked, ..
    } = face
    else {
        return Err("face".into());
    };
    picked.pick = at;
    let dimension: PartDimension = engine.project().part_dimensions[0].clone();
    let mut again = dimension.clone();
    again.refs[0] = BoardRef::Part(FeatureRef::Face {
        placement: part,
        face: picked,
    });
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Edit { dimension: again }),
    )?;
    let traces = &engine.project().model_traces;
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].status, ReferenceStatus::Resolved, "{traces:?}");
    assert!(checks(engine.project()).is_empty());
    Ok(())
}

/// The last `(x, y, z)` a note gives, in millimetres.
fn point_in(note: &str) -> Option<[f64; 3]> {
    let open = note.rfind('(')?;
    let close = open + note[open..].find(')')?;
    let numbers: Vec<f64> = note[open + 1..close]
        .split(',')
        .map(|n| n.trim().trim_end_matches("mm").trim().parse().ok())
        .collect::<Option<_>>()?;
    numbers.try_into().ok()
}

/// A glTF model has no faces: a dimension to one is refused, nothing
/// changes, and the refusal steers the author to a STEP model or a part
/// file.
#[test]
fn a_gltf_face_is_refused_with_the_mesh_finding() -> Fallible<()> {
    let (mut engine, _) = with_library_model(ModelFormat::Gltf, &fixture_glb())?;
    let part = r1(&engine)?;
    let face = FeatureRef::Face {
        placement: part,
        face: ModelFaceRef {
            body: BodyName("body".into()),
            face: FaceName("F1:top".into()),
            pick: [0.0, 0.0, 0.5],
        },
    };
    let before = engine.to_file();
    let rev = engine.revision();
    let edge = right(&engine)?;
    let refused = apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(BoardRef::Part(face), edge),
        }),
    );
    let Err(sparksmith_core::Error::Unsupported(why)) = refused else {
        panic!("{refused:?}")
    };
    assert!(why.contains("layout.dimension_model"), "{why}");
    assert!(why.contains("glTF"), "{why}");
    assert!(why.contains("STEP model or a part file"), "{why}");
    assert_eq!(engine.revision(), rev);
    assert_eq!(engine.to_file(), before);
    Ok(())
}

/// A point on R1's model in the board (board mm, as the 3D view has it):
/// `p` in the model frame through the model placement, the placement and
/// the seating on the top copper.
fn board_point(
    engine: &Engine,
    part: Id,
    package: &PackageDefinition,
    p: [f64; 3],
) -> Fallible<[f64; 3]> {
    let project = engine.project();
    let placement = project.placement_of(part).ok_or("placement")?;
    let local = package.placement.transform().apply(p);
    let at = sparksmith_core::layout::dimensions::view::part_transform(placement)
        .apply([local[0], local[1]]);
    let seat = project
        .stackup
        .copper_span(sparksmith_core::CopperLayer::Top)
        .map_or(0.0, |s| s.1);
    Ok([at[0], at[1], seat + local[2]])
}

/// Slices S4 and S5: a 3D pick on R1's end face (a generated 0603's
/// second termination) names the face in the generated model's own
/// design; dimensioned flush to the right edge, R1 follows the board 10 mm
/// wider.
#[test]
fn a_3d_pick_on_a_generated_end_face_holds_r1_to_the_edge() -> Fallible<()> {
    let _context = crate::looks::test_context().map(|c| c.enter());
    let mut engine = sketched()?;
    let part = r1(&engine)?;
    let package = package_of(engine.project(), part)?;
    let design = named::generated(&package)?;
    let FeatureRef::Face { face, .. } = plus_x_face(design.state(), "termination 2", part)? else {
        return Err("face".into());
    };
    let at = board_point(&engine, part, &package, face.pick)?;
    let (picked, trace) = sparksmith_core::layout::dimensions::model::pick(
        engine.project(),
        &crate::board::SketchBoardEvaluator,
        part,
        at,
        0.0,
    )?;
    let FeatureRef::Face { face: named, .. } = &picked else {
        return Err(format!("{picked:?}").into());
    };
    assert_eq!((&named.body, &named.face), (&face.body, &face.face));
    let FeatureGeometry::Line { a, .. } = trace else {
        return Err(format!("{trace:?}").into());
    };
    let width = engine.project().board_size[0];
    let edge = right(&engine)?;
    apply(
        &mut engine,
        Operation::Dimension(DimensionCommand::Add {
            dimension: flush(BoardRef::Part(picked), edge),
        }),
    )?;
    assert_eq!(
        engine.project().model_traces[0].status,
        ReferenceStatus::Resolved
    );
    assert!((x_of(&engine, part) + a[0] - width).abs() < 1e-6);
    edit(
        &mut engine,
        vec![set("width", &format!("{} mm", width + 10.0))],
    )?;
    assert!((x_of(&engine, part) + a[0] - (width + 10.0)).abs() < 1e-6);
    Ok(())
}

/// A 3D pick on a glTF model is refused with the mesh finding, the words
/// the Dimension tool's card shows.
#[test]
fn a_3d_pick_on_a_gltf_model_is_refused_with_the_mesh_finding() -> Fallible<()> {
    let (engine, _) = with_library_model(ModelFormat::Gltf, &fixture_glb())?;
    let part = r1(&engine)?;
    let position = engine
        .project()
        .placement_of(part)
        .ok_or("placement")?
        .position;
    let refused = sparksmith_core::layout::dimensions::model::pick(
        engine.project(),
        &crate::board::SketchBoardEvaluator,
        part,
        [position[0], position[1], 1.7],
        0.0,
    );
    let Err(sparksmith_core::Error::Unsupported(why)) = refused else {
        panic!("{refused:?}")
    };
    assert!(
        why.contains("glTF") && why.contains("STEP model or a part file"),
        "{why}"
    );
    Ok(())
}
