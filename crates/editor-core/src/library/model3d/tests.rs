//! Slice D1 behaviour: the Model kind's publish rules, `library.import_model`,
//! packages that use a library model, snapshot and reopen rules, bundles
//! (closure, assets, promote refusal) and update-plan lines. Fixtures are
//! original bytes generated here; no vendor model is read.
use super::*;
use crate::diagnostics::Error;
use crate::ids::{DefinitionKind, LibraryId, Revision, SourceLineageId};
use crate::library::bundle::{self, AssetBytes, PackedAsset, asset_hash};
use crate::library::part::Basis;
use crate::library::{Definition, DraftEdit, PackageModel, Published, import};
use crate::materials::MaterialId;
use crate::tests::support::*;
use crate::{Engine, LibraryCommand, Operation, Outcome};

type Fallible<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Original fixture bytes: not a real STEP file, only content to hash.
pub(crate) fn bytes(tag: &str) -> Vec<u8> {
    format!("ISO-10303-21; pcbcraft original test fixture {tag}; END-ISO-10303-21;").into_bytes()
}

pub(crate) fn packed(data: &[u8], media_type: &str) -> PackedAsset {
    PackedAsset {
        hash: asset_hash(data),
        media_type: media_type.into(),
        data: AssetBytes::new(data.to_vec()),
    }
}

pub(crate) fn asset_ref(asset: &PackedAsset, file_name: &str) -> AssetRef {
    AssetRef {
        hash: asset.hash,
        bytes: asset.data.len() as u64,
        media_type: asset.media_type.clone(),
        file_name: file_name.into(),
    }
}

/// A complete two-lead model: 4 × 3 × 1 mm, leads "1" and "2" at x = ∓1.5.
pub(crate) fn model(asset: &PackedAsset) -> ModelDefinition {
    ModelDefinition {
        asset: asset_ref(asset, "two-lead.step"),
        pair: None,
        format: ModelFormat::Step,
        frame: SourceFrame::default(),
        bounds_mm: [[-2.0, -1.5, 0.0], [2.0, 1.5, 1.0]],
        bodies: vec![
            ModelBody {
                key: "body".into(),
                role: BodyRole::Body,
                material: ModelMaterial::Builtin(MaterialId::EpoxyMould),
            },
            ModelBody {
                key: "lead-1".into(),
                role: BodyRole::Lead,
                material: ModelMaterial::Builtin(MaterialId::TinMatte),
            },
        ],
        custom_materials: vec![],
        leads: vec![
            LeadTag {
                key: "lead-1".into(),
                designation: "1".into(),
                contact: [-1.5, 0.0, 0.0],
            },
            LeadTag {
                key: "lead-2".into(),
                designation: "2".into(),
                contact: [1.5, 0.0, 0.0],
            },
        ],
        marking_area: Some(MarkingArea {
            origin: [0.0, 0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
            u_axis: [1.0, 0.0, 0.0],
            size_mm: [3.0, 2.0],
        }),
        hints: ModelHints {
            mount: Some(Mount::Smd),
            pins: Some(2),
            pitch_mm: Some(3.0),
            family: Some("two-lead test body".into()),
        },
        provenance: ModelProvenance {
            vendor: Some("Pcbcraft tests".into()),
            url: None,
            retrieved: Some("2026-11-01".into()),
            redistribution: Redistribution::Permitted,
            record: Provenance {
                origin: None,
                basis: Basis::Authored,
                licence: Some("CC0-1.0".into()),
            },
        },
    }
}

/// [`model`] with the bounds of `envelope` and its leads inside them.
fn sized(asset: &PackedAsset, envelope: [f64; 3]) -> ModelDefinition {
    let [l, w, h] = envelope;
    let mut m = model(asset);
    m.bounds_mm = [[-l / 2.0, -w / 2.0, 0.0], [l / 2.0, w / 2.0, h]];
    m.leads[0].contact = [-l / 4.0, 0.0, 0.0];
    m.leads[1].contact = [l / 4.0, 0.0, 0.0];
    m.marking_area = None;
    m
}

fn codes(model: &ModelDefinition) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    model.findings(DefinitionId(1), &mut out);
    out.into_iter()
        .map(|f| {
            let blocks = f.blocks_stage(Stage::Publish);
            (f.code, blocks)
        })
        .collect()
}

