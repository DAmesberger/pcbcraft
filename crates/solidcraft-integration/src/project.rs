//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Pcbcraft.
//!
//! The native model holds only physical bodies: the laminate, exact copper
//! (pads, tracks, vias and copper fills, [`fill`]) and package envelopes.
//! They are disjoint except where copper joins copper (a track ending in
//! its pad, a fill's spoke on its via), so Solidcraft's interference checks
//! and mass properties of the saved file are physical. Soldermask, plated finish,
//! terminations and solder fillets are display-only layers
//! ([`crate::appearance`]) added to the drawn geometry, never to the file.
use crate::appearance::{self, Body, Land, Part, Path, ViaMark};
use crate::geom::prelude::*;
use crate::material::{self, BodyKind, Detail};
use crate::models::imported::{LibraryDefinition, SourceCache};
use crate::models::{self, ModelDefinition, look};
use crate::native::{
    self, BoardJoint, BoxBody, Created, Expr, JointFrame, Mount, Native, ProfileBody,
};
use crate::pad_outline::{self, Contour, Copper, FLATTEN_TOLERANCE_MM, PLATING_MM};
use crate::scene::{Geometry, Place, Volume};
use crate::{Error, Result};
use pcbcraft_core::geometry::{Transform3, cos_sin};
use pcbcraft_core::library::model3d::{LibraryModel, ModelDefinition as LibraryModelDefinition};
use pcbcraft_core::library::{
    ModelPlacement, PackageDefinition, PackageModel, Pad, PadDefaults, PadShape,
};
use pcbcraft_core::{
    CopperLayer, Fabrication, Id, Layer, PadId, Placement, Point, Project, Stackup, ViaSpan,
    instances, layout,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use solidcraft_engine::render::surface::OpaqueSurface;
use std::collections::{BTreeMap, BTreeSet};

/// Chords of each round end cap of a track's 3D copper. The core checks
/// measure the exact cap; 8 chords keep the native model's vertex count
/// bounded (within 2 % of the radius) on boards of thousands of tracks.
const TRACK_CAP_SEGMENTS: usize = 8;

/// Which Pcbcraft object a native body came from. Stored in the native file's
/// metadata so identity survives reopening, compactly ([`identity`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub pick_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<PadId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<Id>,
    /// The board via a via body came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<Id>,
    /// The copper fill a fill body came from ([`fill`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layer: Option<CopperLayer>,
    pub label: String,
    pub component: u64,
    /// The occurrence of a shared generated-model definition this source
    /// is; `None` for a component of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<u64>,
}

pub struct NativeModel {
    native: Native,
    pub geometry: Vec<Geometry>,
    pub sources: Vec<Source>,
    /// The generated package-model definitions, one per package revision
    /// (a library model's bodies with built-in materials among them).
    pub models: Vec<ModelDefinition>,
    /// The library-model definitions, each with a digest of the model and
    /// use it was built from, for an update to reuse.
    libraries: Vec<(u64, LibraryDefinition)>,
    /// What an update can reuse of this model's native geometry.
    reuse: Reused,
    /// Commands the update that made this model ran; `None` when it was
    /// built anew.
    updated: Option<usize>,
    /// The display layers made for this model, for an update to reuse.
    display: appearance::Memo,
}

/// How [`update`] made its model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rebuilt {
    /// The previous model, updated in place by this many native commands.
    InPlace(usize),
    /// Built anew, and why: the board changed in more than values, or the
    /// update failed (`None`: nothing was built before).
    Anew(Option<String>),
}

/// The identity of each native body's geometry ([`Geometry`] in model
/// order, native bodies first), so an update reuses the meshes and measures
/// of every body it did not change. A laminate slab is also known by its
/// inputs: re-evaluated after an edit before it, it is the same solid.
#[derive(Clone, Default)]
struct Reused {
    keys: Vec<Option<u64>>,
    /// Laminate slabs: name → (input digest, geometry index).
    slabs: BTreeMap<String, (u64, usize)>,
}

impl NativeModel {
    /// Write the model as a new `.solidcraft` file; refuses existing files.
    pub fn save(&self, path: &str) -> Result<()> {
        self.native.save_new(path)
    }

    /// The native design the model was built as.
    pub fn native(&self) -> &Native {
        &self.native
    }
}

/// The previous model's geometry by identity, for [`finish_with`].
struct Previous {
    geometry: Vec<Geometry>,
    by_key: std::collections::HashMap<u64, usize>,
    /// Laminate slabs: name → (input digest, geometry index).
    slabs: BTreeMap<String, (u64, usize)>,
}

impl Previous {
    fn of(geometry: Vec<Geometry>, reuse: Reused) -> Self {
        let by_key = reuse
            .keys
            .iter()
            .enumerate()
            .filter_map(|(index, key)| key.map(|key| (key, index)))
            .collect();
        Self {
            geometry,
            by_key,
            slabs: reuse.slabs,
        }
    }
}

/// A native design under construction, recording each body's kind.
struct Builder {
    native: Native,
    bodies: BTreeMap<String, BodyKind>,
    /// Generated package-model definitions built so far.
    models: Vec<ModelDefinition>,
    /// Definitions whose first occurrence an instance already holds.
    placed: BTreeSet<String>,
    /// Replaying: the definitions the previous build made.
    known: Vec<ModelDefinition>,
    /// Library-model definitions built so far, by digest of their model
    /// and use ([`NativeModel::libraries`]).
    libraries: Vec<(u64, LibraryDefinition)>,
    /// Replaying: the library-model definitions the previous build made.
    known_libraries: Vec<(u64, LibraryDefinition)>,
    /// Library-model bodies with one of the model's own materials, by body
    /// name: drawn with that material's look (the file keeps its opaque
    /// projection).
    custom_looks: BTreeMap<String, (OpaqueSurface, Detail)>,
    /// Library model files read by earlier builds.
    sources: SourceCache,
    /// Each laminate slab's input digest, by body name.
    slabs: BTreeMap<String, u64>,
    /// Replaying: the previous model's geometry.
    previous: Option<Previous>,
    /// Display layers made before ([`appearance::Memo`]).
    display: appearance::Memo,
}

impl Builder {
    fn new(name: &str, sources: &SourceCache) -> Result<Self> {
        let mut native = Native::new(name)?;
        native.record();
        Ok(Self {
            native,
            bodies: BTreeMap::new(),
            models: Vec::new(),
            placed: BTreeSet::new(),
            known: Vec::new(),
            libraries: Vec::new(),
            known_libraries: Vec::new(),
            custom_looks: BTreeMap::new(),
            sources: sources.clone(),
            slabs: BTreeMap::new(),
            previous: None,
            display: appearance::Memo::default(),
        })
    }

    /// A build of an edited board replayed against `native`, the model
    /// `known` and `previous` came from ([`Native::replaying`]).
    fn replaying(
        native: Native,
        known: Vec<ModelDefinition>,
        known_libraries: Vec<(u64, LibraryDefinition)>,
        previous: Previous,
        display: appearance::Memo,
        sources: &SourceCache,
    ) -> Self {
        Self {
            native,
            bodies: BTreeMap::new(),
            models: Vec::new(),
            placed: BTreeSet::new(),
            known,
            libraries: Vec::new(),
            known_libraries,
            custom_looks: BTreeMap::new(),
            sources: sources.clone(),
            slabs: BTreeMap::new(),
            previous: Some(previous),
            display,
        }
    }

    /// Build the generated definition `stem` of `package` unless it is
    /// built. Replaying, the previous build's definition of the same
    /// immutable package revision is taken as it is.
    fn define(&mut self, stem: &str, package: &PackageDefinition) -> Result<()> {
        if self.models.iter().any(|d| d.component == stem) {
            return Ok(());
        }
        let definition = if self.native.is_replaying() {
            match self.known.iter().find(|d| d.component == stem).cloned() {
                Some(definition) if self.native.pass(stem) => definition,
                _ => {
                    return Err(Error::Verification(format!(
                        "the board's model {stem} is new"
                    )));
                }
            }
        } else {
            let start = self.native.step();
            let definition = models::build_definition(&mut self.native, stem, package)?;
            self.native.mark(stem, start);
            definition
        };
        for body in &definition.bodies {
            self.record(&body.name, body.kind())?;
        }
        self.models.push(definition);
        Ok(())
    }

    /// Build the library-model definition `stem` (`model` with its asset
    /// `bytes`, used as `use_`) unless it is built; bounds and lead contacts
    /// are rechecked when built. Replaying, the previous build's definition
    /// of the same model and use is taken as it is.
    fn define_library(
        &mut self,
        stem: &str,
        (model, use_, bytes): (&LibraryModelDefinition, &LibraryModel, &[u8]),
    ) -> Result<()> {
        if self.models.iter().any(|d| d.component == stem) {
            return Ok(());
        }
        let key = digest(&(stem, model, use_));
        let built = if self.native.is_replaying() {
            match self
                .known_libraries
                .iter()
                .find(|(k, d)| *k == key && d.component == stem)
                .map(|(_, d)| d.clone())
            {
                Some(built) if self.native.pass(stem) => built,
                _ => {
                    return Err(Error::Verification(format!(
                        "the board's library model {stem} is new"
                    )));
                }
            }
        } else {
            let start = self.native.step();
            let built = models::imported::build_library_model_cached(
                &mut self.native,
                stem,
                model,
                use_,
                bytes,
                &self.sources,
            )?;
            self.native.mark(stem, start);
            built
        };
        for body in &built.bodies {
            self.record(&body.name, body.kind())?;
            if matches!(
                body.material,
                pcbcraft_core::library::model3d::ModelMaterial::Custom(_)
            ) {
                let look = models::imported::surface(model, body.material)?;
                self.custom_looks.insert(body.name.clone(), look);
            }
        }
        self.models.push(built.generated());
        self.libraries.push((key, built));
        Ok(())
    }

