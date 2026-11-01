//! Slice B1 (contract C0): templates as data. A package-family generator
//! (Rust: a [`PackageForm`] variant, its footprint template, its model
//! builder) is driven by **dimension-table rows**: revisioned library
//! lineages of kind `PackageRow`, with per-row citations, searched by the
//! gallery. A **skeleton** is the symbol and field starting point.
//!
//! A row's values are the family's own typed form, so a row is checked by
//! the same `range_problems` as a package and can never describe something
//! its generator cannot build. Aliases are text, never identity.
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{DefinitionId, PackageRowRef};
use crate::library::bundle::Bundle;
use crate::library::package_form::PackageForm;
use crate::library::part::Provenance;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Rows one library source holds (target).
pub const MAX_PACKAGE_ROWS_PER_SOURCE: usize = 4096;
/// Aliases one row lists.
pub const MAX_ROW_ALIASES: usize = 64;
/// Bytes of one alias.
pub const MAX_ALIAS: usize = 64;
/// Drafts `library.new_from_template` creates in one undo step: component,
/// symbol, package, footprint, binding.
pub const NEW_FROM_TEMPLATE_DRAFTS: usize = 5;

/// A package family: one [`PackageForm`] variant with its generators.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum FamilyKind {
    Chip,
    GullWing,
    NoLead,
    Header,
    /// B2a: SOT-23/223/89, DPAK.
    Tabbed,
    /// B2b: SOD, SMA/SMB/SMC, molded tantalum, MELF.
    Molded,
    /// B2c: BGA, LGA, WLCSP.
    Grid,
    /// B2d: axial, radial, DIP, TO.
    Tht,
}

impl FamilyKind {
    pub const ALL: [Self; 8] = [
        Self::Chip,
        Self::GullWing,
        Self::NoLead,
        Self::Header,
        Self::Tabbed,
        Self::Molded,
        Self::Grid,
        Self::Tht,
    ];

    /// The family of a typed form; `None` for `Generic`.
    pub fn of(form: &PackageForm) -> Option<Self> {
        Some(match form {
            PackageForm::Generic => return None,
            PackageForm::Chip(_) => Self::Chip,
            PackageForm::GullWing(_) => Self::GullWing,
            PackageForm::NoLead(_) => Self::NoLead,
            PackageForm::Header(_) => Self::Header,
            PackageForm::Tabbed(_) => Self::Tabbed,
            PackageForm::Molded(_) => Self::Molded,
            PackageForm::Grid(_) => Self::Grid,
            PackageForm::Tht(_) => Self::Tht,
        })
    }
}

/// The family and family revision a row's values were checked against.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FamilyRef {
    pub family: FamilyKind,
    pub family_rev: u16,
}

/// A chip passive's size code, both systems explicit (`0603` imperial is
/// `1608` metric). `pkg:0402` matches both meanings; nothing is guessed.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SizeCode {
    pub imperial: String,
    pub metric: String,
}

/// Optional land overrides for the family's footprint template, from a
/// cited manufacturer land pattern (template defaults are estimates).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LandValues {
    #[serde(default)]
    pub pad_length: Option<f64>,
    #[serde(default)]
    pub pad_width: Option<f64>,
    /// Gap between opposite lands, or row span centre to centre.
    #[serde(default)]
    pub gap: Option<f64>,
    #[serde(default)]
    pub exposed_pad: Option<[f64; 2]>,
}

/// The symbol and field starting point a new component gets.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "skeleton", rename_all = "snake_case")]
pub enum SkeletonHint {
    /// A 2-terminal passive (R/C/L/D/F/FB chosen by the user).
    TwoTerminal {
        polarised: bool,
    },
    /// Pins down the left and right in package order, all passive.
    IcBox,
    Connector,
    #[default]
    Blank,
}

/// One dimension-table row.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageRowDefinition {
    /// `None` only in an empty draft.
    pub family: Option<FamilyRef>,
    /// Exactly the family's typed parameters.
    pub values: PackageForm,
    /// "SOIC-8", "SO-8": text, never identity.
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub size_code: Option<SizeCode>,
    #[serde(default)]
    pub lands: Option<LandValues>,
    #[serde(default)]
    pub skeleton: SkeletonHint,
    /// Origin and licence of the row.
    #[serde(default)]
    pub provenance: Provenance,
    /// The public outline drawing the values were typed from; `None` marks
    /// the row estimated.
    #[serde(default)]
    pub citation: Option<RowCitation>,
}

/// A public package outline drawing (IEC 60191 / JEDEC registered outline
/// or a manufacturer's package drawing). Never an EDA library or table.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RowCitation {
    pub document: String,
    #[serde(default)]
    pub document_revision: Option<String>,
    #[serde(default)]
    pub page: Option<u16>,
}

impl PackageRowDefinition {
    pub(crate) fn within_bounds(&self) -> bool {
        self.aliases.len() <= MAX_ROW_ALIASES
            && self.aliases.iter().all(|a| a.len() <= MAX_ALIAS)
            && self.values.within_bounds()
    }

    pub(crate) fn values_finite(&self) -> bool {
        self.values.lengths().iter().all(|v| v.is_finite())
    }

    /// Publish findings (`package_row.family`, `package_row.range`,
    /// `package_row.alias`, `package_row.provenance`). Contract: refused
    /// with `package_row.unsupported` until B1.
    pub(crate) fn findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        out.push(Finding::blocking(
            "package_row.unsupported",
            "Package template rows cannot be published yet.".into(),
            vec![ObjectRef::Definition { id }],
            Stage::Publish,
        ));
    }
}

/// Where a template row comes from.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum RowSource {
    /// A published row in the project.
    Project { row: PackageRowRef },
    /// A folder row, carried inline as `library.place_from` carries items.
    Bundle { bundle: Box<Bundle> },
    /// The family's built-in custom entry: values come with the request.
    Custom { family: FamilyKind },
}

/// The `library.new_from_template` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NewFromTemplate {
    pub row: RowSource,
    /// Edited values; the row's when `None` (required for `Custom`).
    #[serde(default)]
    pub values: Option<PackageForm>,
    pub skeleton: SkeletonHint,
    pub name: String,
    #[serde(default)]
    pub designator_prefix: Option<String>,
}

/// The `library.package_from_template` request.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageFromTemplate {
    /// The package draft of the open working set.
    pub draft: DefinitionId,
    pub row: RowSource,
    #[serde(default)]
    pub values: Option<PackageForm>,
    /// Replace a non-empty footprint instead of refusing it.
    #[serde(default)]
    pub replace: bool,
}

/// What `library.package_from_template` changes, previewed before Apply.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackagePlan {
    pub terminals_before: u32,
    pub terminals_after: u32,
    pub pads_after: u32,
    /// The generated model's family revision, if the family builds one.
    pub model_family_rev: Option<u16>,
    pub findings: Vec<Finding>,
}

/// The `library.save_package_row` request: from a package draft's form or
/// from explicit values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum RowFrom {
    Package { draft: DefinitionId },
    Values { values: PackageForm },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SavePackageRow {
    pub from: RowFrom,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub provenance: Provenance,
}

/// Contract stub until B1.
pub fn unsupported() -> Error {
    Error::Unsupported(
        "template.unsupported: data-driven package templates are not available yet (slice B1)"
            .into(),
    )
}

/// The drafts of a new component built from a row. Contract stub until B1.
pub fn new_from_template(
    _request: &NewFromTemplate,
) -> Result<Vec<(super::super::DefinitionMeta, super::super::Definition)>> {
    Err(unsupported())
}
