//! Slice D1 (contract C0): the 3D library is the catalog filtered to
//! models, over the project and every source, plus the generated families
//! as virtual entries. Facets: format, mount, pins, pitch, body L×W×H
//! (from bounds), licence/redistribution, source, `has:leads`,
//! `has:marking`, `has:textures`. Opened from a package, hits are ranked
//! "fits this package": lead-tagged models by `fit_leads` rms against the
//! land pads, the rest by bounds against the envelope. A score is shown,
//! never auto-assigned.
use super::{CatalogContext, Source};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, ModelRef};
use crate::library::Drafts;
use crate::library::model3d::{ModelFormat, ModelHints, Redistribution};
use crate::library::templates::catalog::FamilyKind;
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Hits one `library.models` returns at most.
pub const MAX_MODEL_HITS: usize = 500;

/// How well a model fits the package the chooser was opened from.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "fit", rename_all = "snake_case")]
pub enum FitScore {
    /// Lead contacts against land pads, rms mm.
    Leads { rms_mm: f64 },
    /// Largest excess of model bounds over the envelope, mm (negative:
    /// inside).
    Bounds { excess_mm: f64 },
}

/// What a hit offers.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "hit", rename_all = "snake_case")]
pub enum ModelHitKind {
    Library {
        model: ModelRef,
        format: ModelFormat,
        redistribution: Redistribution,
        has_leads: bool,
        has_marking: bool,
        has_textures: bool,
    },
    /// "Generated · gull-wing": picks a template row.
    Generated { family: FamilyKind },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelHit {
    pub kind: ModelHitKind,
    pub source: Source,
    pub name: String,
    pub hints: ModelHints,
    #[serde(default)]
    pub body_mm: Option<[f64; 3]>,
    #[serde(default)]
    pub fit: Option<FitScore>,
}

/// `library.models`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelReport {
    pub hits: Vec<ModelHit>,
    pub more: usize,
}

/// Contract stub until D1. `fits` is the package draft the chooser was
/// opened from.
pub fn search(
    _project: &Project,
    _drafts: &Drafts,
    _context: &CatalogContext,
    _query: &str,
    _fits: Option<DefinitionId>,
    _limit: usize,
) -> Result<ModelReport> {
    Err(Error::Unsupported(
        "model.unsupported: the 3D model library is not available yet (slice D1)".into(),
    ))
}
