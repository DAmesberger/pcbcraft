//! Slice B1: template rows in the catalog. `library.templates` searches the
//! package rows of the project and of every loaded source, plus each built
//! family's "custom" entry. Reserved filters `pins:`, `pitch:` and `body:`
//! take typed, unit-aware values (`soic pins:14`, `qfn body:5x5
//! pitch:0.5`); `pkg:0402` matches both its imperial and its metric
//! meaning (and package labels), and `pkg:i0402` / `pkg:m0402` name one
//! system only. Nothing is guessed: both codes of a row are explicit data.
use super::query::{BODY_TOLERANCE_MM, PITCH_TOLERANCE_MM, SizeSystem};
use super::{CatalogContext, MatchTier, Source, Term, Vocabulary, fold, parse};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, PackageRowRef, Revision, SourceLineageId};
use crate::library::bundle::SourceRevision;
use crate::library::footprint_source::ParamValue;
use crate::library::package_form::PackageForm;
use crate::library::templates::catalog::{
    FamilyKind, LandValues, MAX_PACKAGE_ROWS_PER_SOURCE, PackageRowDefinition, RowCitation,
    SizeCode, SkeletonHint,
};
use crate::library::templates::starter;
use crate::library::{Drafts, Library};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Hits one `library.templates` returns at most.
pub const MAX_TEMPLATE_HITS: usize = 500;

/// One gallery row.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemplateHit {
    pub family: FamilyKind,
    /// A project row's exact revision; `None` for a folder row and for the
    /// family's built-in custom entry.
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
    /// The citation records the drawing's revision and page; a cited row
    /// without them reads "cited (unverified)".
    #[serde(default)]
    pub citation_checked: bool,
    /// The lands come from a cited land pattern (else estimated defaults).
    pub lands_cited: bool,
    /// A folder row's own revision: the host reads its bundle (io
    /// `read_bundle`) and passes it as `RowSource::Bundle`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_row: Option<SourceRevision>,
    /// The skeleton the row suggests; the request chooses.
    #[serde(default)]
    pub skeleton: SkeletonHint,
    /// The schematic symbol the row suggests, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<crate::library::symbol_style::SymbolKind>,
    /// The outline drawing a cited row was typed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<RowCitation>,
    /// Lead count, pitch and body, as the filters read them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<PackageShape>,
    /// A built-in starter row's unique name (`RowSource::Starter`); the
    /// hit's `source` is then `Project` (nothing to read from a folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starter: Option<String>,
    /// The row's land overrides, which the preview applies exactly as
    /// `library.new_from_template` does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lands: Option<LandValues>,
    pub tier: MatchTier,
}

impl TemplateHit {
    /// The family's built-in custom entry (no row).
    pub fn is_custom(&self) -> bool {
        self.row.is_none() && self.source_row.is_none() && self.starter.is_none()
    }

    /// A row of the built-in starter tables.
    pub fn is_starter(&self) -> bool {
        self.starter.is_some()
    }
}

/// `library.templates`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemplateReport {
    pub hits: Vec<TemplateHit>,
    /// Matches beyond the limit.
    pub more: usize,
    /// The parsed terms, as the gallery shows them.
    #[serde(default)]
    pub chips: Vec<String>,
}

/// A package's typed shape: what `pins:`, `pitch:` and `body:` test.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackageShape {
    /// Leads, not counting an exposed pad.
    pub pins: u32,
    /// Lead pitch in mm; `None` for a two-terminal chip.
    #[serde(default)]
    pub pitch: Option<f64>,
    /// Body length, width and height in mm (a gull-wing body without its
    /// leads; its height includes the standoff).
    #[serde(default)]
    pub body: Option<[f64; 3]>,
}

impl PackageShape {
    /// The shape of a typed form; `None` for `Generic` and for a form with
    /// no leads.
    pub fn of(form: &PackageForm) -> Option<Self> {
        let pins = u32::try_from(form.lead_count()).ok().filter(|n| *n > 0)?;
        let (pitch, body) = match form {
            PackageForm::Generic => return None,
            PackageForm::Chip(c) => (None, Some([c.length, c.width, c.height])),
            PackageForm::GullWing(g) => (
                Some(g.pitch),
                Some([g.body_length, g.body_width, g.standoff + g.body_height]),
            ),
            PackageForm::NoLead(n) => (
                Some(n.pitch),
                Some([n.body_length, n.body_width, n.body_height]),
            ),
            PackageForm::Header(h) => (Some(h.pitch), form.envelope()),
            PackageForm::Tabbed(_)
            | PackageForm::Molded(_)
            | PackageForm::Grid(_)
            | PackageForm::Tht(_) => {
                let pitch = form.values().into_iter().find_map(|v| match v.value {
                    ParamValue::Length(mm) if v.name.0.ends_with("_e") => Some(mm),
                    _ => None,
                });
                (pitch, form.envelope())
            }
        };
        Some(Self { pins, pitch, body })
    }

