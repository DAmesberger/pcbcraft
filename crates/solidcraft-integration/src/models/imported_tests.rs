//! D2 behaviour: original fixtures generated here (a STEP written by
//! Solidcraft's exporter from two boxes, a GLB written byte by byte), read,
//! framed, built, previewed and rechecked.
use super::*;
use crate::models::inspect::{self, ModelInspection};
use crate::native::{BoxBody, Expr};
use pcbcraft_core::PackageTerminalId;
use pcbcraft_core::library::model3d::{AssetRef, LeadMapEntry, LeadTag, ModelBody};
use pcbcraft_core::materials::MaterialId;
use serde_json::{Value, json};

const EPS: f64 = 1e-5;

fn near(a: [f64; 3], b: [f64; 3], eps: f64) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= eps)
}

fn fail(what: &str) -> Error {
    Error::Verification(what.into())
}

/// An axis-aligned box as a closed triangle mesh, outward and
/// counter-clockwise: vertex `i` has x = bit 0, y = bit 1, z = bit 2.
fn box_mesh(lo: [f32; 3], hi: [f32; 3]) -> (Vec<f32>, Vec<u32>) {
    let mut positions = Vec::with_capacity(24);
    for i in 0..8u32 {
        for (axis, bit) in [1u32, 2, 4].into_iter().enumerate() {
            positions.push(if i & bit == 0 { lo[axis] } else { hi[axis] });
        }
    }
    let quads = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ];
    let indices = quads
        .iter()
        .flat_map(|[a, b, c, d]| [*a, *b, *c, *a, *c, *d])
        .collect();
    (positions, indices)
}

/// A GLB of two boxes in metres with Y up: a body (`Epoxy`, clear-coated)
/// standing 0.2 mm off the ground and a lead (`Tin`) touching it at x =
/// 2…2.5 mm.
pub(crate) fn fixture_glb() -> Vec<u8> {
    let meshes = [
        (
            "Body",
            box_mesh([-0.002, 0.0002, -0.0015], [0.002, 0.0012, 0.0015]),
        ),
        (
            "Lead",
            box_mesh([0.002, 0.0, -0.0003], [0.0025, 0.0003, 0.0003]),
        ),
    ];
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut gltf_meshes = Vec::new();
    for (k, (name, (positions, indices))) in meshes.iter().enumerate() {
        let start = bin.len();
        bin.extend(positions.iter().flat_map(|v| v.to_le_bytes()));
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": positions.len() * 4}));
        let (lo, hi) = (&positions[0..3], &positions[21..24]);
        accessors.push(
            json!({"bufferView": 2 * k, "componentType": 5126, "count": 8,
            "type": "VEC3", "min": lo, "max": hi}),
        );
        let start = bin.len();
        bin.extend(indices.iter().flat_map(|v| v.to_le_bytes()));
        views.push(json!({"buffer": 0, "byteOffset": start, "byteLength": indices.len() * 4}));
        accessors.push(json!({"bufferView": 2 * k + 1, "componentType": 5125,
            "count": indices.len(), "type": "SCALAR"}));
        gltf_meshes.push(json!({"name": name, "primitives": [{
            "attributes": {"POSITION": 2 * k}, "indices": 2 * k + 1, "material": k}]}));
    }
    let document = json!({
        "asset": {"version": "2.0", "generator": "pcbcraft test"},
        "extensionsUsed": ["KHR_materials_clearcoat"],
        "scene": 0,
        "scenes": [{"nodes": [0, 1]}],
        "nodes": [{"mesh": 0, "name": "Body"}, {"mesh": 1, "name": "Lead"}],
        "meshes": gltf_meshes,
        "materials": [
            {"name": "Epoxy", "pbrMetallicRoughness": {"baseColorFactor": [0.02, 0.02, 0.02, 1.0],
                "metallicFactor": 0.0, "roughnessFactor": 0.6},
             "extensions": {"KHR_materials_clearcoat": {"clearcoatFactor": 0.5}}},
            {"name": "Tin", "pbrMetallicRoughness": {"baseColorFactor": [0.8, 0.8, 0.8, 1.0],
                "metallicFactor": 1.0, "roughnessFactor": 0.3}}
        ],
        "buffers": [{"byteLength": bin.len()}],
        "bufferViews": views,
        "accessors": accessors,
    });
    solidcraft_engine::io::glb_container(document.to_string().as_bytes(), &bin)
}