    /// Build the part-file definition of `use_` (its file `bytes`) unless
    /// it is built, and return its name (`pkgp<key>`: packages with equal
    /// file and values share it). The part is evaluated through the
    /// process-wide cache, checked, and compared with what the package
    /// stored (`part_file.stale`). Replaying, the previous build's definition of
    /// the same key is taken as it is.
    fn define_part(
        &mut self,
        package: &str,
        use_: &pcbcraft_core::library::package_model::PartFileModel,
        bytes: &[u8],
    ) -> Result<String> {
        use crate::models::part_file;
        let part = part_file::evaluate(bytes, &use_.values)?;
        if let Some(refusal) = part.refusal() {
            return Err(refusal);
        }
        if !part_file::matches(&use_.derived, &part_file::derived(&part)) {
            return Err(Error::Invalid(format!(
                "part_file.stale: the part file of package {package} no longer builds what the package \
                 recorded (bounds, bodies or marking surfaces); set its values again"
            )));
        }
        let stem = part.stem();
        if self.models.iter().any(|d| d.component == stem) {
            return Ok(stem);
        }
        let definition = if self.native.is_replaying() {
            match self.known.iter().find(|d| d.component == stem).cloned() {
                Some(definition) if self.native.pass(&stem) => definition,
                _ => {
                    return Err(Error::Verification(format!(
                        "the board's part model {stem} is new"
                    )));
                }
            }
        } else {
            let start = self.native.step();
            let definition = part_file::insert(&mut self.native, &stem, &part)?;
            self.native.mark(&stem, start);
            definition
        };
        for body in &definition.bodies {
            self.record(&body.name, body.kind())?;
        }
        self.models.push(definition);
        Ok(stem)
    }

    /// An occurrence of the generated definition `stem` of `package`,
    /// unmoved: the definition is built on first use (its first occurrence
    /// returned), then every further call places it once more.
    fn model(&mut self, stem: &str, package: &PackageDefinition) -> Result<Created> {
        self.define(stem, package)?;
        self.model_occurrence(stem)
    }

    /// [`Self::model`] for the library-model definition `stem`
    /// ([`Self::define_library`]).
    fn library_model(
        &mut self,
        stem: &str,
        library: (&LibraryModelDefinition, &LibraryModel, &[u8]),
    ) -> Result<Created> {
        self.define_library(stem, library)?;
        self.model_occurrence(stem)
    }

    /// The first occurrence of the built definition `stem` the first time
    /// it is asked for, a new copy every time after.
    fn model_occurrence(&mut self, stem: &str) -> Result<Created> {
        let created = self.native.find_component(stem).ok_or_else(|| {
            Error::Verification(format!("model definition {stem} has no occurrence"))
        })?;
        if self.placed.insert(stem.to_owned()) {
            return Ok(created);
        }
        Ok(Created {
            component: created.component,
            occurrence: self.native.copy_occurrence(created.component)?,
        })
    }

    fn param(&mut self, name: &str, value: f64, unit: &str) -> Result<()> {
        self.native.parameter(name, format!("{value} {unit}"))
    }

    /// Many parameters in one command (one by one, each addition evaluates
    /// every parameter and joint again).
    fn params(&mut self, params: &[(String, f64, &str)]) -> Result<()> {
        for chunk in params.chunks(MAX_IMPORTED_PARAMS) {
            let rows: Vec<serde_json::Value> = chunk
                .iter()
                .map(|(name, value, unit)| json!({"name": name, "expression": format!("{value} {unit}")}))
                .collect();
            self.native
                .import_parameters(&serde_json::to_string(&rows)?)?;
        }
        Ok(())
    }

    fn body(&mut self, kind: BodyKind, body: BoxBody<'_>) -> Result<()> {
        let name = body.name.to_owned();
        self.native.solid_box(body)?;
        self.record(&name, kind)
    }

    /// Record a created body's kind.
    fn record(&mut self, name: &str, kind: BodyKind) -> Result<()> {
        if self.bodies.insert(name.to_owned(), kind).is_some() {
            return Err(Error::Verification(format!("duplicate body name {name}")));
        }
        Ok(())
    }

    /// Store each kind's physical look (one assignment per kind) and the
    /// body → kind map, then evaluate.
    fn finish(mut self, sources: Vec<Source>) -> Result<NativeModel> {
        self.native.evaluate_deferred()?;
        self.native.activate_root()?;
        // Generated bodies already carry their materials' looks.
        for kind in BodyKind::ALL.into_iter().filter(|k| !k.is_generated()) {
            let names: Vec<&str> = self
                .bodies
                .iter()
                .filter(|(_, k)| **k == kind)
                .map(|(name, _)| name.as_str())
                .collect();
            // Saved bodies look the same for every fabrication; only the
            // display-only layers take the project's mask and finish.
            if !names.is_empty() {
                self.native
                    .appearance(&names, &material::material(kind, &Fabrication::default()))?;
            }
        }
        identity::write(&mut self.native, &sources, &self.bodies)?;
        models::record(&mut self.native, &self.models)?;
        let updated = self
            .native
            .is_replaying()
            .then(|| self.native.settle())
            .transpose()?;
        if updated.is_some() {
            // The kinds were recorded from the previous build's replies: a
            // slab whose edit left it in other pieces is built anew.
            let world = self.native.world_state()?;
            let made: BTreeSet<&str> = world.bodies.iter().map(|b| b.definition_name()).collect();
            if let Some(gone) = self
                .bodies
                .iter()
                .find(|(name, kind)| **kind == BodyKind::Substrate && !made.contains(name.as_str()))
            {
                return Err(Error::Verification(format!(
                    "the laminate is in other pieces ({} is gone)",
                    gone.0
                )));
            }
        }
        self.native.forget_history();
        let mut model = finish_with(
            self.native,
            sources,
            self.models,
            &self.custom_looks,
            self.previous,
            &self.slabs,
        )?;
        model.updated = updated;
        model.libraries = self.libraries;
        Ok(model)
    }
}

/// Every source a saved native model records, in pick order.
pub fn saved_sources(native: &Native) -> Result<Vec<Source>> {
    Ok(identity::read(native)?.0)
}

/// The pick of every world body that places a shared generated-model
/// definition, by world body index. Solidcraft lists a design's world
/// bodies placement by placement (`placements()` order), each placement's
/// bodies in definition order; each run is checked against the
/// placement's transform.
fn shared_picks(native: &Native, sources: &[Source]) -> Result<BTreeMap<usize, u64>> {
    let by_occurrence: BTreeMap<u64, u64> = sources
        .iter()
        .filter_map(|s| s.occurrence.map(|o| (o, s.pick_id)))
        .collect();
    let shared: BTreeSet<u64> = sources
        .iter()
        .filter(|s| s.occurrence.is_some())
        .map(|s| s.component)
        .collect();
    let mut out = BTreeMap::new();
    if shared.is_empty() {
        return Ok(out);
    }
    let doc = &native.session().doc;
    let world = native.world_state()?;
    let placements = doc.placements();
    for component in shared {
        let paths: Vec<&Vec<u64>> = placements
            .iter()
            .filter(|(c, _, _)| *c == component)
            .map(|(_, path, _)| path)
            .collect();
        let bodies: Vec<usize> = world
            .bodies
            .iter()
            .enumerate()
            .filter(|(_, b)| b.component == component)
            .map(|(i, _)| i)
            .collect();
        if paths.is_empty() || !bodies.len().is_multiple_of(paths.len()) {
            return Err(Error::Verification(format!(
                "component {component}: {} bodies over {} placements",
                bodies.len(),
                paths.len()
            )));
        }
        let per = bodies.len() / paths.len();
        for (k, index) in bodies.iter().enumerate() {
            let path = paths
                .get(k / per.max(1))
                .ok_or_else(|| Error::Verification("placement run out of range".into()))?;
            let body = world
                .bodies
                .get(*index)
                .ok_or_else(|| Error::Verification("world body out of range".into()))?;
            let expected = doc.path_transform(component, path);
            let matches = expected.is_some_and(|m| {
                m.iter()
                    .flatten()
                    .zip(body.source_transform.iter().flatten())
                    .all(|(a, b)| (a - b).abs() <= 1e-9 * (1.0 + a.abs()))
            });
            if !matches {
                return Err(Error::Verification(format!(
                    "body {} does not sit at placement {path:?}",
                    body.name
                )));
            }
            let pick = path
                .last()
                .and_then(|o| by_occurrence.get(o))
                .copied()
                .ok_or_else(|| {
                    Error::Verification(format!("placement {path:?} has no Pcbcraft source"))
                })?;
            out.insert(*index, pick);
        }
    }
    Ok(out)
}

#[cfg(test)]
fn finish(native: Native, sources: Vec<Source>) -> Result<NativeModel> {
    finish_with(
        native,
        sources,
        Vec::new(),
        &BTreeMap::new(),
        None,
        &BTreeMap::new(),
    )
}

/// A definition body's measure at one placement: volume, box (low, high)
/// and where that placement moved it.
type Measured = (Volume, [f64; 3], [f64; 3], [f64; 3]);

/// Via definitions by drill and diameter (their bits).
type ViaKey = (u64, u64);

