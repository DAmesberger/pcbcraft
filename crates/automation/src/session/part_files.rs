//! Lean parts (docs/part-templates.md §11 L4): `library.check_part_file`
//! evaluates a Solidcraft part file at literal values and reports the
//! convention check; `library.use_part_file` makes a package draft use one
//! (the file packed as a Model of format `solidcraft` with
//! `library.import_model`'s `assign.part`, or `library.edit_package`
//! `SetModel` when the same file is already a model); and
//! `library.set_part_values` sets new values. Each document change is one
//! core command, one undo step, refused whole on a blocking finding.
//!
//! The operation builders ([`use_part_file_operation`],
//! [`set_part_values_operation`]) only read the engine; the Session and the
//! desktop apply what they return through their own engine path, so UI,
//! CLI and MCP issue the same typed command (docs/mechanical-parts.md M4).
use super::Session;
use crate::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sparksmith_core::commands::LibraryCommand;
use sparksmith_core::ids::{DefinitionId, ModelRef};
use sparksmith_core::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use sparksmith_core::library::model3d::{
    AssetRef, AssignModel, AssignPart, ImportModel, ModelDefinition, ModelFormat, ModelProvenance,
    Redistribution,
};
use sparksmith_core::library::package_model::{PackageEdit, PartFileModel, PartValue};
use sparksmith_core::library::{Basis, Provenance};
use sparksmith_core::library::{Definition, PackageDefinition, PackageModel};
use sparksmith_core::{Engine, Operation};
use sparksmith_solidcraft::models::part_file::{self, PartReport, PartValues};
use std::path::Path;

pub use super::shipped_parts::{BuiltinPart, builtin_parts};

/// Longest accepted path, bytes.
const MAX_PATH: usize = 4095;
/// The media type of a part file asset.
const MEDIA_TYPE: &str = "application/x-solidcraft";

/// A part-file value as the command wire takes it: the stored integer forms
/// (`{"nm": 2465000}`, `{"mdeg": 45000}`, `{"count": 25}`,
/// `{"milli": 500}`) or a number in the parameter's own unit
/// (`{"mm": 2.465}`, `{"deg": 45}`, `{"number": 0.5}`), which is
/// rounded once to the integer the project stores.
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WireValue {
    Nm(i64),
    Mdeg(i64),
    Count(u32),
    Milli(i64),
    Mm(f64),
    Deg(f64),
    Number(f64),
}

impl WireValue {
    fn value(self) -> Option<PartValue> {
        match self {
            Self::Nm(v) => Some(PartValue::Nm(v)),
            Self::Mdeg(v) => Some(PartValue::Mdeg(v)),
            Self::Count(n) => Some(PartValue::Count(n)),
            Self::Milli(v) => Some(PartValue::Milli(v)),
            Self::Mm(v) => PartValue::from_mm(v),
            Self::Deg(v) => PartValue::from_deg(v),
            Self::Number(v) => PartValue::from_number(v),
        }
    }
}

/// Reads a command's `values` ([`WireValue`] each) as stored values.
pub(crate) fn wire_values<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<PartValues, D::Error> {
    let wire: std::collections::BTreeMap<String, WireValue> =
        Deserialize::deserialize(deserializer)?;
    wire.into_iter()
        .map(|(name, value)| {
            let stored = value.value().ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "the value of {name} must be finite and within ±10000"
                ))
            })?;
            Ok((name, stored))
        })
        .collect()
}

/// The schema of a command's `values`.
pub(crate) type WireValues = std::collections::BTreeMap<String, WireValue>;

/// `library.check_part_file`: a file (`path`) or a built-in part file
/// (`builtin`), and values for some of its parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckPartFileParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    /// Literal values by the part's own parameter names:
    /// `{"can_height": {"mm": 2.465}, "rows": {"count": 25}}`.
    #[serde(default, deserialize_with = "wire_values")]
    #[schemars(with = "WireValues")]
    pub values: PartValues,
}

/// `library.use_part_file`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UsePartFileParams {
    /// The package draft that uses the part file.
    pub package: DefinitionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<String>,
    #[serde(default, deserialize_with = "wire_values")]
    #[schemars(with = "WireValues")]
    pub values: PartValues,
    /// The new model's name (the file or built-in name when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A file's licence (SPDX), required for `path`; built-ins are
    /// MIT OR Apache-2.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    /// Whether a file may be redistributed (unknown when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redistribution: Option<Redistribution>,
    pub expected_revision: u64,
}

/// `library.set_part_values`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetPartValuesParams {
    pub package: DefinitionId,
    #[serde(deserialize_with = "wire_values")]
    #[schemars(with = "WireValues")]
    pub values: PartValues,
    pub expected_revision: u64,
}

