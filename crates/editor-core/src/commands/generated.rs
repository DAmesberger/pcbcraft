//! `library.generate` (slice GP1): make a part with a generator module,
//! publish its revisions, embed the module and record where the part came
//! from, as one undo step on the candidate document. The module runs
//! through the engine's host runtime ([`GeneratorRuntime`]); the core
//! never interprets it.
use super::Outcome;
use crate::diagnostics::{Error, Result};
use crate::ids::{
    BindingRef, ContentHash, DefinitionId, DefinitionRef, FootprintRef, PackageRef, Revision,
    SymbolRef,
};
use crate::library::bundle::{AssetBytes, MAX_ASSETS, PackedAsset};
use crate::library::generated::{
    self, GeneratedFrom, GeneratedPart, GeneratorParams, GeneratorRuntime, GeneratorScope,
    MAX_MODULE_BYTES, MAX_PARAMS, MODULE_MEDIA_TYPE, ParamValue, PartKey, expand,
};
use crate::library::symbol_style::ResistorStyle;
use crate::library::{DefinitionMeta, MAX_LABEL, MAX_NOTE, drafts};
use crate::model::Document;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sparksmith_generator_abi::v1;
use std::sync::Arc;

/// `library.generate`'s parameters.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Generate {
    /// The module: its SHA-256, and its bytes unless the project already
    /// embeds it.
    pub module: ModuleSource,
    pub scope: GeneratorScope,
    /// Parameters by key (`library.generators` lists each module's). The
    /// rest are filled in: a parameter a project setting decides from the
    /// project, every other from the module's default. Lengths are integer
    /// nanometres, angles integer millidegrees.
    #[serde(default)]
    pub params: GeneratorParams,
    /// The part's name; the generator's suggestion when absent (the
    /// replaced part's name when `replaces` is given).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The root revision of a generated part to make again: the new part is
    /// published as the next revision of each of its lineages instead of as
    /// new lineages, so `project.preview_update` can move instances to it
    /// with its impact listed. The usual way to take a changed module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<DefinitionRef>,
}

/// A module by hash, with its bytes when they are not embedded yet.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModuleSource {
    pub hash: ContentHash,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<AssetBytes>,
}

/// What `library.generate` made or found.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GenerateReport {
    /// The part's root: its component (complete part) or package revision.
    pub root: DefinitionRef,
    /// Every revision of the part, root first.
    pub revisions: Vec<DefinitionRef>,
    pub key: PartKey,
    /// SHA-256 of the canonical generation.
    pub result: ContentHash,
    /// False when the project already held the part with this key: then
    /// nothing changed.
    pub created: bool,
    /// The module's remarks for the person reviewing the part.
    pub notes: Vec<String>,
}

fn runtime_needed() -> Error {
    Error::Unsupported(
        "generator.runtime: library.generate needs the host's generator runtime; this host has none".into(),
    )
}

/// The module's bytes: the inline copy (checked against its hash) or the
/// project's embedded one.
fn module_bytes(document: &Document, source: &ModuleSource) -> Result<(AssetBytes, bool)> {
    let packed = document
        .project
        .library
        .assets
        .iter()
        .find(|a| a.hash == source.hash);
    if let Some(asset) = packed
        && asset.media_type != MODULE_MEDIA_TYPE
    {
        return Err(Error::Conflict(format!(
            "generated.module: sha256 {} is packed in this project as {}, not as a generator module",
            source.hash, asset.media_type
        )));
    }
    match (&source.data, packed) {
        (Some(data), _) => {
            if data.len() > MAX_MODULE_BYTES {
                return Err(Error::Limit(format!(
                    "generator.too_large: the generator module is {} bytes, more than the {MAX_MODULE_BYTES} allowed. It was not run.",
                    data.len()
                )));
            }
            let actual = data.hash();
            if actual != source.hash {
                return Err(Error::Invalid(format!(
                    "generated.module_changed: the module's bytes hash to sha256 {actual}, not to the sha256 {} named with them. It was not run.",
                    source.hash
                )));
            }
            Ok((data.clone(), packed.is_none()))
        }
        (None, Some(asset)) => Ok((asset.data.clone(), false)),
        (None, None) => Err(Error::NotFound(format!(
            "generated.module_missing: the generator module sha256 {} is not embedded in this project; give its bytes",
            source.hash
        ))),
    }
}