/// [`finish`] for a model whose generated definitions are `models` and
/// whose library-model bodies with their own materials look `custom`. The
/// geometry of a body `previous` holds (the same evaluated body where it
/// was, or a laminate slab of the same inputs `slabs`) is taken from it.
fn finish_with(
    native: Native,
    sources: Vec<Source>,
    models: Vec<ModelDefinition>,
    custom: &BTreeMap<String, (OpaqueSurface, Detail)>,
    previous: Option<Previous>,
    slabs: &BTreeMap<String, u64>,
) -> Result<NativeModel> {
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(error));
    }
    let (_, kinds) = identity::read(&native)?;
    let generated = models::recorded(&native)?;
    let shared = shared_picks(&native, &sources)?;
    let doc = &native.session().doc;
    // The first source of each component of its own, and each source by pick.
    let mut own: BTreeMap<u64, u64> = BTreeMap::new();
    let mut by_pick: BTreeMap<u64, &Source> = BTreeMap::new();
    for s in &sources {
        if s.occurrence.is_none() {
            own.entry(s.component).or_insert(s.pick_id);
        }
        by_pick.entry(s.pick_id).or_insert(s);
    }
    // Placements that only move a definition (every via) measure the same:
    // each definition body is measured once, at its first placement.
    let mut measured: BTreeMap<(u64, String), Measured> = BTreeMap::new();
    let mut geometry = vec![];
    // Every display mesh not taken from `previous` at once, on all cores
    // (each the mesh it makes alone). A placement of a definition shares
    // the definition's mesh, so only that is made, never a moved copy.
    let world = native.world_state()?;
    let bodies = &world.bodies;
    let kept = |key: Option<u64>, name: &str| {
        previous.as_ref().is_some_and(|p| {
            key.is_some_and(|k| p.by_key.contains_key(&k))
                || slabs
                    .get(name)
                    .is_some_and(|d| p.slabs.get(name).is_some_and(|(at, _)| at == d))
        })
    };
    let cold: Vec<_> = bodies
        .iter()
        .filter(|b| !kept(b.geometry_key(), b.definition_name()))
        .collect();
    on_threads(&cold, |body| {
        body.mesh_in_place();
    });
    let mut reuse = Reused::default();
    for (index, body) in native.world_state()?.bodies.iter().enumerate() {
        let name = body.definition_name();
        let kind = *kinds
            .get(name)
            .ok_or_else(|| Error::Verification(format!("body {name} has no Pcbcraft kind")))?;
        let identity = body.geometry_key();
        let slab = (kind == BodyKind::Substrate)
            .then(|| slabs.get(name).copied())
            .flatten();
        reuse.keys.push(identity);
        if let Some(digest) = slab {
            reuse
                .slabs
                .insert(name.to_owned(), (digest, geometry.len()));
        }
        let item = shared
            .get(&index)
            .copied()
            .or_else(|| own.get(&body.component).copied());
        let layer = |kind: BodyKind| {
            item.and_then(|pick| by_pick.get(&pick).copied())
                .filter(|_| !kind.is_envelope() && !kind.is_generated())
                .and_then(|s| s.layer)
                .and_then(|layer| body_layer(name, layer))
        };
        // The same body where it was, or a slab of the same inputs.
        let same = previous.as_ref().and_then(|p| {
            let at = identity
                .and_then(|k| p.by_key.get(&k).copied())
                .or_else(|| {
                    let (digest, at) = p.slabs.get(name)?;
                    (Some(*digest) == slab).then_some(*at)
                })?;
            p.geometry.get(at).filter(|g| g.kind == kind)
        });
        if let Some(same) = same {
            geometry.push(Geometry {
                item,
                layer: layer(kind),
                ..same.clone()
            });
            continue;
        }
        let saved = doc
            .model_body_look(body)
            .ok_or_else(|| Error::Verification(format!("body {name} has no look")))?
            .opaque_surface()
            .map_err(|e| Error::Verification(e.to_string()))?
            .ok_or_else(|| Error::Verification(format!("body {name} has no physical look")))?;
        // A generated body is drawn with its material's full product look
        // (coat, pattern); the file keeps the opaque projection.
        let (surface, detail) = if let Some(look) = custom.get(name) {
            *look
        } else if kind.is_generated() {
            let body = generated.get(name).ok_or_else(|| {
                Error::Verification(format!("generated body {name} has no recorded material"))
            })?;
            let def = look::definition(body.material)?;
            (look::surface(&def)?, look::detail(&def))
        } else {
            // Saved bodies' detail does not depend on the fabrication.
            (
                saved,
                crate::material::detail(kind, &pcbcraft_core::Fabrication::default()),
            )
        };
        let t = body.source_transform;
        let moved = t[3];
        let translation = t[0] == [1.0, 0.0, 0.0, 0.0]
            && t[1] == [0.0, 1.0, 0.0, 0.0]
            && t[2] == [0.0, 0.0, 1.0, 0.0]
            && moved[3] == 1.0;
        let key = (body.component, name.to_owned());
        // A placement of a definition (every via of a size) shares its definition's mesh and
        // says where it lies, instead of holding a moved copy of it.
        let (mesh, placed) = body.mesh_in_place();
        let place =
            (placed != solidcraft_engine::doc::IDENTITY).then(|| Place::from_columns(&placed));
        let (volume, min, max) = match measured.get(&key).filter(|_| translation) {
            Some((volume, min, max, at)) => {
                let shift = |p: &[f64; 3]| std::array::from_fn(|i| p[i] + moved[i] - at[i]);
                (volume.clone(), shift(min), shift(max))
            }
            None => {
                // A placement's box without making its moved mesh.
                let b = body.mesh_bounds();
                let outline = body.body.outline_bounds();
                let span = b.min.dist(b.max).max(1.0);
                let near = |p: crate::geom::Vec3, q: crate::geom::Vec3| p.dist(q) <= span * 1e-12;
                // A box the mesh reaches exactly: the body's vertices and edges' controls
                // span no more than the mesh does (a laminate slab, a pad, a track),
                // so that box is the body's own. Its volume (meshing the body more
                // finely three times) is measured only if asked for; any other body
                // is measured now.
                let (volume, min, max) = if kind == BodyKind::Substrate
                    || (near(b.min, outline.min) && near(b.max, outline.max))
                {
                    let body = body.clone();
                    (
                        Volume::lazy(move || body.measure().map_or(f64::NAN, |m| m.volume)),
                        [b.min.x, b.min.y, b.min.z],
                        [b.max.x, b.max.y, b.max.z],
                    )
                } else {
                    let m = body
                        .measure()
                        .map_err(|e| Error::Verification(e.to_string()))?;
                    (
                        Volume::known(m.volume),
                        [m.bbox.min.x, m.bbox.min.y, m.bbox.min.z],
                        [m.bbox.max.x, m.bbox.max.y, m.bbox.max.z],
                    )
                };
                if translation {
                    measured.insert(
                        key,
                        (volume.clone(), min, max, [moved[0], moved[1], moved[2]]),
                    );
                }
                (volume, min, max)
            }
        };
        geometry.push(Geometry {
            item,
            kind,
            layer: layer(kind),
            surface,
            detail,
            mesh,
            place,
            volume,
            min,
            max,
        });
    }
    // A definition's own body (the first via of a size, where its placements
    // are copied from) shares its mesh with them: place it too, so it is
    // drawn with them.
    let shared: std::collections::HashSet<usize> = geometry
        .iter()
        .filter(|g| g.place.is_some())
        .map(|g| std::sync::Arc::as_ptr(&g.mesh) as usize)
        .collect();
    for g in &mut geometry {
        if g.place.is_none() && shared.contains(&(std::sync::Arc::as_ptr(&g.mesh) as usize)) {
            g.place = Some(Place::IDENTITY);
        }
    }
    Ok(NativeModel {
        native,
        geometry,
        sources,
        models,
        libraries: Vec::new(),
        reuse,
        updated: None,
        display: appearance::Memo::default(),
    })
}

/// `work` on each item, on every core (one by one on the web).
fn on_threads<T: Sync>(items: &[T], work: impl Fn(&T) + Sync) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..threads.min(items.len()) {
                scope.spawn(|| {
                    while let Some(item) =
                        items.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                    {
                        work(item);
                    }
                });
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    items.iter().for_each(work);
}

/// The copper layer of one body of a source on `layer`: a through-hole
/// pad's opposite land lies on the other side, and its plated barrels on
/// no single layer (they span the board, like a via's).
fn body_layer(name: &str, layer: CopperLayer) -> Option<CopperLayer> {
    if name.contains("_barrel_") {
        None
    } else if name.ends_with("_opposite") {
        layer.side().map(|side| other(side).into())
    } else {
        Some(layer)
    }
}

/// The joint origin that holds a generated definition (drawn in its model
/// frame) so the footprint frame lands on the board: the footprint frame
/// expressed in model coordinates, the inverse of `frame` (model →
/// footprint).
fn holding_frame(frame: &Transform3) -> JointFrame {
    let r = frame.rotation;
    let back = |v: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|i| r[0][i] * v[0] + r[1][i] * v[1] + r[2][i] * v[2])
    };
    let o = back(frame.translation);
    JointFrame {
        point: [-o[0], -o[1], -o[2]],
        z: back([0.0, 0.0, 1.0]),
        x: back([1.0, 0.0, 0.0]),
    }
}

/// The model in the frame a board joint holds: the package datum frame.
/// The board places the footprint's datum (`layout::placement_transform`
/// undoes it), so `model_frame(datum, placement)` lands, footprint frame
/// first, at `placement`'s datum-frame pose; the 2D drawing and checks use
/// `model_frame` in the footprint frame.
fn datum_frame(package: &PackageDefinition) -> Transform3 {
    package.placement.transform()
}

/// The display-only terminations, overcoat and fillets draw in the package
/// frame shifted by the placement: right while the placement does not turn.
fn package_frame(frame: &Transform3) -> bool {
    frame.rotation == Transform3::IDENTITY.rotation
}

fn next_pick(sources: &[Source]) -> u64 {
    u64::try_from(sources.len()).map_or(u64::MAX, |n| n.saturating_add(1))
}

/// Most drilled holes (pad holes and pad vias) on one board.
pub const MAX_DRILLS: usize = 65_536;

/// One placed pad and the frames its contours are drawn in.
struct PlacedPad<'a> {
    placed: layout::PlacedPad,
    placement: &'a Placement,
    designator: &'a str,
    /// The footprint's mask and paste defaults.
    defaults: PadDefaults,
    /// The pad origin on the board.
    origin: Point,
}