fn blocking(model: &ModelDefinition) -> Vec<String> {
    codes(model)
        .into_iter()
        .filter(|(_, b)| *b)
        .map(|(c, _)| c)
        .collect()
}

pub(crate) fn import_request(
    name: &str,
    asset: &PackedAsset,
    definition: ModelDefinition,
) -> Operation {
    Operation::Library(LibraryCommand::ImportModel(Box::new(ImportModel {
        name: name.into(),
        assets: vec![asset.clone()],
        definition: Box::new(definition),
        assign: None,
    })))
}

/// Import and publish one model, returning its first revision.
pub(crate) fn published_model(
    engine: &mut Engine,
    definition: ModelDefinition,
    asset: &PackedAsset,
) -> Fallible<ModelRef> {
    let Outcome::DraftCreated { id } =
        apply(engine, import_request("Two-lead body", asset, definition))?
    else {
        return Err("import_model creates a draft".into());
    };
    let published = publish(engine, vec![id])?;
    let r = published.first().ok_or("published")?;
    assert_eq!(r.kind, DefinitionKind::Model);
    Ok(ModelRef::new(r.id, r.revision))
}

#[test]
fn a_complete_model_has_no_findings_and_each_rule_names_its_code() {
    let asset = packed(&bytes("rules"), "model/step");
    let good = model(&asset);
    assert_eq!(codes(&good), Vec::<(String, bool)>::new());

    let mut m = good.clone();
    m.asset.hash = ContentHash::default();
    assert_eq!(blocking(&m), ["model.asset"]);
    let mut m = good.clone();
    m.asset.media_type = "model/gltf-binary".into();
    assert_eq!(
        blocking(&m),
        ["model.asset"],
        "a STEP model with a glTF media type"
    );
    let mut m = good.clone();
    m.pair = Some(m.asset.clone());
    assert_eq!(
        blocking(&m),
        ["model.asset"],
        "a pair that is the asset itself"
    );
    let mut m = good.clone();
    m.format = ModelFormat::Gltf;
    m.asset.media_type = "model/gltf-binary".into();
    m.pair = Some(asset_ref(&packed(b"glb", "model/gltf-binary"), "x.glb"));
    assert_eq!(blocking(&m), ["model.asset"], "only STEP takes a pair");

    let mut m = good.clone();
    m.bounds_mm[1][2] = 0.0;
    assert!(blocking(&m).contains(&"model.bounds".to_owned()));
    let mut m = good.clone();
    m.frame.unit_mm = 0.0;
    assert_eq!(blocking(&m), ["model.bounds"]);
    let mut m = good.clone();
    if let Some(area) = &mut m.marking_area {
        area.u_axis = [0.0, 0.0, 1.0];
    }
    assert_eq!(blocking(&m), ["model.bounds"], "u axis along the normal");

    let mut m = good.clone();
    m.bodies.clear();
    assert_eq!(blocking(&m), ["model.body_unmapped"]);
    let mut m = good.clone();
    m.bodies[1].key = "body".into();
    assert_eq!(blocking(&m), ["model.body_unmapped"], "a body mapped twice");
    let mut m = good.clone();
    m.bodies[0].material = ModelMaterial::Custom(0);
    assert_eq!(
        blocking(&m),
        ["model.body_unmapped"],
        "a missing custom material"
    );
    m.custom_materials.push(CustomMaterial {
        name: "Lens".into(),
        base_colour: [1.0, 1.0, 1.0, 0.2],
        metallic: 0.0,
        roughness: 0.05,
        clearcoat: None,
        transmission: Some(0.9),
        ior: Some(1.5),
        emissive: None,
        textures_ignored: true,
    });
    assert_eq!(blocking(&m), Vec::<String>::new());
    assert!(m.has_textures());
    m.custom_materials[0].roughness = 2.0;
    assert_eq!(blocking(&m), ["model.body_unmapped"]);

    let mut m = good.clone();
    m.leads[1].key = "lead-1".into();
    assert_eq!(blocking(&m), ["model.leads"], "a lead tagged twice");
    let mut m = good.clone();
    m.leads[0].designation = " ".into();
    assert_eq!(blocking(&m), ["model.leads"]);
    let mut m = good.clone();
    m.leads[0].contact = [9.0, 0.0, 0.0];
    assert_eq!(
        blocking(&m),
        ["model.leads"],
        "a contact outside the bounds"
    );

    let mut m = good.clone();
    m.provenance.record.licence = None;
    assert_eq!(blocking(&m), ["model.licence"]);
    let mut m = good;
    m.provenance.redistribution = Redistribution::Unknown;
    assert_eq!(
        codes(&m),
        [("model.licence".to_owned(), false)],
        "unknown redistribution is advisory"
    );
}

