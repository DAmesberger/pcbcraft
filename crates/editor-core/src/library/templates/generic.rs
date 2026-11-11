//! On-demand generic parts: what makes a header 1×4 or a chip resistor 0603
//! the part it is. A [`GenericSpec`] holds the generating parameters; it is
//! recorded on the component it built, so identity comes from those
//! parameters and never from a name. [`GenericSpec::plan`] turns one into the
//! ordinary `library.new_from_template` request; the family's own form rules
//! decide whether the values can be built, so no limit is repeated here.
//!
//! The search grammar that reads a spec from text is `catalog::generic`.
use super::catalog::{self, NewFromTemplate, RowSource, SizeCode};
use super::starter::{self, StarterRow};
use crate::fields::{PropertyEdit, PropertyInput};
use crate::ids::{ComponentRef, DefinitionId};
use crate::library::package_form::{ChipKind, HeaderForm, MAX_FORM_LEADS, PackageForm};
use crate::library::symbol_style::SymbolKind;
use crate::library::{Definition, Drafts, Library};
use crate::quantity::Decimal;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The chip constructions a generic part names.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChipPart {
    Resistor,
    Capacitor,
    Inductor,
    Led,
}

impl ChipPart {
    fn kind(self) -> ChipKind {
        match self {
            Self::Resistor => ChipKind::Resistor,
            Self::Capacitor => ChipKind::Capacitor,
            Self::Inductor => ChipKind::Inductor,
            Self::Led => ChipKind::Led,
        }
    }

    fn of(kind: ChipKind) -> Self {
        match kind {
            ChipKind::Resistor => Self::Resistor,
            ChipKind::Capacitor => Self::Capacitor,
            ChipKind::Inductor => Self::Inductor,
            ChipKind::Led => Self::Led,
        }
    }

    /// "Chip resistor", as a component name starts.
    pub fn label(self) -> &'static str {
        match self {
            Self::Resistor => "Chip resistor",
            Self::Capacitor => "Chip capacitor",
            Self::Inductor => "Chip inductor",
            Self::Led => "Chip LED",
        }
    }

    /// The symbol asked for beyond the row's: the starter table's inductor
    /// bodies are ferrite-bead rows, and a part named an inductor draws one.
    fn symbol(self) -> Option<SymbolKind> {
        matches!(self, Self::Inductor).then_some(SymbolKind::Inductor)
    }
}

/// A generic part's generating parameters: its identity. Per-instance values
/// (10 kΩ, Red) are not part of it: one chip resistor 0603 takes any value.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenericSpec {
    /// A straight pin header; 5×2 and 2×5 are different parts.
    Header {
        rows: u32,
        columns: u32,
        pitch_mm: Decimal,
    },
    /// A chip part by its imperial size code.
    Chip { part: ChipPart, imperial: String },
    /// A diode in a molded package, by the starter row's name.
    Diode { package: String },
}

/// What a spec builds.
#[derive(Clone, Debug, PartialEq)]
pub struct GenericPlan {
    /// The `library.new_from_template` request; its name is the component's.
    pub request: NewFromTemplate,
    pub form: PackageForm,
    /// "cited", "cited (unverified)" or "estimated".
    pub basis: &'static str,
    /// The starter row the request takes unchanged, as it is the row's own
    /// part: the row need not be listed beside it.
    pub covers_row: Option<String>,
}

/// Where a generic part stands in the project.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenericState {
    /// The identical part is published: this is the part.
    Existing {
        component: ComponentRef,
    },
    /// The identical part is an unpublished draft.
    Draft {
        id: DefinitionId,
    },
    NotCreated,
    /// The family cannot build these values.
    Refused {
        reason: String,
    },
}

/// One value a query gave a placed instance, as field text core parses.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenericValue {
    /// The field's name, as the standard fields spell it.
    pub field: String,
    pub text: String,
}

fn rows() -> Result<&'static [StarterRow], String> {
    starter::rows().map_err(str::to_owned)
}

/// A diode row by a word that names it: its leading name ("SMA") or an alias
/// ("SOD123"), folded.
pub fn diode_row(word: &str) -> Option<&'static StarterRow> {
    let word = word.to_lowercase();
    rows().ok()?.iter().find(|row| {
        row.definition.symbol == Some(SymbolKind::Diode)
            && (row
                .name
                .split_whitespace()
                .next()
                .is_some_and(|w| w.to_lowercase() == word)
                || row
                    .definition
                    .aliases
                    .iter()
                    .any(|a| a.to_lowercase() == word))
    })
}