    /// Whether the shape meets a `pins:`, `pitch:` or `body:` term; other
    /// terms are not shape terms and are met.
    pub fn meets(&self, term: &Term) -> bool {
        match term {
            Term::Pins { op, count } => {
                op.holds_within(f64::from(self.pins), f64::from(*count), 0.0)
            }
            Term::Pitch { op, mm } => self
                .pitch
                .is_some_and(|p| op.holds_within(p, *mm, PITCH_TOLERANCE_MM)),
            Term::Body { size } => self.body.is_some_and(|[l, w, h]| {
                let near = |a: f64, b: f64| (a - b).abs() <= BODY_TOLERANCE_MM;
                let plane = match size.as_slice() {
                    [a, b, ..] => (near(l, *a) && near(w, *b)) || (near(l, *b) && near(w, *a)),
                    _ => false,
                };
                plane && size.get(2).is_none_or(|z| near(h, *z))
            }),
            _ => true,
        }
    }
}

/// One package template row of a library folder, at its latest revision.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourceRow {
    pub lineage: SourceLineageId,
    pub revision: Revision,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub row: PackageRowDefinition,
}

/// The latest revision of every package row a source's local library
/// holds, in source-lineage order. More than
/// [`MAX_PACKAGE_ROWS_PER_SOURCE`] rows is a limit error.
pub(crate) fn source_rows(
    held: &Library,
    source_of: &dyn Fn(DefinitionId) -> Option<SourceLineageId>,
) -> Result<Vec<SourceRow>> {
    let mut latest: BTreeMap<SourceLineageId, SourceRow> = BTreeMap::new();
    for published in &held.package_rows {
        let Some(lineage) = source_of(published.id) else {
            continue;
        };
        if latest
            .get(&lineage)
            .is_some_and(|r| r.revision >= published.revision)
        {
            continue;
        }
        latest.insert(
            lineage,
            SourceRow {
                lineage,
                revision: published.revision,
                name: published.meta.name.clone(),
                description: published.meta.description.clone(),
                row: published.definition.clone(),
            },
        );
    }
    if latest.len() > MAX_PACKAGE_ROWS_PER_SOURCE {
        return Err(Error::Limit(format!(
            "a library source holds at most {MAX_PACKAGE_ROWS_PER_SOURCE} package template rows"
        )));
    }
    Ok(latest.into_values().collect())
}

/// One searchable row with its folded text.
struct Candidate {
    hit: TemplateHit,
    /// 0 when a bare text term is the row's imperial size code, 1 when it
    /// is only its metric one, else 2: "0603" lists the imperial 0603
    /// before the metric 0603 (an 0201).
    code_rank: u8,
    name: String,
    words: Vec<String>,
    aliases: Vec<String>,
    /// Description, family label and size codes.
    other: String,
}

impl Candidate {
    fn new(hit: TemplateHit, description: &str) -> Self {
        let name = fold(hit.name.trim());
        let words = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        let aliases = hit.aliases.iter().map(|a| fold(a.trim())).collect();
        let mut other = fold(description);
        other.push('\n');
        other.push_str(&fold(hit.family.label()));
        if let Some(code) = &hit.size_code {
            other.push_str(&format!("\n{}\n{}", code.imperial, code.metric));
        }
        Self {
            hit,
            code_rank: 2,
            name,
            words,
            aliases,
            other,
        }
    }

    fn text_tier(&self, text: &str) -> Option<MatchTier> {
        let code = self
            .hit
            .size_code
            .as_ref()
            .is_some_and(|c| c.imperial == text || c.metric == text);
        if self.name == text || self.aliases.iter().any(|a| a == text) {
            Some(MatchTier::Exact)
        } else if self.name.starts_with(text) || self.aliases.iter().any(|a| a.starts_with(text)) {
            Some(MatchTier::Prefix)
        } else if code || self.words.iter().any(|w| w == text) {
            Some(MatchTier::Word)
        } else if self.name.contains(text)
            || self.aliases.iter().any(|a| a.contains(text))
            || self.other.contains(text)
        {
            Some(MatchTier::Substring)
        } else {
            None
        }
    }

    /// See [`Candidate::code_rank`].
    fn code_rank(&self, terms: &[Term]) -> u8 {
        let Some(code) = &self.hit.size_code else {
            return 2;
        };
        let texts = terms.iter().filter_map(|t| match t {
            Term::Text { text } | Term::Value { text, .. } | Term::Package { text } => {
                Some(text.as_str())
            }
            _ => None,
        });
        let mut rank = 2;
        for text in texts {
            if code.imperial == text {
                rank = 0;
            } else if code.metric == text {
                rank = rank.min(1);
            }
        }
        rank
    }