fn invalid(message: String) -> crate::Error {
    crate::Error::Document(sparksmith_core::Error::Invalid(message))
}

/// A part file's bytes and the name it goes by: read from a path, a
/// built-in or a project Model ([`read_part_file`], [`model_part_file`]).
#[derive(Clone, Debug)]
pub struct PartFileBytes {
    pub bytes: Vec<u8>,
    /// The file name (`NAME.solidcraft`).
    pub file_name: String,
    /// The shipped data of a built-in part file; `None` for any other file.
    pub builtin: Option<&'static BuiltinPart>,
}

/// Read exactly one of a file `path` (bounded, `.solidcraft`) and a
/// `builtin` part file name.
pub fn read_part_file(path: Option<&str>, builtin: Option<&str>) -> Result<PartFileBytes> {
    match (path, builtin) {
        (Some(path), None) => {
            if path.len() > MAX_PATH {
                return Err(invalid("part_file.read: the path is too long".into()));
            }
            let p = Path::new(path);
            if p.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                != Some("solidcraft".into())
            {
                return Err(invalid(
                    "part_file.read: a part file ends in .solidcraft".into(),
                ));
            }
            let file_name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("part.solidcraft")
                .to_owned();
            Ok(PartFileBytes {
                bytes: sparksmith_io::persistence::read_bytes(p)?,
                file_name,
                builtin: None,
            })
        }
        (None, Some(name)) => {
            let builtin = builtin_parts()?.require(name)?;
            Ok(PartFileBytes {
                bytes: builtin.bytes.to_vec(),
                file_name: builtin.file_name(),
                builtin: Some(builtin),
            })
        }
        _ => Err(invalid(
            "part_file.read: give exactly one of path and builtin".into(),
        )),
    }
}

/// Evaluate and refuse on a blocking finding.
fn checked(bytes: &[u8], values: &PartValues) -> Result<std::sync::Arc<part_file::EvaluatedPart>> {
    let part = part_file::evaluate(bytes, values)?;
    if let Some(refusal) = part.refusal() {
        return Err(refusal.into());
    }
    Ok(part)
}

/// The package draft `id`.
fn package_draft(engine: &Engine, id: DefinitionId) -> Result<PackageDefinition> {
    engine
        .drafts()
        .get(id)
        .and_then(|d| match &d.definition {
            Definition::Package(p) => Some(p.clone()),
            _ => None,
        })
        .ok_or_else(|| invalid(format!("part_file.package: {id} is not a package draft")))
}

/// The lead contacts of a package with its footprint: each pad's centre
/// as the designation of the package terminal it lands and its place in
/// the model frame (`model_frame` inverted: footprint datum, then the
/// package's placement). The newest footprint of the package's lineage,
/// draft or published; none when it has no footprint yet.
pub fn footprint_contacts(
    engine: &Engine,
    package_id: DefinitionId,
    package: &PackageDefinition,
) -> Vec<(String, [f64; 2])> {
    let drafted = engine
        .drafts()
        .items
        .iter()
        .find_map(|d| match &d.definition {
            Definition::Footprint(f) if f.package.id == package_id => Some(f.clone()),
            _ => None,
        });
    let footprint = drafted.or_else(|| {
        engine
            .project()
            .library
            .footprints
            .iter()
            .filter(|f| f.definition.package.id == package_id)
            .max_by_key(|f| (f.definition.package.revision, f.revision))
            .map(|f| f.definition.clone())
    });
    let Some(footprint) = footprint else {
        return Vec::new();
    };
    let frame =
        sparksmith_core::library::package_model::model_frame(&footprint.datum, &package.placement);
    footprint
        .pads
        .iter()
        .filter_map(|pad| {
            let terminal = package
                .terminals
                .iter()
                .find(|t| Some(t.id) == pad.terminal)?;
            let [x, y] = pad.position.to_mm_f64();
            // The inverse of a rigid frame: R^T (p - t).
            let d = [
                x - frame.translation[0],
                y - frame.translation[1],
                -frame.translation[2],
            ];
            let r = frame.rotation;
            let local = |i: usize| r[0][i] * d[0] + r[1][i] * d[1] + r[2][i] * d[2];
            Some((terminal.designation.clone(), [local(0), local(1)]))
        })
        .collect()
}