/// The imperial code of a chip part's metric code, if a row has it.
pub fn imperial_of(part: ChipPart, metric: &str) -> Option<String> {
    chip_rows(part)
        .ok()?
        .into_iter()
        .find_map(|(_, code)| (code.metric == metric).then(|| code.imperial.clone()))
}

fn chip_rows(part: ChipPart) -> Result<Vec<(&'static StarterRow, &'static SizeCode)>, String> {
    Ok(rows()?
        .iter()
        .filter_map(|row| match &row.definition.values {
            PackageForm::Chip(c) if c.kind == part.kind() => {
                row.definition.size_code.as_ref().map(|code| (row, code))
            }
            _ => None,
        })
        .collect())
}

impl GenericSpec {
    /// The request that builds this part, or why its family cannot.
    pub fn plan(&self) -> Result<GenericPlan, String> {
        match self {
            Self::Header {
                rows,
                columns,
                pitch_mm,
            } => header_plan(*rows, *columns, *pitch_mm),
            Self::Chip { part, imperial } => chip_plan(*part, imperial),
            Self::Diode { package } => {
                let row = starter::row(package).map_err(|e| e.to_string())?;
                Ok(plan_of(row, None, format!("Diode {package}"), None))
            }
        }
    }

    /// The name a part that cannot be built goes by in a refusal row.
    pub fn label(&self) -> String {
        match self {
            Self::Header {
                rows,
                columns,
                pitch_mm,
            } => format!("Pin header {rows}×{columns} {pitch_mm} mm"),
            Self::Chip { part, imperial } => format!("{} {imperial}", part.label()),
            Self::Diode { package } => format!("Diode {package}"),
        }
    }
}

fn plan_of(
    row: &StarterRow,
    values: Option<PackageForm>,
    name: String,
    symbol: Option<SymbolKind>,
) -> GenericPlan {
    let edited = values.is_some();
    let form = values
        .clone()
        .unwrap_or_else(|| row.definition.values.clone());
    let prefix = catalog::default_prefix_for(
        row.definition.skeleton,
        &form,
        symbol.or(row.definition.symbol),
    );
    let basis = match &row.definition.citation {
        Some(_) if edited => "estimated",
        Some(c) if c.checked() => "cited",
        Some(_) => "cited (unverified)",
        None => "estimated",
    };
    GenericPlan {
        request: NewFromTemplate {
            row: RowSource::Starter {
                name: row.name.clone(),
            },
            values,
            skeleton: row.definition.skeleton,
            name,
            designator_prefix: Some(prefix.to_owned()),
            symbol,
            without_fields: false,
        },
        form,
        basis,
        covers_row: (!edited && symbol.is_none()).then(|| row.name.clone()),
    }
}

/// The form's own refusal of `form`, as a sentence: its persistence bound,
/// then its range rules.
fn form_problem(form: &PackageForm) -> Option<String> {
    if !form.within_bounds() {
        return Some(format!(
            "A package holds at most {MAX_FORM_LEADS} pins; this one has {}.",
            form.lead_count()
        ));
    }
    form.range_problems().into_iter().next()
}

fn header_plan(rows_: u32, columns: u32, pitch: Decimal) -> Result<GenericPlan, String> {
    let mm = pitch.to_f64();
    let same = |h: &HeaderForm, p: f64| (h.pitch() - p).abs() < 1e-9;
    let headers: Vec<(&StarterRow, HeaderForm)> = rows()?
        .iter()
        .filter_map(|row| match &row.definition.values {
            PackageForm::Header(h) => Some((row, **h)),
            _ => None,
        })
        .collect();
    if let Some((row, _)) = headers
        .iter()
        .find(|(_, h)| h.rows == rows_ && h.pins_per_row == columns && same(h, mm))
    {
        return Ok(plan_of(row, None, row.name.clone(), None));
    }
    // Proportions come from a row of this pitch, else from the first row,
    // with the pitch as asked: the form's rules then say if it can be built.
    let (row, base) = headers
        .iter()
        .find(|(_, h)| same(h, mm))
        .or(headers.first())
        .ok_or_else(|| "The starter tables hold no pin header.".to_owned())?;
    let mut form = HeaderForm {
        rows: rows_,
        pins_per_row: columns,
        ..*base
    };
    if !same(base, mm) {
        let pitch_nm = crate::units::Nm::from_mm_f64_rounded(mm).map_err(|e| e.to_string())?;
        form.pitch_nm = pitch_nm;
        form.row_pitch_nm = pitch_nm;
    }
    let form = PackageForm::Header(Box::new(form));
    if let Some(problem) = form_problem(&form) {
        return Err(problem);
    }
    let name = format!("Pin header {rows_}×{columns} {pitch} mm");
    Ok(plan_of(row, Some(form), name, None))
}

