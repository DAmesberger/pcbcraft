//! Slice B1: templates as data. A package-family generator
//! (Rust: a [`PackageForm`] variant, its footprint template, its model
//! builder) is driven by **dimension-table rows**: revisioned library
//! lineages of kind `PackageRow`, with per-row citations, searched by the
//! gallery. A **skeleton** is the symbol and field starting point.
//!
//! A row's values are the family's own typed form, so a row is checked by
//! the same `range_problems` as a package and can never describe something
//! its generator cannot build. Aliases are text, never identity.
use crate::diagnostics::{Error, Finding, ObjectRef, Result, Stage};
use crate::ids::{DefinitionId, MAX_LABEL, PackageRowRef, printable};
use crate::library::bundle::Bundle;
use crate::library::package_form::{
    ChipKind, GridForm, ModuleForm, MoldedForm, PackageForm, TabbedForm, ThtForm, UsbCForm,
};
use crate::library::part::{Basis, Provenance};
use crate::library::samples::{self, Preset, SampleRefs, TemplateSpec};
use crate::library::symbol_style::{SymbolConventions, SymbolKind};
use crate::library::templates::footprint_family::TemplateKind;
use crate::library::{Definition, DefinitionMeta, DraftEdit, Library};
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
/// Bytes of one size code (`01005`, `1608`).
pub const MAX_SIZE_CODE: usize = 8;
/// Bytes of a citation's document revision.
pub const MAX_CITATION_REVISION: usize = 64;
/// Bytes of a designator prefix a template request names.
pub const MAX_PREFIX: usize = 16;
/// The revision of the four families whose generators exist (`Chip`,
/// `GullWing`, `NoLead`, `Header`): forms, lands and model builders at
/// `models::family_rev` 1.
pub const BUILT_FAMILY_REV: u16 = 1;

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
    /// Radio and processor modules: a board with a shield can, edge pad
    /// rows, a ground array and an antenna end.
    Module,
    /// USB Type-C receptacles for USB 2.0: surface-mount contacts and
    /// through-hole shell stakes.
    UsbC,
}

impl FamilyKind {
    pub const ALL: [Self; 10] = [
        Self::Chip,
        Self::GullWing,
        Self::NoLead,
        Self::Header,
        Self::Tabbed,
        Self::Molded,
        Self::Grid,
        Self::Tht,
        Self::Module,
        Self::UsbC,
    ];

    /// The parametric footprint template kind that builds this family's
    /// lands; `None` for a module or a USB-C receptacle, whose lands come
    /// only from its form (`library.package_from_template`).
    pub fn template_kind(self) -> Option<TemplateKind> {
        Some(match self {
            Self::Chip => TemplateKind::Chip,
            Self::GullWing => TemplateKind::GullWing,
            Self::NoLead => TemplateKind::Qfn,
            Self::Header => TemplateKind::PinHeader,
            Self::Tabbed => TemplateKind::Tabbed,
            Self::Molded => TemplateKind::Molded,
            Self::Grid => TemplateKind::Grid,
            Self::Tht => TemplateKind::Tht,
            Self::Module | Self::UsbC => return None,
        })
    }