impl PlacedPad<'_> {
    fn pad(&self) -> &Pad {
        &self.placed.pad
    }

    /// Pad frame → footprint frame, as a vector (no translation).
    fn turn(&self, v: Point) -> Point {
        let (c, s) = cos_sin(self.pad().rotation_deg);
        [c * v[0] - s * v[1], s * v[0] + c * v[1]]
    }

    /// A pad-frame contour on the board.
    fn board(&self, contour: &Contour) -> Contour {
        let t = self.placed.transform;
        let position = self.pad().position;
        contour.mapped(
            |q| {
                let v = self.turn(q);
                t.apply([position[0] + v[0], position[1] + v[1]])
            },
            t.mirror,
        )
    }

    /// A pad-frame contour in the pad component's frame: the component is
    /// jointed to its layer's board face at [`Self::origin`], turned by the
    /// placement angle (and turned over on the bottom face).
    fn local(&self, contour: &Contour) -> Contour {
        let t = self.placed.transform;
        let bottom = self.placed.layer == Layer::Bottom;
        let (c, s) = cos_sin(-self.placement.rotation_deg);
        contour.mapped(
            |q| {
                let b = t.apply_vector(self.turn(q));
                let r = [c * b[0] - s * b[1], s * b[0] + c * b[1]];
                if bottom { [r[0], -r[1]] } else { r }
            },
            t.mirror != bottom,
        )
    }

    /// The pad origin relative to the placement, in the placement's turned
    /// frame (what the joint's expressions rotate by the placement angle).
    fn offset(&self) -> Point {
        let p = self.placement.position;
        let d = [self.origin[0] - p[0], self.origin[1] - p[1]];
        let (c, s) = cos_sin(-self.placement.rotation_deg);
        [c * d[0] - s * d[1], s * d[0] + c * d[1]]
    }

    /// Drilled holes, pad frame: the pad's hole and its vias.
    fn drills(&self) -> Vec<Contour> {
        let pad = self.pad();
        let mut out: Vec<Contour> = pad
            .hole
            .iter()
            .filter_map(|h| pad_outline::hole_contour(h, 0.0))
            .collect();
        out.extend(pad.vias.iter().map(|v| Contour::Circle {
            center: v.at,
            radius: v.drill / 2.0,
        }));
        out
    }

    /// Finished bores, pad frame: drills less their plating.
    fn bores(&self) -> Vec<Contour> {
        let pad = self.pad();
        let mut out: Vec<Contour> = pad.hole.iter().filter_map(pad_outline::bore).collect();
        out.extend(pad.vias.iter().filter_map(|v| {
            let radius = v.drill / 2.0 - PLATING_MM;
            (radius > 0.0).then_some(Contour::Circle {
                center: v.at,
                radius,
            })
        }));
        out
    }

    /// The plated barrels (drill outside, bore inside), pad frame.
    fn barrels(&self) -> Vec<[Contour; 2]> {
        let pad = self.pad();
        let mut out = Vec::new();
        if let Some(hole) = pad.hole.filter(|h| h.plated)
            && let (Some(drill), Some(bore)) = (
                pad_outline::hole_contour(&hole, 0.0),
                pad_outline::bore(&hole),
            )
        {
            out.push([drill, bore]);
        }
        for via in &pad.vias {
            let radius = via.drill / 2.0;
            if radius - PLATING_MM > 0.0 {
                out.push([
                    Contour::Circle {
                        center: via.at,
                        radius,
                    },
                    Contour::Circle {
                        center: via.at,
                        radius: radius - PLATING_MM,
                    },
                ]);
            }
        }
        out
    }
}

/// Every footprint pad of every placement, with or without a net or
/// terminal: they are all copper (or holes) on the board.
fn placed_pads(project: &Project) -> Result<Vec<PlacedPad<'_>>> {
    let placed = layout::placed_pads(project);
    if placed.len() > layout::MAX_PLACED_PADS {
        return Err(Error::Invalid(format!(
            "the board has more than {} pads",
            layout::MAX_PLACED_PADS
        )));
    }
    let drills: usize = placed
        .iter()
        .map(|p| usize::from(p.pad.hole.is_some()) + p.pad.vias.len())
        .sum();
    if drills > MAX_DRILLS {
        return Err(Error::Invalid(format!(
            "the board has more than {MAX_DRILLS} drilled holes"
        )));
    }
    placed
        .into_iter()
        .map(|placed| {
            let placement = project
                .placements
                .iter()
                .find(|p| p.instance == placed.instance)
                .ok_or_else(|| Error::Invalid("missing pad placement".into()))?;
            let instance = project
                .instance(placed.instance)
                .ok_or_else(|| Error::Invalid("missing pad instance".into()))?;
            let defaults = instances::resolve(&project.library, instance)
                .ok()
                .and_then(|r| r.physical)
                .map(|p| p.footprint.definition.defaults)
                .ok_or_else(|| Error::Invalid("missing pad definition".into()))?;
            let designator = instance.designator.as_str();
            let origin = placed.transform.apply(placed.pad.position);
            Ok(PlacedPad {
                placed,
                placement,
                designator,
                defaults,
                origin,
            })
        })
        .collect()
}

/// The layer on the other side of the board.
fn other(layer: Layer) -> Layer {
    match layer {
        Layer::Top => Layer::Bottom,
        Layer::Bottom => Layer::Top,
    }
}

/// A rectangular land without holes whose sides follow the component's
/// axes: drawn as a box with editable size parameters. The flag says its
/// length runs along the component's y axis.
fn axis_box(pad: &PlacedPad<'_>) -> Option<bool> {
    let p = pad.pad();
    if !matches!(p.shape, PadShape::Rect { .. }) || p.hole.is_some() || !p.vias.is_empty() {
        return None;
    }
    let axis = Contour::Circle {
        center: [1.0, 0.0],
        radius: 1.0,
    };
    let Contour::Circle { center: c, .. } = pad.local(&axis) else {
        return None;
    };
    if c[1].abs() < 1e-12 {
        Some(false)
    } else if c[0].abs() < 1e-12 {
        Some(true)
    } else {
        None
    }
}

fn flat(contour: &Contour) -> Result<Vec<Point>> {
    contour.flatten(FLATTEN_TOLERANCE_MM)
}

/// The parameter holding a copper layer's foil thickness. The top's is
/// `copper_thickness`; every other foil as thick follows it.
fn copper_param(layer: CopperLayer) -> String {
    match layer {
        CopperLayer::Top => "copper_thickness".into(),
        CopperLayer::Bottom => "copper_bottom_thickness".into(),
        CopperLayer::Inner(n) => format!("copper_inner_{n}_thickness"),
    }
}

/// The parameter holding `dielectrics[index]`'s thickness (from 1, top
/// down).
fn dielectric_param(index: usize) -> String {
    format!("dielectric_{}_thickness", index + 1)
}

/// The board height (an expression) of an inner layer's lower face: the
/// dielectrics below it, less its foil (inner foils lie at the top of the
/// dielectric below them, as `Stackup::copper_span` places them).
fn inner_base(stackup: &Stackup, layer: CopperLayer) -> Result<String> {
    let index = stackup
        .index(layer)
        .ok_or_else(|| Error::Invalid(format!("the stackup has no {} layer", layer.name())))?;
    let below: Vec<String> = (index..stackup.dielectrics.len())
        .map(dielectric_param)
        .collect();
    Ok(format!("{} - {}", below.join(" + "), copper_param(layer)))
}

/// The stackup's parameters: each dielectric and foil thickness, the board
/// (laminate) thickness as their sum, and the board outline.
fn stackup_params(b: &mut Builder, project: &Project) -> Result<()> {
    let stackup = &project.stackup;
    stackup.check().map_err(|e| Error::Invalid(e.to_string()))?;
    b.param("board_width", project.board_size[0], "mm")?;
    b.param("board_depth", project.board_size[1], "mm")?;
    if let [only] = stackup.dielectrics.as_slice() {
        b.param("board_thickness", only.thickness_mm, "mm")?;
    } else {
        let mut sum = Vec::new();
        for (index, dielectric) in stackup.dielectrics.iter().enumerate() {
            b.param(&dielectric_param(index), dielectric.thickness_mm, "mm")?;
            sum.push(dielectric_param(index));
        }
        b.native.parameter("board_thickness", sum.join(" + "))?;
    }
    let layers = stackup.layers();
    let top = stackup
        .copper
        .first()
        .map(|c| c.thickness_um)
        .ok_or_else(|| Error::Invalid("the stackup has no copper".into()))?;
    for (layer, foil) in layers.iter().zip(&stackup.copper) {
        let name = copper_param(*layer);
        if *layer == CopperLayer::Top {
            b.param(&name, top / 1000.0, "mm")?;
        } else if foil.thickness_um == top {
            b.native.parameter(&name, "copper_thickness")?;
        } else {
            b.param(&name, foil.thickness_um / 1000.0, "mm")?;
        }
    }
    Ok(())
}

/// A closed polyline as a sketch contour.
fn polygon(points: &[Point]) -> Option<Contour> {
    use pcbcraft_core::library::{Outline, Segment};
    let (first, rest) = points.split_first()?;
    let mut segments: Vec<Segment> = rest.iter().map(|&to| Segment::Line { to }).collect();
    segments.push(Segment::Line { to: *first });
    Some(Contour::Outline(Outline {
        start: *first,
        segments,
    }))
}

/// Points sampled on each circle's boundary when testing whether it lies
/// inside another hole (sagitta below 0.1 µm at a 1 mm radius).
const NESTED_SAMPLES: usize = 128;

/// A hole's boundary: a circle, or an outline's vertices (arcs flattened).
#[derive(Clone)]
enum HoleShape {
    Circle(Point, f64),
    Ring(Vec<Point>),
}

impl HoleShape {
    fn of(contour: &Contour) -> Self {
        match contour {
            Contour::Circle { center, radius } => Self::Circle(*center, *radius),
            Contour::Outline(outline) => Self::Ring(outline.flatten(FLATTEN_TOLERANCE_MM)),
        }
    }

    /// The box round the hole: low corner, high corner.
    fn bounds(&self) -> (Point, Point) {
        match self {
            Self::Circle(c, r) => ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r]),
            Self::Ring(points) => {
                points
                    .iter()
                    .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| {
                        (
                            [lo[0].min(p[0]), lo[1].min(p[1])],
                            [hi[0].max(p[0]), hi[1].max(p[1])],
                        )
                    })
            }
        }
    }

    fn boundary(&self) -> Vec<Point> {
        match self {
            Self::Circle(c, r) => (0..NESTED_SAMPLES)
                .map(|k| {
                    let a = std::f64::consts::TAU * k as f64 / NESTED_SAMPLES as f64;
                    [c[0] + r * a.cos(), c[1] + r * a.sin()]
                })
                .collect(),
            Self::Ring(points) => points.clone(),
        }
    }

    /// Whether `p` lies inside the hole or on its edge.
    fn holds(&self, p: Point) -> bool {
        const ON: f64 = 1e-9;
        match self {
            Self::Circle(c, r) => (p[0] - c[0]).hypot(p[1] - c[1]) <= r + ON,
            Self::Ring(ring) => {
                let n = ring.len();
                let mut inside = false;
                for i in 0..n {
                    let (a, b) = (ring[i], ring[(i + 1) % n]);
                    let along = [b[0] - a[0], b[1] - a[1]];
                    let length = along[0].hypot(along[1]);
                    if length > 0.0 {
                        let t = (((p[0] - a[0]) * along[0] + (p[1] - a[1]) * along[1])
                            / (length * length))
                            .clamp(0.0, 1.0);
                        let q = [a[0] + t * along[0], a[1] + t * along[1]];
                        if (p[0] - q[0]).hypot(p[1] - q[1]) <= ON {
                            return true;
                        }
                    }
                    if (a[1] > p[1]) != (b[1] > p[1])
                        && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * along[0]
                    {
                        inside = !inside;
                    }
                }
                inside
            }
        }
    }
}

