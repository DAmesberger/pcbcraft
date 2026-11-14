//! A generated part's 3D model (slice GP2, docs/generator-plugins.md): the
//! part file the module's generation names, found in the generator's
//! bundle or in the project, evaluated through Solidcraft at the
//! generation's values exactly as any part file is, and checked against
//! the generation before `library.generate` stores it: no blocking finding,
//! the bounds the generation's envelope within 10 µm, and every lead
//! contact under a body of a lead material.
use crate::Result;
use sparksmith_core::Engine;
use sparksmith_core::commands::GeneratedModel;
use sparksmith_core::ids::ContentHash;
use sparksmith_core::library::bundle::AssetBytes;
use sparksmith_core::library::model3d::{
    AssetRef, ModelDefinition, ModelFormat, ModelProvenance, Redistribution,
};
use sparksmith_core::library::package_model::PartValue;
use sparksmith_core::library::{Basis, BodyRole, Provenance};
use sparksmith_core::units::Nm;
use sparksmith_generators::abi::v1;
use sparksmith_io::generator_bundle::GeneratorBundle;
use sparksmith_solidcraft::models::part_file::{self, PartValues};

/// How far the part file's bounds may lie from the generation's envelope,
/// and a lead contact outside its lead body, mm.
const TOLERANCE_MM: f64 = 0.01;

fn refused(message: String) -> crate::Error {
    crate::Error::Document(sparksmith_core::Error::Invalid(format!(
        "generated.model: {message}. Nothing was changed."
    )))
}

/// The part file `hash`: from `bundle`, else as a Model the project packs.
/// Its bytes, the Model it becomes (a new one, or the project's), its name
/// and whether the project already packs the bytes.
fn find(
    engine: &Engine,
    hash: ContentHash,
    bundle: Option<&GeneratorBundle>,
) -> Result<(Vec<u8>, ModelDefinition, String, bool)> {
    let library = &engine.project().library;
    let packed = library.assets.iter().find(|a| a.hash == hash);
    let held = library.models.iter().find(|m| {
        m.definition.format == ModelFormat::Solidcraft && m.definition.asset.hash == hash
    });
    if let Some(part) = bundle.and_then(|b| b.part(hash)) {
        let definition = match held {
            Some(m) => m.definition.clone(),
            None => ModelDefinition {
                asset: AssetRef {
                    hash,
                    bytes: u64::try_from(part.bytes.len()).unwrap_or(u64::MAX),
                    media_type: ModelFormat::Solidcraft.media_type().into(),
                    file_name: part
                        .entry
                        .file
                        .rsplit('/')
                        .next()
                        .unwrap_or("part.solidcraft")
                        .to_owned(),
                },
                format: ModelFormat::Solidcraft,
                provenance: ModelProvenance {
                    redistribution: Redistribution::Permitted,
                    record: Provenance {
                        origin: None,
                        basis: Basis::Authored,
                        licence: Some(part.entry.licence.clone()),
                    },
                    ..ModelProvenance::default()
                },
                ..ModelDefinition::default()
            },
        };
        return Ok((
            part.bytes.clone(),
            definition,
            part.entry.title.clone(),
            packed.is_some(),
        ));
    }
    match (held, packed) {
        (Some(model), Some(asset)) => Ok((
            asset.data.as_slice().to_vec(),
            model.definition.clone(),
            model.definition.asset.file_name.clone(),
            true,
        )),
        _ => Err(refused(format!(
            "the generator's part file sha256 {hash} is neither in its bundle nor in this project"
        ))),
    }
}

