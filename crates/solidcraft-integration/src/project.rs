//! Electronics → native Solidcraft. Parametric pads, traces and envelopes are
//! ordinary native features and planar joints, editable without Sparksmith.
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
use crate::models::imported::SourceCache;
use crate::models::{self, ModelDefinition, look};
use crate::native::{
    self, BoardJoint, BoxBody, Created, Expr, JointFrame, Mount, Native, ProfileBody,
};
use crate::pad_outline::{self, Contour, FLATTEN_TOLERANCE_MM};
use crate::scene::{Geometry, Place, Volume};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use solidcraft_viewport::surface::OpaqueSurface;
use sparksmith_core::geometry::{Transform3, cos_sin};
use sparksmith_core::library::model3d::{LibraryModel, ModelDefinition as LibraryModelDefinition};
use sparksmith_core::library::{
    ModelPlacement, PackageDefinition, PackageModel, Pad, PadCopper, PadDefaults, PadShape,
};
use sparksmith_core::{
    CopperLayer, Fabrication, Id, Layer, PadId, Placement, Point, Project, Stackup, instances,
    layout,
};
use std::collections::{BTreeMap, BTreeSet};

/// Chords of each round end cap of a track's 3D copper. The core checks
/// measure the exact cap; 8 chords keep the native model's vertex count
/// bounded (within 2 % of the radius) on boards of thousands of tracks.
const TRACK_CAP_SEGMENTS: usize = 8;

/// Which Sparksmith object a native body came from. Stored in the native file's
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

/// How a model holds the board's flat bodies (laminate, pad, track, via
/// and fill copper), all described by the board plan ([`plan`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Flat {
    /// Built in the native design: the exact model, as saved and
    /// exported.
    #[default]
    Native,
    /// Meshed from the plan ([`mesh`]) for the live view: the native
    /// design holds the parts only, and the model is never saved.
    Meshed,
}

/// The component a source names when its object has no native body (a
/// meshed flat body, [`Flat::Meshed`]).
const NO_COMPONENT: u64 = u64::MAX;

pub struct NativeModel {
    /// The board's native design; `None` for a meshed model
    /// ([`Flat::Meshed`]), which has no design of the board.
    native: Option<Native>,
    /// How the board's flat bodies are held.
    flat: Flat,
    /// The flat bodies' meshes made for this model ([`Flat::Meshed`]), for
    /// an update to reuse.
    meshes: mesh::Memo,
    /// The part designs this model places ([`Flat::Meshed`]): one per
    /// package revision, each its own evaluated design ([`designs`]), for
    /// an update to reuse. They hold the bodies the parts' geometry
    /// measures.
    designs: designs::Memo,
    /// The fills' plans made for this model, for an update to reuse.
    plans: plan::Memo,
    pub geometry: Vec<Geometry>,
    pub sources: Vec<Source>,
    /// The board's layer stack ([`crate::stack::Stack::none`] for a
    /// package preview).
    pub stack: crate::stack::Stack,
    /// The generated package-model definitions, one per package revision
    /// (a library model's bodies with built-in materials among them).
    pub models: Vec<ModelDefinition>,
    /// The display layers made for this model, for an update to reuse.
    display: appearance::Memo,
    /// The evaluated bodies [`Geometry::volume`] measures: geometry
    /// reaches them only while this model lives.
    /// `None` for a meshed model, whose part bodies its designs hold.
    world: Option<std::sync::Arc<solidcraft_engine::doc::ModelState>>,
    /// How many of [`Self::geometry`]'s entries, from the first, are the
    /// native design's bodies (the display-only layers follow); 0 for a
    /// meshed model.
    native_bodies: usize,
    /// The evaluated body of each of the first [`Self::native_bodies`]
    /// entries of [`Self::geometry`]: its index in [`Self::world`].
    world_bodies: Vec<usize>,
}

/// How [`update`] made its model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rebuilt {
    /// A meshed model made from what the previous one kept (its meshes,
    /// fill plans, display layers and part designs), running no native
    /// command but those of a part design it did not hold.
    InPlace,
    /// Built anew, and why: the previous model was the exact one (built,
    /// never updated), or the update failed.
    Anew(Option<String>),
}

impl NativeModel {
    /// Write the model as a new `.solidcraft` file; refuses existing files,
    /// and a model whose flat bodies are meshed ([`Flat::Meshed`]): it has
    /// no design of the board.
    pub fn save(&self, path: &str) -> Result<()> {
        match (&self.native, self.flat) {
            (Some(native), Flat::Native) => native.save_new(path),
            _ => Err(Error::Invalid(
                "a model with meshed board bodies is drawn, never saved; build the exact model"
                    .into(),
            )),
        }
    }

    /// How the model holds the board's flat bodies.
    pub fn flat(&self) -> Flat {
        self.flat
    }

    /// The native design the model was built as; `None` for a meshed
    /// model ([`Flat::Meshed`]), which places its parts' own designs.
    pub fn native(&self) -> Option<&Native> {
        self.native.as_ref()
    }

    /// The native design, without the rest of the model; `None` for a
    /// meshed model.
    pub(crate) fn into_native(self) -> Option<Native> {
        self.native
    }

    /// The evaluated world of the board's native design; `None` for a
    /// meshed model.
    pub fn world(&self) -> Option<&solidcraft_engine::doc::ModelState> {
        self.world.as_deref()
    }

    /// How many part designs a meshed model places: one per package
    /// revision, each evaluated once ([`designs`]).
    pub fn part_designs(&self) -> usize {
        self.designs.len()
    }

    /// The geometry of the board's native design, one entry per body (an
    /// exported interface's reference bodies left out), without the
    /// display-only layers. Refused for a meshed model, which has no
    /// design of the board.
    pub fn native_geometry(&self) -> Result<&[Geometry]> {
        if self.native.is_none() {
            return Err(no_design());
        }
        self.geometry
            .get(..self.native_bodies)
            .ok_or_else(|| Error::Verification("the model has fewer bodies than its design".into()))
    }

    /// The evaluated body of [`Self::native_geometry`]'s entry `index`
    /// (its exact B-rep); `None` for a meshed model or past its bodies.
    pub fn native_body(&self, index: usize) -> Option<&solidcraft_engine::kernel::Body> {
        let world = self.world.as_deref()?;
        let at = *self.world_bodies.get(index)?;
        world.bodies.get(at).map(|b| &b.body)
    }

