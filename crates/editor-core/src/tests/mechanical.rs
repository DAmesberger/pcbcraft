//! Mechanical parts placed directly on the board (docs/mechanical-parts.md):
//! the variant set, dedupe, re-pointing, removal and the assembly outputs.
use super::support::{Snapshot, apply};
use crate::fabrication::assembly::{PlacementConvention, rows};
use crate::ids::ContentHash;
use crate::layout::LayoutCommand;
use crate::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use crate::library::mechanical::{
    LandShape, MechanicalLand, PadUse, PartModelSource, PartSpec, find_variant, variant_key,
};
use crate::library::model3d::{AssetRef, ModelDefinition, ModelFormat};
use crate::library::package_model::{PartDerived, PartValue};
use crate::library::{PackageModel, PadKind, PasteRule};
use crate::model::Layer;
use crate::tests::support::{mdeg, nm, pt};
use crate::{Engine, Operation, Outcome, Result};
use std::collections::BTreeMap;

fn missing(what: &str) -> crate::Error {
    crate::Error::NotFound(what.into())
}

const FILE: &[u8] = b"{\"format\": \"stand-in part file bytes\"}";

fn file_source() -> PartModelSource {
    let hash = asset_hash(FILE);
    PartModelSource::File {
        name: "nut".into(),
        definition: Box::new(ModelDefinition {
            asset: AssetRef {
                hash,
                bytes: FILE.len() as u64,
                media_type: "application/x-solidcraft".into(),
                file_name: "nut.solidcraft".into(),
            },
            format: ModelFormat::Solidcraft,
            bounds_nm: crate::tests::support::nm_bounds([[-2.5, -2.5, 0.0], [2.5, 2.5, 3.0]]),
            provenance: crate::library::model3d::ModelProvenance {
                record: crate::library::Provenance {
                    licence: Some("MIT OR Apache-2.0".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..ModelDefinition::default()
        }),
        asset: PackedAsset {
            hash,
            media_type: "application/x-solidcraft".into(),
            data: AssetBytes::new(FILE.to_vec()),
        },
    }
}

fn nut(height: f64, pads: PadUse) -> PartSpec {
    PartSpec {
        name: "SMD nut".into(),
        designator_prefix: "H".into(),
        source: "smd-nut".into(),
        model: file_source(),
        values: BTreeMap::from([("height".to_owned(), PartValue::Mm(height))]),
        derived: PartDerived {
            bounds_nm: crate::tests::support::nm_bounds([[-2.5, -2.5, 0.0], [2.5, 2.5, height]]),
            bodies: 1,
            surfaces: vec![],
            datums: vec![],
        },
        lands: vec![MechanicalLand {
            name: "1".into(),
            at: pt([0.0, 0.0]),
            shape: LandShape::Circle {
                diameter: nm(5.6),
                hole: Some(nm(3.2)),
            },
        }],
        pads,
        bom: true,
        pick_and_place: None,
    }
}

fn heatsink() -> PartSpec {
    PartSpec {
        name: "Finned heatsink".into(),
        designator_prefix: "HS".into(),
        source: "finned-heatsink".into(),
        values: BTreeMap::new(),
        derived: PartDerived {
            bounds_nm: crate::tests::support::nm_bounds([[-10.0, -10.0, 0.0], [10.0, 10.0, 10.0]]),
            bodies: 7,
            surfaces: vec![],
            datums: vec![],
        },
        lands: vec![],
        ..nut(3.0, PadUse::Mechanical)
    }
}

fn place(engine: &mut Engine, part: PartSpec, at: [f64; 2]) -> Result<(u64, bool)> {
    match apply(
        engine,
        Operation::Layout(LayoutCommand::PlacePart {
            part: Box::new(part),
            position: at,
            rotation_deg: 0.0,
            layer: Layer::Top,
            designator: None,
        }),
    )? {
        Outcome::PartPlaced {
            instance, created, ..
        } => Ok((instance, created)),
        other => Err(crate::Error::Invalid(format!(
            "unexpected outcome {other:?}"
        ))),
    }
}

#[test]
fn a_placed_part_is_an_off_sheet_instance_of_a_generated_set() -> Result<()> {
    let mut engine = Engine::default();
    let before = engine.revision();
    let (id, created) = place(&mut engine, nut(3.0, PadUse::Mechanical), [20.0, 20.0])?;
    assert!(created);
    assert_eq!(engine.revision(), before + 1, "one undo step");
    let project = engine.project();
    let instance = project.instance(id).ok_or_else(|| missing("instance"))?;
    assert_eq!(instance.designator, "H1");
    assert!(project.occurrences_of(id).next().is_none(), "off-sheet");
    assert_eq!(instance.unused_units.len(), 1);
    let placement = project.placement_of(id).ok_or_else(|| missing("placed"))?;
    assert_eq!(placement.position, [20.0, 20.0]);
    let library = &project.library;
    let component = library
        .component(instance.component)
        .ok_or_else(|| missing("component"))?;
    assert!(component.definition.terminals.is_empty(), "mechanical pads");
    let record = crate::library::mechanical::mechanical_of(library, instance.component)
        .ok_or_else(|| missing("record"))?;
    assert!(record.bom && record.pick_and_place, "copper lands: in both");
    assert_eq!(library.models.len(), 1);
    assert_eq!(library.assets.len(), 1);
    let refs = crate::library::mechanical::variant_refs(library, instance.component)
        .ok_or_else(|| missing("refs"))?;
    let package = library
        .package(refs.package)
        .ok_or_else(|| missing("package"))?;
    assert!(matches!(
        package.definition.model,
        PackageModel::PartFile(_)
    ));
    let footprint = &library
        .footprint(refs.footprint)
        .ok_or_else(|| missing("footprint"))?
        .definition;
    let pad = &footprint.pads[0];
    assert_eq!(pad.kind, PadKind::Mechanical);
    assert!(pad.terminal.is_none());
    assert!(pad.hole.as_ref().is_some_and(|h| !h.plated));
    assert!(matches!(&pad.paste, PasteRule::Windows(w) if w.len() == 4));
    // The generated courtyard is checked like any part's.
    assert_eq!(
        crate::layout::placed_courtyards(project)
            .iter()
            .filter(|c| c.instance == id)
            .count(),
        1
    );
    // Undo removes the instance and the set as one step.
    engine.undo()?;
    assert!(engine.project().instances.is_empty());
    assert!(engine.project().library.components.is_empty());
    Ok(())
}

#[test]
fn equal_specs_share_one_variant_and_the_model_is_shared_by_hash() -> Result<()> {
    let mut engine = Engine::default();
    let (a, first) = place(&mut engine, nut(3.0, PadUse::Mechanical), [10.0, 10.0])?;
    let (b, second) = place(&mut engine, nut(3.0, PadUse::Mechanical), [30.0, 10.0])?;
    let (c, third) = place(&mut engine, nut(4.0, PadUse::Mechanical), [50.0, 10.0])?;
    assert!(first && !second && third);
    let project = engine.project();
    let component = |id| project.instance(id).map(|i| i.component);
    assert_eq!(component(a), component(b));
    assert_ne!(component(a), component(c));
    assert_eq!(project.library.components.len(), 2);
    assert_eq!(project.library.models.len(), 1, "one Model per file");
    let designators: Vec<_> = project
        .instances
        .iter()
        .map(|i| i.designator.as_str())
        .collect();
    assert_eq!(designators, ["H1", "H2", "H3"]);
    let key = variant_key(&nut(3.0, PadUse::Mechanical), asset_hash(FILE))?;
    assert!(find_variant(&project.library, key).is_some());
    assert!(find_variant(&project.library, ContentHash([0; 32])).is_none());
    Ok(())
}

/// The variant key's canonical form is millimetre lands with their wire
/// names, so a key stored before lands were nanometres still finds its
/// variant. Pinned to the value computed when lands were f64 mm.
#[test]
fn variant_keys_stay_valid_across_the_nanometre_lands() -> Result<()> {
    let mut spec = nut(3.0, PadUse::Terminals);
    spec.lands.push(MechanicalLand {
        name: "2".into(),
        at: pt([-3.25, 1.5]),
        shape: LandShape::Rect {
            size: pt([1.2, 0.6]),
            rotation_deg: crate::units::Degrees(mdeg(90.0)),
        },
    });
    spec.lands.push(MechanicalLand {
        name: "3".into(),
        at: pt([2.0, -0.125]),
        shape: LandShape::Hole { diameter: nm(1.05) },
    });
    let key = variant_key(&spec, asset_hash(FILE))?;
    assert_eq!(
        key.to_string(),
        "4471db07556989250fab610ab9a2917519ac11ce9091c6b4da9961a2aa007eaf"
    );
    Ok(())
}

#[test]
fn terminal_pads_land_terminals_and_the_bom_and_pick_and_place_follow_the_flags() -> Result<()> {
    let mut engine = Engine::default();
    let (nut_id, _) = place(&mut engine, nut(3.0, PadUse::Terminals), [10.0, 10.0])?;
    let (sink, _) = place(&mut engine, heatsink(), [40.0, 30.0])?;
    let project = engine.project();
    let instance = project.instance(nut_id).ok_or_else(|| missing("nut"))?;
    let component = project
        .library
        .component(instance.component)
        .ok_or_else(|| missing("c"))?;
    assert_eq!(component.definition.terminals.len(), 1);
    assert_eq!(
        project.instance(sink).map(|i| i.designator.as_str()),
        Some("HS1")
    );
    // The heatsink has no copper: in the BOM, not in pick-and-place.
    let placed: Vec<_> = rows(project, &PlacementConvention::default())?
        .into_iter()
        .map(|r| r.designator)
        .collect();
    assert_eq!(placed, ["H1"]);
    let bom = crate::completeness::bom(project)?;
    let listed: Vec<_> = bom
        .lines
        .iter()
        .flat_map(|l| l.designators.clone())
        .collect();
    assert_eq!(listed, ["H1", "HS1"]);
    // Out of the BOM: a variant of its own.
    let mut hidden = heatsink();
    hidden.bom = false;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetPart {
            instance: sink,
            part: Box::new(hidden),
        }),
    )?;
    let bom = crate::completeness::bom(engine.project())?;
    let listed: Vec<_> = bom
        .lines
        .iter()
        .flat_map(|l| l.designators.clone())
        .collect();
    assert_eq!(listed, ["H1"]);
    Ok(())
}

#[test]
fn editing_values_moves_the_instance_to_the_variant_and_keeps_its_identity() -> Result<()> {
    let mut engine = Engine::default();
    let (id, _) = place(&mut engine, nut(3.0, PadUse::Mechanical), [12.0, 14.0])?;
    let old = engine.project().instance(id).map(|i| i.component);
    let outcome = apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetPart {
            instance: id,
            part: Box::new(nut(5.0, PadUse::Mechanical)),
        }),
    )?;
    assert!(
        matches!(outcome, Outcome::PartPlaced { instance, created: true, .. } if instance == id)
    );
    let project = engine.project();
    let instance = project.instance(id).ok_or_else(|| missing("kept"))?;
    assert_eq!(instance.designator, "H1");
    assert_ne!(Some(instance.component), old);
    assert_eq!(
        project.placement_of(id).map(|p| p.position),
        Some([12.0, 14.0])
    );
    let refs = crate::library::mechanical::variant_refs(&project.library, instance.component)
        .ok_or_else(|| missing("refs"))?;
    let package = project
        .library
        .package(refs.package)
        .ok_or_else(|| missing("package"))?;
    let PackageModel::PartFile(file) = &package.definition.model else {
        return Err(missing("a part file package"));
    };
    assert_eq!(file.values.get("height"), Some(&PartValue::Mm(5.0)));
    // Back to the first values: the existing variant, nothing made.
    let outcome = apply(
        &mut engine,
        Operation::Layout(LayoutCommand::SetPart {
            instance: id,
            part: Box::new(nut(3.0, PadUse::Mechanical)),
        }),
    )?;
    assert!(matches!(
        outcome,
        Outcome::PartPlaced { created: false, .. }
    ));
    assert_eq!(engine.project().instance(id).map(|i| i.component), old);
    Ok(())
}