/// `holes` without each one lying wholly inside another (of two alike,
/// the first stays). A sketch takes a loop inside a loop as an island, so
/// a via ring under a wide track's copper would leave laminate standing in
/// the track's cut. Holes are looked up by a 1 mm grid of their boxes.
fn without_nested(holes: Vec<Contour>) -> Vec<Contour> {
    const CELL_MM: f64 = 1.0;
    const MAX_CELLS: i64 = 64;
    let shapes: Vec<HoleShape> = holes.iter().map(HoleShape::of).collect();
    let bounds: Vec<(Point, Point)> = shapes.iter().map(HoleShape::bounds).collect();
    let cell = |v: f64| (v / CELL_MM).floor() as i64;
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    let mut large = Vec::new();
    for (index, (lo, hi)) in bounds.iter().enumerate() {
        let (x0, y0, x1, y1) = (cell(lo[0]), cell(lo[1]), cell(hi[0]), cell(hi[1]));
        if (x1 - x0 + 1).saturating_mul(y1 - y0 + 1) > MAX_CELLS {
            large.push(index);
            continue;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                grid.entry((x, y)).or_default().push(index);
            }
        }
    }
    let inside = |a: usize, b: usize| {
        let ((alo, ahi), (blo, bhi)) = (bounds[a], bounds[b]);
        blo[0] <= alo[0] + 1e-9
            && blo[1] <= alo[1] + 1e-9
            && bhi[0] >= ahi[0] - 1e-9
            && bhi[1] >= ahi[1] - 1e-9
            && shapes[a].boundary().iter().all(|p| shapes[b].holds(*p))
    };
    let mut kept = Vec::with_capacity(holes.len());
    for (a, hole) in holes.into_iter().enumerate() {
        let (lo, hi) = bounds[a];
        let centre = (cell((lo[0] + hi[0]) / 2.0), cell((lo[1] + hi[1]) / 2.0));
        let nested = grid
            .get(&centre)
            .into_iter()
            .flatten()
            .chain(&large)
            .any(|&b| b != a && inside(a, b) && (b < a || !inside(b, a)));
        if !nested {
            kept.push(hole);
        }
    }
    kept
}

/// A track's copper stadium ([`layout::track_shape`]): round end caps of
/// `TRACK_CAP_SEGMENTS` chords each, vertices on the true arc,
/// counter-clockwise. The track body and an inner layer's laminate cut
/// share it, so the two meet exactly.
fn track_outline(start: Point, end: Point, width: f64) -> Vec<Point> {
    let d = [end[0] - start[0], end[1] - start[1]];
    let length = d[0].hypot(d[1]);
    if length <= 0.0 {
        return Vec::new();
    }
    let r = width / 2.0;
    let u = [d[0] / length, d[1] / length];
    let mut out = Vec::with_capacity(2 * TRACK_CAP_SEGMENTS + 2);
    // Each cap turns half a circle about its end, from the right side of
    // the direction it faces to the left.
    for (center, toward) in [(end, u), (start, [-u[0], -u[1]])] {
        let side = [-toward[1], toward[0]];
        for k in 0..=TRACK_CAP_SEGMENTS {
            let a = std::f64::consts::PI * (k as f64 / TRACK_CAP_SEGMENTS as f64 - 0.5);
            let (s, c) = a.sin_cos();
            out.push([
                center[0] + (toward[0] * c + side[0] * s) * r,
                center[1] + (toward[1] * c + side[1] * s) * r,
            ]);
        }
    }
    out
}

pub fn build(project: &Project) -> Result<NativeModel> {
    build_with(project, &SourceCache::default(), None)
}

