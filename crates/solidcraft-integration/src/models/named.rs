//! Generated, envelope and STEP models as standalone named designs
//! (docs/board-sketch-references.md §6.2; slice S4).
//!
//! Solidcraft names a face after the feature that made it, by feature id,
//! so a face name is stable exactly as long as the commands that build its
//! design are. Each model kind without a part file gets a design of its
//! own, built by a fixed command sequence from the package alone:
//!
//! - **Generated**: the family builder in the root of a new design
//!   ([`super::build_standalone`]), parameters and bodies by their own
//!   names. The board inserts this design ([`super::part_file::
//!   insert_design`]), so the 3D build, the saved `.solidcraft` and face
//!   references share the one build.
//! - **Envelope**: one box feature, `envelope`, from the package envelope.
//!   The board keeps its own per-placement box (its size is a placement
//!   parameter a 2D edit replays in place); the box is the same.
//! - **STEP**: one import feature holding the file's bodies, named by their
//!   keys in the file, then the source frame's scale, turn and offset. The
//!   parse is shared with the board build's source cache
//!   ([`super::imported::checked_source`]). The import numbers each
//!   body's faces in file order (`F1:face:4`): the same for the same
//!   bytes, but no evidence of the same face in another file.
//!
//! Designs are kept in one process-wide cache by what they are built from,
//! least recently used last, like the part-file cache.
use super::imported::{self, checked_source};
use super::{GeneratedBody, is_generated};
use crate::native::{BoxBody, Expr, Native};
use crate::{Error, Result};
use pcbcraft_core::library::PackageDefinition;
use pcbcraft_core::library::bundle::asset_hash;
use pcbcraft_core::library::model3d::{ModelDefinition, ModelFormat};
use serde_json::json;
use solidcraft_engine::doc::{Document, ModelState};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};

/// Designs the cache keeps (a target bound: more than the distinct
/// packages of a large board, so a rebuild does not build them again).
pub const CACHE_ENTRIES: usize = 256;
/// The envelope design's one body.
pub const ENVELOPE_BODY: &str = "envelope";

/// A model built as a design of its own.
pub struct NamedDesign {
    /// SHA-256 of what it is built from.
    key: [u8; 32],
    /// The design, evaluated.
    source: Arc<Document>,
    /// Its bodies with their persistent face names, in the model frame.
    state: Arc<ModelState>,
    /// A generated model's bodies by their names in the design, with role,
    /// material and terminal (what the board inserts); empty otherwise.
    pub generated: Vec<GeneratedBody>,
}

impl std::fmt::Debug for NamedDesign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NamedDesign")
            .field("bodies", &self.state.bodies.len())
            .field("generated", &self.generated.len())
            .finish_non_exhaustive()
    }
}

impl NamedDesign {
    /// The design, as built.
    pub fn source(&self) -> &Document {
        &self.source
    }

    /// The evaluated bodies, model frame: what dimension references to
    /// the model's faces are traced and picked on.
    pub fn state(&self) -> &ModelState {
        &self.state
    }
}

struct Cache {
    entries: VecDeque<Arc<NamedDesign>>,
    hits: u64,
    builds: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(Cache {
            entries: VecDeque::new(),
            hits: 0,
            builds: 0,
        })
    })
}

/// (cache hits, designs built) so far, process-wide.
pub fn cache_counts() -> (u64, u64) {
    cache().lock().map_or((0, 0), |c| (c.hits, c.builds))
}

/// The design described by `what`, from the cache or built now.
fn cached(
    what: &serde_json::Value,
    build: impl FnOnce() -> Result<NamedDesign>,
) -> Result<Arc<NamedDesign>> {
    let key = asset_hash(&serde_json::to_vec(what)?).0;
    if let Ok(mut c) = cache().lock()
        && let Some(at) = c.entries.iter().position(|e| e.key == key)
        && let Some(hit) = c.entries.remove(at)
    {
        c.hits += 1;
        c.entries.push_front(Arc::clone(&hit));
        return Ok(hit);
    }
    let mut design = build()?;
    design.key = key;
    let design = Arc::new(design);
    if let Ok(mut c) = cache().lock() {
        c.builds += 1;
        c.entries.push_front(Arc::clone(&design));
        c.entries.truncate(CACHE_ENTRIES);
    }
    Ok(design)
}