/// The value of a project setting a parameter declares as its context, if
/// the core knows the setting.
fn context_value(document: &Document, param: &v1::Param) -> Option<v1::ParamValue> {
    match param.context.as_deref()? {
        "project.land.chip_gap_min" => {
            let mm = document.project.rules.land_estimators().land.chip_gap_min;
            crate::units::Nm::from_mm_f64_rounded(mm)
                .ok()
                .map(|nm| v1::ParamValue::Int(nm.get()))
        }
        "project.symbols.resistor_style" => {
            let key = match document.drafts.symbols.resistor {
                ResistorStyle::Iec => "iec",
                ResistorStyle::Us => "us",
            };
            Some(v1::ParamValue::Text(key.into()))
        }
        _ => None,
    }
}

/// `library.generate`.
pub(crate) fn generate(
    document: &mut Document,
    request: Generate,
    runtime: Option<&dyn GeneratorRuntime>,
) -> Result<Outcome> {
    let runtime = runtime.ok_or_else(runtime_needed)?;
    if request.params.len() > MAX_PARAMS {
        return Err(Error::Limit(format!(
            "generated.params: a request gives at most {MAX_PARAMS} parameters"
        )));
    }
    if let Some(name) = &request.name
        && (name.trim().is_empty() || name.len() > MAX_LABEL)
    {
        return Err(Error::Invalid(format!(
            "generated.name: a part's name is 1–{MAX_LABEL} bytes"
        )));
    }
    let (bytes, embed) = module_bytes(document, &request.module)?;
    let description = runtime
        .describe(&bytes)
        .map_err(|f| generated::failure(&f))?;
    let mut given = generated::wire_params(&request.params);
    for param in &description.params {
        if given.contains_key(&param.key) {
            continue;
        }
        if let Some(value) = context_value(document, param)
            && sparksmith_generator_abi::params::fits(&param.ty, &value)
        {
            given.insert(param.key.clone(), value);
        }
    }
    let wire = generated::complete(&description, request.scope.wire(), &given)
        .map_err(|f| generated::failure(&f))?;
    let key = generated::key(request.module.hash, &wire)?;
    if let Some(existing) = generated::find(&document.project.library, &key) {
        return Ok(Outcome::Generated {
            report: GenerateReport {
                root: existing.root,
                revisions: existing.expansion.clone(),
                key,
                result: existing.from.result,
                created: false,
                notes: Vec::new(),
            },
        });
    }
    let scope = request.scope;
    // The lineages and their base revisions: the replaced part's, or new.
    let replaced = match request.replaces {
        Some(root) => {
            let library = &document.project.library;
            let part = generated::record_of(library, root).ok_or_else(|| {
                Error::NotFound(format!(
                    "generated.replaces: {} {} is not the root of a generated part",
                    root.id, root.revision
                ))
            })?;
            if part.from.scope != scope {
                return Err(Error::Invalid(format!(
                    "generated.replaces: {} was made in the other scope",
                    generated::describe(library, part)
                )));
            }
            let mut lineages = Vec::with_capacity(part.expansion.len());
            for r in &part.expansion {
                let latest = library
                    .latest(r.id)
                    .map(|(_, revision)| revision)
                    .ok_or_else(|| {
                        Error::NotFound(format!("generated.replaces: {} is gone", r.id))
                    })?;
                lineages.push((r.id, Some(latest)));
            }
            let name = library.definition(root).map(|(meta, _)| meta.name);
            Some((lineages, name))
        }
        None => None,
    };
    let made = runtime
        .generate(&bytes, &wire)
        .map_err(|f| generated::failure(&f))?;
    let result = ContentHash(made.result_hash);
    let project = Arc::make_mut(&mut document.project);
    let lineages: Vec<(DefinitionId, Option<Revision>)> = match &replaced {
        Some((lineages, _)) => lineages.clone(),
        None => {
            let mut fresh = Vec::with_capacity(scope.kinds().len());
            for _ in scope.kinds() {
                fresh.push((DefinitionId(project.allocate()?), None));
            }
            fresh
        }
    };
    let mut expansion = Vec::with_capacity(lineages.len());
    for (kind, (id, base)) in scope.kinds().iter().zip(&lineages) {
        let revision = match base {
            Some(base) => base.next().ok_or_else(|| {
                Error::Limit(format!("generated.replaces: {id} has no next revision"))
            })?,
            None => Revision::FIRST,
        };
        expansion.push(DefinitionRef {
            kind: *kind,
            id: *id,
            revision,
        });
    }
    let at = |i: usize| -> Result<DefinitionRef> {
        expansion
            .get(i)
            .copied()
            .ok_or_else(|| Error::Invalid("generated.expansion: wrong number of lineages".into()))
    };
    let refs = match scope {
        GeneratorScope::Component => {
            let (component, symbol, package, footprint, binding) =
                (at(0)?, at(1)?, at(2)?, at(3)?, at(4)?);
            expand::Refs {
                component: Some((
                    component.id,
                    SymbolRef::new(symbol.id, symbol.revision),
                    BindingRef::new(binding.id, binding.revision),
                )),
                package: PackageRef::new(package.id, package.revision),
                footprint: FootprintRef::new(footprint.id, footprint.revision),
            }
        }
        GeneratorScope::Package => {
            let (package, footprint) = (at(0)?, at(1)?);
            expand::Refs {
                component: None,
                package: PackageRef::new(package.id, package.revision),
                footprint: FootprintRef::new(footprint.id, footprint.revision),
            }
        }
    };
    let ids: Vec<DefinitionId> = lineages.iter().map(|(id, _)| *id).collect();
    let bases: Vec<Option<Revision>> = lineages.iter().map(|(_, base)| *base).collect();
    let definitions = generated::expand_at(&made.generation, refs, &expansion)?;
    let base_name = request
        .name
        .clone()
        .or_else(|| replaced.as_ref().and_then(|(_, name)| name.clone()))
        .unwrap_or_else(|| {
            made.generation
                .component
                .as_ref()
                .map(|c| c.name.clone())
                .unwrap_or_else(|| format!("{} package", description.name))
        });
    let provenance = bounded(format!(
        "Made by the generator ‘{}’ (module sha256 {}, parameters sha256 {}, result sha256 {result}).",
        description.name, key.module, key.params
    ));
    let notes_text = bounded(made.generation.notes.join(" "));
    if embed {
        if project.library.assets.len() >= MAX_ASSETS {
            return Err(Error::Limit(format!(
                "a project packs at most {MAX_ASSETS} assets"
            )));
        }
        project.library.assets.push(PackedAsset {
            hash: request.module.hash,
            media_type: MODULE_MEDIA_TYPE.into(),
            data: bytes,
        });
    }
    let suffix = |kind: crate::ids::DefinitionKind| -> &'static str {
        use crate::ids::DefinitionKind as K;
        match (scope, kind) {
            (GeneratorScope::Component, K::Component) => "",
            (GeneratorScope::Package, K::Package) => "",
            (_, K::Symbol) => " symbol",
            (_, K::Package) => " package",
            (_, K::Footprint) => " footprint",
            (_, K::Binding) => " binding",
            _ => "",
        }
    };
    let library = &document.project.library;
    let drafts_mut = Arc::make_mut(&mut document.drafts);
    for ((r, definition), base) in definitions.into_iter().zip(bases) {
        let meta = DefinitionMeta {
            name: label(&base_name, suffix(r.kind)),
            description: notes_text.clone(),
            provenance: provenance.clone(),
        };
        drafts::create(drafts_mut, library, r.id, base, meta, definition)?;
    }
    super::apply_library(
        document,
        super::LibraryCommand::Publish {
            drafts: ids,
            close: true,
        },
        None,
    )?;
    let root = expansion
        .first()
        .copied()
        .ok_or_else(|| Error::Invalid("generated.expansion: nothing was made".into()))?;
    let part = GeneratedPart {
        root,
        expansion: expansion.clone(),
        from: GeneratedFrom {
            module: request.module.hash,
            abi: wire.abi,
            scope,
            params: wire
                .params
                .iter()
                .map(|(k, v)| (k.clone(), ParamValue::from_wire(v)))
                .collect(),
            result,
        },
    };
    Arc::make_mut(&mut document.project)
        .library
        .generated
        .push(part);
    Ok(Outcome::Generated {
        report: GenerateReport {
            root,
            revisions: expansion,
            key,
            result,
            created: true,
            notes: made.generation.notes,
        },
    })
}

/// `name` with `suffix`, within the label bound.
fn label(name: &str, suffix: &str) -> String {
    let mut end = name.len().min(MAX_LABEL.saturating_sub(suffix.len()));
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &name[..end])
}

/// `text` within the note bound.
fn bounded(text: String) -> String {
    if text.len() <= MAX_NOTE {
        return text;
    }
    let mut end = MAX_NOTE;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