/// [`build`] that stops with [`Error::Cancelled`] at its next native
/// command once `cancel` is set (a newer board replaced this one).
pub fn build_cancellable(
    project: &Project,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<NativeModel> {
    build_with(project, &SourceCache::default(), Some(cancel))
}

/// [`build`] reading library model files through `sources`, which the
/// caller keeps between builds (the model worker), so an unchanged model is
/// neither re-hashed nor re-read (entries this build did not use are
/// dropped), and stopping with [`Error::Cancelled`] at its next native
/// command once `cancel` is set.
pub fn build_with(
    project: &Project,
    sources: &SourceCache,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<NativeModel> {
    sources.begin();
    let built = Builder::new(&project.name, sources).and_then(|mut b| {
        if let Some(cancel) = cancel {
            b.native.cancel_with(cancel);
        }
        build_on(b, project)
    });
    sources.sweep();
    built
}

/// The model of `project`, made from `previous` (a model of an earlier
/// state of the same board) when the board changed only in values: the
/// previous design is updated in place ([`Native::replaying`]: changed
/// parameters, redrawn sketches, moved via placements), its evaluation
/// keeps every feature the edit did not reach, and every body it did not
/// change keeps its mesh. Otherwise the board is built anew. The result is
/// the model [`build`] makes of `project`. An update (comparing, then
/// changing a few features) always finishes; a build anew stops with
/// [`Error::Cancelled`] at its next native command once `cancel` is set.
pub fn update(
    previous: NativeModel,
    project: &Project,
    sources: &SourceCache,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(NativeModel, Rebuilt)> {
    let NativeModel {
        native,
        geometry,
        models,
        libraries,
        reuse,
        display,
        ..
    } = previous;
    let mut why = None;
    if let Some(native) = native.replaying() {
        let b = Builder::replaying(
            native,
            models,
            libraries,
            Previous::of(geometry, reuse),
            display,
            sources,
        );
        sources.begin();
        let updated = build_on(b, project);
        sources.sweep();
        match updated {
            Ok(model) => {
                let ran = model.updated.unwrap_or(0);
                return Ok((model, Rebuilt::InPlace(ran)));
            }
            // Refused, it would be refused anew.
            Err(error @ Error::Invalid(_)) => return Err(error),
            // Changed in more than values, or the update failed.
            Err(error) => why = Some(error.to_string()),
        }
    }
    Ok((build_with(project, sources, cancel)?, Rebuilt::Anew(why)))
}

/// Build `project` with `b`: a new design, or one replayed.
fn build_on(mut b: Builder, project: &Project) -> Result<NativeModel> {
    pcbcraft_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    if project
        .vias
        .iter()
        .any(|v| !matches!(v.span, ViaSpan::Through))
    {
        return Err(Error::Invalid(
            "blind and buried vias are not built in the native model yet".into(),
        ));
    }
    let stackup = &project.stackup;
    // The board is evaluated once its parts are in, not after every command.
    b.native.defer_evaluation()?;
    stackup_params(&mut b, project)?;
    let pads = placed_pads(project)?;
    b.params(&object_params(project, &pads)?)?;
    // What an edit rarely changes comes first in the design's history, so
    // evaluating an edit reuses it: the generated and library package
    // models and the via definitions, then the parts and copper. The
    // laminate comes last (see there).
    for placement in &project.placements {
        let Some(physical) = project
            .instance(placement.instance)
            .and_then(|instance| instances::resolve(&project.library, instance).ok())
            .and_then(|r| r.physical)
        else {
            continue;
        };
        let package = &physical.package.definition;
        let stem = models::definition_name(physical.package.id, physical.package.revision.0);
        match &package.model {
            PackageModel::Library(use_) => {
                b.define_library(&stem, library_model(project, use_)?)?
            }
            PackageModel::PartFile(use_) => {
                b.define_part(&stem, use_, part_bytes(project, use_)?)?;
            }
            _ if models::is_generated(package) => b.define(&stem, package)?,
            _ => {}
        }
    }
    let mut styles = via_styles(&mut b, project)?;
    let substrate = b.native.component("PCB substrate", true)?;
    let board = substrate.occurrence;
    let mut sources = vec![];
    let mut parts = vec![];
    // Board joints of every part, made at the end (see there).
    let mut joints: Vec<(BoardJoint, JointFrame, Option<String>)> = vec![];
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let physical = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
            .ok_or_else(|| {
                Error::Invalid("placed instance has no physical implementation".into())
            })?;
        let package = &physical.package.definition;
        let footprint = &physical.footprint.definition;
        let prefix = format!("instance_{}", instance.id);
        let library = match &package.model {
            PackageModel::Library(use_) => Some(library_model(project, use_)?),
            _ => None,
        };
        let part = match &package.model {
            PackageModel::PartFile(use_) => {
                let stem =
                    models::definition_name(physical.package.id, physical.package.revision.0);
                Some(b.define_part(&stem, use_, part_bytes(project, use_)?)?)
            }
            _ => None,
        };
        if library.is_some() || part.is_some() || models::is_generated(package) {
            let stem = models::definition_name(physical.package.id, physical.package.revision.0);
            let generated = library.is_none() && part.is_none();
            let created = match (library, part) {
                (Some(library), _) => b.library_model(&stem, library)?,
                (None, Some(part)) => b.model_occurrence(&part)?,
                (None, None) => b.model(&stem, package)?,
            };
            let frame = datum_frame(package);
            let pick = next_pick(&sources);
            // Display-only solder fillets of a chip, while its placement
            // does not turn it out of the package frame they draw in.
            if generated && package.chip_kind().is_some() && package_frame(&frame) {
                parts.push(Part {
                    pick: Some(pick),
                    package,
                    footprint: Some(footprint),
                    body: Body::of(package),
                    placement: Some(placement),
                    generated: true,
                });
            }
            joints.push((
                BoardJoint {
                    board,
                    occurrence: created.occurrence,
                    x: format!("{prefix}_x").into(),
                    y: format!("{prefix}_y").into(),
                    angle: format!("{prefix}_angle").into(),
                    mount: placement.layer.into(),
                },
                holding_frame(&frame),
                Some(copper_param(placement.layer.into())),
            ));
            sources.push(Source {
                pick_id: pick,
                instance: Some(instance.id),
                pad: None,
                track: None,
                via: None,
                fill: None,
                net: None,
                layer: Some(placement.layer.into()),
                label: format!("{} model", instance.designator),
                component: created.component,
                occurrence: Some(created.occurrence),
            });
            continue;
        }
        let label = format!("{} envelope", instance.designator);
        let created = b.native.component(&label, false)?;
        let body = Body::of(package);
        // The box in its model frame (z = 0 the seating plane), held at
        // its placement in the datum frame as a generated model is.
        b.body(
            body.kind(),
            BoxBody {
                name: &label,
                corner: [
                    format!("- {prefix}_length / 2").into(),
                    format!("- {prefix}_width / 2").into(),
                    0.into(),
                ],
                size: [
                    format!("{prefix}_length").into(),
                    format!("{prefix}_width").into(),
                    format!("{prefix}_height").into(),
                ],
            },
        )?;
        let frame = datum_frame(package);
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: format!("{prefix}_x").into(),
                y: format!("{prefix}_y").into(),
                angle: format!("{prefix}_angle").into(),
                mount: placement.layer.into(),
            },
            holding_frame(&frame),
            Some(copper_param(placement.layer.into())),
        ));
        let pick = next_pick(&sources);
        // The display-only terminations, overcoat and fillets are drawn in
        // the package frame: only while the model frame is that frame.
        if package_frame(&frame) {
            parts.push(Part {
                pick: Some(pick),
                package,
                footprint: Some(footprint),
                body,
                placement: Some(placement),
                generated: false,
            });
        }
        sources.push(Source {
            pick_id: pick,
            instance: Some(instance.id),
            pad: None,
            track: None,
            via: None,
            fill: None,
            net: None,
            layer: Some(placement.layer.into()),
            label,
            component: created.component,
            occurrence: None,
        });
    }
    let mut lands = Vec::with_capacity(pads.len());
    for pad in &pads {
        let local = pad.pad();
        let pick = next_pick(&sources);
        let copper = pad_outline::copper(local);
        let layers: Vec<Layer> = match copper {
            Copper::None => vec![],
            Copper::OneSide => vec![pad.placed.layer],
            Copper::Through => vec![pad.placed.layer, other(pad.placed.layer)],
        };
        // Display: copper, bores and mask openings per outer layer.
        let shape = pad_outline::shape_contour(&local.shape, 0.0);
        let mask = pad_outline::mask_opening(local, &pad.defaults);
        let drills: Vec<Vec<Point>> = pad
            .drills()
            .iter()
            .map(|d| flat(&pad.board(d)))
            .collect::<Result<_>>()?;
        let bores = pad.bores();
        let opening_layers = if drills.is_empty() && copper != Copper::None {
            layers.clone()
        } else {
            vec![Layer::Top, Layer::Bottom]
        };
        for layer in opening_layers {
            let has_copper = layers.contains(&layer);
            let mut openings = drills.clone();
            if (has_copper || copper == Copper::None)
                && let Some(mask) = &mask
            {
                openings.push(flat(&pad.board(mask))?);
            }
            let (copper_outline, bore) = match (&shape, has_copper) {
                (Some(shape), true) => (
                    Some(flat(&pad.board(shape))?),
                    match bores.first() {
                        Some(bore) if local.hole.is_some() => Some(flat(&pad.board(bore))?),
                        _ => None,
                    },
                ),
                _ => (None, None),
            };
            lands.push(Land {
                pick: has_copper.then_some(pick),
                layer,
                copper: copper_outline,
                bore,
                openings,
            });
        }
        if copper == Copper::None {
            continue;
        }
        let Some(shape) = shape else {
            return Err(Error::Invalid(format!(
                "{} pad {} has no copper outline",
                pad.designator, local.id
            )));
        };
        let prefix = format!("instance_{}", pad.placed.instance);
        let pp = format!("pad_{}_{}", pad.placed.instance, local.id);
        let created = b.native.component(&pp, false)?;
        let near_copper = copper_param(pad.placed.layer.into());
        let far_copper = copper_param(other(pad.placed.layer).into());
        let land = |b: &mut Builder, name: &str, offset: Expr, height: &str| -> Result<()> {
            let mut contours = vec![pad.local(&shape)];
            contours.extend(bores.iter().map(|c| pad.local(c)));
            b.native.profile_body(ProfileBody {
                name,
                offset,
                contours: &contours,
                height: height.into(),
            })?;
            b.record(name, BodyKind::PadCopper)
        };
        match axis_box(pad) {
            Some(along_y) => {
                let (x, y) = if along_y {
                    (format!("{pp}_width"), format!("{pp}_length"))
                } else {
                    (format!("{pp}_length"), format!("{pp}_width"))
                };
                b.body(
                    BodyKind::PadCopper,
                    BoxBody {
                        name: &pp,
                        corner: [
                            format!("-{x} / 2").into(),
                            format!("-{y} / 2").into(),
                            0.into(),
                        ],
                        size: [x.into(), y.into(), near_copper.as_str().into()],
                    },
                )?;
            }
            None => land(&mut b, &pp, 0.into(), &near_copper)?,
        }
        if copper == Copper::Through {
            land(
                &mut b,
                &format!("{pp}_opposite"),
                format!("- board_thickness - {far_copper}").into(),
                &far_copper,
            )?;
        }
        for (index, [drill, bore]) in pad.barrels().iter().enumerate() {
            let name = format!("{pp}_barrel_{index}");
            b.native.profile_body(ProfileBody {
                name: &name,
                offset: "- board_thickness".into(),
                contours: &[pad.local(drill), pad.local(bore)],
                height: "board_thickness".into(),
            })?;
            b.record(&name, BodyKind::PadCopper)?;
        }
        // Rounded to a nanometre, so moving a part (whose offsets carry
        // rounding of its position) leaves the joint as it was.
        let [dx, dy] = pad.offset().map(|v| native::mm((v * 1e9).round() / 1e9));
        let x = format!("{prefix}_x + cos({prefix}_angle) * {dx} - sin({prefix}_angle) * {dy}");
        let y = format!("{prefix}_y + sin({prefix}_angle) * {dx} + cos({prefix}_angle) * {dy}");
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: x.into(),
                y: y.into(),
                angle: format!("{prefix}_angle").into(),
                mount: pad.placed.layer.into(),
            },
            JointFrame::ORIGIN,
            None,
        ));
        sources.push(Source {
            pick_id: pick,
            instance: Some(pad.placed.instance),
            pad: Some(local.id),
            track: None,
            via: None,
            fill: None,
            net: pad.placed.net,
            layer: Some(pad.placed.layer.into()),
            label: format!("{} pad {}", pad.designator, local.number),
            component: created.component,
            occurrence: None,
        });
    }
    let mut paths = Vec::with_capacity(project.tracks.len());
    for track in &project.tracks {
        let mount = match track.layer.side() {
            Some(side) => Mount::Side(side),
            None => Mount::Height(inner_base(stackup, track.layer)?.into()),
        };
        let prefix = format!("trace_{}", track.id);
        let created = b.native.component(&prefix, false)?;
        let angle = format!("atan2({prefix}_y1-{prefix}_y0; {prefix}_x1-{prefix}_x0)");
        // The stadium in the track's frame: start at the origin, along +x.
        let length = (track.end[0] - track.start[0]).hypot(track.end[1] - track.start[1]);
        let contour = polygon(&track_outline([0.0, 0.0], [length, 0.0], track.width))
            .ok_or_else(|| Error::Invalid(format!("track {} has no length", track.id)))?;
        b.native.whole_profile_body(ProfileBody {
            name: &prefix,
            offset: 0.into(),
            contours: &[contour],
            height: copper_param(track.layer).into(),
        })?;
        b.record(&prefix, BodyKind::CopperTrack)?;
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: format!("{prefix}_x0").into(),
                y: format!("{prefix}_y0").into(),
                angle: angle.into(),
                mount,
            },
            JointFrame::ORIGIN,
            None,
        ));
        let pick = next_pick(&sources);
        // Inner copper is buried: no mask cap or finish over it.
        if let Some(side) = track.layer.side() {
            paths.push(Path {
                pick,
                start: track.start,
                end: track.end,
                width: track.width,
                layer: side,
            });
        }
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: Some(track.id),
            via: None,
            fill: None,
            net: Some(track.net),
            layer: Some(track.layer),
            label: format!("Trace {}", track.id),
            component: created.component,
            occurrence: None,
        });
    }
    let display = appearance::Board::of(project);
    let marks = vias(&mut b, project, &mut styles, &mut sources)?;
    let fills = fill::build(&mut b, project, board, &mut sources, &mut joints)?;
    for mark in &marks {
        lands.extend(mark.lands(&display));
    }
    let picks: BTreeMap<Id, u64> = sources
        .iter()
        .filter(|s| s.pad.is_none())
        .filter_map(|s| s.instance.map(|i| (i, s.pick_id)))
        .collect();
    // The display-only layers (mask, finish, silkscreen, edge) need no native
    // model: they are drawn while the native one gets its laminate and joints
    // and is finished and meshed.
    let mut memo = std::mem::take(&mut b.display);
    let layers = |memo: &mut appearance::Memo| -> Result<Vec<Geometry>> {
        let mut layers = appearance::board_with(&display, &lands, &paths, &marks, &parts, memo)?;
        layers.extend(fill::films(&display, &lands, &marks, &fills)?);
        layers.extend(crate::silkscreen::films(
            project,
            &crate::silkscreen::Legend {
                board: &display,
                lands: &lands,
                paths: &paths,
                vias: &marks,
                picks: &picks,
            },
        )?);
        layers.extend(crate::stackup_edge::bands(stackup, &display)?);
        Ok(layers)
    };
    let native = |mut b: Builder| -> Result<NativeModel> {
        // The laminate last: its sketches hold every drill, which each
        // command after it would otherwise read again.
        b.native.activate(substrate.component)?;
        laminate(&mut b, project, &pads)?;
        // Every joint goes in once the parts are built, solved together (one
        // by one, each would solve all the joints made before it); a copy of
        // a shared definition starts where its first occurrence stands.
        b.native.board_joints(&joints)?;
        b.finish(sources)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (model, layers) = std::thread::scope(|scope| {
        let drawn = scope.spawn(|| layers(&mut memo));
        let model = native(b);
        let layers = drawn.join().unwrap_or_else(|_| {
            Err(Error::Verification(
                "drawing the board's layers failed".into(),
            ))
        });
        (model, layers)
    });
    #[cfg(target_arch = "wasm32")]
    let (model, layers) = (native(b), layers(&mut memo));
    let mut model = model?;
    model.geometry.extend(layers?);
    model.display = memo;
    Ok(model)
}

