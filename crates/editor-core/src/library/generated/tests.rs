//! Generated parts through the engine with a stand-in runtime: the core's
//! side of GP1 (expansion, record, embedding, dedupe, replacement and the
//! checks a host runs on load). The real module runs in the session tests.
use super::{GeneratorParams, GeneratorRuntime, GeneratorScope, verify};
use crate::commands::{Generate, GenerateReport, LibraryCommand, ModuleSource, Outcome};
use crate::diagnostics::Error;
use crate::ids::Revision;
use crate::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use crate::library::generated;
use crate::{Engine, Operation};
use sparksmith_generator_abi::v1::*;
use sparksmith_generator_abi::{Failure, Generated};
use std::sync::Arc;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A stand-in module: a two-terminal part whose pads sit `length / 2 −
/// 0.35 mm` from the origin. Its "bytes" choose a variant: `b"v2"` is
/// version 2 and moves the pads, as a changed module would; `REBUILT` is
/// version 1 built to other bytes; `OTHER` is another generator.
struct StandIn;

const MODULE: &[u8] = b"stand-in module";
const CHANGED: &[u8] = b"v2";
const REBUILT: &[u8] = b"stand-in module, rebuilt";
const OTHER: &[u8] = b"another generator";
/// A module whose model names another part file than the host evaluated.
const OTHER_PART: &[u8] = b"names another part file";
/// The stand-in's part file (the core never evaluates it).
const PART: &[u8] = b"stand-in part file";

/// The stand-in's model: its part file at the body length.
fn model(length: i64, part: &[u8]) -> Model {
    Model::PartFile {
        asset: asset_hash(part).to_string(),
        values: [("length".to_string(), ParamValue::Int(length))]
            .into_iter()
            .collect(),
    }
}