    /// The gallery's outline names of the family.
    pub fn outline_label(self) -> &'static str {
        match self {
            Self::Module => "Radio / MCU module",
            Self::UsbC => "USB Type-C receptacle",
            _ => self.template_kind().map_or("", TemplateKind::label),
        }
    }

    /// The family revision rows are checked against now.
    pub fn family_rev(self) -> u16 {
        BUILT_FAMILY_REV
    }

    /// The gallery's family label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chip => "Chip",
            Self::GullWing => "Gull-wing",
            Self::NoLead => "No-lead",
            Self::Header => "Pin header",
            Self::Tabbed => "Tabbed gull-wing",
            Self::Molded => "Molded 2-terminal",
            Self::Grid => "Grid array",
            Self::Tht => "Through-hole",
            Self::Module => "Module",
            Self::UsbC => "USB-C receptacle",
        }
    }

    /// The values of the family's built-in "custom" entry: original typical
    /// values a user then edits (estimated, never a cited row).
    pub fn custom_values(self) -> PackageForm {
        match self {
            Self::Chip => Preset::Chip0603.form(),
            Self::GullWing => Preset::Soic8.form(),
            Self::NoLead => Preset::Qfn16.form(),
            Self::Header => Preset::Header2x5.form(),
            Self::Tabbed => PackageForm::Tabbed(Box::new(TabbedForm::typical())),
            Self::Molded => PackageForm::Molded(Box::new(MoldedForm::typical())),
            Self::Grid => PackageForm::Grid(Box::new(GridForm::typical())),
            Self::Tht => PackageForm::Tht(Box::new(ThtForm::typical())),
            Self::Module => PackageForm::Module(Box::new(ModuleForm::typical())),
            Self::UsbC => PackageForm::UsbC(Box::new(UsbCForm::typical())),
        }
    }

    /// The skeleton a new row of `form` starts from: a chip or molded body
    /// is a 2-terminal part (molded ones polarised), a header or a
    /// receptacle a connector,
    /// anything else an IC box. Never a category guess beyond the family.
    pub fn default_skeleton(form: &PackageForm) -> SkeletonHint {
        match form {
            PackageForm::Chip(_) => SkeletonHint::TwoTerminal { polarised: false },
            PackageForm::Molded(_) => SkeletonHint::TwoTerminal { polarised: true },
            PackageForm::Header(_) | PackageForm::UsbC(_) => SkeletonHint::Connector,
            PackageForm::Generic => SkeletonHint::Blank,
            _ => SkeletonHint::IcBox,
        }
    }

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
            PackageForm::Module(_) => Self::Module,
            PackageForm::UsbC(_) => Self::UsbC,
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

impl SizeCode {
    /// Both codes are 1–[`MAX_SIZE_CODE`] ASCII digits.
    pub fn valid(&self) -> bool {
        let code = |c: &str| {
            !c.is_empty() && c.len() <= MAX_SIZE_CODE && c.bytes().all(|b| b.is_ascii_digit())
        };
        code(&self.imperial) && code(&self.metric)
    }

    /// "0402 imperial · 1005 metric": both meanings, nothing guessed.
    pub fn label(&self) -> String {
        format!("{} imperial · {} metric", self.imperial, self.metric)
    }
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
    /// The schematic symbol the row suggests for its skeleton (a tantalum
    /// row's polarised capacitor, a ferrite chip's bead); `None` leaves it
    /// to the skeleton, form and prefix (`samples::choose_symbol`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<SymbolKind>,
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

impl RowCitation {
    /// The citation records the drawing's revision and page, so a reader
    /// can check the values against it. A citation naming only the
    /// document reads "cited (unverified)".
    pub fn checked(&self) -> bool {
        self.document_revision.is_some() && self.page.is_some()
    }
}

impl PackageRowDefinition {
    pub(crate) fn within_bounds(&self) -> bool {
        self.aliases.len() <= MAX_ROW_ALIASES
            && self.aliases.iter().all(|a| a.len() <= MAX_ALIAS)
            && self.values.within_bounds()
            && self.size_code.as_ref().is_none_or(|c| {
                c.imperial.len() <= MAX_SIZE_CODE && c.metric.len() <= MAX_SIZE_CODE
            })
            && self.citation.as_ref().is_none_or(|c| {
                c.document.len() <= MAX_LABEL
                    && c.document_revision
                        .as_ref()
                        .is_none_or(|r| r.len() <= MAX_CITATION_REVISION)
            })
            && self
                .provenance
                .licence
                .as_ref()
                .is_none_or(|l| l.len() <= MAX_LABEL)
    }

    pub(crate) fn values_finite(&self) -> bool {
        self.values.lengths().iter().all(|v| v.is_finite())
            && self.lands.as_ref().is_none_or(LandValues::finite)
    }

    /// The row's values are cited from an outline drawing.
    pub fn cited(&self) -> bool {
        self.citation.is_some()
    }

    /// The row carries land overrides, which its citation covers (a row
    /// without a citation has estimated lands even when it overrides them).
    /// A USB-C receptacle's values are themselves its land pattern.
    pub fn lands_cited(&self) -> bool {
        (self.lands.is_some() || matches!(self.values, PackageForm::UsbC(_)))
            && self.citation.is_some()
    }