#[test]
fn bounds_and_finite_values_are_draft_refusals() {
    let asset = packed(&bytes("bounds"), "model/step");
    let mut m = model(&asset);
    assert!(m.within_bounds() && m.values_finite());
    m.leads[0].contact[0] = f64::NAN;
    assert!(!m.values_finite());
    let mut m = model(&asset);
    m.asset.file_name = "x".repeat(MAX_FILE_NAME + 1);
    assert!(!m.within_bounds());
    let mut m = model(&asset);
    m.provenance.url = Some("u".repeat(MAX_URL + 1));
    assert!(!m.within_bounds());
}

#[test]
fn import_model_packs_the_asset_and_drafts_in_one_undo_step() -> Fallible<()> {
    let mut engine = Engine::default();
    let asset = packed(&bytes("import"), "model/step");
    let before = engine.to_file();
    let revision = engine.revision();
    let Outcome::DraftCreated { id } = apply(
        &mut engine,
        import_request("Two-lead body", &asset, model(&asset)),
    )?
    else {
        return Err("a draft".into());
    };
    assert_eq!(engine.revision(), revision + 1);
    let draft = engine.drafts().get(id).ok_or("draft")?;
    assert_eq!(draft.meta.name, "Two-lead body");
    assert!(draft.meta.provenance.contains("two-lead.step"));
    assert!(matches!(&draft.definition, Definition::Model(_)));
    assert!(
        engine
            .project()
            .library
            .assets
            .iter()
            .any(|a| a.hash == asset.hash)
    );
    engine.undo()?;
    assert_eq!(
        engine.to_file(),
        before,
        "one undo step removes draft and asset"
    );
    Ok(())
}

#[test]
fn import_model_refusals_change_nothing() -> Fallible<()> {
    let mut engine = Engine::default();
    let asset = packed(&bytes("refuse"), "model/step");
    let before = engine.to_file();
    let revision = engine.revision();
    let other = packed(&bytes("other"), "model/step");
    let mut wrong_size = model(&asset);
    wrong_size.asset.bytes += 1;
    let mut corrupt = asset.clone();
    corrupt.data.make_mut().push(0);
    let attempts = [
        import_request(" ", &asset, model(&asset)),
        import_request("Unnamed asset", &other, model(&asset)),
        import_request("Wrong size", &asset, wrong_size),
        import_request("Corrupt", &corrupt, model(&asset)),
        // Named but neither staged nor packed.
        Operation::Library(LibraryCommand::ImportModel(Box::new(ImportModel {
            name: "Nothing staged".into(),
            assets: vec![],
            definition: Box::new(model(&asset)),
            assign: None,
        }))),
    ];
    for attempt in attempts {
        let shown = format!("{attempt:?}").chars().take(120).collect::<String>();
        assert!(engine.apply(revision, attempt).is_err(), "{shown}");
        assert_eq!(engine.revision(), revision);
        assert_eq!(engine.to_file(), before);
    }
    Ok(())
}

#[test]
fn a_second_import_reuses_the_packed_bytes() -> Fallible<()> {
    let mut engine = Engine::default();
    let asset = packed(&bytes("reuse"), "model/step");
    apply(&mut engine, import_request("First", &asset, model(&asset)))?;
    // Bytes already packed: the request may omit them.
    apply(
        &mut engine,
        Operation::Library(LibraryCommand::ImportModel(Box::new(ImportModel {
            name: "Second".into(),
            assets: vec![],
            definition: Box::new(model(&asset)),
            assign: None,
        }))),
    )?;
    apply(&mut engine, import_request("Third", &asset, model(&asset)))?;
    let held = engine
        .project()
        .library
        .assets
        .iter()
        .filter(|a| a.hash == asset.hash)
        .count();
    assert_eq!(held, 1);
    Ok(())
}

