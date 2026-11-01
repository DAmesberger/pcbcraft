//! Slice B1 (contract C0): template rows in the catalog. `library.templates`
//! searches package rows of the project and every source plus each
//! family's built-in "custom" entry. Facets: family, pin count, pitch,
//! body L×W×H, mount, exposed pad, size code. Reserved filters `pins:`,
//! `pitch:` and `body:` take typed, unit-aware values (`soic pins:14`,
//! `qfn body:5x5 pitch:0.5`); `pkg:0402` matches both imperial and metric
//! meanings and `pkg:i0402` / `pkg:m0402` disambiguate.
use super::{CatalogContext, Source};
use crate::diagnostics::{Error, Result};
use crate::ids::PackageRowRef;
use crate::library::Drafts;
use crate::library::package_form::PackageForm;
use crate::library::templates::catalog::{FamilyKind, SizeCode};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Hits one `library.templates` returns at most.
pub const MAX_TEMPLATE_HITS: usize = 500;

/// One gallery row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemplateHit {
    pub family: FamilyKind,
    /// `None` for the family's built-in custom entry.
    #[serde(default)]
    pub row: Option<PackageRowRef>,
    pub source: Source,
    pub name: String,
    pub aliases: Vec<String>,
    pub values: PackageForm,
    #[serde(default)]
    pub size_code: Option<SizeCode>,
    /// The row cites an outline drawing (else its values are estimated).
    pub cited: bool,
    /// The lands come from a cited land pattern (else estimated defaults).
    pub lands_cited: bool,
}

/// `library.templates`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemplateReport {
    pub hits: Vec<TemplateHit>,
    /// Matches beyond the limit.
    pub more: usize,
}

/// Contract stub until B1.
pub fn search(
    _project: &Project,
    _drafts: &Drafts,
    _context: &CatalogContext,
    _query: &str,
    _limit: usize,
) -> Result<TemplateReport> {
    Err(Error::Unsupported(
        "template.unsupported: the template gallery is not available yet (slice B1)".into(),
    ))
}