    /// Publish findings: `package_row.family` (no family, a family that
    /// does not match the values, or a family revision the generators do
    /// not build), `package_row.range` (the family's own range rules and
    /// land overrides), `package_row.alias` (blank, repeated or
    /// unprintable aliases and malformed size codes) and
    /// `package_row.provenance` (a malformed citation, or a Part-reference
    /// basis, which a row cannot have). A row without a citation is
    /// estimated, which is allowed.
    pub(crate) fn findings(&self, id: DefinitionId, out: &mut Vec<Finding>) {
        let this = || vec![ObjectRef::Definition { id }];
        let block =
            |code: &str, message: String| Finding::blocking(code, message, this(), Stage::Publish);
        let actual = FamilyKind::of(&self.values);
        match (self.family, actual) {
            (None, _) => out.push(block(
                "package_row.family",
                "A package template row needs a family and family revision.".into(),
            )),
            (Some(_), None) => out.push(block(
                "package_row.family",
                "A package template row needs typed family values.".into(),
            )),
            (Some(family), Some(actual)) if family.family != actual => out.push(block(
                "package_row.family",
                format!(
                    "The row names the {} family but its values are {}.",
                    family.family.label(),
                    actual.label()
                ),
            )),
            (Some(family), Some(_)) => {
                let rev = family.family.family_rev();
                if family.family_rev == 0 || family.family_rev > rev {
                    out.push(block(
                        "package_row.family",
                        format!(
                            "The row was checked against {} family revision {}; revision {rev} is built.",
                            family.family.label(),
                            family.family_rev
                        ),
                    ));
                }
            }
        }
        for problem in self.values.range_problems() {
            out.push(block("package_row.range", problem));
        }
        if let Some(lands) = &self.lands {
            for problem in lands.problems() {
                out.push(block("package_row.range", problem));
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for alias in &self.aliases {
            if !printable(alias) || alias.len() > MAX_ALIAS {
                out.push(block(
                    "package_row.alias",
                    format!("An alias needs 1–{MAX_ALIAS} bytes of printable text."),
                ));
            } else if !seen.insert(fold_text(alias.trim())) {
                out.push(block(
                    "package_row.alias",
                    format!("The alias ‘{}’ is listed twice.", alias.trim()),
                ));
            }
        }
        if let Some(code) = &self.size_code {
            if !code.valid() {
                out.push(block(
                    "package_row.alias",
                    format!(
                        "A size code gives both its imperial and its metric code, 1–{MAX_SIZE_CODE} digits each."
                    ),
                ));
            }
            if !matches!(self.values, PackageForm::Chip(_)) {
                out.push(block(
                    "package_row.alias",
                    "Only chip rows carry a size code.".into(),
                ));
            }
        }
        if let Some(citation) = &self.citation {
            if !printable(&citation.document) {
                out.push(block(
                    "package_row.provenance",
                    "A citation names its public outline drawing in 1–256 bytes of printable text."
                        .into(),
                ));
            }
            if citation
                .document_revision
                .as_ref()
                .is_some_and(|r| !printable(r) || r.len() > MAX_CITATION_REVISION)
            {
                out.push(block(
                    "package_row.provenance",
                    format!(
                        "A citation's document revision is 1–{MAX_CITATION_REVISION} bytes of printable text."
                    ),
                ));
            }
            if citation.page == Some(0) {
                out.push(block(
                    "package_row.provenance",
                    "Citation pages are numbered from 1.".into(),
                ));
            }
        }
        if matches!(self.provenance.basis, Basis::FromReference(_)) {
            out.push(block(
                "package_row.provenance",
                "A row cites its drawing with a citation, not a Part reference.".into(),
            ));
        }
    }

    /// The text a component built from this row records as its drafts'
    /// provenance: the row by name and exact identity (`identity`, see
    /// [`ResolvedRow::identity`]), and its citation or that it is
    /// estimated.
    pub fn provenance_text(&self, name: &str, identity: &str) -> String {
        let basis = match &self.citation {
            Some(c) => {
                let mut text = format!("values from {}", c.document);
                if let Some(rev) = &c.document_revision {
                    text.push_str(&format!(" rev. {rev}"));
                }
                if let Some(page) = c.page {
                    text.push_str(&format!(" p. {page}"));
                }
                if !c.checked() {
                    text.push_str(", citation unverified");
                }
                text
            }
            None => "estimated values".into(),
        };
        let lands = if self.lands_cited() {
            "lands from the cited drawing"
        } else {
            "estimated lands"
        };
        format!(
            "From package template row ‘{name}’ [{identity}] ({basis}; {lands}); verify against the selected component datasheet."
        )
    }
}

impl LandValues {
    fn finite(&self) -> bool {
        [self.pad_length, self.pad_width, self.gap]
            .into_iter()
            .flatten()
            .chain(self.exposed_pad.into_iter().flatten())
            .all(f64::is_finite)
    }

    /// Range problems of the overrides: every given size is positive and
    /// at most the typed-form limit.
    pub fn problems(&self) -> Vec<String> {
        use crate::library::package_form::{FORM_LENGTH_LIMIT_MM, FORM_LENGTH_MIN_MM};
        let mut out = Vec::new();
        let mut check = |name: &str, v: f64| {
            if !(v.is_finite() && (FORM_LENGTH_MIN_MM..=FORM_LENGTH_LIMIT_MM).contains(&v)) {
                out.push(format!(
                    "{name} must be {FORM_LENGTH_MIN_MM}–{FORM_LENGTH_LIMIT_MM} mm (is {v})."
                ));
            }
        };
        if let Some(v) = self.pad_length {
            check("Land length", v);
        }
        if let Some(v) = self.pad_width {
            check("Land width", v);
        }
        if let Some(v) = self.gap {
            check("Land span", v);
        }
        if let Some([x, y]) = self.exposed_pad {
            check("Exposed land length", x);
            check("Exposed land width", y);
        }
        out
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
    /// A row of the built-in starter tables, by its unique name
    /// (`library::templates::starter`).
    Starter { name: String },
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
    /// The schematic symbol to draw; `None` takes the prefix's kind, the
    /// row's suggestion or the form's (`samples::choose_symbol`). Its
    /// terminal count must match the package's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<SymbolKind>,
    /// Leave out the standard fields a chip row otherwise selects and
    /// imports: for a client that defines its own.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub without_fields: bool,
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
    /// How the package's current terminals carry over to the new form's
    /// (and with them the binding's mappings): by number unless the
    /// request chooses position.
    #[serde(default)]
    pub terminals: TerminalMatch,
    /// With [`TerminalMatch::ByNumber`]: current terminals that are the
    /// new form's terminal under another number (a datasheet's "49" for
    /// the form's `EP`), stated explicitly; each keeps its identity and
    /// takes the new number. Never inferred.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renumber: Vec<Renumber>,
}

/// One explicit renumbering of `library.package_from_template`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Renumber {
    /// The current terminal's number.
    pub from: String,
    /// The new form's number it becomes.
    pub to: String,
}

/// How `library.package_from_template` matches the package's current
/// terminals to the terminals of the new form.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalMatch {
    /// A current terminal is kept (identity, and so its mappings) when the
    /// new form gives a terminal its number (designation); every other
    /// new terminal is fresh and unmapped, every other current terminal is
    /// removed, and both are reported. Nothing is matched by list order.
    #[default]
    ByNumber,
    /// The explicit choice to reuse the current terminals in list order
    /// whatever their numbers, renumbered from the new form; reported as
    /// `binding.mapping_retained` for review.
    ByPosition,
}

impl TerminalMatch {
    pub fn label(self) -> &'static str {
        match self {
            Self::ByNumber => "by number",
            Self::ByPosition => "by position",
        }
    }
}

/// How the package's terminals were matched (`PackagePlan::terminals`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TerminalMatchReport {
    pub strategy: TerminalMatch,
    /// Current terminals kept with their identity.
    pub kept: u32,
    /// New terminals with fresh identities (unmapped).
    pub added: u32,
    /// Current terminals the new package removes.
    pub removed: u32,
    /// Kept terminals whose number changes (only by position).
    pub renumbered: u32,
    /// One line saying the strategy and the counts.
    pub summary: String,
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
    /// The footprint draft that lands the package: the one already in the
    /// working set, or the one the command creates.
    #[serde(default)]
    pub footprint: Option<DefinitionId>,
    /// No footprint draft landed the package, so the command creates one.
    #[serde(default)]
    pub footprint_created: bool,
    /// The binding draft a created footprint is listed in: the one binding
    /// draft of the package. `None` with `binding.footprint_unbound` when
    /// there are none or several to choose from.
    #[serde(default)]
    pub footprint_bound: Option<DefinitionId>,
    /// How the current terminals carried over to the new form.
    #[serde(default)]
    pub terminals: TerminalMatchReport,
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

/// A template row ready to build: its display name and definition. A
/// `Custom` source gives an estimated row of the request's values.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedRow {
    pub name: String,
    pub row: PackageRowDefinition,
    /// The row's exact identity, as built drafts record it: "project row
    /// 12 r1", "library 7 lineage 3 r2", "built-in starter table
    /// chip.json format 1" or "custom entry".
    pub identity: String,
}