/// What `part` evaluated to, with its lead bodies bound to the terminals of
/// `package`'s footprint pads (slice B2): each lead body over a pad takes
/// that pad's terminal; a body over pads of two terminals stays unbound.
fn with_leads(
    engine: &Engine,
    package_id: DefinitionId,
    package: &PackageDefinition,
    part: &part_file::EvaluatedPart,
) -> Result<sparksmith_core::library::package_model::PartDerived> {
    let derived = part_file::derived(part)?;
    let has_leads = part.bodies.iter().any(|b| {
        part_file::role_of(b.material) == sparksmith_core::library::model3d::BodyRole::Termination
    });
    if !has_leads {
        return Ok(derived);
    }
    let contacts = footprint_contacts(engine, package_id, package);
    let leads = part_file::lead_bodies(part, &contacts, false)
        .map_err(|e| invalid(format!("part_file.leads: {e}")))?;
    Ok(sparksmith_core::library::package_model::PartDerived { leads, ..derived })
}

/// A Model revision's definition, published or the pending revision of a
/// draft.
fn model_definition(engine: &Engine, model: ModelRef) -> Result<ModelDefinition> {
    match engine.project().library.model(model) {
        Some(published) => Ok(published.definition.clone()),
        None => engine
            .drafts()
            .get(model.id)
            .filter(|d| d.pending() == Some(model.revision))
            .and_then(|d| match &d.definition {
                Definition::Model(m) => Some(m.as_ref().clone()),
                _ => None,
            })
            .ok_or_else(|| invalid(format!("part_file.model: the model {model} is missing"))),
    }
}

/// The part file of a project Model revision (published, or the pending
/// revision of a draft) of format `solidcraft`, from its packed asset.
pub fn model_part_file(engine: &Engine, model: ModelRef) -> Result<PartFileBytes> {
    let definition = model_definition(engine, model)?;
    if definition.format != ModelFormat::Solidcraft {
        return Err(invalid(format!(
            "part_file.model: the model {model} is not a SolidCraft part file"
        )));
    }
    let bytes = engine
        .project()
        .library
        .assets
        .iter()
        .find(|a| a.hash == definition.asset.hash)
        .map(|a| a.data.as_slice().to_vec())
        .ok_or_else(|| {
            invalid(format!(
                "part_file.model: the file of {model} is not packed"
            ))
        })?;
    Ok(PartFileBytes {
        bytes,
        file_name: definition.asset.file_name.clone(),
        builtin: None,
    })
}

/// A Model (published or drafted) whose asset is `hash` and format
/// `solidcraft`: its newest revision.
fn model_with(engine: &Engine, hash: sparksmith_core::ids::ContentHash) -> Option<ModelRef> {
    let library = &engine.project().library;
    let published = library
        .models
        .iter()
        .filter(|m| {
            m.definition.asset.hash == hash && m.definition.format == ModelFormat::Solidcraft
        })
        .max_by_key(|m| (m.id, m.revision))
        .map(|m| ModelRef::new(m.id, m.revision));
    published.or_else(|| {
        engine
            .drafts()
            .items
            .iter()
            .find_map(|d| match &d.definition {
                Definition::Model(m)
                    if m.asset.hash == hash && m.format == ModelFormat::Solidcraft =>
                {
                    d.pending().map(|r| ModelRef::new(d.id, r))
                }
                _ => None,
            })
    })
}

/// What `library.use_part_file` asks, with the file already read.
#[derive(Clone, Debug)]
pub struct UsePartFile {
    /// The package draft that uses the part file.
    pub package: DefinitionId,
    pub file: PartFileBytes,
    pub values: PartValues,
    /// The new model's name (the file name without `.solidcraft` when
    /// absent).
    pub name: Option<String>,
    /// A file's licence (SPDX): required unless the file is a built-in or
    /// already a Model in the project.
    pub licence: Option<String>,
    pub redistribution: Option<Redistribution>,
}