/// A STEP file of the same two boxes in millimetres, Z up, written by
/// Solidcraft's exporter.
pub(crate) fn fixture_step() -> Result<Vec<u8>> {
    let mut native = Native::new("fixture")?;
    for (name, corner, size) in [
        ("Body", [-2.0, -1.5, 0.2], [4.0, 3.0, 1.0]),
        ("Lead", [2.0, -0.3, 0.0], [0.5, 0.6, 0.3]),
    ] {
        native.solid_box(BoxBody {
            name,
            corner: corner.map(Expr::from),
            size: size.map(Expr::from),
        })?;
    }
    let state = native.session().model.state();
    solidcraft_engine::io::export(
        &state,
        &["Body".to_owned(), "Lead".to_owned()],
        solidcraft_engine::io::Format::Step,
        "fixture",
    )
    .map_err(|e| Error::Invalid(e.to_string()))
}

const BOUNDS: [[f64; 3]; 2] = [[-2.0, -1.5, 0.0], [2.5, 1.5, 1.2]];

pub(crate) fn model(format: ModelFormat, bytes: &[u8], seen: &ModelInspection) -> ModelDefinition {
    let mut bodies = Vec::new();
    for body in &seen.bodies {
        let lead = body.key.contains("Lead");
        bodies.push(ModelBody {
            key: body.key.clone(),
            role: if lead { BodyRole::Lead } else { BodyRole::Body },
            material: if lead {
                ModelMaterial::Builtin(MaterialId::TinMatte)
            } else if format == ModelFormat::Gltf {
                ModelMaterial::Custom(0)
            } else {
                ModelMaterial::Builtin(MaterialId::EpoxyMould)
            },
        });
    }
    ModelDefinition {
        asset: AssetRef {
            hash: asset_hash(bytes),
            bytes: bytes.len() as u64,
            media_type: "model/gltf-binary".into(),
            file_name: "fixture.glb".into(),
        },
        format,
        frame: default_frame(format),
        bounds_mm: seen.bounds_mm,
        bodies,
        custom_materials: seen.materials.clone(),
        leads: Vec::new(),
        ..ModelDefinition::default()
    }
}

pub(crate) fn lead_key(seen: &ModelInspection) -> Result<String> {
    seen.bodies
        .iter()
        .find(|b| b.key.contains("Lead"))
        .map(|b| b.key.clone())
        .ok_or_else(|| fail("no lead body"))
}

pub(crate) fn use_lead(terminal: u32) -> LibraryModel {
    LibraryModel {
        lead_map: vec![LeadMapEntry {
            lead: 0,
            terminal: PackageTerminalId(terminal),
        }],
        ..LibraryModel::default()
    }
}

#[test]
fn glb_inspection_reads_bodies_materials_and_the_default_frame() -> Result<()> {
    let bytes = fixture_glb();
    let seen = inspect::inspect(&bytes, ModelFormat::Gltf)?;
    assert_eq!(seen.frame, default_frame(ModelFormat::Gltf));
    let keys: Vec<&str> = seen.bodies.iter().map(|b| b.key.as_str()).collect();
    assert_eq!(keys, ["Body", "Lead"]);
    assert!(
        near(seen.bounds_mm[0], BOUNDS[0], EPS),
        "{:?}",
        seen.bounds_mm
    );
    assert!(
        near(seen.bounds_mm[1], BOUNDS[1], EPS),
        "{:?}",
        seen.bounds_mm
    );
    // Y up in metres became Z up in millimetres.
    let lead = &seen.bodies[1];
    assert!(
        near(lead.bounds_mm[0], [2.0, -0.3, 0.0], EPS),
        "{:?}",
        lead.bounds_mm
    );
    assert!(
        near(lead.bounds_mm[1], [2.5, 0.3, 0.3], EPS),
        "{:?}",
        lead.bounds_mm
    );
    assert_eq!(seen.materials.len(), 2);
    assert_eq!(seen.materials[0].name, "Epoxy");
    assert_eq!(seen.materials[0].clearcoat, Some(0.5));
    assert_eq!(
        seen.bodies[0].material,
        Some(0),
        "a suggestion from the factors"
    );
    assert_eq!(seen.bodies[1].material, Some(1));
    assert!(seen.bodies[1].colour.is_some());
    assert!(!seen.has_textures);
    Ok(())
}