/// The row a request names, checked: a published project row, the root
/// record of a folder bundle (which must be a package row; nothing is
/// imported), or the family's custom entry. A row with blocking findings is
/// refused.
pub fn resolve_row(library: &Library, source: &RowSource) -> Result<ResolvedRow> {
    let resolved = match source {
        RowSource::Project { row } => {
            let published = library.package_row(*row).ok_or_else(|| {
                Error::NotFound(format!(
                    "template.row_missing: package template row {} r{} is not in the project library",
                    row.id, row.revision
                ))
            })?;
            ResolvedRow {
                name: published.meta.name.clone(),
                row: published.definition.clone(),
                identity: format!("project row {} r{}", row.id, row.revision),
            }
        }
        RowSource::Bundle { bundle } => {
            crate::library::bundle::validate_bundle(bundle)?;
            let record = bundle
                .records
                .iter()
                .find(|r| r.lineage == bundle.root.lineage && r.revision == bundle.root.revision)
                .ok_or_else(|| {
                    Error::Invalid("template.row_missing: the bundle lacks its root record".into())
                })?;
            let Definition::PackageRow(row) = &record.definition else {
                return Err(Error::Invalid(format!(
                    "template.row_kind: the bundle's root is a {}, not a package template row",
                    crate::library::names::noun(record.kind)
                )));
            };
            ResolvedRow {
                name: record.meta.name.clone(),
                row: (**row).clone(),
                identity: format!(
                    "library {} lineage {} r{}",
                    bundle.library, record.lineage, record.revision
                ),
            }
        }
        RowSource::Custom { family } => {
            let values = family.custom_values();
            ResolvedRow {
                name: format!("Custom {}", family.label().to_lowercase()),
                row: PackageRowDefinition {
                    family: Some(FamilyRef {
                        family: *family,
                        family_rev: BUILT_FAMILY_REV,
                    }),
                    skeleton: FamilyKind::default_skeleton(&values),
                    values,
                    ..PackageRowDefinition::default()
                },
                identity: "custom entry".into(),
            }
        }
        RowSource::Starter { name } => {
            let starter = super::starter::row(name)?;
            ResolvedRow {
                name: starter.name.clone(),
                row: starter.definition.clone(),
                identity: format!(
                    "built-in starter table {} format {}",
                    starter.file,
                    super::starter::FORMAT
                ),
            }
        }
    };
    let mut findings = Vec::new();
    resolved.row.findings(DefinitionId(0), &mut findings);
    if let Some(first) = findings.iter().find(|f| f.blocks.is_some()) {
        return Err(Error::Invalid(format!(
            "{}: the template row ‘{}’ cannot be used: {}",
            first.code, resolved.name, first.message
        )));
    }
    Ok(resolved)
}