#[test]
fn refusals_change_nothing() -> Result<()> {
    let mut engine = Engine::default();
    let (id, _) = place(&mut engine, nut(3.0, PadUse::Mechanical), [10.0, 10.0])?;
    let kept = Snapshot::of(&engine);
    let mut bad = nut(3.0, PadUse::Mechanical);
    bad.designator_prefix = "h1".into();
    assert!(place(&mut engine, bad, [20.0, 20.0]).is_err());
    let mut bad = nut(3.0, PadUse::Mechanical);
    bad.lands[0].shape = LandShape::Circle {
        diameter: nm(3.0),
        hole: Some(nm(3.0)),
    };
    assert!(place(&mut engine, bad, [20.0, 20.0]).is_err());
    kept.assert_kept(&engine, "invalid specs");
    // A non-mechanical instance cannot be re-pointed or removed this way.
    assert!(
        apply(
            &mut engine,
            Operation::Layout(LayoutCommand::RemovePart {
                instance: id + 1000
            }),
        )
        .is_err()
    );
    kept.assert_kept(&engine, "unknown instance");
    Ok(())
}

#[test]
fn removing_a_board_only_part_takes_its_placement() -> Result<()> {
    let mut engine = Engine::default();
    let (id, _) = place(&mut engine, heatsink(), [30.0, 30.0])?;
    apply(
        &mut engine,
        Operation::Layout(LayoutCommand::RemovePart { instance: id }),
    )?;
    assert!(engine.project().instance(id).is_none());
    assert!(engine.project().placements.is_empty());
    Ok(())
}
