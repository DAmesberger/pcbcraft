//! Lean parts (docs/part-templates.md §11 L4): `library.check_part_file`
//! evaluates a Solidcraft part file at literal values and reports the
//! convention check; `library.use_part_file` makes a package draft use one
//! (the file packed as a Model of format `solidcraft` with
//! `library.import_model`'s `assign.part`, or `library.edit_package`
//! `SetModel` when the same file is already a model); and
//! `library.set_part_values` sets new values. Each document change is one
//! core command, one undo step, refused whole on a blocking finding.
use super::Session;
use crate::Result;
use pcbcraft_core::Operation;
use pcbcraft_core::commands::LibraryCommand;
use pcbcraft_core::ids::{DefinitionId, ModelRef};
use pcbcraft_core::library::bundle::{AssetBytes, PackedAsset, asset_hash};
use pcbcraft_core::library::model3d::{
    AssetRef, AssignModel, AssignPart, ImportModel, ModelDefinition, ModelFormat, ModelProvenance,
    Redistribution,
};
use pcbcraft_core::library::package_model::{PackageEdit, PartFileModel};
use pcbcraft_core::library::{Basis, Provenance};
use pcbcraft_core::library::{Definition, PackageDefinition, PackageModel};
use pcbcraft_solidcraft::models::part_file::{self, PartValues};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

/// Longest accepted path, bytes.
const MAX_PATH: usize = 4095;
/// The media type of a part file asset.
const MEDIA_TYPE: &str = "application/x-solidcraft";
/// Licence of Pcbcraft's built-in part files.
const BUILTIN_LICENCE: &str = "MIT OR Apache-2.0";

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
    #[serde(default)]
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
    #[serde(default)]
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
    pub values: PartValues,
    pub expected_revision: u64,
}

fn invalid(message: String) -> crate::Error {
    crate::Error::Document(pcbcraft_core::Error::Invalid(message))
}