/// The values a request builds: its edited `values`, or the row's. Edited
/// values stay in the row's family and pass the family's range rules;
/// edited values make the result a custom package, so the row's land
/// overrides and citation no longer apply (returned `false`).
pub fn checked_values(
    row: &PackageRowDefinition,
    values: Option<&PackageForm>,
) -> Result<(PackageForm, bool)> {
    let (form, as_row) = match values {
        Some(v) if v != &row.values => (v.clone(), false),
        _ => (row.values.clone(), true),
    };
    let family = row.family.map(|f| f.family);
    if FamilyKind::of(&form) != family || family.is_none() {
        return Err(Error::Invalid(format!(
            "template.values: edited values must stay in the row's {} family",
            family.map_or("(none)", FamilyKind::label)
        )));
    }
    if !(form.within_bounds() && form.lengths().iter().all(|v| v.is_finite())) {
        return Err(Error::Limit(
            "template.values: the values exceed the form's limits".into(),
        ));
    }
    if let Some(problem) = form.range_problems().into_iter().next() {
        return Err(Error::Invalid(format!("package.form_range: {problem}")));
    }
    Ok((form, as_row))
}

/// The default designator prefix of a skeleton and form: R, C or L for a
/// chip of that construction, D for another polarised two-terminal part,
/// J for a connector, U otherwise. A starting value the user edits.
pub fn default_prefix(skeleton: SkeletonHint, form: &PackageForm) -> &'static str {
    default_prefix_for(skeleton, form, None)
}