#[test]
fn an_incomplete_model_cannot_publish() -> Fallible<()> {
    let mut engine = Engine::default();
    let asset = packed(&bytes("incomplete"), "model/step");
    let mut m = model(&asset);
    m.bodies.clear();
    let Outcome::DraftCreated { id } = apply(&mut engine, import_request("Bare", &asset, m))?
    else {
        return Err("draft".into());
    };
    let revision = engine.revision();
    let result = engine.apply(
        revision,
        Operation::Library(LibraryCommand::Publish { drafts: vec![id] }),
    );
    let Err(Error::Conflict(message)) = result else {
        return Err(format!("refused as a conflict: {result:?}").into());
    };
    assert!(message.contains("body"), "{message}");
    assert_eq!(engine.revision(), revision);
    Ok(())
}

/// The demo with its R/C package drafts using a published library model.
struct Assigned {
    engine: Engine,
    fixture: Fixture,
    model: ModelRef,
    asset: PackedAsset,
}

fn assigned(lead_map: Vec<LeadMapEntry>) -> Fallible<Assigned> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    let asset = packed(&bytes("assigned"), "model/step");
    let envelope = match &engine
        .drafts()
        .get(fixture.package)
        .ok_or("package")?
        .definition
    {
        Definition::Package(p) => p.envelope,
        _ => return Err("a package".into()),
    };
    let model_ref = published_model(&mut engine, sized(&asset, envelope), &asset)?;
    with(&mut engine, fixture.package, package_body, |p| {
        p.model = PackageModel::Library(LibraryModel {
            model: model_ref,
            lead_map,
        });
    })?;
    Ok(Assigned {
        engine,
        fixture,
        model: model_ref,
        asset,
    })
}

fn terminal_ids(engine: &Engine, package: DefinitionId) -> Fallible<Vec<PackageTerminalId>> {
    match &engine
        .drafts()
        .get(package)
        .ok_or("package draft")?
        .definition
    {
        Definition::Package(p) => Ok(p.terminals.iter().map(|t| t.id).collect()),
        _ => Err("a package".into()),
    }
}

#[test]
fn a_package_publishes_with_its_library_model_and_reopens() -> Fallible<()> {
    let Assigned {
        mut engine,
        fixture,
        model: model_ref,
        asset,
    } = assigned(vec![])?;
    let terminals = terminal_ids(&engine, fixture.package)?;
    let map: Vec<LeadMapEntry> = terminals
        .iter()
        .enumerate()
        .map(|(i, t)| LeadMapEntry {
            lead: u16::try_from(i).unwrap_or(u16::MAX),
            terminal: *t,
        })
        .collect();
    with(&mut engine, fixture.package, package_body, |p| {
        p.model = PackageModel::Library(LibraryModel {
            model: model_ref,
            lead_map: map.clone(),
        });
    })?;
    let target = cascade(&mut engine, &fixture, |_| Ok(()))?;
    let library = &engine.project().library;
    let package = library
        .packages
        .iter()
        .filter(|p| p.id == fixture.package)
        .max_by_key(|p| p.revision)
        .ok_or("package")?;
    assert!(matches!(&package.definition.model, PackageModel::Library(m) if m.model == model_ref));
    // The closure of the component carries the model, before the package.
    let members = bundle::closure(library, target.erased())?;
    let at = |kind| members.iter().position(|m| m.kind == kind);
    assert!(at(DefinitionKind::Model) < at(DefinitionKind::Package));
    assert!(at(DefinitionKind::Model).is_some());
    // The file round trip keeps model and bytes.
    let bytes_out = crate::format::encode(&engine.to_file())?;
    let reopened = Engine::from_file(crate::format::decode(&bytes_out)?)?;
    assert_eq!(reopened.project().library.models, library.models);
    // A file whose asset went missing is refused, naming the asset.
    let mut file = engine.to_file();
    file.project.library.assets.retain(|a| a.hash != asset.hash);
    let Err(error) = Engine::from_file(file) else {
        return Err("a missing asset refuses the file".into());
    };
    assert!(error.to_string().contains("two-lead.step"), "{error}");
    Ok(())
}

