//! Generator modules in the session (slice GP1, `docs/generator-plugins.md`).
//!
//! `library.generators` lists the shipped modules (`library/generators/`),
//! the modules this project embeds and its generated parts, each with the
//! state of its module: `current` (the shipped module of its generator is
//! the embedded one), `shipped_differs` (a different module of the same
//! generator ships: the project keeps its copy, and nothing changes until
//! the person makes the part again with the shipped one) or `not_shipped`
//! (the project's copy is the only one, which is enough to reopen and
//! check it). `library.generate_part` reads a module (shipped by name, or
//! embedded by hash) and applies the one typed command `library.generate`,
//! so UI, CLI and MCP make a part the same way.
use super::Session;
use crate::Result;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sparksmith_core::Operation;
use sparksmith_core::commands::{Generate, LibraryCommand, ModuleSource};
use sparksmith_core::ids::{ContentHash, DefinitionRef};
use sparksmith_core::library::bundle::{AssetBytes, asset_hash};
use sparksmith_core::library::generated::{
    self, GeneratorParams, GeneratorRuntime, GeneratorScope, ParamValue,
};
use sparksmith_generators::abi::v1::{self, Description};
use sparksmith_io::generator_bundle::GeneratorBundle;
use sparksmith_io::shipped::ShippedLibrary;
use std::collections::BTreeMap;
use std::sync::Arc;

/// `library.generate_part`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneratePartParams {
    /// A shipped generator by name (`library.generators`), e.g. `chip`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<String>,
    /// Or a module this project embeds, by its SHA-256.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<ContentHash>,
    /// `component` (component, symbol, package, footprint and binding) or
    /// `package` (package and footprint).
    pub scope: GeneratorScope,
    /// A preset of the module (`library.generators` lists them), whose
    /// values `params` override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Parameters by key; lengths in integer nanometres, angles in integer
    /// millidegrees. The rest come from project settings and the module's
    /// defaults.
    #[serde(default)]
    pub params: GeneratorParams,
    /// The part's name; the generator's suggestion when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The root revision of a generated part to make again as the next
    /// revision of its lineages (taking a changed module); then
    /// `project.preview_update` moves instances with the impact listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<DefinitionRef>,
    pub expected_revision: u64,
}

fn invalid(message: String) -> crate::Error {
    crate::Error::Document(sparksmith_core::Error::Invalid(message))
}

/// A generator module's bytes with where they came from: a shipped
/// bundle (with its part files) or the project.
struct ModuleBytes {
    hash: ContentHash,
    bytes: Vec<u8>,
    bundle: Option<GeneratorBundle>,
}

impl Session {
    /// The shipped library: the one this session was given, else the one
    /// found from the environment and the executable.
    pub(super) fn shipped(&self) -> Result<ShippedLibrary> {
        match &self.shipped {
            Some(library) => Ok(library.clone()),
            None => Ok(ShippedLibrary::locate()?),
        }
    }

    /// Use `library` as the shipped library (tests, hosts that bundle it
    /// elsewhere).
    pub fn set_shipped(&mut self, library: ShippedLibrary) {
        self.shipped = Some(library);
    }

    /// Give the engine a generator runtime when it has none: a worker
    /// thread per call with the host's limits, a wall time and no state.
    pub(super) fn ensure_generators(&mut self) {
        if !self.engine.has_generators() {
            self.engine
                .set_generators(Arc::new(sparksmith_generators::worker::Worker::default()));
        }
    }

    fn runtime(&self) -> sparksmith_generators::worker::Worker {
        sparksmith_generators::worker::Worker::default()
    }

    fn module_bytes(
        &self,
        generator: Option<&str>,
        module: Option<ContentHash>,
    ) -> Result<ModuleBytes> {
        match (generator, module) {
            (Some(name), None) => {
                let bundle = self.shipped()?.generator(name)?;
                Ok(ModuleBytes {
                    hash: asset_hash(&bundle.module),
                    bytes: bundle.module.clone(),
                    bundle: Some(bundle),
                })
            }
            (None, Some(hash)) => {
                let asset = generated::module(&self.engine.project().library, hash).ok_or_else(
                    || {
                        invalid(format!(
                            "generated.module_missing: this project embeds no generator module sha256 {hash}"
                        ))
                    },
                )?;
                Ok(ModuleBytes {
                    hash,
                    bytes: asset.data.as_slice().to_vec(),
                    bundle: None,
                })
            }
            _ => Err(invalid(
                "library.generate_part: give exactly one of generator and module".into(),
            )),
        }
    }

    fn describe(&self, bytes: &[u8]) -> Result<Description> {
        self.runtime()
            .describe(bytes)
            .map_err(|f| crate::Error::Document(generated::failure(&f)))
    }