/// What the host gives `library.generate` for the stand-in's part file
/// at `length`: the Model it becomes, its bytes, the values in mm and
/// what it evaluated to.
fn evaluated(length: i64) -> crate::commands::GeneratedModel {
    use crate::library::model3d::{AssetRef, ModelDefinition, ModelFormat, ModelProvenance};
    use crate::library::package_model::{PartDerived, PartValue};
    use crate::units::Nm;
    let nm = |v: i64| Nm::new(v).unwrap_or_default();
    let bounds = [
        [nm(-length / 2), nm(-400_000), nm(0)],
        [nm(length / 2), nm(400_000), nm(450_000)],
    ];
    crate::commands::GeneratedModel {
        definition: ModelDefinition {
            asset: AssetRef {
                hash: asset_hash(PART),
                bytes: PART.len() as u64,
                media_type: ModelFormat::Solidcraft.media_type().into(),
                file_name: "stand-in.solidcraft".into(),
            },
            format: ModelFormat::Solidcraft,
            bounds_nm: bounds,
            provenance: ModelProvenance {
                record: crate::library::part::Provenance {
                    licence: Some("MIT OR Apache-2.0".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..ModelDefinition::default()
        },
        name: "Stand-in part".into(),
        data: Some(AssetBytes::new(PART.to_vec())),
        values: [("length".to_string(), PartValue::Mm(length as f64 / 1e6))]
            .into_iter()
            .collect(),
        derived: PartDerived {
            bounds_nm: bounds,
            bodies: 1,
            ..PartDerived::default()
        },
    }
}

fn length_param() -> Param {
    Param {
        key: "length".into(),
        label: "Body length".into(),
        help: None,
        ty: ParamType::Length {
            min_nm: 500_000,
            max_nm: 10_000_000,
            step_nm: None,
        },
        default: ParamValue::Int(1_600_000),
        group: None,
        context: None,
    }
}

fn graphic(shape: Primitive) -> Graphic {
    Graphic {
        shape,
        stroke_nm: 150_000,
        fill: Fill::None,
    }
}

fn generation(length: i64, shift: i64, scope: Scope, part: &[u8]) -> Generation {
    let x = length / 2 - 350_000 + shift;
    let pad = |key: &str, at: Point| Pad {
        key: key.into(),
        terminal: key.into(),
        shape: PadShape::Rect {
            size: [900_000, 1_000_000],
        },
        at,
        rot_mdeg: 0,
        side: Side::Top,
        hole: None,
        mask: Opening::Default,
        paste: Opening::Default,
    };
    let edge = x + 700_000;
    let component = scope == Scope::Component;
    Generation {
        component: component.then(|| Component {
            terminals: ["1", "2"]
                .iter()
                .map(|k| Terminal {
                    key: (*k).into(),
                    name: (*k).into(),
                    direction: Direction::Passive,
                })
                .collect(),
            prefix: "R".into(),
            name: "Stand-in resistor".into(),
            fields: vec!["value".into()],
        }),
        symbol: component.then(|| Symbol {
            units: vec![Unit {
                key: "A".into(),
                pins: vec![
                    SymbolPin {
                        terminal: "1".into(),
                        at: [-5_080_000, 0],
                        length_nm: 2_540_000,
                        rot_mdeg: 0,
                        show_name: false,
                        show_number: false,
                    },
                    SymbolPin {
                        terminal: "2".into(),
                        at: [5_080_000, 0],
                        length_nm: 2_540_000,
                        rot_mdeg: 180_000,
                        show_name: false,
                        show_number: false,
                    },
                ],
                graphics: vec![
                    Graphic {
                        shape: Primitive::Rect {
                            min: [-2_540_000, -1_000_000],
                            max: [2_540_000, 1_000_000],
                        },
                        stroke_nm: 250_000,
                        fill: Fill::Body,
                    },
                    graphic(Primitive::Text {
                        at: [0, 1_500_000],
                        height_nm: 1_270_000,
                        content: TextContent::Designator,
                        h_align: HAlign::Center,
                        v_align: VAlign::Bottom,
                    }),
                    graphic(Primitive::Arc {
                        center: [0, 0],
                        radius_nm: 500_000,
                        start_mdeg: 180_000,
                        sweep_mdeg: -180_000,
                    }),
                ],
            }],
        }),
        package: Package {
            terminals: vec!["1".into(), "2".into()],
            geometry: PackageGeometry {
                envelope_nm: [length, 800_000, 450_000],
                seating_nm: 0,
                leads: Vec::new(),
                pin1: "1".into(),
            },
        },
        footprint: Footprint {
            pads: vec![pad("1", [-x, 0]), pad("2", [x, 0])],
            courtyard: vec![
                [-edge, -750_000],
                [edge, -750_000],
                [edge, 750_000],
                [-edge, 750_000],
            ],
            silk: Vec::new(),
            assembly: vec![graphic(Primitive::Rect {
                min: [-length / 2, -400_000],
                max: [length / 2, 400_000],
            })],
        },
        binding: if component {
            ["1", "2"]
                .iter()
                .map(|k| BindingEntry {
                    package_terminal: (*k).into(),
                    terminal: (*k).into(),
                })
                .collect()
        } else {
            Vec::new()
        },
        model: Some(model(length, part)),
        fields: Vec::new(),
        notes: vec!["A stand-in.".into()],
    }
}

impl GeneratorRuntime for StandIn {
    fn describe(&self, module: &[u8]) -> std::result::Result<Description, Failure> {
        Ok(Description {
            abi: ABI,
            id_hint: if module == OTHER { "other" } else { "stand-in" }.into(),
            version: if module == CHANGED { 2 } else { 1 },
            name: "Stand-in".into(),
            families: vec!["chip".into()],
            scopes: vec![Scope::Component, Scope::Package],
            params: vec![length_param()],
            rules: Vec::new(),
            presets: Vec::new(),
            grammar: Vec::new(),
            instance_fields: Vec::new(),
        })
    }

    fn generate(
        &self,
        module: &[u8],
        request: &Request,
    ) -> std::result::Result<Generated, Failure> {
        let length = match request.params.get("length") {
            Some(ParamValue::Int(v)) => *v,
            _ => return Err(Failure::one("param.missing", "length".into())),
        };
        let shift = if module == CHANGED { 10_000 } else { 0 };
        let part = if module == OTHER_PART { CHANGED } else { PART };
        let generation = generation(length, shift, request.scope, part);
        let canonical = sparksmith_generator_abi::encode(&generation)
            .map_err(|e| Failure::one("generator.invalid_output", e.to_string()))?;
        Ok(Generated {
            result_hash: sparksmith_generator_abi::result_hash(&canonical),
            canonical,
            generation,
            fuel_used: None,
        })
    }
}

fn engine() -> Engine {
    let mut engine = Engine::default();
    engine.set_generators(Arc::new(StandIn));
    engine
}

fn request(module: &[u8], scope: GeneratorScope, length: i64) -> Generate {
    let mut params = GeneratorParams::new();
    params.insert("length".into(), generated::ParamValue::Int(length));
    Generate {
        module: ModuleSource {
            hash: asset_hash(module),
            data: Some(AssetBytes::new(module.to_vec())),
        },
        scope,
        params,
        name: None,
        replaces: None,
        model: Some(evaluated(length)),
    }
}

fn generate(engine: &mut Engine, generate: Generate) -> crate::Result<GenerateReport> {
    let outcome = engine.apply(
        engine.revision(),
        Operation::Library(LibraryCommand::Generate(Box::new(generate))),
    )?;
    match outcome {
        Outcome::Generated { report } => Ok(report),
        other => Err(Error::Invalid(format!("unexpected outcome {other:?}"))),
    }
}

fn run(module: &PackedAsset, request: &Request) -> std::result::Result<Generated, Failure> {
    StandIn.generate(&module.data, request)
}

#[test]
fn a_part_is_published_embedded_and_recorded_in_one_step() -> TestResult {
    let mut engine = engine();
    let report = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    assert!(report.created);
    assert_eq!(report.revisions.len(), 5);
    assert_eq!(report.notes, vec!["A stand-in.".to_string()]);
    assert_eq!(report.key.module, asset_hash(MODULE));
    let library = &engine.project().library;
    assert_eq!(library.generated.len(), 1);
    assert_eq!(library.components.len(), 1);
    assert_eq!(library.footprints.len(), 1);
    assert!(
        engine.drafts().items.is_empty(),
        "the drafts were published and closed"
    );
    let module = generated::module(library, asset_hash(MODULE)).ok_or("module not embedded")?;
    assert_eq!(module.data.as_slice(), MODULE);
    let part = &library.generated[0];
    // Complete parameters, the default filled in, and the result hash.
    assert_eq!(part.from.params.len(), 1);
    assert_eq!(part.from.result, report.result);
    assert_eq!(part.root, report.root);
    // The file round-trips and the host's check passes.
    let file = engine.to_file();
    file.validate()?;
    let bytes = crate::format::encode(&file)?;
    let reread = crate::format::decode(&bytes)?;
    assert_eq!(reread, file);
    verify(&reread.project.library, &mut run)?;
    // The expansion is exact: symbol arcs at quarter turns, pins along the
    // axes, pads where the generation put them.
    // The package's model is the module's part file, published as a Model
    // of the project, at the module's values.
    let package = library.packages.first().ok_or("no package")?;
    let crate::library::PackageModel::PartFile(part) = &package.definition.model else {
        return Err("the package has no part file".into());
    };
    let model = library
        .model(part.model)
        .ok_or("the part file is not a Model")?;
    assert_eq!(model.definition.asset.hash, asset_hash(PART));
    assert_eq!(
        part.values.get("length"),
        Some(&crate::library::package_model::PartValue::Mm(1.6))
    );
    // The stored package's values edited behind the record: refused on
    // load.
    let mut edited = engine.to_file();
    let packages = &mut edited.project.library.packages;
    if let Some(crate::library::PackageModel::PartFile(part)) =
        packages.first_mut().map(|p| &mut p.definition.model)
    {
        part.values.insert(
            "length".into(),
            crate::library::package_model::PartValue::Mm(1.7),
        );
    }
    let error = verify(&edited.project.library, &mut run)
        .err()
        .ok_or("accepted")?;
    assert_eq!(error.rule(), Some("generated.expansion_changed"), "{error}");
    let footprint = library.footprints.first().ok_or("no footprint")?;
    assert_eq!(footprint.definition.pads[0].position.x_nm.get(), -450_000);
    assert_eq!(footprint.definition.pads[1].number, "2");
    Ok(())
}

#[test]
fn the_same_key_finds_the_part_and_changes_nothing() -> TestResult {
    let mut engine = engine();
    let first = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    let revision = engine.revision();
    // The module is embedded now: no bytes needed.
    let mut again = request(MODULE, GeneratorScope::Component, 1_600_000);
    again.module.data = None;
    let second = generate(&mut engine, again)?;
    assert!(!second.created);
    assert_eq!(second.key, first.key);
    assert_eq!(second.root, first.root);
    assert_eq!(engine.revision(), revision);
    // Another length is another part, sharing the embedded module.
    let other = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Package, 2_000_000),
    )?;
    assert!(other.created);
    assert_eq!(other.revisions.len(), 2);
    assert_ne!(other.key, first.key);
    let library = &engine.project().library;
    // The module and the part file, each packed once; one Model of it.
    assert_eq!(library.assets.len(), 2);
    assert_eq!(library.models.len(), 1);
    assert_eq!(library.generated.len(), 2);
    Ok(())
}

#[test]
fn refusals_leave_the_document_unchanged() -> TestResult {
    let mut engine = Engine::default();
    let before = engine.to_file();
    // No runtime.
    let none = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    );
    assert!(matches!(none, Err(Error::Unsupported(m)) if m.starts_with("generator.runtime")));
    engine.set_generators(Arc::new(StandIn));
    // Bytes that are not the named module.
    let mut forged = request(MODULE, GeneratorScope::Component, 1_600_000);
    forged.module.hash = asset_hash(b"something else");
    let refused = generate(&mut engine, forged);
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.starts_with("generated.module_changed")),
        "{refused:?}"
    );
    // A module that is not embedded, given without bytes.
    let mut missing = request(MODULE, GeneratorScope::Component, 1_600_000);
    missing.module.data = None;
    assert!(matches!(
        generate(&mut engine, missing),
        Err(Error::NotFound(m)) if m.starts_with("generated.module_missing")
    ));
    // A model the host did not evaluate: refused, a part never exists
    // without its model.
    let mut unevaluated = request(MODULE, GeneratorScope::Component, 1_600_000);
    unevaluated.model = None;
    let refused = generate(&mut engine, unevaluated);
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.starts_with("generated.model_unevaluated")),
        "{refused:?}"
    );
    // The host evaluated another part file than the module names.
    let other = generate(
        &mut engine,
        request(OTHER_PART, GeneratorScope::Component, 1_600_000),
    );
    assert!(
        matches!(&other, Err(Error::Invalid(m)) if m.starts_with("generated.model:")),
        "{other:?}"
    );
    // Other values than the module's.
    let mut values = request(MODULE, GeneratorScope::Component, 1_600_000);
    if let Some(model) = &mut values.model {
        model.values.insert(
            "length".into(),
            crate::library::package_model::PartValue::Mm(1.7),
        );
    }
    let refused = generate(&mut engine, values);
    assert!(
        matches!(&refused, Err(Error::Invalid(m)) if m.contains("value \"length\" is not the generator's")),
        "{refused:?}"
    );
    // A parameter out of the module's range.
    let small = generate(&mut engine, request(MODULE, GeneratorScope::Component, 1));
    assert!(
        matches!(&small, Err(Error::Invalid(m)) if m.starts_with("param.out_of_range")),
        "{small:?}"
    );
    assert_eq!(engine.to_file(), before);
    assert_eq!(engine.revision(), 0);
    Ok(())
}