    /// The worst tier over the text terms, `Filter` without text, or
    /// `None` when a term fails.
    fn tier(&self, terms: &[Term]) -> Option<MatchTier> {
        let mut worst = None::<MatchTier>;
        for term in terms {
            match term {
                Term::Text { text } | Term::Value { text, .. } => {
                    let tier = self.text_tier(text)?;
                    worst = Some(worst.map_or(tier, |w| w.max(tier)));
                }
                Term::Package { text } => {
                    let code = self
                        .hit
                        .size_code
                        .as_ref()
                        .is_some_and(|c| &c.imperial == text || &c.metric == text);
                    if !(code
                        || self.name.contains(text.as_str())
                        || self.aliases.iter().any(|a| a.contains(text.as_str())))
                    {
                        return None;
                    }
                }
                Term::SizeCode { system, code } => {
                    let own = self.hit.size_code.as_ref().map(|c| match system {
                        SizeSystem::Imperial => &c.imperial,
                        SizeSystem::Metric => &c.metric,
                    });
                    if own != Some(code) {
                        return None;
                    }
                }
                Term::Project => {
                    if self.hit.source != Source::Project
                        || self.hit.is_custom()
                        || self.hit.is_starter()
                    {
                        return None;
                    }
                }
                Term::Pins { .. } | Term::Pitch { .. } | Term::Body { .. } => {
                    if !self.hit.shape.is_some_and(|s| s.meets(term)) {
                        return None;
                    }
                }
                // Refused by `check_terms`.
                Term::Field { .. }
                | Term::Has { .. }
                | Term::Components
                | Term::Parts
                | Term::Mpn { .. } => return None,
            }
        }
        Some(worst.unwrap_or(MatchTier::Filter))
    }
}

/// Filters that name component or Part data do not apply to rows.
fn check_terms(terms: &[Term]) -> Result<()> {
    for term in terms {
        let key = match term {
            Term::Field { key, .. } => key.as_str(),
            Term::Has { present: true, .. } => "has",
            Term::Has { present: false, .. } => "missing",
            Term::Components | Term::Parts => "kind",
            Term::Mpn { .. } => "mpn",
            _ => continue,
        };
        return Err(Error::Invalid(format!(
            "query: ‘{key}:’ does not apply to package template rows; use pins:, pitch:, body:, pkg: or src:project"
        )));
    }
    Ok(())
}

fn hit_of(
    name: &str,
    row: &PackageRowDefinition,
    source: Source,
    project_row: Option<PackageRowRef>,
    source_row: Option<SourceRevision>,
) -> Option<TemplateHit> {
    let family = row
        .family
        .map(|f| f.family)
        .or(FamilyKind::of(&row.values))?;
    Some(TemplateHit {
        family,
        row: project_row,
        source,
        name: name.to_owned(),
        aliases: row.aliases.clone(),
        values: row.values.clone(),
        size_code: row.size_code.clone(),
        cited: row.cited(),
        citation_checked: row.citation.as_ref().is_some_and(RowCitation::checked),
        lands_cited: row.lands_cited(),
        source_row,
        skeleton: row.skeleton,
        symbol: row.symbol,
        citation: row.citation.clone(),
        shape: PackageShape::of(&row.values),
        starter: None,
        lands: row.lands.clone(),
        tier: MatchTier::Filter,
    })
}

/// Search text of each family's custom entry beyond its label: the
/// outline names a user types, so a search for one never comes back
/// empty while the family can build it.
fn custom_aliases(family: FamilyKind) -> &'static [&'static str] {
    match family {
        FamilyKind::Chip => &["resistor", "capacitor", "inductor", "passive", "chip"],
        FamilyKind::GullWing => &[
            "SOIC", "SOP", "SSOP", "TSSOP", "MSOP", "QFP", "LQFP", "TQFP",
        ],
        FamilyKind::NoLead => &["QFN", "DFN", "SON", "MLF"],
        FamilyKind::Header => &["header", "connector", "pin header"],
        FamilyKind::Tabbed => &[
            "SOT", "SOT-23", "SOT-89", "SOT-223", "SC-70", "DPAK", "D2PAK",
        ],
        FamilyKind::Molded => &["SOD", "SMA", "SMB", "SMC", "MELF", "tantalum", "diode"],
        FamilyKind::Grid => &["BGA", "FBGA", "LGA", "WLCSP", "CSP"],
        FamilyKind::Tht => &["DIP", "PDIP", "TO-220", "TO-92", "axial", "radial", "THT"],
    }
}