    pub(super) fn generate_part(&mut self, params: GeneratePartParams) -> Result<Value> {
        let module = self.module_bytes(params.generator.as_deref(), params.module)?;
        let mut values = GeneratorParams::new();
        if let Some(preset) = &params.preset {
            let description = self.describe(&module.bytes)?;
            let found = description
                .presets
                .iter()
                .find(|p| &p.key == preset)
                .ok_or_else(|| {
                    let keys: Vec<&str> =
                        description.presets.iter().map(|p| p.key.as_str()).collect();
                    invalid(format!(
                        "generated.preset: the generator ‘{}’ has no preset {preset}; presets: {}",
                        description.name,
                        keys.join(", ")
                    ))
                })?;
            values.extend(
                found
                    .params
                    .iter()
                    .map(|(k, v)| (k.clone(), ParamValue::from_wire(v))),
            );
        }
        values.extend(params.params);
        let embedded = generated::module(&self.engine.project().library, module.hash).is_some();
        let mut generate = Generate {
            module: ModuleSource {
                hash: module.hash,
                data: (!embedded).then(|| AssetBytes::new(module.bytes)),
            },
            scope: params.scope,
            params: values,
            name: params.name,
            replaces: params.replaces,
            model: None,
        };
        // The model first (GP2): the module's part file, evaluated here at
        // the values of the request the engine will run.
        let runtime = self.runtime();
        let (bytes, wire) = sparksmith_core::commands::generator_request(
            self.engine.document(),
            &generate,
            &runtime,
        )?;
        let made = runtime
            .generate(&bytes, &wire)
            .map_err(|f| crate::Error::Document(generated::failure(&f)))?;
        generate.model = Some(super::generated_models::generated_model(
            &self.engine,
            &made.generation,
            module.bundle.as_ref(),
        )?);
        let operation = Operation::Library(LibraryCommand::Generate(Box::new(generate)));
        self.ensure_generators();
        self.apply(params.expected_revision, operation)
    }

    pub(super) fn generators(&self) -> Result<Value> {
        let library = &self.engine.project().library;
        // The shipped modules: name, hash and what each offers.
        let mut shipped_by_lineage: BTreeMap<String, (String, ContentHash)> = BTreeMap::new();
        let mut shipped = Vec::new();
        let located = self.shipped();
        let names = match &located {
            Ok(found) => found.generators()?,
            Err(_) => Vec::new(),
        };
        if let Ok(found) = &located {
            for name in names {
                let bundle = found.generator(&name)?;
                let bundle_hash = bundle.hash;
                let bytes = bundle.module;
                let hash = asset_hash(&bytes);
                match self.describe(&bytes) {
                    Ok(description) => {
                        shipped_by_lineage
                            .insert(description.id_hint.clone(), (name.clone(), hash));
                        shipped.push(json!({
                            "name": name,
                            "module": hash,
                            "bundle": bundle_hash,
                            "bytes": bytes.len(),
                            "description": summary(&description),
                        }));
                    }
                    Err(error) => shipped.push(json!({
                        "name": name,
                        "module": hash,
                        "bundle": bundle_hash,
                        "refused": error.to_string(),
                    })),
                }
            }
        }
        // The embedded modules, each described once.
        let mut lineage_of: BTreeMap<ContentHash, Option<String>> = BTreeMap::new();
        let mut embedded = Vec::new();
        for asset in library
            .assets
            .iter()
            .filter(|a| a.media_type == generated::MODULE_MEDIA_TYPE)
        {
            let description = self.describe(&asset.data).ok();
            let lineage = description.as_ref().map(|d| d.id_hint.clone());
            lineage_of.insert(asset.hash, lineage.clone());
            embedded.push(json!({
                "module": asset.hash,
                "bytes": asset.data.len(),
                "generator": lineage,
                "name": description.as_ref().map(|d| d.name.clone()),
                "parts": library.generated.iter().filter(|g| g.from.module == asset.hash).count(),
            }));
        }
        let mut parts = Vec::new();
        for part in library.generated.iter() {
            let lineage = lineage_of.get(&part.from.module).cloned().flatten();
            let shipped_now = lineage.as_ref().and_then(|l| shipped_by_lineage.get(l));
            let (state, shipped_module) = match shipped_now {
                Some((_, hash)) if *hash == part.from.module => ("current", None),
                Some((_, hash)) => ("shipped_differs", Some(*hash)),
                None => ("not_shipped", None),
            };
            parts.push(json!({
                "root": part.root,
                "name": generated::describe(library, part),
                "scope": part.from.scope,
                "module": part.from.module,
                "key": part.from.key()?,
                "result": part.from.result,
                "generator": lineage,
                "state": state,
                "shipped_module": shipped_module,
            }));
        }
        Ok(json!({
            "shipped": shipped,
            "shipped_library": located.as_ref().ok().map(|l| l.root().display().to_string()),
            "embedded": embedded,
            "parts": parts,
        }))
    }
}

/// What a module offers, for people and models choosing parameters.
fn summary(description: &Description) -> Value {
    let params: Vec<Value> = description
        .params
        .iter()
        .map(|p| {
            json!({
                "key": p.key,
                "label": p.label,
                "help": p.help,
                "type": p.ty,
                "default": p.default,
                "group": p.group,
                "context": p.context,
            })
        })
        .collect();
    let presets: Vec<Value> = description
        .presets
        .iter()
        .map(|p| json!({"key": p.key, "name": p.name, "aliases": p.aliases}))
        .collect();
    json!({
        "generator": description.id_hint,
        "name": description.name,
        "families": description.families,
        "scopes": description.scopes,
        "params": params,
        "rules": description.rules.iter().map(|r| r.text.clone()).collect::<Vec<_>>(),
        "presets": presets,
        "abi": v1::ABI,
    })
}