#[test]
fn a_changed_stored_part_or_result_is_refused_naming_both_hashes() -> TestResult {
    let mut engine = engine();
    generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    let file = engine.to_file();
    // The stored footprint edited behind the record's back.
    let mut edited = file.clone();
    let footprints = &mut edited.project.library.footprints;
    let footprint = footprints.first_mut().ok_or("no footprint")?;
    footprint.definition.pads[0].position.x_nm = crate::units::Nm::new(-460_000)?;
    let error = verify(&edited.project.library, &mut run)
        .err()
        .ok_or("accepted")?;
    let message = error.to_string();
    assert_eq!(
        error.rule(),
        Some("generated.expansion_changed"),
        "{message}"
    );
    assert_eq!(message.matches("sha256 ").count(), 2, "{message}");
    // The module now making something else from the same parameters.
    let mut changed = |_: &PackedAsset, request: &Request| StandIn.generate(CHANGED, request);
    let error = verify(&file.project.library, &mut changed)
        .err()
        .ok_or("accepted")?;
    let message = error.to_string();
    assert_eq!(error.rule(), Some("generated.result_changed"), "{message}");
    assert!(message.contains(&file.project.library.generated[0].from.result.to_string()));
    // Embedded bytes that no longer hash to the record's module.
    let mut swapped = file.clone();
    let asset = swapped
        .project
        .library
        .assets
        .first_mut()
        .ok_or("no asset")?;
    asset.data = AssetBytes::new(CHANGED.to_vec());
    let error = verify(&swapped.project.library, &mut run)
        .err()
        .ok_or("accepted")?;
    let message = error.to_string();
    assert_eq!(error.rule(), Some("generated.module_changed"), "{message}");
    assert!(
        message.contains(&asset_hash(MODULE).to_string()),
        "{message}"
    );
    assert!(
        message.contains(&asset_hash(CHANGED).to_string()),
        "{message}"
    );
    Ok(())
}