#[test]
fn step_inspection_reads_the_exported_bodies() -> Result<()> {
    let bytes = fixture_step()?;
    let seen = inspect::inspect(&bytes, ModelFormat::Step)?;
    assert_eq!(seen.bodies.len(), 2, "{:?}", seen.bodies);
    assert!(
        seen.bodies.iter().any(|b| b.key.contains("Body")),
        "{:?}",
        seen.bodies
    );
    assert!(
        seen.bodies.iter().any(|b| b.key.contains("Lead")),
        "{:?}",
        seen.bodies
    );
    assert!(
        near(seen.bounds_mm[0], BOUNDS[0], EPS),
        "{:?}",
        seen.bounds_mm
    );
    assert!(
        near(seen.bounds_mm[1], BOUNDS[1], EPS),
        "{:?}",
        seen.bounds_mm
    );
    assert!(
        seen.materials.is_empty(),
        "STEP colours are hints, not materials"
    );
    Ok(())
}

#[test]
fn an_explicit_frame_scales_turns_and_offsets() -> Result<()> {
    let bytes = fixture_step()?;
    let frame = SourceFrame {
        unit_mm: 2.0,
        up: UpAxis::Y,
        origin_offset: [1.0, 0.0, -0.5],
    };
    let seen = inspect::inspect_in(&bytes, ModelFormat::Step, &frame)?;
    let expected = frame_bounds(&frame, ModelFormat::Step, BOUNDS)?;
    // (x, y, z) → (2x + 1, −2z, 2y − 0.5).
    assert!(near(expected[0], [-3.0, -2.4, -3.5], EPS), "{expected:?}");
    assert!(near(expected[1], [6.0, 0.0, 2.5], EPS), "{expected:?}");
    assert!(
        near(seen.bounds_mm[0], expected[0], EPS),
        "{:?}",
        seen.bounds_mm
    );
    assert!(
        near(seen.bounds_mm[1], expected[1], EPS),
        "{:?}",
        seen.bounds_mm
    );
    let bad = SourceFrame {
        unit_mm: 0.0,
        ..frame
    };
    assert!(matches!(
        inspect::inspect_in(&bytes, ModelFormat::Step, &bad),
        Err(Error::Invalid(m)) if m.starts_with("model.frame")
    ));
    Ok(())
}

#[test]
fn lead_contacts_are_the_lowest_points_of_the_tag() -> Result<()> {
    for (format, bytes) in [
        (ModelFormat::Gltf, fixture_glb()),
        (ModelFormat::Step, fixture_step()?),
    ] {
        let seen = inspect::inspect(&bytes, format)?;
        let mut m = model(format, &bytes, &seen);
        m.leads = vec![LeadTag {
            key: lead_key(&seen)?,
            designation: "1".into(),
            contact: [0.0; 3],
        }];
        let contacts = lead_contacts(&m, &bytes)?;
        assert_eq!(contacts.len(), 1);
        assert!(
            near(contacts[0], [2.25, 0.0, 0.0], EPS),
            "{format:?}: {contacts:?}"
        );
        m.leads[0].key = "nothing".into();
        assert!(matches!(
            lead_contacts(&m, &bytes),
            Err(Error::Invalid(e)) if e.starts_with("model.leads")
        ));
    }
    Ok(())
}

