//! `library.generate` (slice GP1): make a part with a generator module,
//! publish its revisions, embed the module and record where the part came
//! from, as one undo step on the candidate document. The module runs
//! through the engine's host runtime ([`GeneratorRuntime`]); the core
//! never interprets it.
use super::Outcome;
use crate::diagnostics::{Error, Result};
use crate::ids::ModelRef;
use crate::ids::{
    BindingRef, ContentHash, DefinitionId, DefinitionRef, FootprintRef, PackageRef, Revision,
    SymbolRef,
};
use crate::library::PackageModel;
use crate::library::bundle::{AssetBytes, MAX_ASSETS, PackedAsset};
use crate::library::generated::{
    self, GeneratedFrom, GeneratedPart, GeneratorParams, GeneratorRuntime, GeneratorScope,
    MAX_MODULE_BYTES, MAX_PARAMS, MODULE_MEDIA_TYPE, ParamValue, PartKey, expand,
};
use crate::library::model3d::{self, ImportModel, ModelDefinition, ModelFormat};
use crate::library::package_model::{PartDerived, PartFileModel, PartValue};
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
    /// The part's 3D model as the host evaluated it: the part file the
    /// module names, at its values (slice GP2). Required: a part never
    /// exists without its model, and the core never evaluates one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<GeneratedModel>,
}

/// A generated part's 3D model, from the host: the part file the module's
/// generation names (by SHA-256) evaluated at its values. The core checks
/// that it is that file and those values; the bounds and surfaces are the
/// host's evaluation, rechecked by every board build.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GeneratedModel {
    /// The Model the part file becomes when the project holds none of
    /// those bytes yet: format `solidcraft`, its asset, bounds as the file
    /// is, licence and provenance.
    pub definition: ModelDefinition,
    /// That Model's name.
    pub name: String,
    /// The part file's bytes, unless the project packs them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<AssetBytes>,
    /// The generation's values in the part file's own units.
    pub values: std::collections::BTreeMap<String, PartValue>,
    /// What the part file evaluated to at those values.
    pub derived: PartDerived,
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
    /// The part this one is the next revision of, when an earlier version
    /// of the same generator made it (or `replaces` named it). Instances
    /// still pin the old revision until `project.apply_update` moves them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced: Option<DefinitionRef>,
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
            // Without a fab profile the board has no land margins.
            let mm = document
                .project
                .rules
                .land_estimators()
                .ok()?
                .land
                .chip_gap_min;
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

/// What `library.generate` will run: the module's bytes (and whether the
/// project must embed them), its description and the complete request.
struct Prepared {
    bytes: AssetBytes,
    embed: bool,
    description: v1::Description,
    wire: v1::Request,
}

/// The module bytes and complete request `request` runs on `document`:
/// what the host evaluates the part's model from before it applies
/// `library.generate` (the core never evaluates a part file).
pub fn generator_request(
    document: &Document,
    request: &Generate,
    runtime: &dyn GeneratorRuntime,
) -> Result<(AssetBytes, v1::Request)> {
    let prepared = prepare(document, request, runtime)?;
    Ok((prepared.bytes, prepared.wire))
}

fn prepare(
    document: &Document,
    request: &Generate,
    runtime: &dyn GeneratorRuntime,
) -> Result<Prepared> {
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
    Ok(Prepared {
        bytes,
        embed,
        description,
        wire,
    })
}