#[test]
fn records_need_their_module_and_revisions() -> TestResult {
    let mut engine = engine();
    generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    let file = engine.to_file();
    let mut no_module = file.clone();
    no_module
        .project
        .library
        .assets
        .retain(|a| a.media_type != generated::MODULE_MEDIA_TYPE);
    let error = no_module.validate().err().ok_or("accepted")?;
    assert_eq!(error.rule(), Some("generated.module_missing"), "{error}");
    let mut twice = file.clone();
    let part = twice.project.library.generated[0].clone();
    twice.project.library.generated.push(part);
    assert!(twice.validate().is_err());
    let mut wrong_kinds = file;
    wrong_kinds.project.library.generated[0]
        .expansion
        .swap(1, 2);
    assert!(wrong_kinds.validate().is_err());
    Ok(())
}

#[test]
fn a_changed_module_makes_the_next_revisions_of_the_same_lineages() -> TestResult {
    let mut engine = engine();
    let first = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    let mut update = request(CHANGED, GeneratorScope::Component, 1_600_000);
    update.replaces = Some(first.root);
    let second = generate(&mut engine, update)?;
    assert!(second.created);
    assert_eq!(second.root.id, first.root.id, "the same lineage");
    assert_eq!(
        second.root.revision,
        Revision::FIRST.next().ok_or("revision")?
    );
    for (old, new) in first.revisions.iter().zip(&second.revisions) {
        assert_eq!((old.kind, old.id), (new.kind, new.id));
        assert_eq!(new.revision.0, 2);
    }
    let library = &engine.project().library;
    assert_eq!(
        library.generated.len(),
        2,
        "both revisions keep their record"
    );
    let modules = library
        .assets
        .iter()
        .filter(|a| a.media_type == generated::MODULE_MEDIA_TYPE)
        .count();
    assert_eq!(modules, 2, "both modules are embedded");
    verify(library, &mut |module: &PackedAsset, request: &Request| {
        StandIn.generate(&module.data, request)
    })?;
    // The new component revision pins the new symbol and binding.
    let component = library
        .component(crate::ids::ComponentRef::new(
            second.root.id,
            second.root.revision,
        ))
        .ok_or("no component r2")?;
    assert_eq!(component.definition.representations[0].revision.0, 2);
    assert_eq!(component.definition.implementations[0].revision.0, 2);
    Ok(())
}