    /// Each part body of the exact model's design (a part's model or
    /// envelope; not a pad, nor the board's flat bodies), by its name in
    /// the design, with the part it belongs to, sorted by name. SolidCraft
    /// names a package's bodies by the package, so this is what says
    /// which part a body is ([`PartBody`]). Refused for a meshed model.
    ///
    /// [`PartBody`]: sparksmith_core::interface::sidecar::PartBody
    pub fn part_bodies(
        &self,
        project: &Project,
    ) -> Result<Vec<sparksmith_core::interface::sidecar::PartBody>> {
        let geometry = self.native_geometry()?;
        let Some(world) = self.world.as_deref() else {
            return Err(no_design());
        };
        let by_pick: BTreeMap<u64, &Source> =
            self.sources.iter().map(|s| (s.pick_id, s)).rev().collect();
        // Each geometry entry's body in the design, as the model was built
        // from them (the same map `native_body` reads).
        let bodies: Vec<&str> = (0..geometry.len())
            .map(|i| {
                self.world_bodies
                    .get(i)
                    .and_then(|&at| world.bodies.get(at))
                    .map(|b| b.name.as_str())
                    .ok_or_else(|| {
                        Error::Verification(
                            "the model's bodies and its design's do not line up".into(),
                        )
                    })
            })
            .collect::<Result<_>>()?;
        let mut parts: Vec<_> = bodies
            .into_iter()
            .zip(geometry)
            .filter_map(|(body, g)| {
                let source = by_pick.get(&g.item?)?;
                if source.pad.is_some() {
                    return None;
                }
                let instance = project.instance(source.instance?)?;
                Some(sparksmith_core::interface::sidecar::PartBody {
                    body: body.to_owned(),
                    instance: instance.id,
                    designator: instance.designator.clone(),
                })
            })
            .collect();
        parts.sort();
        parts.dedup_by(|a, b| a.body == b.body);
        Ok(parts)
    }
}

#[cfg(test)]
impl NativeModel {
    /// The exact model's design; refused for a meshed one.
    pub(crate) fn exact(&self) -> Result<&Native> {
        self.native.as_ref().ok_or_else(no_design)
    }
}

/// A meshed model asked for the board's design.
pub(crate) fn no_design() -> Error {
    Error::Verification("a meshed model has no design of the board".into())
}

/// A native design under construction, recording each body's kind.
struct Builder {
    native: Native,
    /// Fill plans made before ([`plan::Memo`]).
    plans: plan::Memo,
    bodies: BTreeMap<String, BodyKind>,
    /// Generated package-model definitions built so far.
    models: Vec<ModelDefinition>,
    /// Definitions whose first occurrence an instance already holds.
    placed: BTreeSet<String>,
    /// Library-model bodies with one of the model's own materials, by body
    /// name: drawn with that material's look (the file keeps its opaque
    /// projection).
    custom_looks: BTreeMap<String, (OpaqueSurface, Detail)>,
    /// Library model files read by earlier builds.
    sources: SourceCache,
    /// Display layers made before ([`appearance::Memo`]).
    display: appearance::Memo,
}

impl Builder {
    fn new(name: &str, sources: &SourceCache) -> Result<Self> {
        Ok(Self::on(Native::new(name)?, sources))
    }

    /// A build that continues `native`, after what it holds already (the
    /// slots of an exported board interface).
    fn on(native: Native, sources: &SourceCache) -> Self {
        Self {
            native,
            plans: plan::Memo::default(),
            bodies: BTreeMap::new(),
            models: Vec::new(),
            placed: BTreeSet::new(),
            custom_looks: BTreeMap::new(),
            sources: sources.clone(),
            display: appearance::Memo::default(),
        }
    }

    /// Build the generated definition `stem` of `package` unless it is
    /// built.
    fn define(&mut self, stem: &str, package: &PackageDefinition) -> Result<()> {
        if self.models.iter().any(|d| d.component == stem) {
            return Ok(());
        }
        // The model's own design, built once per process and inserted
        // like a part file: its face names are the design's, and
        // dimension references are traced on the same build.
        let design = models::named::generated(package)?;
        let definition = models::part_file::insert_design(
            &mut self.native,
            stem,
            design.source(),
            &design.generated,
        )?;
        for body in &definition.bodies {
            self.record(&body.name, body.kind())?;
        }
        self.models.push(definition);
        Ok(())
    }

    /// Build the library-model definition `stem` (`model` with its asset
    /// `bytes`, used as `use_`) unless it is built; bounds and lead contacts
    /// are rechecked when built.
    fn define_library(
        &mut self,
        stem: &str,
        (model, use_, bytes): (&LibraryModelDefinition, &LibraryModel, &[u8]),
    ) -> Result<()> {
        if self.models.iter().any(|d| d.component == stem) {
            return Ok(());
        }
        let built = models::imported::build_library_model_cached(
            &mut self.native,
            stem,
            model,
            use_,
            bytes,
            &self.sources,
        )?;
        for body in &built.bodies {
            self.record(&body.name, body.kind())?;
            if matches!(
                body.material,
                sparksmith_core::library::model3d::ModelMaterial::Custom(_)
            ) {
                let look = models::imported::surface(model, body.material)?;
                self.custom_looks.insert(body.name.clone(), look);
            }
        }
        self.models.push(built.generated());
        Ok(())
    }

    /// Build the part-file definition of `use_` (its file `bytes`) unless
    /// it is built, and return its name (`pkgp<key>`: packages with equal
    /// file and values share it). The part is evaluated through the
    /// process-wide cache, checked, and compared with what the package
    /// stored (`part_file.stale`).
    fn define_part(
        &mut self,
        package: &str,
        use_: &sparksmith_core::library::package_model::PartFileModel,
        bytes: &[u8],
    ) -> Result<String> {
        use crate::models::part_file;
        let part = part_file::evaluate(bytes, &use_.values)?;
        if let Some(refusal) = part.refusal() {
            return Err(refusal);
        }
        if !part_file::matches(&use_.derived, &part_file::derived(&part)?) {
            return Err(Error::Invalid(format!(
                "part_file.stale: the part file of package {package} no longer builds what the package \
                 recorded (bounds, bodies or marking surfaces); set its values again"
            )));
        }
        let stem = part.stem();
        if self.models.iter().any(|d| d.component == stem) {
            return Ok(stem);
        }
        let definition = part_file::insert(&mut self.native, &stem, &part)?;
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
        self.native.stage(crate::export::Stage::Evaluating)?;
        self.native.evaluate_deferred()?;
        self.native.stage(crate::export::Stage::Building)?;
        self.native.activate_root()?;
        // Generated bodies already carry their materials' looks.
        let stack = crate::stack::Stack::none()?;
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
                self.native.appearance(
                    &names,
                    &material::material(kind, &Fabrication::default(), &stack),
                )?;
            }
        }
        identity::write(&mut self.native, &sources, &self.bodies)?;
        models::record(&mut self.native, &self.models)?;
        self.native.forget_history();
        finish_with(self.native, sources, self.models, &self.custom_looks)
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
                    Error::Verification(format!("placement {path:?} has no Sparksmith source"))
                })?;
            out.insert(*index, pick);
        }
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn finish(native: Native, sources: Vec<Source>) -> Result<NativeModel> {
    finish_with(native, sources, Vec::new(), &BTreeMap::new())
}