/// [`default_prefix`] for a row that suggests `symbol`: a two-terminal
/// skeleton takes the symbol's customary prefix (C for a tantalum row's
/// polarised capacitor, FB for a ferrite bead).
pub fn default_prefix_for(
    skeleton: SkeletonHint,
    form: &PackageForm,
    symbol: Option<SymbolKind>,
) -> &'static str {
    if let (SkeletonHint::TwoTerminal { .. }, Some(kind)) = (skeleton, symbol)
        && kind.terminal_count() == Some(2)
    {
        return kind.prefix();
    }
    match (skeleton, form) {
        (SkeletonHint::TwoTerminal { polarised: true }, _) => "D",
        (SkeletonHint::TwoTerminal { .. }, PackageForm::Chip(c)) => match c.kind {
            ChipKind::Resistor => "R",
            ChipKind::Capacitor => "C",
            ChipKind::Inductor => "L",
            ChipKind::Led => "D",
        },
        (SkeletonHint::TwoTerminal { .. }, _) => "R",
        (SkeletonHint::Connector, _) => "J",
        (SkeletonHint::IcBox | SkeletonHint::Blank, _) => "U",
    }
}

/// The five drafts of a new component built from a row
/// (`library.new_from_template`), in `refs`' lineages: component first,
/// then symbol, package, footprint and binding. Pure; refuses a blank
/// name, a bad prefix, values outside the row's family or ranges, and a
/// skeleton that does not fit the package.
pub fn new_from_template(
    library: &Library,
    request: &NewFromTemplate,
    refs: SampleRefs,
) -> Result<Vec<DraftEdit>> {
    new_from_template_with(
        library,
        request,
        refs,
        &crate::rules::DesignRules::default(),
        SymbolConventions::default(),
    )
}

/// [`new_from_template`] with estimated lands following `rules` (the
/// project's effective design rules) and generated symbols following
/// `conventions` (the project's).
pub fn new_from_template_with(
    library: &Library,
    request: &NewFromTemplate,
    refs: SampleRefs,
    rules: &crate::rules::DesignRules,
    conventions: SymbolConventions,
) -> Result<Vec<DraftEdit>> {
    if !printable(&request.name) {
        return Err(Error::Invalid(
            "template.name: a new component needs a printable name of 1–256 bytes".into(),
        ));
    }
    let resolved = resolve_row(library, &request.row)?;
    let (form, as_row) = checked_values(&resolved.row, request.values.as_ref())?;
    let prefix = match &request.designator_prefix {
        Some(p) => {
            let p = p.trim();
            if p.is_empty()
                || p.len() > MAX_PREFIX
                || !p.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                return Err(Error::Invalid(format!(
                    "template.prefix: a designator prefix is 1–{MAX_PREFIX} letters or digits"
                )));
            }
            p.to_owned()
        }
        None => default_prefix_for(request.skeleton, &form, resolved.row.symbol).to_owned(),
    };
    let provenance = provenance(&resolved, as_row);
    let lands = resolved.row.lands.as_ref().filter(|_| as_row);
    let edits = samples::from_template_with(
        &TemplateSpec {
            name: request.name.trim(),
            form,
            skeleton: request.skeleton,
            symbol: request.symbol,
            row_symbol: resolved.row.symbol,
            conventions,
            prefix: &prefix,
            lands,
            provenance: &provenance,
        },
        refs,
        rules,
    )?;
    if edits.len() != NEW_FROM_TEMPLATE_DRAFTS {
        return Err(Error::Invalid(
            "template.drafts: a template builds exactly five drafts".into(),
        ));
    }
    Ok(edits)
}