#[test]
fn a_newer_version_makes_the_next_revisions_and_an_older_one_is_refused() -> TestResult {
    let mut engine = engine();
    let first = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    assert_eq!(first.replaced, None);
    // Version 2 with the same parameters: the same lineages, revision 2.
    let second = generate(
        &mut engine,
        request(CHANGED, GeneratorScope::Component, 1_600_000),
    )?;
    assert!(second.created);
    assert_eq!(second.replaced, Some(first.root));
    for (old, new) in first.revisions.iter().zip(&second.revisions) {
        assert_eq!((old.kind, old.id), (new.kind, new.id));
        assert_eq!(new.revision.0, 2);
    }
    // Instances still pin revision 1: the update preview moves them.
    assert_eq!(engine.project().library.components.len(), 2);
    // Version 2 again: nothing happens.
    let revision = engine.revision();
    let mut again = request(CHANGED, GeneratorScope::Component, 1_600_000);
    again.module.data = None;
    let third = generate(&mut engine, again)?;
    assert!(!third.created);
    assert_eq!(third.root, second.root);
    assert_eq!(engine.revision(), revision);
    // Version 1 again, embedded or rebuilt to other bytes: refused, never
    // a downgrade, and the document is unchanged.
    let before = engine.to_file();
    for module in [MODULE, REBUILT] {
        let mut older = request(module, GeneratorScope::Component, 1_600_000);
        if module == MODULE {
            older.module.data = None;
        }
        let refused = generate(&mut engine, older);
        assert!(
            matches!(&refused, Err(Error::Conflict(m))
                if m.starts_with("generated.older_version")
                    && m.contains("version 2")
                    && m.contains("Nothing was changed")),
            "{refused:?}"
        );
    }
    // Naming the part to replace does not get round it.
    let mut named = request(MODULE, GeneratorScope::Component, 1_600_000);
    named.module.data = None;
    named.replaces = Some(second.root);
    assert!(matches!(
        generate(&mut engine, named),
        Err(Error::Conflict(m)) if m.starts_with("generated.older_version")
    ));
    assert_eq!(engine.to_file(), before);
    assert_eq!(engine.revision(), revision);
    verify(&engine.project().library, &mut run)?;
    Ok(())
}