/// The import wizard inspects in the reader's frame and moves the bounds
/// with `frame_bounds`; that gives the bounds an inspection in the frame
/// reports, and the contacts it then measures pass a build's recheck.
#[test]
fn the_wizard_path_matches_what_a_build_rechecks() -> Result<()> {
    for (format, bytes) in [
        (ModelFormat::Gltf, fixture_glb()),
        (ModelFormat::Step, fixture_step()?),
    ] {
        let raw = inspect::inspect_in(&bytes, format, &reader_frame(format))?;
        let framed = inspect::inspect(&bytes, format)?;
        let moved = frame_bounds(&default_frame(format), format, raw.bounds_mm)?;
        assert!(
            near(moved[0], framed.bounds_mm[0], EPS) && near(moved[1], framed.bounds_mm[1], EPS),
            "{format:?}: {moved:?} vs {:?}",
            framed.bounds_mm
        );
        let mut m = model(format, &bytes, &framed);
        m.bounds_mm = moved;
        m.leads = vec![LeadTag {
            key: lead_key(&raw)?,
            designation: "1".into(),
            contact: [0.0; 3],
        }];
        m.leads[0].contact = lead_contacts(&m, &bytes)?[0];
        let mut native = Native::new("Board")?;
        build_library_model(&mut native, "pkg5r1", &m, &use_lead(1), &bytes)?;
    }
    Ok(())
}

#[test]
fn a_face_tag_touches_down_on_that_face() -> Result<()> {
    let bytes = fixture_step()?;
    let seen = inspect::inspect(&bytes, ModelFormat::Step)?;
    let mut m = model(ModelFormat::Step, &bytes, &seen);
    let lead = lead_key(&seen)?;
    // Some face of the lead box is its top (z = 0.3) and one its bottom.
    let mut lows = Vec::new();
    for face in 0..6 {
        m.leads = vec![LeadTag {
            key: format!("{lead}{FACE_KEY}{face}"),
            designation: "1".into(),
            contact: [0.0; 3],
        }];
        lows.push(lead_contacts(&m, &bytes)?[0][2]);
    }
    assert!(lows.iter().any(|z| (z - 0.3).abs() < EPS), "{lows:?}");
    assert!(lows.iter().any(|z| z.abs() < EPS), "{lows:?}");
    Ok(())
}

#[test]
fn build_maps_bodies_terminals_and_looks() -> Result<()> {
    let bytes = fixture_glb();
    let seen = inspect::inspect(&bytes, ModelFormat::Gltf)?;
    let mut m = model(ModelFormat::Gltf, &bytes, &seen);
    m.leads = vec![LeadTag {
        key: lead_key(&seen)?,
        designation: "1".into(),
        contact: [2.25, 0.0, 0.0],
    }];
    let mut native = Native::new("Board")?;
    let built = build_library_model(&mut native, "pkg7r1", &m, &use_lead(3), &bytes)?;
    assert_eq!(built.component, "pkg7r1");
    let summary: Vec<(&str, BodyRole, Option<u32>)> = built
        .bodies
        .iter()
        .map(|b| (b.key.as_str(), b.role, b.terminal))
        .collect();
    assert_eq!(
        summary,
        [
            ("Body", BodyRole::Body, None),
            ("Lead", BodyRole::Lead, Some(3))
        ]
    );
    assert_eq!(built.bodies[1].kind(), BodyKind::ModelLead);
    // Built-in bodies are recorded as generated ones; the custom body is not.
    let generated = built.generated();
    assert_eq!(generated.bodies.len(), 1);
    assert_eq!(generated.bodies[0].material, MaterialId::TinMatte);
    assert_eq!(generated.bodies[0].terminal, Some(3));
    // The definition is in the model frame and the saved looks are the
    // mapped materials.
    let state = native.session().model.state();
    let doc = &native.session().doc;
    for b in &built.bodies {
        let body = state.body(&b.name).ok_or_else(|| fail("built body"))?;
        let saved = doc
            .model_body_look(body)
            .ok_or_else(|| fail("saved look"))?;
        match b.material {
            ModelMaterial::Custom(_) => assert_eq!(saved.name, "Pcbcraft Epoxy"),
            ModelMaterial::Builtin(_) => {
                assert!(saved.name.starts_with("Pcbcraft "), "{}", saved.name)
            }
        }
    }
    let (surface, _) = surface(&m, ModelMaterial::Custom(0))?;
    assert!(
        surface.clear_coat().is_some(),
        "the glTF clear coat is drawn"
    );
    // Building it again in the same design needs a new name.
    assert!(matches!(
        build_library_model(&mut native, "pkg7r1", &m, &use_lead(3), &bytes),
        Err(Error::Invalid(e)) if e.starts_with("model.name")
    ));
    let again = build_library_model(&mut native, "pkg8r1", &m, &use_lead(3), &bytes)?;
    assert!(again.bodies.iter().all(|b| b.name.starts_with("pkg8r1 ")));
    Ok(())
}