#[test]
fn lead_maps_are_checked_against_the_package_and_the_model() -> Fallible<()> {
    let Assigned {
        engine,
        fixture,
        model: model_ref,
        ..
    } = assigned(vec![])?;
    let terminals = terminal_ids(&engine, fixture.package)?;
    let library = &engine.project().library;
    let Definition::Package(package) = &engine
        .drafts()
        .get(fixture.package)
        .ok_or("draft")?
        .definition
    else {
        return Err("package".into());
    };
    let first = *terminals.first().ok_or("terminal")?;
    let find = |lead_map: Vec<LeadMapEntry>, model: ModelRef| {
        let model = LibraryModel { model, lead_map };
        let mut out = Vec::new();
        package_findings(package, &model, fixture.package, &mut out);
        library_findings(library, package, &model, fixture.package, &mut out);
        out.into_iter()
            .map(|f| (f.code.clone(), f.blocks_stage(Stage::Publish)))
            .collect::<Vec<_>>()
    };
    let entry = |lead, terminal| LeadMapEntry { lead, terminal };
    assert_eq!(
        find(vec![entry(0, first)], model_ref),
        [("model.leads".to_owned(), false)],
        "an unmapped terminal is advisory"
    );
    assert!(
        find(vec![entry(7, first)], model_ref).contains(&("model.leads".to_owned(), true)),
        "a lead index past the model's tags"
    );
    let missing = ModelRef::new(model_ref.id, Revision(9));
    assert_eq!(
        find(vec![], missing),
        [("reference.missing".to_owned(), true)]
    );
    Ok(())
}

#[test]
fn a_dangling_or_oversized_library_model_is_refused_or_reported() -> Fallible<()> {
    let Assigned {
        mut engine,
        fixture,
        model: model_ref,
        ..
    } = assigned(vec![])?;
    let terminals = terminal_ids(&engine, fixture.package)?;
    // A lead index the model does not tag cannot be published: the
    // snapshot rule refuses the whole set.
    with(&mut engine, fixture.package, package_body, |p| {
        p.model = PackageModel::Library(LibraryModel {
            model: model_ref,
            lead_map: vec![LeadMapEntry {
                lead: 5,
                terminal: terminals[0],
            }],
        });
    })?;
    retarget(&mut engine, &fixture)?;
    let revision = engine.revision();
    let library = engine.project().library.clone();
    let result = engine.apply(
        revision,
        Operation::Library(LibraryCommand::Publish {
            drafts: fixture.all(),
        }),
    );
    let message = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(message.contains("names lead 5"), "{message}");
    assert_eq!(engine.project().library, library);
    // A model larger than the envelope is reported, not refused.
    let asset = packed(&bytes("large"), "model/step");
    let mut large = model(&asset);
    large.bounds_mm = [[-5.0, -1.5, 0.0], [5.0, 1.5, 1.0]];
    let big = published_model(&mut engine, large, &asset)?;
    let Definition::Package(package) = &engine
        .drafts()
        .get(fixture.package)
        .ok_or("draft")?
        .definition
    else {
        return Err("package".into());
    };
    let mut out = Vec::new();
    let use_big = LibraryModel {
        model: big,
        lead_map: vec![],
    };
    library_findings(
        &engine.project().library,
        package,
        &use_big,
        fixture.package,
        &mut out,
    );
    let bounds: Vec<_> = out.iter().filter(|f| f.code == "model.bounds").collect();
    assert_eq!(bounds.len(), 1, "{out:?}");
    assert!(!bounds[0].blocks_stage(Stage::Publish));
    assert!(
        bounds_excess(
            &engine.project().library.model(big).ok_or("big")?.definition,
            package.envelope
        ) > BOUNDS_TOLERANCE_MM
    );
    Ok(())
}

#[test]
fn bundles_carry_model_assets_and_promote_respects_redistribution() -> Fallible<()> {
    let Assigned {
        mut engine,
        fixture,
        asset,
        ..
    } = assigned(vec![])?;
    let target = cascade(&mut engine, &fixture, |_| Ok(()))?;
    let library = engine.project().library.clone();
    let fresh: Vec<SourceLineageId> = (1..=32).map(SourceLineageId).collect();
    let plan = import::export(&library, target.erased(), LibraryId(7), "Shared", &fresh)?;
    let record = plan
        .bundle
        .records
        .iter()
        .find(|r| r.kind == DefinitionKind::Model)
        .ok_or("the model travels with the package")?;
    assert_eq!(record.assets, vec![asset.hash]);
    assert!(plan.bundle.assets.iter().any(|a| a.hash == asset.hash));
    // Importing into an empty project brings model and bytes.
    let mut fresh_project = crate::library::Library::default();
    let mut next = 100u64;
    let mut allocate = || {
        next += 1;
        Ok(DefinitionId(next))
    };
    import::import(&mut fresh_project, &plan.bundle, &mut allocate)?;
    assert_eq!(fresh_project.models.len(), 1);
    assert!(fresh_project.assets.iter().any(|a| a.hash == asset.hash));
    crate::library::validate(&fresh_project)?;
    // A model that may not be redistributed refuses promote.
    for redistribution in [Redistribution::Forbidden, Redistribution::Unknown] {
        let mut blocked = library.clone();
        for m in &mut blocked.models {
            m.definition.provenance.redistribution = redistribution;
        }
        let refused = import::export(&blocked, target.erased(), LibraryId(7), "Shared", &fresh);
        let Err(Error::Invalid(message)) = refused else {
            return Err(format!("refused: {refused:?}").into());
        };
        assert!(message.starts_with("model.licence"), "{message}");
    }
    Ok(())
}