/// `library.generate`.
pub(crate) fn generate(
    document: &mut Document,
    request: Generate,
    runtime: Option<&dyn GeneratorRuntime>,
) -> Result<Outcome> {
    let runtime = runtime.ok_or_else(runtime_needed)?;
    let Prepared {
        bytes,
        embed,
        description,
        wire,
    } = prepare(document, &request, runtime)?;
    let key = generated::key(request.module.hash, &wire)?;
    if let Some(existing) = generated::find(&document.project.library, &key) {
        // A part a newer version has since made again is not handed back
        // at its old revision.
        let newest = document
            .project
            .library
            .generated
            .iter()
            .filter(|p| p.root.id == existing.root.id)
            .max_by_key(|p| p.root.revision);
        if let Some(newest) = newest
            && newest.root != existing.root
        {
            refuse_older(document, runtime, &description, newest)?;
        }
        return Ok(found(existing, key));
    }
    let scope = request.scope;
    // The part this request makes again: the one named, or the newest part
    // an earlier version of the same generator made from the same
    // parameters (the regenerate rule, `docs/generator-plugins.md`).
    let replaces = match request.replaces {
        Some(root) => Some(root),
        None => match predecessor(document, runtime, &description, &request, &wire)? {
            Some((part, version)) if version == description.version => {
                // The same version of the generator, built to other bytes:
                // the part it made stands, and nothing changes.
                let existing_key = part.from.key()?;
                return Ok(found(part, existing_key));
            }
            Some((part, _)) => Some(part.root),
            None => None,
        },
    };
    // The lineages and their base revisions: the replaced part's, or new.
    let replaced = match replaces {
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
            refuse_older(document, runtime, &description, part)?;
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
            Some((lineages, name, root))
        }
        None => None,
    };
    let made = runtime
        .generate(&bytes, &wire)
        .map_err(|f| generated::failure(&f))?;
    let result = ContentHash(made.result_hash);
    let (asset, part_values) = expand::part_file(&made.generation)?;
    let supplied = request.model.as_ref().ok_or_else(|| {
        Error::Invalid(format!(
            "generated.model_unevaluated: the part's 3D model, the part file sha256 {asset}, was not evaluated by the host. Nothing was changed."
        ))
    })?;
    expand::part_values_match(part_values, &supplied.values)?;
    if supplied.definition.asset.hash != asset
        || supplied.definition.format != ModelFormat::Solidcraft
    {
        return Err(Error::Invalid(format!(
            "generated.model: the host evaluated the part file sha256 {}, but the generator names sha256 {asset}. Nothing was changed.",
            supplied.definition.asset.hash
        )));
    }
    // The part file as a Model of the project: the newest one packing
    // those bytes, or a new lineage published with the part.
    let held = document
        .project
        .library
        .models
        .iter()
        .filter(|m| {
            m.definition.format == ModelFormat::Solidcraft && m.definition.asset.hash == asset
        })
        .max_by_key(|m| (m.id, m.revision))
        .map(|m| ModelRef::new(m.id, m.revision));
    let new_model = match held {
        Some(_) => None,
        None => {
            let import = ImportModel {
                name: supplied.name.clone(),
                assets: supplied
                    .data
                    .iter()
                    .map(|data| PackedAsset {
                        hash: asset,
                        media_type: ModelFormat::Solidcraft.media_type().into(),
                        data: data.clone(),
                    })
                    .collect(),
                definition: Box::new(supplied.definition.clone()),
                assign: None,
            };
            if let Some(data) = &supplied.data
                && data.hash() != asset
            {
                return Err(Error::Invalid(format!(
                    "generated.model: the part file's bytes hash to sha256 {}, not to sha256 {asset}. Nothing was changed.",
                    data.hash()
                )));
            }
            let (meta, definition) = model3d::import_model(&import)?;
            let fresh = model3d::assets_to_pack(&document.project.library, &import)?;
            Some((meta, definition, fresh))
        }
    };
    let project = Arc::make_mut(&mut document.project);
    let model_ref = match (held, &new_model) {
        (Some(held), _) => held,
        (None, _) => ModelRef::new(DefinitionId(project.allocate()?), Revision::FIRST),
    };
    let package_model = PackageModel::PartFile(Box::new(PartFileModel {
        model: model_ref,
        values: supplied.values.clone(),
        derived: supplied.derived.clone(),
    }));
    let lineages: Vec<(DefinitionId, Option<Revision>)> = match &replaced {
        Some((lineages, _, _)) => lineages.clone(),
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
    let mut ids: Vec<DefinitionId> = lineages.iter().map(|(id, _)| *id).collect();
    let bases: Vec<Option<Revision>> = lineages.iter().map(|(_, base)| *base).collect();
    let definitions = generated::expand_at(&made.generation, refs, &expansion, package_model)?;
    let base_name = request
        .name
        .clone()
        .or_else(|| replaced.as_ref().and_then(|(_, name, _)| name.clone()))
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
    if let Some((meta, definition, fresh)) = new_model {
        let project = Arc::make_mut(&mut document.project);
        if project.library.assets.len() + fresh.len() > MAX_ASSETS {
            return Err(Error::Limit(format!(
                "a project packs at most {MAX_ASSETS} assets"
            )));
        }
        project.library.assets.extend(fresh);
        let drafts_mut = Arc::make_mut(&mut document.drafts);
        drafts::create(
            drafts_mut,
            &project.library,
            model_ref.id,
            None,
            meta,
            definition,
        )?;
        ids.insert(0, model_ref.id);
    }
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
            replaced: replaced.map(|(_, _, root)| root),
            notes: made.generation.notes,
        },
    })
}