/// One via definition per drill and diameter: its plated barrel through
/// the laminate and an annular ring on each copper layer, standing on the
/// board's bottom face at its origin. Every via of that size is an
/// occurrence of it, so a board of thousands of vias builds (and meshes)
/// each size of via once and places the rest.
struct ViaStyle {
    component: u64,
    /// Where the definition's first occurrence stands; `None` while it is
    /// unplaced, with that occurrence.
    origin: Option<Point>,
    first: u64,
}

/// The definition of `via`'s size.
fn via_style(b: &mut Builder, project: &Project, via: &pcbcraft_core::Via) -> Result<ViaStyle> {
    let stackup = &project.stackup;
    let layers = stackup.layers();
    let name = format!("via_{}_{}", mm_slug(via.drill), mm_slug(via.diameter));
    let created = b.native.component(&name, false)?;
    let drill = via.drill / 2.0;
    let bore = drill - PLATING_MM;
    let ring = via.diameter / 2.0;
    let circle = |radius| Contour::Circle {
        center: [0.0, 0.0],
        radius,
    };
    if bore > 0.0 {
        let body = format!("{name}_barrel");
        b.native.profile_body(ProfileBody {
            name: &body,
            offset: 0.into(),
            contours: &[circle(drill), circle(bore)],
            height: "board_thickness".into(),
        })?;
        b.record(&body, BodyKind::ViaCopper)?;
    }
    // Through vias join every layer.
    for layer in layers.iter().filter(|l| via.on(**l, &layers)) {
        if bore <= 0.0 && layer.side().is_some() {
            continue;
        }
        let body = format!("{name}_ring_{}", layer_slug(*layer));
        let height = copper_param(*layer);
        // Outer rings cap the barrel; inner rings stand round it, in the
        // circle the laminate's foil level leaves for them.
        let (offset, contours): (Expr, [Contour; 2]) = match layer {
            CopperLayer::Top => ("board_thickness".into(), [circle(ring), circle(bore)]),
            CopperLayer::Bottom => (format!("- {height}").into(), [circle(ring), circle(bore)]),
            CopperLayer::Inner(_) => (
                inner_base(stackup, *layer)?.into(),
                [circle(ring), circle(drill)],
            ),
        };
        b.native.profile_body(ProfileBody {
            name: &body,
            offset,
            contours: &contours,
            height: height.as_str().into(),
        })?;
        b.record(&body, BodyKind::ViaCopper)?;
    }
    Ok(ViaStyle {
        component: created.component,
        origin: None,
        first: created.occurrence,
    })
}

/// A length in body and component names: `0.3` → `0p3`.
fn mm_slug(value: f64) -> String {
    format!("{value}").replace('.', "p").replace('-', "m")
}

/// One definition per via size ([`via_style`]), built before the laminate.
fn via_styles(b: &mut Builder, project: &Project) -> Result<BTreeMap<ViaKey, ViaStyle>> {
    let mut styles: BTreeMap<ViaKey, ViaStyle> = BTreeMap::new();
    for via in &project.vias {
        let key = (via.drill.to_bits(), via.diameter.to_bits());
        if let std::collections::btree_map::Entry::Vacant(slot) = styles.entry(key) {
            slot.insert(via_style(b, project, via)?);
        }
    }
    Ok(styles)
}

/// The published model a package's library model names, how it is used and
/// its packed asset bytes; refused, naming them, when the project does not
/// hold them.
pub fn library_model<'a>(
    project: &'a Project,
    use_: &'a LibraryModel,
) -> Result<(&'a LibraryModelDefinition, &'a LibraryModel, &'a [u8])> {
    let model = project.library.model(use_.model).ok_or_else(|| {
        Error::Invalid(format!(
            "model.asset: the library model {} is not in the project",
            use_.model
        ))
    })?;
    let definition = &model.definition;
    let asset = project
        .library
        .assets
        .iter()
        .find(|a| a.hash == definition.asset.hash)
        .ok_or_else(|| {
            Error::Invalid(format!(
                "model.asset: the packed asset {} ({}) is not in the project",
                definition.asset.file_name, definition.asset.hash
            ))
        })?;
    Ok((definition, use_, asset.data.as_slice()))
}

/// The packed bytes of a package's part file: its Model revision (format
/// `solidcraft`) and that model's asset in the project.
pub fn part_bytes<'a>(
    project: &'a Project,
    use_: &pcbcraft_core::library::package_model::PartFileModel,
) -> Result<&'a [u8]> {
    let model = project.library.model(use_.model).ok_or_else(|| {
        Error::Invalid(format!(
            "part_file.model: the part file {} is not in the project",
            use_.model
        ))
    })?;
    part_asset(project, &model.definition)
}

/// The packed asset of a part-file model definition.
pub fn part_asset<'a>(project: &'a Project, model: &LibraryModelDefinition) -> Result<&'a [u8]> {
    if model.format != pcbcraft_core::library::model3d::ModelFormat::Solidcraft {
        return Err(Error::Invalid(format!(
            "part_file.model: a part file is a SolidCraft design, not {}",
            model.format.label()
        )));
    }
    project
        .library
        .assets
        .iter()
        .find(|a| a.hash == model.asset.hash)
        .map(|a| a.data.as_slice())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "part_file.model: the packed part file {} ({}) is not in the project",
                model.asset.file_name, model.asset.hash
            ))
        })
}

/// Every board via as saved copper: an occurrence of its size's definition
/// (`styles`, from [`via_styles`]) moved to the via. Like the drills, a via
/// sits where it is now. Returns what the display layers draw of each.
fn vias(
    b: &mut Builder,
    project: &Project,
    styles: &mut BTreeMap<ViaKey, ViaStyle>,
    sources: &mut Vec<Source>,
) -> Result<Vec<ViaMark>> {
    // Each via's definition, and the copies each definition still needs:
    // (via index, offset from the definition's first occurrence).
    let mut style_of = Vec::with_capacity(project.vias.len());
    let mut copies: BTreeMap<ViaKey, Vec<(usize, [f64; 3])>> = BTreeMap::new();
    let mut occurrences: Vec<Option<u64>> = vec![None; project.vias.len()];
    for (index, via) in project.vias.iter().enumerate() {
        let key = (via.drill.to_bits(), via.diameter.to_bits());
        let style = styles
            .get_mut(&key)
            .ok_or_else(|| Error::Verification("via definition missing".into()))?;
        let [x, y] = via.position;
        match style.origin {
            None => {
                b.native.activate_root()?;
                b.native.move_occurrence(
                    style.first,
                    [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, 0.0]],
                )?;
                style.origin = Some(via.position);
                occurrences[index] = Some(style.first);
            }
            Some([x0, y0]) => copies
                .entry(key)
                .or_default()
                .push((index, [x - x0, y - y0, 0.0])),
        }
        style_of.push(style.component);
    }
    // Every further via of a size is placed in one command.
    for (key, wanted) in copies {
        let style = styles
            .get(&key)
            .ok_or_else(|| Error::Verification("via definition missing".into()))?;
        let offsets: Vec<[f64; 3]> = wanted.iter().map(|(_, t)| *t).collect();
        let placed = b.native.copy_occurrences(style.component, &offsets)?;
        if placed.len() != wanted.len() {
            return Err(Error::Verification("via copies went missing".into()));
        }
        for ((index, _), occurrence) in wanted.into_iter().zip(placed) {
            if let Some(slot) = occurrences.get_mut(index) {
                *slot = Some(occurrence);
            }
        }
    }
    let mut marks = Vec::with_capacity(project.vias.len());
    for ((via, component), occurrence) in project.vias.iter().zip(style_of).zip(occurrences) {
        let occurrence =
            occurrence.ok_or_else(|| Error::Verification(format!("via {} is unplaced", via.id)))?;
        let pick = next_pick(sources);
        sources.push(Source {
            pick_id: pick,
            instance: None,
            pad: None,
            track: None,
            via: Some(via.id),
            fill: None,
            net: Some(via.net),
            layer: None,
            label: format!("Via {}", via.id),
            component,
            occurrence: Some(occurrence),
        });
        marks.push(ViaMark {
            pick,
            at: via.position,
            drill: via.drill,
            diameter: via.diameter,
            tented: via.tented,
        });
    }
    Ok(marks)
}

/// A copper layer's name in body names.
fn layer_slug(layer: CopperLayer) -> String {
    match layer {
        CopperLayer::Top => "top".into(),
        CopperLayer::Bottom => "bottom".into(),
        CopperLayer::Inner(n) => format!("inner_{n}"),
    }
}

/// Most parameters per import.
const MAX_IMPORTED_PARAMS: usize = 8192;