/// The provenance text drafts built from `resolved` record: the row's
/// own (`as_row`), or edited custom values that name the row they started
/// from.
pub fn provenance(resolved: &ResolvedRow, as_row: bool) -> String {
    if as_row {
        resolved
            .row
            .provenance_text(&resolved.name, &resolved.identity)
    } else {
        format!(
            "Custom values edited from package template row ‘{}’ [{}] (estimated values and lands); verify against the selected component datasheet.",
            resolved.name, resolved.identity
        )
    }
}

/// A new package-row draft body and meta (`library.save_package_row`):
/// the family and family revision of `values`, its aliases and
/// provenance, no citation (the row is estimated until one is added).
pub fn row_draft(
    request: &SavePackageRow,
    values: PackageForm,
) -> Result<(DefinitionMeta, Definition)> {
    if !printable(&request.name) {
        return Err(Error::Invalid(
            "template.name: a package template row needs a printable name of 1–256 bytes".into(),
        ));
    }
    let family = FamilyKind::of(&values).ok_or_else(|| {
        Error::Invalid(
            "template.family: a row needs typed family values, not a generic envelope".into(),
        )
    })?;
    let family_rev = family.family_rev();
    if let Some(problem) = values.range_problems().into_iter().next() {
        return Err(Error::Invalid(format!("package.form_range: {problem}")));
    }
    let mut aliases: Vec<String> = Vec::new();
    for alias in &request.aliases {
        let alias = alias.trim();
        if !printable(alias) || alias.len() > MAX_ALIAS {
            return Err(Error::Invalid(format!(
                "package_row.alias: an alias is 1–{MAX_ALIAS} bytes of printable text"
            )));
        }
        let folded = fold_text(alias);
        if !aliases.iter().any(|a| fold_text(a) == folded) {
            aliases.push(alias.to_owned());
        }
    }
    if aliases.len() > MAX_ROW_ALIASES {
        return Err(Error::Limit(format!(
            "package_row.alias: a row lists at most {MAX_ROW_ALIASES} aliases"
        )));
    }
    if matches!(request.provenance.basis, Basis::FromReference(_)) {
        return Err(Error::Invalid(
            "package_row.provenance: a row cites its drawing with a citation, not a Part reference"
                .into(),
        ));
    }
    let row = PackageRowDefinition {
        family: Some(FamilyRef { family, family_rev }),
        skeleton: FamilyKind::default_skeleton(&values),
        values,
        aliases,
        size_code: None,
        lands: None,
        provenance: request.provenance.clone(),
        citation: None,
        symbol: None,
    };
    let meta = DefinitionMeta {
        name: request.name.trim().to_owned(),
        description: String::new(),
        provenance: "Saved from package values; estimated until a citation is added.".into(),
    };
    Ok((meta, Definition::PackageRow(Box::new(row))))
}

/// The preset forms as estimated rows: the first rows of the starter
/// table and test data. No citation: these are original typical values.
pub fn preset_rows() -> Vec<(String, PackageRowDefinition)> {
    Preset::ALL
        .into_iter()
        .filter_map(|preset| {
            let values = preset.form();
            let family = FamilyKind::of(&values)?;
            let size_code = match preset {
                Preset::Chip0603 => Some(("0603", "1608")),
                Preset::Chip0805 => Some(("0805", "2012")),
                _ => None,
            }
            .map(|(imperial, metric)| SizeCode {
                imperial: imperial.into(),
                metric: metric.into(),
            });
            Some((
                preset.label().to_owned(),
                PackageRowDefinition {
                    family: Some(FamilyRef {
                        family,
                        family_rev: BUILT_FAMILY_REV,
                    }),
                    skeleton: FamilyKind::default_skeleton(&values),
                    aliases: vec![preset.label().to_owned()],
                    values,
                    size_code,
                    ..PackageRowDefinition::default()
                },
            ))
        })
        .collect()
}

/// Case folding for alias comparison (the catalog's folding: lower case,
/// micro sign as Greek mu).
fn fold_text(text: &str) -> String {
    text.to_lowercase().replace('\u{b5}', "\u{3bc}")
}