/// `values` (the generation's integers) typed by the part file's own
/// units: nanometres for a length parameter, millidegrees for an angle,
/// the count for a unitless one.
fn part_values(as_is: &part_file::EvaluatedPart, values: &v1::Params) -> Result<PartValues> {
    let mut out = PartValues::new();
    for (name, value) in values {
        let v1::ParamValue::Int(int) = value else {
            return Err(refused(format!(
                "the generator gives the part file's \"{name}\" a value that is not a whole number"
            )));
        };
        let parameter = as_is
            .parameters
            .iter()
            .find(|p| &p.name == name)
            .ok_or_else(|| {
                refused(format!(
                    "the generator gives a value to \"{name}\", which its part file does not have"
                ))
            })?;
        let value = match parameter.unit.as_str() {
            "mm" => PartValue::Nm(
                Nm::new(*int)
                    .map_err(|e| refused(format!("\"{name}\": {e}")))?
                    .get(),
            ),
            "deg" => PartValue::Mdeg(*int),
            "" => PartValue::Count(
                u32::try_from(*int)
                    .map_err(|_| refused(format!("\"{name}\" is a count; {int} is not one")))?,
            ),
            other => {
                return Err(refused(format!(
                    "the part file's \"{name}\" is in {other}, which a generator cannot set"
                )));
            }
        };
        out.insert(name.clone(), value);
    }
    Ok(out)
}

/// The model of `generation` for `library.generate`: its part file
/// evaluated at its values and checked against the generation.
pub(super) fn generated_model(
    engine: &Engine,
    generation: &v1::Generation,
    bundle: Option<&GeneratorBundle>,
) -> Result<GeneratedModel> {
    let Some(v1::Model::PartFile { asset, values }) = &generation.model else {
        return Err(refused(
            "the generator made no 3D model; a part needs one".into(),
        ));
    };
    let hash = ContentHash::parse(asset)
        .ok_or_else(|| refused(format!("\"{asset}\" is not a SHA-256")))?;
    let (bytes, definition, name, packed) = find(engine, hash, bundle)?;
    let as_is = part_file::evaluate(&bytes, &PartValues::new())?;
    let values = part_values(&as_is, values)?;
    let part = part_file::evaluate(&bytes, &values)?;
    if let Some(refusal) = part.refusal() {
        return Err(refused(format!(
            "its part file at the generator's values: {refusal}"
        )));
    }
    // The bounds against the envelope, centred in x and y on the seating
    // plane.
    let geometry = &generation.package.geometry;
    let mm = |v: i64| Nm::new(v).map(Nm::to_mm_f64).unwrap_or(f64::NAN);
    let [l, w, h] = geometry.envelope_nm.map(mm);
    let [lo, hi] = part.bounds_mm;
    let envelope = [[-l / 2.0, -w / 2.0, 0.0], [l / 2.0, w / 2.0, h]];
    let off = lo
        .iter()
        .chain(&hi)
        .zip(envelope[0].iter().chain(&envelope[1]))
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f64::max);
    if off.is_nan() || off > TOLERANCE_MM {
        return Err(refused(format!(
            "its part file at the generator's values spans {lo:?} to {hi:?} mm, not the generation's {l} × {w} × {h} mm envelope"
        )));
    }
    for lead in &geometry.leads {
        let [x, y] = [mm(lead.contact[0]), mm(lead.contact[1])];
        let under = part.bodies.iter().any(|b| {
            part_file::role_of(b.material) == BodyRole::Termination
                && x >= b.bounds_mm[0][0] - TOLERANCE_MM
                && x <= b.bounds_mm[1][0] + TOLERANCE_MM
                && y >= b.bounds_mm[0][1] - TOLERANCE_MM
                && y <= b.bounds_mm[1][1] + TOLERANCE_MM
        });
        if !under {
            return Err(refused(format!(
                "lead {} touches the board at ({x}, {y}) mm, under no lead body of its part file",
                lead.key
            )));
        }
    }
    let mut definition = definition;
    if definition.bounds_nm == ModelDefinition::default().bounds_nm {
        definition.bounds_nm = sparksmith_core::library::model3d::bounds_nm(as_is.bounds_mm)?;
    }
    Ok(GeneratedModel {
        definition,
        name,
        data: (!packed).then(|| AssetBytes::new(bytes)),
        derived: part_file::derived(&part)?,
        values,
    })
}