/// Every placement's, pad's and track's own parameters, added together
/// before the objects that use them: each placement's position and angle
/// (and an envelope's size), an axis-aligned land's size, and each track's
/// ends and width.
fn object_params(
    project: &Project,
    pads: &[PlacedPad<'_>],
) -> Result<Vec<(String, f64, &'static str)>> {
    let mut out = Vec::new();
    for placement in &project.placements {
        let instance = project
            .instance(placement.instance)
            .ok_or_else(|| Error::Invalid("missing instance".into()))?;
        let physical = instances::resolve(&project.library, instance)
            .ok()
            .and_then(|r| r.physical)
            .ok_or_else(|| {
                Error::Invalid("placed instance has no physical implementation".into())
            })?;
        let prefix = format!("instance_{}", instance.id);
        out.push((format!("{prefix}_x"), placement.position[0], "mm"));
        out.push((format!("{prefix}_y"), placement.position[1], "mm"));
        out.push((format!("{prefix}_angle"), placement.rotation_deg, "deg"));
        let package = &physical.package.definition;
        if !models::is_generated(package) {
            for (axis, value) in ["length", "width", "height"]
                .into_iter()
                .zip(package.envelope)
            {
                out.push((format!("{prefix}_{axis}"), value, "mm"));
            }
        }
    }
    for pad in pads {
        let local = pad.pad();
        if pad_outline::copper(local) == Copper::None
            || pad_outline::shape_contour(&local.shape, 0.0).is_none()
            || axis_box(pad).is_none()
        {
            continue;
        }
        let pp = format!("pad_{}_{}", pad.placed.instance, local.id);
        let [length, width] = local.shape.extent();
        out.push((format!("{pp}_length"), length, "mm"));
        out.push((format!("{pp}_width"), width, "mm"));
    }
    for track in &project.tracks {
        let prefix = format!("trace_{}", track.id);
        for (suffix, value) in [
            ("x0", track.start[0]),
            ("y0", track.start[1]),
            ("x1", track.end[0]),
            ("y1", track.end[1]),
        ] {
            out.push((format!("{prefix}_{suffix}"), value, "mm"));
        }
    }
    Ok(out)
}

/// The sum of `dielectrics[range]`'s thickness parameters, `0` for none.
fn dielectric_sum(stackup: &Stackup, range: std::ops::Range<usize>) -> String {
    if let [_] = stackup.dielectrics.as_slice() {
        return if range.is_empty() {
            "0 mm".into()
        } else {
            "board_thickness".into()
        };
    }
    let terms: Vec<String> = range.map(dielectric_param).collect();
    if terms.is_empty() {
        "0 mm".into()
    } else {
        terms.join(" + ")
    }
}

/// The laminate as the stack it is made of: each dielectric a slab
/// extruded from the board outline with every drill as a hole, and where
/// an inner foil lies at the top of a dielectric, the foil's level a slab
/// of its own whose holes are also that layer's copper (tracks and via
/// rings, united by the sketch where they overlap; inner fills and the
/// copper they touch as one union, [`fill::foil_level`]). The saved bodies
/// are disjoint and the laminate's mass is physical, without a Boolean: a
/// board of thousands of vias is built in time linear in their number.
/// A rectangular board's outline follows `board_width` and `board_depth`;
/// a sketched board's is its solved outline, with its cutouts, slots and
/// mounting holes as holes of every slab ([`crate::board::laminate`]),
/// and it does not follow those parameters. The holes sit
/// where the board's copper is now (they do not follow placement
/// parameters edited in Solidcraft); the slabs' heights follow the stackup
/// parameters.
fn laminate(b: &mut Builder, project: &Project, pads: &[PlacedPad<'_>]) -> Result<()> {
    let stackup = &project.stackup;
    let pad_drills: Vec<Contour> = pads
        .iter()
        .flat_map(|pad| pad.drills().into_iter().map(|d| pad.board(&d)))
        .collect();
    if pad_drills.len().saturating_add(project.vias.len()) > MAX_DRILLS {
        return Err(Error::Invalid(format!(
            "the board has more than {MAX_DRILLS} drilled holes"
        )));
    }
    let via_drills = project.vias.iter().map(|via| Contour::Circle {
        center: via.position,
        radius: via.drill / 2.0,
    });
    // A rectangular board's outline is the board size parameters'
    // rectangle ([`Native::board_slab`]); a sketched board's is its solved
    // outline, and its cutouts, slots and mounting holes are holes of
    // every slab. The contours are the holes.
    let shaped = crate::board::laminate(project);
    let through: Vec<Contour> = shaped
        .as_ref()
        .map(|l| l.through.clone())
        .unwrap_or_default();
    let mut dielectric = pad_drills.clone();
    dielectric.extend(via_drills);
    dielectric.extend(through.iter().cloned());
    let dielectric = without_nested(dielectric);
    let layers = stackup.layers();
    let count = stackup.dielectrics.len();
    let mut slab = 0usize;
    let mut name = || {
        slab += 1;
        if slab == 1 {
            "Substrate".to_owned()
        } else {
            format!("Substrate {slab}")
        }
    };
    // What every slab's solid depends on besides its holes and heights.
    let outline = shaped.map(|l| l.outline);
    let base = digest(&(stackup, project.board_size, &outline));
    let extrude = |b: &mut Builder, body: ProfileBody<'_>| -> Result<()> {
        let inputs = digest(&(base, body.name, &body.offset, &body.height, body.contours));
        let name = body.name.to_owned();
        // A slab in pieces (laminate left in a fill's clearance) is one
        // body per piece, each a slab of these inputs.
        let made = match &outline {
            None => b.native.board_slab(body, BOARD_SIZE)?,
            Some(outline) => b.native.outlined_slab(body, outline)?,
        };
        if !made.contains(&name) {
            return Err(Error::Verification(format!("slab {name} made no body")));
        }
        for piece in made {
            b.slabs.insert(piece.clone(), inputs);
            b.record(&piece, BodyKind::Substrate)?;
        }
        Ok(())
    };
    for index in 0..count {
        let low = dielectric_sum(stackup, index + 1..count);
        let thickness = dielectric_sum(stackup, index..index + 1);
        // An inner foil at this dielectric's top (`inner_base`).
        let foil = layers
            .iter()
            .copied()
            .find(|l| l.side().is_none() && stackup.index(*l) == Some(index));
        let Some(layer) = foil else {
            let name = name();
            extrude(
                b,
                ProfileBody {
                    name: &name,
                    offset: low.as_str().into(),
                    contours: &dielectric,
                    height: thickness.as_str().into(),
                },
            )?;
            continue;
        };
        // The foil's level: its copper is cut out with the pad drills and
        // the board's cutouts (via drills lie inside their rings); with a
        // fill on it, the fill's copper too ([`fill::foil_level`]).
        let mut holes = pad_drills.clone();
        holes.extend(through.iter().cloned());
        let mut level = holes.clone();
        level.extend(
            project
                .tracks
                .iter()
                .filter(|t| t.layer == layer)
                .filter_map(|t| polygon(&track_outline(t.start, t.end, t.width))),
        );
        level.extend(
            project
                .vias
                .iter()
                .filter(|v| v.on(layer, &layers))
                .map(|v| Contour::Circle {
                    center: v.position,
                    radius: v.diameter / 2.0,
                }),
        );
        // A via ring or drill inside a wide track's copper is already cut
        // with it; drawn too, it would be an island of laminate.
        let level = match fill::foil_level(project, layer, &holes) {
            Some(level) => level,
            None => without_nested(level),
        };
        let foil_name = name();
        let foil_height = copper_param(layer);
        extrude(
            b,
            ProfileBody {
                name: &foil_name,
                offset: inner_base(stackup, layer)?.into(),
                contours: &level,
                height: foil_height.as_str().into(),
            },
        )?;
        let below = name();
        extrude(
            b,
            ProfileBody {
                name: &below,
                offset: low.as_str().into(),
                contours: &dielectric,
                height: format!("{thickness} - {foil_height}").into(),
            },
        )?;
    }
    Ok(())
}

/// The board outline's size parameters (`stackup_params`).
const BOARD_SIZE: [&str; 2] = ["board_width", "board_depth"];

/// A digest of `value` as debug-printed (every number in its shortest
/// round-trip form: equal values, equal digests).
fn digest(value: &impl std::fmt::Debug) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{value:?}").hash(&mut h);
    h.finish()
}

/// The native envelope of one package definition, draft or published, with
/// the same terminations and overcoat a placed chip part shows.
pub fn preview(package: &PackageDefinition) -> Result<NativeModel> {
    if models::is_generated(package) {
        return preview_model(package);
    }
    if !package.envelope_valid() {
        return Err(Error::Invalid(
            "package envelope dimensions must be finite and positive".into(),
        ));
    }
    let mut b = Builder::new("Package envelope", &SourceCache::default())?;
    for (name, value) in ["length", "width", "height"]
        .into_iter()
        .zip(package.envelope)
    {
        b.param(name, value, "mm")?;
    }
    let body = Body::of(package);
    let kind = match body {
        Body::Moulded => BodyKind::PartBody,
        chip => chip.kind(),
    };
    // The box in its model frame, moved by the placement as a generated
    // model's occurrence is.
    let created = b.native.component("Package envelope", false)?;
    b.body(
        kind,
        BoxBody {
            name: "Package envelope",
            corner: ["-length/2".into(), "-width/2".into(), 0.into()],
            size: ["length".into(), "width".into(), "height".into()],
        },
    )?;
    b.native.activate_root()?;
    move_by(&mut b.native, created.occurrence, &package.placement)?;
    let mut model = b.finish(vec![])?;
    // Display-only terminations and overcoat are drawn in the package
    // frame, shifted but not turned.
    if package.placement.transform().rotation == Transform3::IDENTITY.rotation {
        model.geometry.extend(appearance::preview(&Part {
            pick: None,
            package,
            footprint: None,
            body,
            placement: None,
            generated: false,
        })?);
    }
    Ok(model)
}

/// Move an unmoved occurrence by a model placement (model → datum frame).
fn move_by(native: &mut Native, occurrence: u64, placement: &ModelPlacement) -> Result<()> {
    let t = placement.transform();
    if t == Transform3::IDENTITY {
        return Ok(());
    }
    let r = t.rotation;
    let p = t.translation;
    native.move_occurrence(
        occurrence,
        [
            [r[0][0], r[0][1], r[0][2], p[0]],
            [r[1][0], r[1][1], r[1][2], p[1]],
            [r[2][0], r[2][1], r[2][2], p[2]],
        ],
    )
}

/// The part-file model of one package, draft or published, built from
/// the part file's `bytes` at the package's values and placement (the
/// Component 3D preview).
pub fn preview_part(package: &PackageDefinition, bytes: &[u8]) -> Result<NativeModel> {
    let PackageModel::PartFile(use_) = &package.model else {
        return Err(Error::Invalid(
            "model.unsupported: the package does not use a part file".into(),
        ));
    };
    if !package.placement.finite() {
        return Err(Error::Invalid(
            "package.placement: placement values must be finite and within ±10000".into(),
        ));
    }
    let mut b = Builder::new("Package model", &SourceCache::default())?;
    let stem = b.define_part(DRAFT_MODEL, use_, bytes)?;
    let created = b.model_occurrence(&stem)?;
    move_by(&mut b.native, created.occurrence, &package.placement)?;
    b.finish(vec![])
}

/// The definition name of an unpublished (draft) package's model.
pub const DRAFT_MODEL: &str = "pkgdraft";

/// The generated model of one package, draft or published, at its
/// placement in the datum frame (z = 0 the seating plane).
fn preview_model(package: &PackageDefinition) -> Result<NativeModel> {
    if !package.placement.finite() {
        return Err(Error::Invalid(
            "package.placement: placement values must be finite and within ±10000".into(),
        ));
    }
    let mut b = Builder::new("Package model", &SourceCache::default())?;
    let created = b.model(DRAFT_MODEL, package)?;
    move_by(&mut b.native, created.occurrence, &package.placement)?;
    b.finish(vec![])
}

mod fill;
mod identity;
#[cfg(test)]
mod realism_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod update_tests;