#[test]
fn named_assets_lists_model_and_pair() -> Fallible<()> {
    let asset = packed(&bytes("named"), "model/step");
    let pair = packed(b"glb pair", "model/gltf-binary");
    let mut m = model(&asset);
    m.pair = Some(asset_ref(&pair, "two-lead.glb"));
    let mut expected = vec![asset.hash, pair.hash];
    expected.sort();
    assert_eq!(
        bundle::named_assets(&Definition::Model(Box::new(m)))?,
        expected
    );
    Ok(())
}

#[test]
fn a_new_model_revision_lists_its_physical_changes_in_the_update_plan() -> Fallible<()> {
    let Assigned {
        mut engine,
        fixture,
        model: model_ref,
        ..
    } = assigned(vec![])?;
    let r1 = by_designator(&engine, "R1")?;
    let first = cascade(&mut engine, &fixture, |_| Ok(()))?;
    let plan = engine.preview_update(crate::updates::Change::ComponentRevision {
        instances: vec![r1],
        target: first,
        resolutions: Default::default(),
    });
    engine.apply_update(plan.revision, plan.change, false)?;
    // Model r2: longer, with a lead moved (its draft stays after publish).
    let mut moved = engine
        .project()
        .library
        .model(model_ref)
        .ok_or("r1")?
        .definition
        .clone();
    moved.bounds_mm[1][0] += 0.5;
    moved.leads[1].contact[0] += 0.3;
    edit(
        &mut engine,
        vec![DraftEdit {
            id: model_ref.id,
            meta: meta("Two-lead body"),
            definition: Definition::Model(Box::new(moved)),
        }],
    )?;
    let r2 = publish(&mut engine, vec![model_ref.id])?;
    let r2 = ModelRef::new(model_ref.id, r2.first().ok_or("r2")?.revision);
    let second = cascade(&mut engine, &fixture, |e| {
        with(e, fixture.package, package_body, |p| {
            if let PackageModel::Library(m) = &mut p.model {
                m.model = r2;
            }
        })
    })?;
    let plan = engine.preview_update(crate::updates::Change::ComponentRevision {
        instances: vec![r1],
        target: second,
        resolutions: Default::default(),
    });
    let has = |text: &str| plan.changes.iter().any(|c| c.contains(text));
    assert!(has("3D model bounds "), "{:?}", plan.changes);
    assert!(
        has("lead contacts move by up to 0.300 mm (lead ‘2’)"),
        "{:?}",
        plan.changes
    );
    Ok(())
}

#[test]
fn model_details_are_empty_for_equal_models() {
    let asset = packed(&bytes("details"), "model/step");
    let m = Published {
        id: DefinitionId(1),
        revision: Revision(1),
        meta: Default::default(),
        definition: model(&asset),
    };
    assert!(crate::updates::model_details(Some(&m), Some(&m)).is_empty());
    let mut n = m.clone();
    n.definition.provenance.redistribution = Redistribution::Forbidden;
    n.definition.marking_area = None;
    let lines = crate::updates::model_details(Some(&m), Some(&n));
    assert_eq!(
        lines,
        [
            "3D model marking area removed.",
            "3D model redistribution permitted → forbidden."
        ]
    );
}