#[test]
fn the_same_version_built_to_other_bytes_changes_nothing() -> TestResult {
    let mut engine = engine();
    let first = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    let before = engine.to_file();
    let rebuilt = generate(
        &mut engine,
        request(REBUILT, GeneratorScope::Component, 1_600_000),
    )?;
    assert!(!rebuilt.created);
    assert_eq!(rebuilt.root, first.root);
    assert_eq!(rebuilt.key, first.key);
    assert_eq!(
        engine.to_file(),
        before,
        "the rebuilt module is not embedded"
    );
    Ok(())
}

#[test]
fn other_parameters_scopes_or_generators_make_new_lineages() -> TestResult {
    let mut engine = engine();
    let first = generate(
        &mut engine,
        request(MODULE, GeneratorScope::Component, 1_600_000),
    )?;
    for (module, scope, length) in [
        (CHANGED, GeneratorScope::Component, 2_000_000),
        (CHANGED, GeneratorScope::Package, 1_600_000),
        (OTHER, GeneratorScope::Component, 1_600_000),
    ] {
        let made = generate(&mut engine, request(module, scope, length))?;
        assert!(made.created);
        assert_eq!(made.replaced, None);
        assert_ne!(made.root.id, first.root.id);
        assert_eq!(made.root.revision, Revision::FIRST);
    }
    Ok(())
}