/// A part file's bytes and the name it goes by.
fn source(path: Option<&str>, builtin: Option<&str>) -> Result<(Vec<u8>, String, bool)> {
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
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("part.solidcraft")
                .to_owned();
            Ok((pcbcraft_io::persistence::read_bytes(p)?, name, false))
        }
        (None, Some(name)) => {
            let bytes = part_file::builtin(name).ok_or_else(|| {
                let known: Vec<&str> = part_file::BUILTIN.iter().map(|(n, _)| *n).collect();
                invalid(format!(
                    "part_file.read: no built-in part file {name}; built-ins: {}",
                    known.join(", ")
                ))
            })?;
            Ok((bytes.to_vec(), format!("{name}.solidcraft"), true))
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

impl Session {
    /// The package draft `id`.
    fn package_draft(&self, id: DefinitionId) -> Result<PackageDefinition> {
        self.engine
            .drafts()
            .get(id)
            .and_then(|d| match &d.definition {
                Definition::Package(p) => Some(p.clone()),
                _ => None,
            })
            .ok_or_else(|| invalid(format!("part_file.package: {id} is not a package draft")))
    }

    /// The bytes of a Model revision's asset, published or the pending
    /// revision of a draft.
    fn model_bytes(&self, model: ModelRef) -> Result<Vec<u8>> {
        let library = &self.engine.project().library;
        let definition = match library.model(model) {
            Some(published) => published.definition.clone(),
            None => self
                .engine
                .drafts()
                .get(model.id)
                .filter(|d| d.pending() == Some(model.revision))
                .and_then(|d| match &d.definition {
                    Definition::Model(m) => Some(m.as_ref().clone()),
                    _ => None,
                })
                .ok_or_else(|| invalid(format!("part_file.model: the model {model} is missing")))?,
        };
        if definition.format != ModelFormat::Solidcraft {
            return Err(invalid(format!(
                "part_file.model: the model {model} is not a SolidCraft part file"
            )));
        }
        library
            .assets
            .iter()
            .find(|a| a.hash == definition.asset.hash)
            .map(|a| a.data.as_slice().to_vec())
            .ok_or_else(|| {
                invalid(format!(
                    "part_file.model: the file of {model} is not packed"
                ))
            })
    }

    /// A Model (published or drafted) whose asset is `hash` and format
    /// `solidcraft`: its newest revision.
    fn model_with(&self, hash: pcbcraft_core::ids::ContentHash) -> Option<ModelRef> {
        let library = &self.engine.project().library;
        let published = library
            .models
            .iter()
            .filter(|m| {
                m.definition.asset.hash == hash && m.definition.format == ModelFormat::Solidcraft
            })
            .max_by_key(|m| (m.id, m.revision))
            .map(|m| ModelRef::new(m.id, m.revision));
        published.or_else(|| {
            self.engine
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

    pub(super) fn check_part_file(&self, params: CheckPartFileParams) -> Result<Value> {
        let (bytes, name, _) = source(params.path.as_deref(), params.builtin.as_deref())?;
        let part = part_file::evaluate(&bytes, &params.values)?;
        let (hits, evaluations) = part_file::cache_counts();
        Ok(json!({
            "file": name,
            "report": part.report(),
            "materials": part_file::materials().into_iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "cache": {"hits": hits, "evaluations": evaluations},
        }))
    }

    pub(super) fn use_part_file(&mut self, params: UsePartFileParams) -> Result<Value> {
        let (bytes, file_name, builtin) =
            source(params.path.as_deref(), params.builtin.as_deref())?;
        self.package_draft(params.package)?;
        let part = checked(&bytes, &params.values)?;
        let derived = part_file::derived(&part);
        let hash = asset_hash(&bytes);
        let operation = match self.model_with(hash) {
            Some(model) => Operation::Library(LibraryCommand::EditPackage {
                draft: params.package,
                edits: vec![PackageEdit::SetModel(PackageModel::PartFile(
                    PartFileModel {
                        model,
                        values: params.values.clone(),
                        derived,
                    },
                ))],
            }),
            None => {
                let licence = if builtin {
                    Some(BUILTIN_LICENCE.to_owned())
                } else {
                    Some(
                        params
                            .licence
                            .clone()
                            .filter(|l| !l.trim().is_empty())
                            .ok_or_else(|| {
                                invalid(
                                    "part_file.licence: a part file needs its licence stated"
                                        .into(),
                                )
                            })?,
                    )
                };
                // The model's own bounds: the file as it is.
                let as_is = checked(&bytes, &PartValues::new())?;
                let definition = ModelDefinition {
                    asset: AssetRef {
                        hash,
                        bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
                        media_type: MEDIA_TYPE.into(),
                        file_name: file_name.clone(),
                    },
                    format: ModelFormat::Solidcraft,
                    bounds_mm: as_is.bounds_mm,
                    provenance: ModelProvenance {
                        redistribution: if builtin {
                            Redistribution::Permitted
                        } else {
                            params.redistribution.unwrap_or_default()
                        },
                        record: Provenance {
                            origin: None,
                            basis: if builtin {
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
                let name = params
                    .name
                    .clone()
                    .unwrap_or_else(|| file_name.trim_end_matches(".solidcraft").to_owned());
                Operation::Library(LibraryCommand::ImportModel(Box::new(ImportModel {
                    name,
                    assets: vec![PackedAsset {
                        hash,
                        media_type: MEDIA_TYPE.into(),
                        data: AssetBytes::new(bytes),
                    }],
                    definition: Box::new(definition),
                    assign: Some(AssignModel {
                        package: params.package,
                        lead_map: Vec::new(),
                        part: Some(AssignPart {
                            values: params.values.clone(),
                            derived,
                        }),
                    }),
                })))
            }
        };
        let mut reply = self.apply(params.expected_revision, operation)?;
        reply["part"] = serde_json::to_value(part.report())?;
        Ok(reply)
    }

    pub(super) fn set_part_values(&mut self, params: SetPartValuesParams) -> Result<Value> {
        let package = self.package_draft(params.package)?;
        let PackageModel::PartFile(current) = &package.model else {
            return Err(invalid(format!(
                "part_file.package: package {} does not use a part file",
                params.package
            )));
        };
        let bytes = self.model_bytes(current.model)?;
        let part = checked(&bytes, &params.values)?;
        let operation = Operation::Library(LibraryCommand::EditPackage {
            draft: params.package,
            edits: vec![PackageEdit::SetModel(PackageModel::PartFile(
                PartFileModel {
                    model: current.model,
                    values: params.values.clone(),
                    derived: part_file::derived(&part),
                },
            ))],
        });
        let mut reply = self.apply(params.expected_revision, operation)?;
        reply["part"] = serde_json::to_value(part.report())?;
        Ok(reply)
    }
}

#[cfg(test)]
#[path = "part_files_tests.rs"]
mod tests;