#[test]
fn a_step_model_builds_with_builtin_materials() -> Result<()> {
    let bytes = fixture_step()?;
    let seen = inspect::inspect(&bytes, ModelFormat::Step)?;
    let m = model(ModelFormat::Step, &bytes, &seen);
    let mut native = Native::new("Board")?;
    let built = build_library_model(&mut native, "pkg9r2", &m, &LibraryModel::default(), &bytes)?;
    assert_eq!(built.generated().bodies.len(), 2);
    let names: Vec<String> = built.bodies.iter().map(|b| b.name.clone()).collect();
    let b = bounds_of(&native, &names)?;
    assert!(
        near(b[0], BOUNDS[0], EPS) && near(b[1], BOUNDS[1], EPS),
        "{b:?}"
    );
    assert!(native.evaluation_error().is_none());
    Ok(())
}

#[test]
fn rechecks_refuse_naming_the_asset() -> Result<()> {
    let bytes = fixture_glb();
    let seen = inspect::inspect(&bytes, ModelFormat::Gltf)?;
    let base = model(ModelFormat::Gltf, &bytes, &seen);
    let refused = |m: &ModelDefinition, bytes: &[u8]| -> String {
        let mut native = match Native::new("Board") {
            Ok(n) => n,
            Err(e) => return e.to_string(),
        };
        match build_library_model(&mut native, "pkg1r1", m, &LibraryModel::default(), bytes) {
            Ok(_) => "built".into(),
            Err(Error::Invalid(e)) => e,
            Err(e) => e.to_string(),
        }
    };
    // Bytes that are not the packed asset.
    let mut other = bytes.clone();
    other.push(0);
    let e = refused(&base, &other);
    assert!(
        e.starts_with("model.asset") && e.contains("fixture.glb"),
        "{e}"
    );
    // Bounds that moved since import.
    let mut moved = base.clone();
    moved.bounds_mm[1][2] += 0.5;
    let e = refused(&moved, &bytes);
    assert!(
        e.starts_with("model.asset") && e.contains("fixture.glb"),
        "{e}"
    );
    // A file body left unmapped, never defaulted.
    let mut unmapped = base.clone();
    unmapped.bodies.pop();
    let e = refused(&unmapped, &bytes);
    assert!(
        e.starts_with("model.body_unmapped") && e.contains("Lead"),
        "{e}"
    );
    // A mapping naming no file body.
    let mut stray = base.clone();
    stray.bodies[0].key = "Elsewhere".into();
    assert!(refused(&stray, &bytes).starts_with("model.asset"));
    // A custom material that does not exist.
    let mut missing = base.clone();
    missing.custom_materials.clear();
    assert!(refused(&missing, &bytes).starts_with("model.body_unmapped"));
    // A lead contact that moved.
    let mut lead = base.clone();
    lead.leads = vec![LeadTag {
        key: "Lead".into(),
        designation: "1".into(),
        contact: [2.25, 0.0, 0.2],
    }];
    let e = refused(&lead, &bytes);
    assert!(
        e.starts_with("model.leads") && e.contains("fixture.glb"),
        "{e}"
    );
    // A lead map naming a lead that is not tagged.
    let mut native = Native::new("Board")?;
    assert!(matches!(
        build_library_model(&mut native, "pkg1r1", &base, &use_lead(1), &bytes),
        Err(Error::Invalid(e)) if e.starts_with("model.leads")
    ));
    // A bad definition name.
    assert!(matches!(
        build_library_model(&mut native, "body", &base, &LibraryModel::default(), &bytes),
        Err(Error::Invalid(e)) if e.starts_with("model.name")
    ));
    assert!(
        native.find_component("pkg1r1").is_none(),
        "refusals add nothing"
    );
    Ok(())
}