/// The report for a part the project already holds: nothing changed.
fn found(existing: &GeneratedPart, key: PartKey) -> Outcome {
    Outcome::Generated {
        report: GenerateReport {
            root: existing.root,
            revisions: existing.expansion.clone(),
            key,
            result: existing.from.result,
            created: false,
            replaced: None,
            notes: Vec::new(),
        },
    }
}

/// The description of an embedded module, described at most once per hash.
fn described<'a>(
    document: &Document,
    runtime: &dyn GeneratorRuntime,
    cache: &'a mut Vec<(ContentHash, v1::Description)>,
    hash: ContentHash,
) -> Result<&'a v1::Description> {
    let at = match cache.iter().position(|(h, _)| *h == hash) {
        Some(at) => at,
        None => {
            let asset = generated::module(&document.project.library, hash).ok_or_else(|| {
                Error::NotFound(format!(
                    "generated.module_missing: the generator module sha256 {hash} is not embedded in this project"
                ))
            })?;
            let description = runtime
                .describe(&asset.data)
                .map_err(|f| generated::failure(&f))?;
            cache.push((hash, description));
            cache.len() - 1
        }
    };
    cache
        .get(at)
        .map(|(_, d)| d)
        .ok_or_else(|| Error::Invalid("generated.module: a description was lost".into()))
}

/// Whether a part recorded with `old` parameters was made from the same
/// parameters as `wire`: every recorded value is the same, and a parameter
/// the newer module adds was not given (it takes the module's default or
/// the project's setting).
fn same_params(old: &GeneratorParams, wire: &v1::Request, given: &GeneratorParams) -> bool {
    old.iter()
        .all(|(k, v)| wire.params.get(k) == Some(&v.wire()))
        && wire
            .params
            .keys()
            .all(|k| old.contains_key(k) || !given.contains_key(k))
}

/// The newest part another module of the same generator (`id_hint`) made
/// from the same parameters in the same scope, with that module's version.
fn predecessor<'a>(
    document: &'a Document,
    runtime: &dyn GeneratorRuntime,
    description: &v1::Description,
    request: &Generate,
    wire: &v1::Request,
) -> Result<Option<(&'a GeneratedPart, u32)>> {
    let mut cache = Vec::new();
    let mut best: Option<(&GeneratedPart, u32)> = None;
    for part in document.project.library.generated.iter() {
        if part.from.scope != request.scope
            || part.from.module == request.module.hash
            || !same_params(&part.from.params, wire, &request.params)
        {
            continue;
        }
        let old = described(document, runtime, &mut cache, part.from.module)?;
        if old.id_hint != description.id_hint {
            continue;
        }
        let rank = |p: &GeneratedPart, v: u32| (v, p.root.revision, p.root.id);
        if best.is_none_or(|(p, v)| rank(part, old.version) > rank(p, v)) {
            best = Some((part, old.version));
        }
    }
    Ok(best)
}

/// Refuse making `part` again with an older version of its generator.
fn refuse_older(
    document: &Document,
    runtime: &dyn GeneratorRuntime,
    description: &v1::Description,
    part: &GeneratedPart,
) -> Result<()> {
    let mut cache = Vec::new();
    let old = described(document, runtime, &mut cache, part.from.module)?;
    if old.id_hint == description.id_hint && old.version > description.version {
        return Err(Error::Conflict(format!(
            "generated.older_version: {} was made by version {} of the generator ‘{}’; this module is version {}, which is older. A generated part is never downgraded. Nothing was changed.",
            generated::describe(&document.project.library, part),
            old.version,
            old.name,
            description.version
        )));
    }
    Ok(())
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