/// The wizard's Finish: `library.import_model` with `assign` packs the
/// bytes, creates the Model draft and sets it, with its lead map, on the
/// package draft, all in one undo step; an assignment the package refuses
/// refuses the import.
#[test]
fn import_and_assign_is_one_undo_step() -> Fallible<()> {
    let mut engine = Engine::demo()?;
    let fixture = demo_fixture(&engine)?;
    let terminals = terminal_ids(&engine, fixture.package)?;
    let asset = packed(&bytes("import-assign"), "model/step");
    let lead_map = vec![LeadMapEntry {
        lead: 0,
        terminal: *terminals.first().ok_or("terminal")?,
    }];
    let before = engine.to_file();
    let revision = engine.revision();
    let request = |assign| {
        Operation::Library(LibraryCommand::ImportModel(Box::new(ImportModel {
            name: "Assigned body".into(),
            assets: vec![asset.clone()],
            definition: Box::new(model(&asset)),
            assign,
        })))
    };
    // A package draft that does not exist refuses the whole import.
    let missing = request(Some(AssignModel {
        package: DefinitionId(999_999),
        lead_map: lead_map.clone(),
        part: None,
    }));
    assert!(engine.apply(revision, missing).is_err());
    assert_eq!(engine.to_file(), before);
    let Outcome::DraftCreated { id } = apply(
        &mut engine,
        request(Some(AssignModel {
            package: fixture.package,
            lead_map: lead_map.clone(),
            part: None,
        })),
    )?
    else {
        return Err("a draft".into());
    };
    assert_eq!(engine.revision(), revision + 1, "one step");
    let Definition::Package(package) = &engine
        .drafts()
        .get(fixture.package)
        .ok_or("package draft")?
        .definition
    else {
        return Err("a package".into());
    };
    let PackageModel::Library(used) = &package.model else {
        return Err("the package uses the model".into());
    };
    assert_eq!(used.model.id, id);
    assert_eq!(used.lead_map, lead_map);
    engine.undo()?;
    assert_eq!(
        engine.to_file(),
        before,
        "one undo restores the state before the import"
    );
    Ok(())
}

/// Bytes already packed under another media type are refused, not
/// accepted now and refused later by the snapshot.
#[test]
fn a_hash_packed_with_another_media_type_is_refused() -> Fallible<()> {
    let mut engine = Engine::default();
    let asset = packed(&bytes("media"), "model/step");
    apply(&mut engine, import_request("First", &asset, model(&asset)))?;
    let mut gltf = asset.clone();
    gltf.media_type = "model/gltf-binary".into();
    let mut definition = model(&gltf);
    definition.format = ModelFormat::Gltf;
    let before = engine.to_file();
    let revision = engine.revision();
    let result = engine.apply(revision, import_request("Second", &gltf, definition));
    let Err(error) = result else {
        return Err("accepted a second media type".into());
    };
    assert!(
        error.to_string().contains("already packed as model/step"),
        "{error}"
    );
    assert_eq!(engine.to_file(), before);
    Ok(())
}

/// With a 64 MiB model packed, a library edit neither copies nor
/// re-hashes the bytes: the edited document, the undo snapshot and the
/// imported request share one allocation, whose hash was computed once.
/// Measured: the edit takes milliseconds (printed), against the hundreds a
/// re-hash of 64 MiB costs.
#[test]
fn library_edits_share_and_do_not_rehash_a_large_packed_asset() -> Fallible<()> {
    let mut engine = Engine::default();
    let big = packed(&vec![3u8; 64 * 1024 * 1024], "model/step");
    let start = std::time::Instant::now();
    apply(&mut engine, import_request("Large", &big, model(&big)))?;
    let first = start.elapsed();
    let held = |e: &Engine| {
        e.project()
            .library
            .assets
            .iter()
            .find(|a| a.hash == big.hash)
            .map(|a| a.data.clone())
    };
    let packed_bytes = held(&engine).ok_or("packed")?;
    assert!(packed_bytes.shares(&big.data), "packed without a copy");
    // A second library change (another model packed) revalidates the
    // library, the large asset included.
    let small = packed(&bytes("small"), "model/step");
    let start = std::time::Instant::now();
    apply(&mut engine, import_request("Small", &small, model(&small)))?;
    let edit = start.elapsed();
    let after = held(&engine).ok_or("still packed")?;
    assert!(after.shares(&packed_bytes), "the edit copied the bytes");
    eprintln!("64 MiB packed: import {first:?}, next library edit {edit:?}");
    assert!(edit.as_millis() < 250, "{edit:?}");
    engine.undo()?;
    assert!(held(&engine).ok_or("after undo")?.shares(&packed_bytes));
    Ok(())
}