#[test]
fn preview_draws_every_body_at_the_placement() -> Result<()> {
    let bytes = fixture_glb();
    let seen = inspect::inspect(&bytes, ModelFormat::Gltf)?;
    let m = model(ModelFormat::Gltf, &bytes, &seen);
    let placement = ModelPlacement {
        translation: [1.0, 2.0, 0.5],
        rotation_deg: [0.0, 0.0, 0.0],
    };
    let geometry = preview(&m, &LibraryModel::default(), &bytes, &placement)?;
    assert_eq!(geometry.len(), 2);
    let lo = geometry
        .iter()
        .map(|g| g.min)
        .fold([f64::INFINITY; 3], |a, b| [0, 1, 2].map(|i| a[i].min(b[i])));
    assert!(near(lo, [-1.0, 0.5, 0.5], EPS), "{lo:?}");
    assert!(geometry.iter().any(|g| g.kind == BodyKind::ModelLead));
    assert!(geometry.iter().all(|g| g.volume() > 0.0));
    Ok(())
}

#[test]
fn solidcraft_designs_are_inspected_but_not_built() -> Result<()> {
    let mut native = Native::new("parametric")?;
    native.parameter("w", "3 mm")?;
    native.solid_box(BoxBody {
        name: "Housing",
        corner: [0.0.into(), 0.0.into(), 0.0.into()],
        size: ["w".into(), 2.0.into(), 1.0.into()],
    })?;
    let bytes = solidcraft_engine::io::write_design(&native.session().doc);
    let seen = inspect::inspect(&bytes, ModelFormat::Solidcraft)?;
    assert_eq!(seen.bodies.len(), 1);
    assert_eq!(seen.bodies[0].key, "Housing");
    assert!(
        near(seen.bounds_mm[1], [3.0, 2.0, 1.0], EPS),
        "{:?}",
        seen.bounds_mm
    );
    let m = ModelDefinition {
        asset: AssetRef {
            hash: asset_hash(&bytes),
            bytes: bytes.len() as u64,
            ..AssetRef::default()
        },
        format: ModelFormat::Solidcraft,
        bounds_mm: seen.bounds_mm,
        ..ModelDefinition::default()
    };
    let mut board = Native::new("Board")?;
    assert!(matches!(
        build_library_model(&mut board, "pkg2r1", &m, &LibraryModel::default(), &bytes),
        Err(Error::Invalid(e)) if e.starts_with("model.unsupported")
    ));
    Ok(())
}

#[test]
fn malformed_files_refuse() {
    for (format, bytes) in [
        (ModelFormat::Gltf, b"glTF\x02\x00\x00\x00".to_vec()),
        (
            ModelFormat::Gltf,
            b"{\"asset\": {\"version\": \"2.0\"}}".to_vec(),
        ),
        (ModelFormat::Step, b"ISO-10303-21;\nnot really".to_vec()),
        (ModelFormat::Solidcraft, b"{}".to_vec()),
        (ModelFormat::Step, Vec::new()),
    ] {
        assert!(
            matches!(inspect::inspect(&bytes, format), Err(Error::Invalid(_))),
            "{format:?}"
        );
    }
    let huge = vec![0u8; inspect::MAX_INSPECT_BYTES + 1];
    assert!(matches!(
        inspect::inspect(&huge, ModelFormat::Step),
        Err(Error::Invalid(e)) if e.contains("at most")
    ));
}

#[test]
fn keys_are_unique_in_file_order() {
    let keys = unique_keys(
        [
            "Lead",
            "Lead",
            "",
            "Lead#2",
            "Lead",
            "x".repeat(400).as_str(),
        ]
        .map(str::to_owned)
        .to_vec(),
    );
    assert_eq!(keys[..5], ["Lead", "Lead#2", "body", "Lead#2#2", "Lead#3"]);
    assert!(keys[5].len() <= MAX_KEY);
    let set: BTreeSet<&String> = keys.iter().collect();
    assert_eq!(set.len(), keys.len());
}

#[test]
fn inspection_serializes_for_the_wizard() -> Result<()> {
    let seen = inspect::inspect(&fixture_glb(), ModelFormat::Gltf)?;
    let value: Value = serde_json::to_value(&seen)?;
    assert_eq!(value["format"], "gltf");
    assert_eq!(value["bodies"][0]["key"], "Body");
    assert_eq!(value["frame"]["up"], "y");
    let back: ModelInspection = serde_json::from_value(value)?;
    assert_eq!(back, seen);
    Ok(())
}