/// A finished design.
fn finish(native: &Native, generated: Vec<GeneratedBody>) -> Result<NamedDesign> {
    let session = native.session();
    if let Some(error) = native.evaluation_error() {
        return Err(Error::Verification(format!(
            "model.build: the model's design did not evaluate: {error}"
        )));
    }
    Ok(NamedDesign {
        key: [0; 32],
        source: Arc::clone(&session.doc),
        state: crate::native::world_state(session)?,
        generated,
    })
}

/// The design of a package's model when it has no part file or library
/// model: the generated model, or else the envelope (as the board builds
/// it).
pub fn package(package: &PackageDefinition) -> Result<Arc<NamedDesign>> {
    if is_generated(package) {
        generated(package)
    } else {
        envelope(package)
    }
}

/// The standalone design of a generated model.
pub fn generated(package: &PackageDefinition) -> Result<Arc<NamedDesign>> {
    let terminals: Vec<u32> = package.terminals.iter().map(|t| t.id.0).collect();
    let what = json!({
        "generated": package.form,
        "model": package.model,
        "terminals": terminals,
    });
    cached(&what, || {
        let mut native = Native::new("Package model")?;
        // Evaluated once at the end, as the board build evaluates it.
        native.defer_evaluation()?;
        let bodies = super::build_standalone(&mut native, package)?;
        native.evaluate_deferred()?;
        native.forget_history();
        finish(&native, bodies)
    })
}

/// The one-box design of a package envelope: corner (−l/2, −w/2, 0), size
/// l × w × h, in the model frame.
pub fn envelope(package: &PackageDefinition) -> Result<Arc<NamedDesign>> {
    if !package.envelope_valid() {
        return Err(Error::Invalid(
            "package envelope dimensions must be finite and positive".into(),
        ));
    }
    let what = json!({ "envelope": package.envelope });
    cached(&what, || {
        let mut native = Native::new("Package envelope")?;
        for (name, value) in ["length", "width", "height"]
            .into_iter()
            .zip(package.envelope)
        {
            native.parameter(name, crate::native::mm(value))?;
        }
        native.solid_box(BoxBody {
            name: ENVELOPE_BODY,
            corner: [
                Expr::from("-length / 2"),
                Expr::from("-width / 2"),
                Expr::from(0),
            ],
            size: [
                Expr::from("length"),
                Expr::from("width"),
                Expr::from("height"),
            ],
        })?;
        native.forget_history();
        // The board does not insert an envelope design.
        finish(&native, Vec::new())
    })
}

/// The design that imports a STEP library model (its packed `bytes`): its
/// bodies named by their keys in the file, in the model frame.
pub fn step(model: &ModelDefinition, bytes: &[u8]) -> Result<Arc<NamedDesign>> {
    if model.format != ModelFormat::Step {
        return Err(Error::Invalid(format!(
            "model.unsupported: only STEP models are imported into a design here, not {}",
            model.format.label()
        )));
    }
    let what = json!({ "step": model.asset.hash, "frame": model.frame });
    cached(&what, || {
        let source = checked_source(model, bytes)?;
        let names = source.keys.clone();
        let mut native = Native::new("Package model")?;
        imported::add_import(
            &mut native,
            None,
            "model",
            imported::Source::clone(&source),
            (&model.frame, model.format),
            &names,
        )?;
        native.forget_history();
        finish(&native, Vec::new())
    })
}

#[cfg(test)]
#[path = "named_tests.rs"]
mod tests;