fn chip_plan(part: ChipPart, imperial: &str) -> Result<GenericPlan, String> {
    let rows = chip_rows(part)?;
    let Some((row, code)) = rows.iter().find(|(_, code)| code.imperial == imperial) else {
        let sizes: Vec<&str> = rows.iter().map(|(_, c)| c.imperial.as_str()).collect();
        let label = part.label().to_lowercase();
        if let Some(metric) = imperial.strip_suffix('M') {
            return Err(format!(
                "No {label} of metric size {metric}. Imperial sizes: {}.",
                sizes.join(", ")
            ));
        }
        let hint =
            rows.iter()
                .find(|(_, c)| c.metric == imperial)
                .map_or(String::new(), |(_, c)| {
                    format!(
                        " {imperial}M is the metric code of imperial {}.",
                        c.imperial
                    )
                });
        return Err(format!(
            "No {label} of imperial size {imperial}. Imperial sizes: {}.{hint}",
            sizes.join(", ")
        ));
    };
    let name = match part.symbol() {
        Some(_) => format!(
            "{} {} ({} metric)",
            part.label(),
            code.imperial,
            code.metric
        ),
        None => row.name.clone(),
    };
    Ok(plan_of(row, None, name, part.symbol()))
}

/// The spec a template request amounts to: a person who chose the row
/// "Chip resistor 0603" made the same part a search for "resistor 0603"
/// would, so a later search finds it. `None` when the request differs from
/// what any spec builds (edited values, another symbol or prefix).
pub fn recognise(request: &NewFromTemplate, form: &PackageForm) -> Option<GenericSpec> {
    let spec = match form {
        PackageForm::Header(h) => GenericSpec::Header {
            rows: h.rows,
            columns: h.pins_per_row,
            pitch_mm: Decimal::from_f64(h.pitch()).ok()?,
        },
        PackageForm::Chip(c) => {
            let part = ChipPart::of(c.kind);
            let (_, code) = chip_rows(part)
                .ok()?
                .into_iter()
                .find(|(row, _)| row.definition.values == *form)?;
            GenericSpec::Chip {
                part,
                imperial: code.imperial.clone(),
            }
        }
        PackageForm::Molded(_) => match &request.row {
            RowSource::Starter { name }
                if starter::row(name).ok()?.definition.symbol == Some(SymbolKind::Diode) =>
            {
                GenericSpec::Diode {
                    package: name.clone(),
                }
            }
            _ => return None,
        },
        _ => return None,
    };
    let plan = spec.plan().ok()?;
    let prefix = |r: &NewFromTemplate| r.designator_prefix.as_ref().map(|p| p.trim().to_owned());
    let same = plan.request.row == request.row
        && plan.form == *form
        && plan.request.skeleton == request.skeleton
        && plan.request.symbol == request.symbol
        && (request.designator_prefix.is_none() || prefix(&plan.request) == prefix(request));
    same.then_some(spec)
}

/// The published part or draft already built from `spec`. A later revision
/// that swapped the package is no longer this part: `form` is what it must
/// still land.
pub fn existing(
    library: &Library,
    drafts: &Drafts,
    spec: &GenericSpec,
    form: &PackageForm,
) -> Option<GenericState> {
    let published = library
        .latest_components()
        .into_iter()
        .filter(|c| c.definition.generic.as_deref() == Some(spec))
        .find(|c| {
            c.definition
                .implementations
                .first()
                .and_then(|b| library.binding(*b))
                .and_then(|b| library.package(b.definition.package))
                .is_some_and(|p| p.definition.form == *form)
        });
    if let Some(component) = published {
        return Some(GenericState::Existing {
            component: ComponentRef::new(component.id, component.revision),
        });
    }
    drafts
        .items
        .iter()
        .find(|d| matches!(&d.definition, Definition::Component(c) if c.generic.as_deref() == Some(spec)))
        .map(|d| GenericState::Draft { id: d.id })
}

/// The instance property edits of `values`, for the fields `component`
/// selects by these names; a value with no such field is left out.
pub fn properties(
    library: &Library,
    component: ComponentRef,
    values: &[GenericValue],
) -> Vec<PropertyEdit> {
    let Some(published) = library.component(component) else {
        return Vec::new();
    };
    values
        .iter()
        .filter_map(|value| {
            let selection = published.definition.fields.iter().find(|s| {
                library
                    .field(s.field)
                    .is_some_and(|f| f.meta.name == value.field)
            })?;
            Some(PropertyEdit {
                field: selection.field.id,
                input: PropertyInput::Text(value.text.clone()),
            })
        })
        .collect()
}