/// Every searchable row: the project's published rows at their latest
/// revisions, each source's rows in `context.sources` order, then each
/// built family's custom entry. Draft rows are not listed: a template
/// builds from an exact published revision.
fn candidates(project: &Project, context: &CatalogContext) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut latest: BTreeMap<DefinitionId, Revision> = BTreeMap::new();
    for row in &project.library.package_rows {
        let slot = latest.entry(row.id).or_insert(row.revision);
        *slot = (*slot).max(row.revision);
    }
    for row in &project.library.package_rows {
        if latest.get(&row.id) != Some(&row.revision) {
            continue;
        }
        let r = PackageRowRef::new(row.id, row.revision);
        if let Some(hit) = hit_of(
            &row.meta.name,
            &row.definition,
            Source::Project,
            Some(r),
            None,
        ) {
            out.push(Candidate::new(hit, &row.meta.description));
        }
    }
    if let Ok(rows) = starter::rows().map(|r| {
        if context.without_starter_rows {
            &[][..]
        } else {
            r
        }
    }) {
        for row in rows {
            if let Some(mut hit) = hit_of(&row.name, &row.definition, Source::Project, None, None) {
                hit.starter = Some(row.name.clone());
                out.push(Candidate::new(hit, &row.description));
            }
        }
    }
    for source in &context.sources {
        for row in &source.rows {
            let at = SourceRevision {
                lineage: row.lineage,
                revision: row.revision,
            };
            if let Some(hit) = hit_of(
                &row.name,
                &row.row,
                Source::Library(source.library),
                None,
                Some(at),
            ) {
                out.push(Candidate::new(hit, &row.description));
            }
        }
    }
    for family in FamilyKind::ALL {
        let values = family.custom_values();
        let row = PackageRowDefinition {
            family: None,
            skeleton: FamilyKind::default_skeleton(&values),
            values,
            aliases: custom_aliases(family)
                .iter()
                .map(|a| (*a).to_owned())
                .collect(),
            ..PackageRowDefinition::default()
        };
        let name = format!("Custom {}", family.label().to_lowercase());
        if let Some(mut hit) = hit_of(&name, &row, Source::Project, None, None) {
            hit.family = family;
            // The family's outline names ("SOIC / SOP", "QFN / DFN") make
            // the custom entry findable by them.
            let description = format!(
                "{}: edit the values of a new package of this family",
                family.template_kind().label()
            );
            out.push(Candidate::new(hit, &description));
        }
    }
    out
}

/// Order two folded names naturally: runs of digits compare by value
/// ("dfn-8" before "dfn-10"), everything else by character.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let run = |it: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits
                };
                let (x, y) = (run(&mut a), run(&mut b));
                let (tx, ty) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                let order = tx
                    .len()
                    .cmp(&ty.len())
                    .then_with(|| tx.cmp(ty))
                    .then_with(|| x.len().cmp(&y.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
                a.next();
                b.next();
            }
        }
    }
}

/// `library.templates`: rows matching `query`; rows before custom
/// entries, then by match tier, a bare size code's imperial meaning
/// before its metric one, the project before sources, then folded name in
/// natural order ("DFN-8" before "DFN-10") and source row. At most
/// `limit` (1–500) hits; `more` counts the rest. Equal inputs always give
/// the same order.
pub fn search(
    project: &Project,
    _drafts: &Drafts,
    context: &CatalogContext,
    query: &str,
    limit: usize,
) -> Result<TemplateReport> {
    let parsed = parse(query, &Vocabulary::default())?;
    check_terms(&parsed.terms)?;
    let mut matched: Vec<Candidate> = candidates(project, context)
        .into_iter()
        .filter_map(|mut c| {
            c.hit.tier = c.tier(&parsed.terms)?;
            c.code_rank = c.code_rank(&parsed.terms);
            Some(c)
        })
        .collect();
    matched.sort_by(|a, b| {
        a.hit
            .is_custom()
            .cmp(&b.hit.is_custom())
            .then_with(|| a.hit.tier.cmp(&b.hit.tier))
            .then_with(|| a.code_rank.cmp(&b.code_rank))
            .then_with(|| a.hit.source.cmp(&b.hit.source))
            .then_with(|| a.hit.is_starter().cmp(&b.hit.is_starter()))
            .then_with(|| natural(&a.name, &b.name))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| {
                a.hit
                    .row
                    .map(|r| (r.id, r.revision))
                    .cmp(&b.hit.row.map(|r| (r.id, r.revision)))
            })
            .then_with(|| a.hit.source_row.cmp(&b.hit.source_row))
    });
    let limit = limit.clamp(1, MAX_TEMPLATE_HITS);
    let more = matched.len().saturating_sub(limit);
    matched.truncate(limit);
    Ok(TemplateReport {
        hits: matched.into_iter().map(|c| c.hit).collect(),
        more,
        chips: parsed.chips(),
    })
}

#[cfg(test)]
mod tests;