/// A definition body's measure at one placement: volume, box (low, high)
/// and where that placement moved it.
type Measured = (Volume, [f64; 3], [f64; 3], [f64; 3]);

/// Via definitions by drill and diameter (their bits).
type ViaKey = (u64, u64);

/// [`finish`] for a model whose generated definitions are `models` and
/// whose library-model bodies with their own materials look `custom`.
fn finish_with(
    native: Native,
    sources: Vec<Source>,
    models: Vec<ModelDefinition>,
    custom: &BTreeMap<String, (OpaqueSurface, Detail)>,
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
    let mut world_bodies = vec![];
    // Every display mesh at once, on all cores (each the mesh it makes
    // alone). A placement of a definition shares the definition's mesh, so
    // only that is made, never a moved copy.
    let world = native.world_state()?;
    let bodies: Vec<_> = world.bodies.iter().collect();
    on_threads(&bodies, |body| {
        body.mesh_in_place();
    });
    // A body's lazy volume reaches it through the model, never holding it.
    let reach = std::sync::Arc::downgrade(&world);
    let lazy_volume = |index: usize| {
        let world = reach.clone();
        Volume::lazy(move || {
            let world = world.upgrade()?;
            let body = world.bodies.get(index)?;
            Some(body.measure().map_or(f64::NAN, |m| m.volume))
        })
    };
    // An exported board interface's slots come before the board, in a
    // component of their own (`crate::interface`): reference geometry, not
    // part of the board's model.
    let reference: Option<u64> = doc
        .components
        .iter()
        .find(|c| c.name == crate::interface::REFERENCE_COMPONENT)
        .map(|c| c.id);
    for (index, body) in world.bodies.iter().enumerate() {
        if reference.is_some_and(|c| body.component == c) {
            continue;
        }
        let name = body.definition_name();
        let kind = *kinds
            .get(name)
            .ok_or_else(|| Error::Verification(format!("body {name} has no Sparksmith kind")))?;
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
                crate::material::detail(
                    kind,
                    &sparksmith_core::Fabrication::default(),
                    crate::looks::get()?,
                ),
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
                    (
                        lazy_volume(index),
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
        world_bodies.push(index);
        geometry.push(Geometry {
            item,
            kind,
            layer: layer(kind),
            dielectric: None,
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
    let geometry_len = geometry.len();
    Ok(NativeModel {
        native: Some(native),
        flat: Flat::Native,
        meshes: mesh::Memo::default(),
        designs: designs::Memo::default(),
        plans: plan::Memo::default(),
        geometry,
        sources,
        stack: crate::stack::Stack::none()?,
        models,
        display: appearance::Memo::default(),
        native_bodies: geometry_len,
        world_bodies,
        world: Some(world),
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
        let (c, s) = cos_sin(self.pad().rotation_deg());
        [c * v[0] - s * v[1], s * v[0] + c * v[1]]
    }

    /// A pad-frame contour on the board.
    fn board(&self, contour: &Contour) -> Contour {
        let t = self.placed.transform;
        let position = self.pad().position();
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
        let mut out: Vec<Contour> = pad.hole.iter().filter_map(pad_outline::drilled).collect();
        out.extend(pad.vias.iter().map(|v| Contour::Circle {
            center: v.at(),
            radius: pad_outline::plated_drill_radius(v.drill()),
        }));
        out
    }

    /// Finished bores, pad frame: each hole's stated size.
    fn bores(&self) -> Vec<Contour> {
        let pad = self.pad();
        let mut out: Vec<Contour> = pad.hole.iter().filter_map(pad_outline::bore).collect();
        out.extend(pad.vias.iter().filter_map(|v| {
            let radius = v.drill() / 2.0;
            (radius > 0.0).then_some(Contour::Circle {
                center: v.at(),
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
            && let (Some(drill), Some(bore)) =
                (pad_outline::drilled(&hole), pad_outline::bore(&hole))
        {
            out.push([drill, bore]);
        }
        for via in &pad.vias {
            let radius = via.drill() / 2.0;
            if radius > 0.0 {
                out.push([
                    Contour::Circle {
                        center: via.at(),
                        radius: pad_outline::plated_drill_radius(via.drill()),
                    },
                    Contour::Circle {
                        center: via.at(),
                        radius,
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
    // Each placed part once, by instance: a lookup per pad, not a scan.
    let mut parts = std::collections::HashMap::new();
    for placement in project.placements.iter() {
        parts.insert(placement.instance, (placement, None));
    }
    for instance in project.instances.iter() {
        if let Some((_, slot)) = parts.get_mut(&instance.id) {
            *slot = Some(instance);
        }
    }
    let mut defaults_of = std::collections::HashMap::new();
    placed
        .into_iter()
        .map(|placed| {
            let (placement, instance) = parts
                .get(&placed.instance)
                .copied()
                .ok_or_else(|| Error::Invalid("missing pad placement".into()))?;
            let instance = instance.ok_or_else(|| Error::Invalid("missing pad instance".into()))?;
            let defaults = match defaults_of.get(&instance.id) {
                Some(defaults) => *defaults,
                None => {
                    let defaults = instances::resolve(&project.library, instance)
                        .ok()
                        .and_then(|r| r.physical)
                        .map(|p| p.footprint.definition.defaults)
                        .ok_or_else(|| Error::Invalid("missing pad definition".into()))?;
                    defaults_of.insert(instance.id, defaults);
                    defaults
                }
            };
            let designator = instance.designator.as_str();
            let origin = placed.transform.apply(placed.pad.position());
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

/// What a placed part's model is made of.
#[derive(Clone, Copy)]
enum PartModel<'a> {
    /// A library model (STEP or glTF): the model, how the package uses it
    /// and its asset bytes.
    Library((&'a LibraryModelDefinition, &'a LibraryModel, &'a [u8])),
    /// A part file at the package's values, and its bytes.
    PartFile(
        &'a sparksmith_core::library::package_model::PartFileModel,
        &'a [u8],
    ),
    /// The package's generated model.
    Generated,
    /// The package envelope, a box.
    Envelope,
}

/// One placed part and what its model is made of.
struct PlacedPart<'a> {
    placement: &'a Placement,
    instance: &'a sparksmith_core::ComponentInstance,
    package: &'a PackageDefinition,
    footprint: &'a sparksmith_core::library::FootprintDefinition,
    /// The model definition's name: one per package revision.
    stem: String,
    model: PartModel<'a>,
}

impl<'a> PlacedPart<'a> {
    /// The part's source label.
    fn label(&self) -> String {
        match self.model {
            PartModel::Envelope => format!("{} envelope", self.instance.designator),
            _ => format!("{} model", self.instance.designator),
        }
    }

    /// The part's source, picked as `pick`: its bodies in `component`, as
    /// `occurrence` of a shared definition.
    fn source(&self, pick: u64, component: u64, occurrence: Option<u64>) -> Source {
        Source {
            pick_id: pick,
            instance: Some(self.instance.id),
            pad: None,
            track: None,
            via: None,
            fill: None,
            net: None,
            layer: Some(self.placement.layer.into()),
            label: self.label(),
            component,
            occurrence,
        }
    }

    /// The display-only layers drawn for the part (terminations, overcoat
    /// and solder fillets), picked as `pick`: drawn in the package frame,
    /// so only while the model frame is that frame; of a generated model
    /// only the solder joints of a chip, gull-wing feet or header pins.
    fn display(&self, pick: u64) -> Option<Part<'a>> {
        use sparksmith_core::library::PackageForm;
        if !package_frame(&datum_frame(self.package)) {
            return None;
        }
        let generated = match self.model {
            PartModel::Generated => true,
            PartModel::Envelope => false,
            PartModel::Library(_) | PartModel::PartFile(..) => return None,
        };
        let joints = matches!(
            self.package.form,
            PackageForm::Chip(_)
                | PackageForm::GullWing(_)
                | PackageForm::Tabbed(_)
                | PackageForm::Header(_)
        );
        if generated && !joints {
            return None;
        }
        Some(Part {
            pick: Some(pick),
            package: self.package,
            footprint: Some(self.footprint),
            body: Body::of(self.package),
            placement: Some(self.placement),
            generated,
        })
    }
}

/// Every placed part, in placement order, and what its model is made of;
/// refused when a placement's instance, physical implementation or model
/// file is missing.
fn placed_parts(project: &Project) -> Result<Vec<PlacedPart<'_>>> {
    project
        .placements
        .iter()
        .map(|placement| {
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
            let model = match &package.model {
                PackageModel::Library(use_) => PartModel::Library(library_model(project, use_)?),
                PackageModel::PartFile(use_) => {
                    PartModel::PartFile(use_, part_bytes(project, use_)?)
                }
                _ if models::is_generated(package) => PartModel::Generated,
                _ => PartModel::Envelope,
            };
            Ok(PlacedPart {
                placement,
                instance,
                package,
                footprint: &physical.footprint.definition,
                stem: models::definition_name(physical.package.id, physical.package.revision.0),
                model,
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
    use sparksmith_core::library::{Outline, Segment};
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

/// [`build`] continuing `native`: the board's model made after what the
/// design holds already, which keeps its feature ids (the exported board
/// interface's slots, `crate::interface`).
pub fn build_onto(native: Native, project: &Project) -> Result<NativeModel> {
    let sources = SourceCache::default();
    sources.begin();
    let built = build_on(Builder::on(native, &sources), project);
    sources.sweep();
    built
}

/// [`build`] for an export: each native command counted on `watch`, its
/// stages said there, stopping with [`Error::Cancelled`] at the next
/// command once it is cancelled ([`crate::export`]).
pub fn build_watched(
    project: &Project,
    watch: std::sync::Arc<crate::export::Watch>,
) -> Result<NativeModel> {
    let sources = SourceCache::default();
    sources.begin();
    let built = Builder::new(&project.name, &sources).and_then(|mut b| {
        b.native.watch_with(watch);
        build_on(b, project)
    });
    sources.sweep();
    built
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
    build_flat(project, sources, cancel, Flat::Native)
}

/// [`build_with`], holding the board's flat bodies as `flat` says: built
/// natively (the exact model) or meshed from the board plan (the live
/// view's, [`mesh`], with each part its own design, [`designs`]). An
/// [`update`] of the model keeps its way.
pub fn build_flat(
    project: &Project,
    sources: &SourceCache,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    flat: Flat,
) -> Result<NativeModel> {
    sources.begin();
    let built = match flat {
        Flat::Native => Builder::new(&project.name, sources).and_then(|mut b| {
            if let Some(cancel) = cancel {
                b.native.cancel_with(cancel);
            }
            build_on(b, project)
        }),
        Flat::Meshed => build_meshed(project, Kept::default(), sources, cancel.as_ref()),
    };
    sources.sweep();
    built
}

/// The model of `project`, made from `previous` (a model of an earlier
/// state of the same board).
///
/// A meshed model ([`Flat::Meshed`], the live view's) is made again from
/// what the previous one kept (the meshes, fill plans, display layers and
/// part designs it can reuse), running no native command but those of a
/// part design it does not hold: the board after any edit, undo or redo
/// costs about as much as the edit changed, and nothing else is kept for
/// it. The exact model ([`Flat::Native`]) is built for export and as the
/// test reference, never updated: it is built anew, stopping with
/// [`Error::Cancelled`] at its next native command once `cancel` is set.
pub fn update(
    previous: NativeModel,
    project: &Project,
    sources: &SourceCache,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<(NativeModel, Rebuilt)> {
    let NativeModel {
        flat,
        meshes,
        designs,
        plans,
        display,
        ..
    } = previous;
    let why = match flat {
        Flat::Meshed => {
            let kept = Kept {
                meshes,
                plans,
                display,
                designs,
            };
            sources.begin();
            let made = build_meshed(project, kept, sources, cancel.as_ref());
            sources.sweep();
            match made {
                Ok(model) => return Ok((model, Rebuilt::InPlace)),
                // Refused or cancelled, it would be anew.
                Err(error @ (Error::Invalid(_) | Error::Cancelled)) => return Err(error),
                Err(error) => error.to_string(),
            }
        }
        Flat::Native => "the exact model is built, never updated".to_owned(),
    };
    Ok((
        build_flat(project, sources, cancel, flat)?,
        Rebuilt::Anew(Some(why)),
    ))
}

/// What a meshed model keeps for the next one ([`build_meshed`]).
#[derive(Default)]
struct Kept {
    meshes: mesh::Memo,
    plans: plan::Memo,
    display: appearance::Memo,
    designs: designs::Memo,
}

/// The live view's model of `project` ([`Flat::Meshed`]). No design of
/// the board is made: its flat bodies are meshed from the board plan
/// ([`mesh`]), and each part is its package revision's own design
/// ([`designs`]), evaluated once and shared by every placement, placed by
/// the document's transform (the pose the exact build's joints solve,
/// [`sparksmith_core::interface::package::model_to_board`]). What `kept`
/// holds is taken instead of made again. The sources (and so the picks)
/// are the exact build's, in its order. A part design not held is built
/// on every core, stopping with [`Error::Cancelled`] at its next native
/// command once `cancel` is set.
fn build_meshed(
    project: &Project,
    kept: Kept,
    library: &SourceCache,
    cancel: Option<&std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<NativeModel> {
    sparksmith_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    let stackup = &project.stackup;
    stackup.check().map_err(|e| Error::Invalid(e.to_string()))?;
    let Kept {
        mut meshes,
        mut plans,
        display: mut memo,
        designs: held,
    } = kept;
    let pads = placed_pads(project)?;
    let plan = plan::of_pads(project, &pads, &mut plans)?;
    let placed = placed_parts(project)?;
    let (designs, made) = designs::prepare(&held, &placed, library, cancel)?;
    drop(held);
    let mut sources = vec![];
    let fills = fill::marks(project, &plan.fills, &mut sources)?;
    let mut parts = vec![];
    let mut placed_geometry = vec![];
    for (part, design) in placed.iter().zip(&made) {
        let pick = next_pick(&sources);
        parts.extend(part.display(pick));
        sources.push(part.source(pick, NO_COMPONENT, None));
        let pose = sparksmith_core::interface::package::model_to_board(
            part.placement,
            part.package,
            stackup,
        )
        .map_err(|e| Error::Invalid(e.to_string()))?;
        placed_geometry.extend(design.place(&pose, pick, part.placement.layer.into()));
    }
    let mut lands = Vec::with_capacity(pads.len());
    for pad in &pads {
        let pick = next_pick(&sources);
        if let Some(source) = pad_lands(pad, pick, &mut lands)? {
            sources.push(source);
        }
    }
    let mut paths = Vec::with_capacity(project.tracks.len());
    for track in &project.tracks {
        let pick = next_pick(&sources);
        sources.push(track_source(track, pick, NO_COMPONENT, &mut paths));
    }
    let display = appearance::Board::of(project)?;
    let none = vec![None; project.vias.len()];
    let marks = marks(project, &none, &none, &mut sources)?;
    for mark in &marks {
        lands.extend(mark.lands(&display));
    }
    let drawn = Drawn {
        project,
        display: &display,
        lands: &lands,
        paths: &paths,
        marks: &marks,
        fills: &fills,
        parts: &parts,
        picks: part_picks(&sources),
    };
    // The flat bodies are meshed while the display layers are drawn, picked
    // as the sources name them.
    let meshed = |meshes: &mut mesh::Memo| -> Result<Vec<Geometry>> {
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        let (bodies, stats) = mesh::bodies(&plan, &sources, meshes)?;
        #[cfg(not(target_arch = "wasm32"))]
        if std::env::var_os("SPARKSMITH_TIMINGS").is_some() {
            eprintln!(
                "sparksmith: {} flat bodies meshed in {:.1} ms ({} meshes made, {} reused)",
                stats.bodies,
                started.elapsed().as_secs_f64() * 1e3,
                stats.meshed,
                stats.reused
            );
        }
        Ok(bodies.into_iter().map(|m| m.geometry).collect())
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (layers, flat) = std::thread::scope(|scope| {
        let layers = scope.spawn(|| drawn.layers(&mut memo));
        let flat = meshed(&mut meshes);
        let layers = layers.join().unwrap_or_else(|_| {
            Err(Error::Verification(
                "drawing the board's layers failed".into(),
            ))
        });
        (layers, flat)
    });
    #[cfg(target_arch = "wasm32")]
    let (layers, flat) = (drawn.layers(&mut memo), meshed(&mut meshes));
    let mut geometry = placed_geometry;
    geometry.extend(flat?);
    geometry.extend(layers?);
    // Each package revision's definition once, in placement order.
    let mut models: Vec<ModelDefinition> = Vec::new();
    for design in &made {
        for definition in design.definitions() {
            if !models.iter().any(|d| d.component == definition.component) {
                models.push(definition.clone());
            }
        }
    }
    Ok(NativeModel {
        native: None,
        flat: Flat::Meshed,
        meshes,
        designs,
        plans,
        geometry,
        sources,
        stack: crate::stack::Stack::of(stackup, project.fabrication.pad_finish)?,
        models,
        display: memo,
        world: None,
        native_bodies: 0,
        world_bodies: Vec::new(),
    })
}

/// Build `project`'s exact model with `b`.
fn build_on(mut b: Builder, project: &Project) -> Result<NativeModel> {
    sparksmith_core::validate::project(project).map_err(|e| Error::Invalid(e.to_string()))?;
    let stackup = &project.stackup;
    let pads = placed_pads(project)?;
    // The board's flat bodies, described once ([`plan`]): the fills, pads,
    // tracks, vias and laminate below are built from it.
    let plan = plan::of_pads(project, &pads, &mut b.plans)?;
    let plans = std::mem::take(&mut b.plans);
    // The board is evaluated once its parts are in, not after every command.
    b.native.defer_evaluation()?;
    stackup_params(&mut b, project)?;
    b.params(&object_params(project, &pads)?)?;
    // What an edit rarely changes comes first in the design's history, so
    // evaluating an edit reuses it: the generated and library package
    // models and the via definitions, then the fills, then the parts and
    // copper. The laminate comes last (see there).
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
    let mut styles = via_styles(&mut b, project, &plan.vias)?;
    let substrate = b.native.component("PCB substrate", true)?;
    let board = substrate.occurrence;
    let mut sources = vec![];
    let mut parts = vec![];
    // Board joints of every part, made at the end (see there).
    let mut joints: Vec<(BoardJoint, JointFrame, Option<String>)> = vec![];
    // The fills before the parts, pads, tracks and vias: a fill's body
    // reads only its own sketch and the stackup's parameters, never the
    // copper built before it, but Solidcraft keys each feature's cached
    // result on every body made before it. After the parts and tracks,
    // every edit to them would extrude every fill again. (A refill's
    // changed tiles are built again at the end of the history, and every
    // feature after the first one deleted is evaluated again: measured on
    // the ESP32 board, the fills just before the laminate made a refill
    // 1.6 s faster and a track edit 2.6 s slower.)
    let fills = fill::build(
        &mut b,
        project,
        &plan.fills,
        board,
        &mut sources,
        &mut joints,
    )?;
    for part in placed_parts(project)? {
        let prefix = format!("instance_{}", part.instance.id);
        let (created, occurrence) = match part.model {
            PartModel::Library(library) => {
                let created = b.library_model(&part.stem, library)?;
                (created, Some(created.occurrence))
            }
            PartModel::PartFile(use_, bytes) => {
                let stem = b.define_part(&part.stem, use_, bytes)?;
                let created = b.model_occurrence(&stem)?;
                (created, Some(created.occurrence))
            }
            PartModel::Generated => {
                let created = b.model(&part.stem, part.package)?;
                (created, Some(created.occurrence))
            }
            PartModel::Envelope => {
                let created = b.native.component(&part.label(), false)?;
                // The box in its model frame (z = 0 the seating plane), held at
                // its placement in the datum frame as a generated model is.
                b.body(
                    Body::of(part.package).kind(),
                    BoxBody {
                        name: &part.label(),
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
                (created, None)
            }
        };
        joints.push((
            BoardJoint {
                board,
                occurrence: created.occurrence,
                x: format!("{prefix}_x").into(),
                y: format!("{prefix}_y").into(),
                angle: format!("{prefix}_angle").into(),
                mount: part.placement.layer.into(),
            },
            holding_frame(&datum_frame(part.package)),
            Some(copper_param(part.placement.layer.into())),
        ));
        let pick = next_pick(&sources);
        parts.extend(part.display(pick));
        sources.push(part.source(pick, created.component, occurrence));
    }
    let mut lands = Vec::with_capacity(pads.len());
    if plan.pads.len() != pads.len() {
        return Err(Error::Verification(
            "the board plan holds another number of pads".into(),
        ));
    }
    for (pad, planned) in pads.iter().zip(&plan.pads) {
        let local = pad.pad();
        let pick = next_pick(&sources);
        let Some(source) = pad_lands(pad, pick, &mut lands)? else {
            continue;
        };
        let prefix = format!("instance_{}", pad.placed.instance);
        let pp = format!("pad_{}_{}", pad.placed.instance, local.id);
        let created = b.native.component(&pp, false)?;
        let near_copper = copper_param(pad.placed.layer.into());
        let far_copper = copper_param(other(pad.placed.layer).into());
        let profile = |b: &mut Builder, body: &plan::PlanBody, offset: Expr, height: &str| {
            b.native.profile_body(ProfileBody {
                name: &body.name,
                offset,
                contours: &body.loops,
                height: height.into(),
            })?;
            b.record(&body.name, BodyKind::PadCopper)
        };
        for body in &planned.bodies {
            let plan::Origin::Pad { part, .. } = body.source else {
                return Err(Error::Verification(format!(
                    "pad {pp}'s plan holds body {}",
                    body.name
                )));
            };
            match part {
                plan::PadPart::Land => match axis_box(pad) {
                    Some(along_y) => {
                        let (x, y) = if along_y {
                            (format!("{pp}_width"), format!("{pp}_length"))
                        } else {
                            (format!("{pp}_length"), format!("{pp}_width"))
                        };
                        b.body(
                            BodyKind::PadCopper,
                            BoxBody {
                                name: &body.name,
                                corner: [
                                    format!("-{x} / 2").into(),
                                    format!("-{y} / 2").into(),
                                    0.into(),
                                ],
                                size: [x.into(), y.into(), near_copper.as_str().into()],
                            },
                        )?;
                    }
                    None => profile(&mut b, body, 0.into(), &near_copper)?,
                },
                plan::PadPart::Opposite => profile(
                    &mut b,
                    body,
                    format!("- board_thickness - {far_copper}").into(),
                    &far_copper,
                )?,
                plan::PadPart::Barrel(_) => {
                    profile(&mut b, body, "- board_thickness".into(), "board_thickness")?
                }
            }
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
            component: created.component,
            ..source
        });
    }
    let mut paths = Vec::with_capacity(project.tracks.len());
    if plan.tracks.len() != project.tracks.len() {
        return Err(Error::Verification(
            "the board plan holds another number of tracks".into(),
        ));
    }
    for (track, planned) in project.tracks.iter().zip(&plan.tracks) {
        let mount = match track.layer.side() {
            Some(side) => Mount::Side(side),
            None => Mount::Height(inner_base(stackup, track.layer)?.into()),
        };
        let prefix = format!("trace_{}", track.id);
        let created = b.native.component(&prefix, false)?;
        let angle = format!("atan2({prefix}_y1-{prefix}_y0; {prefix}_x1-{prefix}_x0)");
        // The stadium in the track's frame: start at the origin, along +x.
        b.native.whole_profile_body(ProfileBody {
            name: &planned.body.name,
            offset: 0.into(),
            contours: &planned.body.loops,
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
        sources.push(track_source(track, pick, created.component, &mut paths));
    }
    let display = appearance::Board::of(project)?;
    let marks = vias(&mut b, project, &mut styles, &mut sources)?;
    for mark in &marks {
        lands.extend(mark.lands(&display));
    }
    // The display-only layers (mask, finish, silkscreen, edge) need no native
    // model: they are drawn while the native one gets its laminate and joints
    // and is finished and meshed.
    let mut memo = std::mem::take(&mut b.display);
    let drawn = Drawn {
        project,
        display: &display,
        lands: &lands,
        paths: &paths,
        marks: &marks,
        fills: &fills,
        parts: &parts,
        picks: part_picks(&sources),
    };
    let native = |mut b: Builder| -> Result<NativeModel> {
        // The laminate last: its sketches hold every drill, which each
        // command after it would otherwise read again.
        b.native.activate(substrate.component)?;
        laminate(&mut b, project, &plan.laminate)?;
        // Every joint goes in once the parts are built, solved together (one
        // by one, each would solve all the joints made before it); a copy of
        // a shared definition starts where its first occurrence stands.
        b.native.board_joints(&joints)?;
        b.finish(sources)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let (model, layers) = std::thread::scope(|scope| {
        let layers = scope.spawn(|| drawn.layers(&mut memo));
        let model = native(b);
        let layers = layers.join().unwrap_or_else(|_| {
            Err(Error::Verification(
                "drawing the board's layers failed".into(),
            ))
        });
        (model, layers)
    });
    #[cfg(target_arch = "wasm32")]
    let (model, layers) = (native(b), drawn.layers(&mut memo));
    let mut model = model?;
    model.geometry.extend(layers?);
    model.display = memo;
    model.plans = plans;
    model.stack = crate::stack::Stack::of(stackup, project.fabrication.pad_finish)?;
    Ok(model)
}

/// A placed pad's display lands (its copper, bore and mask openings on
/// each outer layer, picked as `pick` where it has copper) into `lands`,
/// and its source when it has copper, its bodies in no component yet.
fn pad_lands(pad: &PlacedPad<'_>, pick: u64, lands: &mut Vec<Land>) -> Result<Option<Source>> {
    let local = pad.pad();
    let copper = local.kind.copper();
    // The outer layers the pad has copper on, its own side first.
    let layers: Vec<Layer> = [pad.placed.layer, other(pad.placed.layer)]
        .into_iter()
        .filter(|l| pad.placed.on_copper(*l))
        .collect();
    let shape = pad_outline::shape_contour(&local.shape, 0.0);
    let mask = pad_outline::mask_opening(local, &pad.defaults);
    let drills: Vec<Vec<Point>> = pad
        .drills()
        .iter()
        .map(|d| flat(&pad.board(d)))
        .collect::<Result<_>>()?;
    let bores = pad.bores();
    let opening_layers = if drills.is_empty() && copper != PadCopper::None {
        layers.clone()
    } else {
        vec![Layer::Top, Layer::Bottom]
    };
    for layer in opening_layers {
        let has_copper = layers.contains(&layer);
        let mut openings = drills.clone();
        if (has_copper || copper == PadCopper::None)
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
    if copper == PadCopper::None {
        return Ok(None);
    }
    Ok(Some(Source {
        pick_id: pick,
        instance: Some(pad.placed.instance),
        pad: Some(local.id),
        track: None,
        via: None,
        fill: None,
        net: pad.placed.net,
        layer: Some(pad.placed.layer.into()),
        label: format!("{} pad {}", pad.designator, local.number),
        component: NO_COMPONENT,
        occurrence: None,
    }))
}

/// A track's source, picked as `pick` with its body in `component`; an
/// outer track's path (for the mask cap and finish over it) into `paths`.
fn track_source(
    track: &sparksmith_core::Track,
    pick: u64,
    component: u64,
    paths: &mut Vec<Path>,
) -> Source {
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
    Source {
        pick_id: pick,
        instance: None,
        pad: None,
        track: Some(track.id),
        via: None,
        fill: None,
        net: Some(track.net),
        layer: Some(track.layer),
        label: format!("Trace {}", track.id),
        component,
        occurrence: None,
    }
}

/// Each placed instance's pick (its model or envelope's source).
fn part_picks(sources: &[Source]) -> BTreeMap<Id, u64> {
    sources
        .iter()
        .filter(|s| s.pad.is_none())
        .filter_map(|s| s.instance.map(|i| (i, s.pick_id)))
        .collect()
}

/// What the display-only layers of a board are drawn from.
struct Drawn<'a> {
    project: &'a Project,
    display: &'a appearance::Board,
    lands: &'a [Land],
    paths: &'a [Path],
    marks: &'a [ViaMark],
    fills: &'a [fill::FillMark],
    parts: &'a [Part<'a>],
    picks: BTreeMap<Id, u64>,
}

impl Drawn<'_> {
    /// The display-only layers (mask, finish, films over the fills, bare
    /// laminate, legend and edge bands), each kind on a core of its own.
    fn layers(&self, memo: &mut appearance::Memo) -> Result<Vec<Geometry>> {
        let display = self.display;
        let films = || fill::films(display, self.lands, self.marks, self.fills);
        let open = || {
            appearance::open_laminate(display, self.lands, self.paths, &fill::outlines(self.fills))
        };
        let pours = fill::regions(self.fills);
        let legend_memo = memo.take_legend();
        let legend = || {
            crate::silkscreen::films(
                self.project,
                &crate::silkscreen::Legend {
                    board: display,
                    lands: self.lands,
                    paths: self.paths,
                    vias: self.marks,
                    pours: &pours,
                    picks: &self.picks,
                },
                &legend_memo,
            )
        };
        // The films over the fills, the bare laminate and the legend need
        // nothing of each other: each is drawn on a core of its own.
        #[cfg(not(target_arch = "wasm32"))]
        let (layers, films, open, legend) = std::thread::scope(|scope| {
            let films = scope.spawn(films);
            let open = scope.spawn(open);
            let legend = scope.spawn(legend);
            let layers = appearance::board_with(
                display, self.lands, self.paths, self.marks, self.parts, memo,
            );
            let failed = || Err(Error::Verification("drawing a display layer failed".into()));
            (
                layers,
                films.join().unwrap_or_else(|_| failed()),
                open.join().unwrap_or_else(|_| failed()),
                legend.join().unwrap_or_else(|_| {
                    Err(Error::Verification("drawing the legend failed".into()))
                }),
            )
        });
        #[cfg(target_arch = "wasm32")]
        let (layers, films, open, legend) = (
            appearance::board_with(
                display, self.lands, self.paths, self.marks, self.parts, memo,
            ),
            films(),
            open(),
            legend(),
        );
        let mut layers = layers?;
        let (legend, legend_memo) = legend?;
        memo.set_legend(legend_memo);
        layers.extend(films?);
        layers.extend(open?);
        layers.extend(legend);
        layers.extend(crate::stackup_edge::bands(&self.project.stackup, display)?);
        Ok(layers)
    }
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

/// The definition of `via`'s size, its bodies the via's plan's.
fn via_style(
    b: &mut Builder,
    project: &Project,
    via: &sparksmith_core::Via,
    planned: &plan::ViaPlan,
) -> Result<ViaStyle> {
    let stackup = &project.stackup;
    let name = format!("via_{}_{}", mm_slug(via.drill), mm_slug(via.diameter));
    let created = b.native.component(&name, false)?;
    for body in &planned.bodies {
        let plan::Origin::Via { part, .. } = body.source else {
            return Err(Error::Verification(format!(
                "via {}'s plan holds body {}",
                via.id, body.name
            )));
        };
        // Outer rings cap the barrel; inner rings stand round it, in the
        // circle the laminate's foil level leaves for them.
        let (offset, height): (Expr, String) = match part {
            plan::ViaPart::Barrel => (0.into(), "board_thickness".into()),
            plan::ViaPart::Ring(layer) => {
                let height = copper_param(layer);
                let offset = match layer {
                    CopperLayer::Top => "board_thickness".into(),
                    CopperLayer::Bottom => format!("- {height}").into(),
                    CopperLayer::Inner(_) => inner_base(stackup, layer)?.into(),
                };
                (offset, height)
            }
        };
        b.native.profile_body(ProfileBody {
            name: &body.name,
            offset,
            contours: &body.loops,
            height: height.as_str().into(),
        })?;
        b.record(&body.name, BodyKind::ViaCopper)?;
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
fn via_styles(
    b: &mut Builder,
    project: &Project,
    planned: &[plan::ViaPlan],
) -> Result<BTreeMap<ViaKey, ViaStyle>> {
    if planned.len() != project.vias.len() {
        return Err(Error::Verification(
            "the board plan holds another number of vias".into(),
        ));
    }
    let mut styles: BTreeMap<ViaKey, ViaStyle> = BTreeMap::new();
    for (via, planned) in project.vias.iter().zip(planned) {
        let key = (via.drill.to_bits(), via.diameter.to_bits());
        if let std::collections::btree_map::Entry::Vacant(slot) = styles.entry(key) {
            slot.insert(via_style(b, project, via, planned)?);
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
    use_: &sparksmith_core::library::package_model::PartFileModel,
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
    if model.format != sparksmith_core::library::model3d::ModelFormat::Solidcraft {
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
        style_of.push(Some(style.component));
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
    marks(project, &style_of, &occurrences, sources)
}

/// Each via's source and what the display layers draw of it: the via
/// placed as `occurrences` of the definitions `components` (each `None`
/// while the via has no native body, [`Flat::Meshed`]).
fn marks(
    project: &Project,
    components: &[Option<u64>],
    occurrences: &[Option<u64>],
    sources: &mut Vec<Source>,
) -> Result<Vec<ViaMark>> {
    let mut marks = Vec::with_capacity(project.vias.len());
    let expansion = project.rules.effective().mask.expansion;
    for ((via, component), occurrence) in project.vias.iter().zip(components).zip(occurrences) {
        let (component, occurrence) =
            match component {
                Some(component) => (
                    *component,
                    Some(occurrence.ok_or_else(|| {
                        Error::Verification(format!("via {} is unplaced", via.id))
                    })?),
                ),
                None => (NO_COMPONENT, None),
            };
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
            occurrence,
        });
        marks.push(ViaMark {
            pick,
            at: via.position,
            drill: via.drill,
            diameter: via.diameter,
            tented: via.tented,
            mask_opening: via.mask_opening(expansion),
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
                .zip(package.envelope())
            {
                out.push((format!("{prefix}_{axis}"), value, "mm"));
            }
        }
    }
    for pad in pads {
        let local = pad.pad();
        if local.kind.copper() == PadCopper::None
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

/// The laminate's slabs ([`plan::BoardPlan::laminate`]), each extruded
/// with its holes: the saved bodies are disjoint and the laminate's mass
/// is physical, without a Boolean, so a board of thousands of vias is
/// built in time linear in their number. A rectangular board's outline
/// follows `board_width` and `board_depth`; a sketched board's is its
/// solved outline ([`crate::board::laminate`]), and it does not follow
/// those parameters. The holes sit where the board's copper is now (they
/// do not follow placement parameters edited in Solidcraft); the slabs'
/// heights follow the stackup parameters.
fn laminate(b: &mut Builder, project: &Project, slabs: &[plan::PlanBody]) -> Result<()> {
    let stackup = &project.stackup;
    let count = stackup.dielectrics.len();
    // A sketched board's outline; a rectangular one's follows the size
    // parameters ([`Native::board_slab`]).
    let shaped = !project.board_shape.is_rectangle();
    for slab in slabs {
        let plan::Origin::Laminate(level) = slab.source else {
            return Err(Error::Verification(format!(
                "the laminate's plan holds body {}",
                slab.name
            )));
        };
        let (outer, holes) = slab
            .loops
            .split_first()
            .ok_or_else(|| Error::Verification(format!("slab {} has no outline", slab.name)))?;
        let (offset, height): (String, String) = match level {
            plan::Level::Dielectric(index) => (
                dielectric_sum(stackup, index + 1..count),
                dielectric_sum(stackup, index..index + 1),
            ),
            plan::Level::Foil { layer, .. } => (inner_base(stackup, layer)?, copper_param(layer)),
            plan::Level::UnderFoil(index) => {
                let foil = stackup
                    .layers()
                    .into_iter()
                    .find(|l| l.side().is_none() && stackup.index(*l) == Some(index))
                    .ok_or_else(|| {
                        Error::Verification(format!("slab {} has no foil above it", slab.name))
                    })?;
                (
                    dielectric_sum(stackup, index + 1..count),
                    format!(
                        "{} - {}",
                        dielectric_sum(stackup, index..index + 1),
                        copper_param(foil)
                    ),
                )
            }
        };
        let body = ProfileBody {
            name: &slab.name,
            offset: offset.as_str().into(),
            contours: holes,
            height: height.as_str().into(),
        };
        // A slab in pieces (laminate left in a fill's clearance) is one
        // body per piece.
        let made = if shaped {
            b.native.outlined_slab(body, outer)?
        } else {
            b.native.board_slab(body, BOARD_SIZE)?
        };
        if !made.contains(&slab.name) {
            return Err(Error::Verification(format!(
                "slab {} made no body",
                slab.name
            )));
        }
        for piece in made {
            b.record(&piece, BodyKind::Substrate)?;
        }
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
        .zip(package.envelope())
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
    let mut b = Builder::new("Package model", &SourceCache::default())?;
    let created = b.model(DRAFT_MODEL, package)?;
    move_by(&mut b.native, created.occurrence, &package.placement)?;
    b.finish(vec![])
}

pub mod designs;
mod fill;
pub mod fit;
mod identity;
pub mod mesh;
pub mod plan;
#[cfg(test)]
mod realism_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod update_tests;