/// The one core operation `library.use_part_file` applies, and the check
/// report: `library.import_model` with `assign.part` for a new file, or
/// `library.edit_package` `SetModel` when the project already holds the
/// file as a Model. Refused on a blocking finding. The engine is only read;
/// UI, CLI and MCP apply the operation through their own engine path.
pub fn use_part_file_operation(
    engine: &Engine,
    request: &UsePartFile,
) -> Result<(Operation, PartReport)> {
    let file = &request.file;
    let package = package_draft(engine, request.package)?;
    let part = checked(&file.bytes, &request.values)?;
    let derived = with_leads(engine, request.package, &package, &part)?;
    let hash = asset_hash(&file.bytes);
    let operation = match model_with(engine, hash) {
        Some(model) => Operation::Library(LibraryCommand::EditPackage {
            draft: request.package,
            edits: vec![PackageEdit::SetModel(PackageModel::PartFile(Box::new(
                PartFileModel {
                    model,
                    values: request.values.clone(),
                    derived,
                },
            )))],
        }),
        None => {
            let licence = if let Some(builtin) = file.builtin {
                Some(builtin.licence.clone())
            } else {
                Some(
                    request
                        .licence
                        .clone()
                        .filter(|l| !l.trim().is_empty())
                        .ok_or_else(|| {
                            invalid(
                                "part_file.licence: a part file needs its licence stated".into(),
                            )
                        })?,
                )
            };
            // The model's own bounds: the file as it is.
            let as_is = checked(&file.bytes, &PartValues::new())?;
            let definition = ModelDefinition {
                asset: AssetRef {
                    hash,
                    bytes: u64::try_from(file.bytes.len()).unwrap_or(u64::MAX),
                    media_type: MEDIA_TYPE.into(),
                    file_name: file.file_name.clone(),
                },
                format: ModelFormat::Solidcraft,
                bounds_nm: sparksmith_core::library::model3d::bounds_nm(as_is.bounds_mm)?,
                provenance: ModelProvenance {
                    redistribution: if file.builtin.is_some() {
                        Redistribution::Permitted
                    } else {
                        request.redistribution.unwrap_or_default()
                    },
                    record: Provenance {
                        origin: None,
                        basis: if file.builtin.is_some() {
                            Basis::Authored
                        } else {
                            Basis::Imported
                        },
                        licence,
                    },
                    ..ModelProvenance::default()
                },
                ..ModelDefinition::default()
            };
            let name = request
                .name
                .clone()
                .unwrap_or_else(|| file.file_name.trim_end_matches(".solidcraft").to_owned());
            Operation::Library(LibraryCommand::ImportModel(Box::new(ImportModel {
                name,
                assets: vec![PackedAsset {
                    hash,
                    media_type: MEDIA_TYPE.into(),
                    data: AssetBytes::new(file.bytes.clone()),
                }],
                definition: Box::new(definition),
                assign: Some(AssignModel {
                    package: request.package,
                    lead_map: Vec::new(),
                    part: Some(AssignPart {
                        values: request.values.clone(),
                        derived,
                    }),
                }),
            })))
        }
    };
    Ok((operation, part.report()))
}

/// The one core operation `library.set_part_values` applies, and the check
/// report: `library.edit_package` `SetModel` with the package's part file
/// at `values`. Refused for a package that uses no part file, or on a
/// blocking finding.
pub fn set_part_values_operation(
    engine: &Engine,
    package: DefinitionId,
    values: &PartValues,
) -> Result<(Operation, PartReport)> {
    let definition = package_draft(engine, package)?;
    let PackageModel::PartFile(current) = &definition.model else {
        return Err(invalid(format!(
            "part_file.package: package {package} does not use a part file"
        )));
    };
    let file = model_part_file(engine, current.model)?;
    let part = checked(&file.bytes, values)?;
    let derived = with_leads(engine, package, &definition, &part)?;
    let operation = Operation::Library(LibraryCommand::EditPackage {
        draft: package,
        edits: vec![PackageEdit::SetModel(PackageModel::PartFile(Box::new(
            PartFileModel {
                model: current.model,
                values: values.clone(),
                derived,
            },
        )))],
    });
    Ok((operation, part.report()))
}

impl Session {
    pub(super) fn check_part_file(&self, params: CheckPartFileParams) -> Result<Value> {
        let file = read_part_file(params.path.as_deref(), params.builtin.as_deref())?;
        let part = part_file::evaluate(&file.bytes, &params.values)?;
        let (hits, evaluations) = part_file::cache_counts();
        Ok(json!({
            "file": file.file_name,
            "report": part.report(),
            "materials": part_file::materials().into_iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "cache": {"hits": hits, "evaluations": evaluations},
        }))
    }

    pub(super) fn use_part_file(&mut self, params: UsePartFileParams) -> Result<Value> {
        let file = read_part_file(params.path.as_deref(), params.builtin.as_deref())?;
        let request = UsePartFile {
            package: params.package,
            file,
            values: params.values,
            name: params.name,
            licence: params.licence,
            redistribution: params.redistribution,
        };
        let (operation, report) = use_part_file_operation(&self.engine, &request)?;
        let mut reply = self.apply(params.expected_revision, operation)?;
        reply["part"] = serde_json::to_value(report)?;
        Ok(reply)
    }

    pub(super) fn set_part_values(&mut self, params: SetPartValuesParams) -> Result<Value> {
        let (operation, report) =
            set_part_values_operation(&self.engine, params.package, &params.values)?;
        let mut reply = self.apply(params.expected_revision, operation)?;
        reply["part"] = serde_json::to_value(report)?;
        Ok(reply)
    }
}

#[cfg(test)]
#[path = "part_files_tests.rs"]
mod tests;
