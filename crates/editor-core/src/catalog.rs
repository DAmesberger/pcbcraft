//! The parts catalog: search entries derived from library data, the query
//! language, deterministic ranking and data-derived facets.
//!
//! Entries are derived, never stored. There is one entry per component
//! lineage and one per manufacturer Part lineage, each at its latest
//! published revision (plus, on request, lineages that exist only as
//! drafts). The project snapshot is the only source until
//! library folders exist; the [`Source`] tag keeps room for them. Nothing
//! here branches on a component category, field name or unit: filters come
//! from the field definitions and quantity kinds the data carries.
mod query;
pub mod synthetic;

pub use query::{
    Badge, CatalogQuery, FieldTest, MAX_QUERY, MAX_TERMS, Op, QueryError, RESERVED, Term, Test,
    Vocabulary, VocabularyField, VocabularyKind, fold, parse, parse_query, rkm,
};

use crate::diagnostics::{Error, Result};
use crate::fields::{
    self, FieldSelection, FieldValue, RatedSpec, Requirement, SpecMeaning, ToleranceBasis,
    ValueType,
};
use crate::ids::{
    BindingRef, ComponentRef, DefinitionId, DefinitionKind, FootprintRef, PackageRef, PartRef,
    Revision, SymbolRef,
};
use crate::library::{ComponentDefinition, DefinitionMeta, Drafts, Library, PartDefinition};
use crate::model::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Results one search returns.
pub const MAX_RESULTS: usize = 500;
/// Results returned when the caller gives no limit.
pub const DEFAULT_LIMIT: usize = 50;
/// Distinct values listed per facet.
pub const MAX_FACET_VALUES: usize = 12;
/// Entries one index holds.
pub const MAX_INDEX_ENTRIES: usize = 200_000;
/// Favourite and recent lineages a search context carries.
pub const MAX_CONTEXT: usize = 256;

/// Where an entry comes from. Library folders join in a later slice.
#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Project,
}

/// Separate completeness badges. There is no single "complete" flag.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct Badges {
    /// A listed symbol revision resolves.
    pub symbol: bool,
    /// A listed binding resolves with at least one footprint.
    pub footprint: bool,
    /// A bound package has a valid envelope, the native 3D model.
    pub model: bool,
    /// A manufacturer part number: the entry is a Part, or a component that
    /// a published Part implements.
    pub mpn: bool,
}

impl Badges {
    pub fn get(&self, badge: Badge) -> bool {
        match badge {
            Badge::Symbol => self.symbol,
            Badge::Footprint => self.footprint,
            Badge::Model => self.model,
            Badge::Mpn => self.mpn,
        }
    }
}

/// One effective field value of an entry: the component default, else the
/// field default.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EntryField {
    /// The field lineage.
    pub field: DefinitionId,
    pub label: String,
    pub value: FieldValue,
    /// The value formatted in core.
    pub text: String,
    /// Whether the component shows this field on its symbol: a key value.
    pub shown: bool,
    /// On a Part entry, how the rated value meets a requirement. A field
    /// predicate with `:` or a bare value then means "meets": a minimum
    /// rating of 25 V meets `16V`, a maximum tolerance of ±0.5 % meets
    /// `±1%`. Explicit comparisons keep their literal meaning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meaning: Option<SpecMeaning>,
    /// On a Part entry, what `fields::satisfies` needs to compare the rated
    /// value with a query predicate.
    #[serde(skip)]
    pub rated: Option<RatedReading>,
}

/// A Part's rated value as `fields::satisfies` reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct RatedReading {
    pub spec: RatedSpec,
    /// The value type of the spec's own field revision.
    pub value_type: ValueType,
    /// For a tolerance: the Part's one rated quantity in the absolute
    /// basis's quantity kind, the nominal a cross-basis comparison needs.
    pub nominal: Option<f64>,
}

/// One searchable lineage.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CatalogEntry {
    pub source: Source,
    pub kind: DefinitionKind,
    pub lineage: DefinitionId,
    /// The latest published revision; `None` for a lineage that exists only
    /// as a draft and cannot be placed yet.
    pub revision: Option<Revision>,
    pub name: String,
    pub description: String,
    /// The lineage has unpublished draft changes.
    pub draft: bool,
    pub fields: Vec<EntryField>,
    /// Package names, as text labels only.
    pub packages: Vec<String>,
    pub badges: Badges,
    /// Instances in the project that pin any revision of this lineage.
    pub used: usize,
    /// A Part entry's manufacturer, as the user wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// A Part entry's complete manufacturer part number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    /// A Part entry's pinned generic component revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implements: Option<ComponentRef>,
}

impl CatalogEntry {
    /// The placeable exact revision of a component entry; `None` for a
    /// Part entry.
    pub fn component(&self) -> Option<ComponentRef> {
        (self.kind == DefinitionKind::Component)
            .then_some(self.revision)
            .flatten()
            .map(|r| ComponentRef::new(self.lineage, r))
    }
    /// The assignable exact revision of a Part entry.
    pub fn part(&self) -> Option<PartRef> {
        (self.kind == DefinitionKind::Part)
            .then_some(self.revision)
            .flatten()
            .map(|r| PartRef::new(self.lineage, r))
    }
    /// Shown field values, then the first package: what a result row lists.
    pub fn key_values(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .fields
            .iter()
            .filter(|f| f.shown)
            .chain(self.fields.iter().filter(|f| !f.shown))
            .map(|f| f.text.as_str())
            .collect();
        out.extend(self.packages.first().map(String::as_str));
        out
    }
}

/// Build the entry of one component body. `meta` names it.
fn entry_of(
    library: &Library,
    lineage: DefinitionId,
    revision: Option<Revision>,
    meta: &DefinitionMeta,
    component: &ComponentDefinition,
) -> CatalogEntry {
    let fields = component
        .fields
        .iter()
        .filter_map(|selection| entry_field(library, selection))
        .collect();
    let mut packages = Vec::new();
    let mut badges = Badges {
        symbol: component
            .representations
            .iter()
            .any(|s| library.symbol(*s).is_some()),
        ..Badges::default()
    };
    for binding in component
        .implementations
        .iter()
        .filter_map(|b| library.binding(*b))
    {
        if binding
            .definition
            .footprints
            .iter()
            .any(|f| library.footprint(*f).is_some())
        {
            badges.footprint = true;
        }
        if let Some(package) = library.package(binding.definition.package) {
            badges.model |= package.definition.envelope_valid();
            if !packages.contains(&package.meta.name) {
                packages.push(package.meta.name.clone());
            }
        }
    }
    CatalogEntry {
        source: Source::Project,
        kind: DefinitionKind::Component,
        lineage,
        revision,
        name: meta.name.clone(),
        description: meta.description.clone(),
        draft: false,
        fields,
        packages,
        badges,
        used: 0,
        manufacturer: None,
        mpn: None,
        implements: None,
    }
}

/// Build the entry of one Part body: its rated values as fields, badges
/// and packages from the pinned component and binding.
fn part_entry(
    library: &Library,
    lineage: DefinitionId,
    revision: Option<Revision>,
    meta: &DefinitionMeta,
    part: &PartDefinition,
) -> CatalogEntry {
    let component = library.component(part.component).map(|c| &c.definition);
    let fields = part
        .specs
        .iter()
        .filter_map(|spec| {
            let view = library.field_view(spec.field)?;
            let value_type = view.field.definition.value_type.clone();
            Some(EntryField {
                field: spec.field.id,
                label: view.field.meta.name.clone(),
                text: fields::display(&spec.value, &value_type, view.kind_body()),
                value: spec.value.clone(),
                shown: component
                    .and_then(|c| c.field(spec.field.id))
                    .is_some_and(|s| s.show_on_symbol),
                meaning: Some(spec.meaning),
                rated: Some(RatedReading {
                    nominal: rated_nominal(library, part, spec, &value_type),
                    spec: spec.clone(),
                    value_type,
                }),
            })
        })
        .collect();
    let mut badges = Badges {
        symbol: component.is_some_and(|c| {
            c.representations
                .iter()
                .any(|s| library.symbol(*s).is_some())
        }),
        mpn: true,
        ..Badges::default()
    };
    let mut packages = Vec::new();
    if let Some(binding) = library.binding(part.binding) {
        badges.footprint = binding
            .definition
            .footprints
            .iter()
            .any(|f| library.footprint(*f).is_some());
        if let Some(package) = library.package(binding.definition.package) {
            badges.model = package.definition.envelope_valid();
            packages.push(package.meta.name.clone());
        }
    }
    CatalogEntry {
        source: Source::Project,
        kind: DefinitionKind::Part,
        lineage,
        revision,
        name: meta.name.clone(),
        description: meta.description.clone(),
        draft: false,
        fields,
        packages,
        badges,
        used: 0,
        manufacturer: Some(part.manufacturer.clone()),
        mpn: Some(part.mpn.clone()),
        implements: Some(part.component),
    }
}

fn entry_field(library: &Library, selection: &FieldSelection) -> Option<EntryField> {
    let view = library.field_view(selection.field)?;
    let value = selection
        .default
        .clone()
        .or_else(|| view.field.definition.default.clone())?;
    Some(EntryField {
        field: selection.field.id,
        label: view.field.meta.name.clone(),
        text: fields::display(&value, &view.field.definition.value_type, view.kind_body()),
        value,
        shown: selection.show_on_symbol,
        meaning: None,
        rated: None,
    })
}

/// The nominal a rated tolerance applies to: the Part's one rated quantity
/// measured in the absolute basis's quantity kind (the spec's own basis or
/// that of the field's latest revision).
fn rated_nominal(
    library: &Library,
    part: &PartDefinition,
    spec: &RatedSpec,
    value_type: &ValueType,
) -> Option<f64> {
    let absolute = |t: &ValueType| match t {
        ValueType::Tolerance {
            basis: ToleranceBasis::Absolute { kind },
        } => Some(kind.id),
        _ => None,
    };
    let latest = library
        .latest(spec.field.id)
        .and_then(|(_, revision)| {
            library.field_view(crate::ids::FieldRef::new(spec.field.id, revision))
        })
        .and_then(|v| absolute(&v.field.definition.value_type));
    let kind = absolute(value_type).or(latest)?;
    let mut measured = part.specs.iter().filter_map(|s| {
        let view = library.field_view(s.field)?;
        match (&view.field.definition.value_type, &s.value) {
            (ValueType::Quantity { kind: k, .. }, FieldValue::Quantity { si, .. })
                if k.id == kind && s.meaning == SpecMeaning::ExactNominal =>
            {
                Some(*si)
            }
            _ => None,
        }
    });
    match (measured.next(), measured.next()) {
        (Some(si), None) => Some(si),
        _ => None,
    }
}

/// Every component lineage of the project, then every Part lineage:
/// published lineages at their latest revision, then draft-only lineages,
/// each in lineage order.
pub fn entries(project: &Project, drafts: &Drafts) -> Vec<CatalogEntry> {
    let library = &project.library;
    let mut used: BTreeMap<DefinitionId, usize> = BTreeMap::new();
    for instance in &project.instances {
        *used.entry(instance.component.id).or_default() += 1;
        if let Some(part) = instance.part {
            *used.entry(part.id).or_default() += 1;
        }
    }
    let latest_parts = library.latest_parts();
    let implemented: BTreeSet<DefinitionId> = latest_parts
        .iter()
        .map(|p| p.definition.component.id)
        .collect();
    let mut out: Vec<CatalogEntry> = library
        .latest_components()
        .into_iter()
        .map(|c| {
            let mut entry = entry_of(library, c.id, Some(c.revision), &c.meta, &c.definition);
            entry.draft = drafts.get(c.id).is_some();
            entry.used = used.get(&c.id).copied().unwrap_or(0);
            entry.badges.mpn = implemented.contains(&c.id);
            entry
        })
        .collect();
    let published: BTreeSet<DefinitionId> = out.iter().map(|e| e.lineage).collect();
    for draft in &drafts.items {
        if let crate::library::Definition::Component(component) = &draft.definition
            && !published.contains(&draft.id)
        {
            let mut entry = entry_of(library, draft.id, None, &draft.meta, component);
            entry.draft = true;
            out.push(entry);
        }
    }
    for p in latest_parts {
        let mut entry = part_entry(library, p.id, Some(p.revision), &p.meta, &p.definition);
        entry.draft = drafts.get(p.id).is_some();
        entry.used = used.get(&p.id).copied().unwrap_or(0);
        out.push(entry);
    }
    let published: BTreeSet<DefinitionId> = library.parts.iter().map(|p| p.id).collect();
    for draft in &drafts.items {
        if let crate::library::Definition::Part(part) = &draft.definition
            && !published.contains(&draft.id)
        {
            let mut entry = part_entry(library, draft.id, None, &draft.meta, part);
            entry.draft = true;
            out.push(entry);
        }
    }
    out
}

/// How a result matched its text, best first.
#[derive(
    Clone, Copy, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum MatchTier {
    /// The name or lineage ID equals the text.
    Exact,
    /// The name starts with the text.
    Prefix,
    /// A word of the name equals the text.
    Word,
    /// The name, description or a package label contains the text.
    Substring,
    /// Only a field value matched.
    Field,
    /// The query has no text terms; filters alone selected the entry.
    Filter,
}

/// Lineages the host marks, ranked after the match tier. The core never
/// reads settings; the host passes these in.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CatalogContext {
    pub favourites: BTreeSet<DefinitionId>,
    /// Most recent first.
    pub recent: Vec<DefinitionId>,
}

/// Case-folded text of one entry, built once per index.
struct Folded {
    name: String,
    /// A Part's MPN; an exact MPN ranks as an exact name.
    mpn: Option<String>,
    words: Vec<String>,
    lineage: String,
    other: String,
    fields: String,
}

impl Folded {
    fn of(entry: &CatalogEntry) -> Self {
        let name = fold(entry.name.trim());
        let words = name
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect();
        let mut other = fold(&entry.description);
        for text in entry.mpn.iter().chain(&entry.manufacturer) {
            other.push('\n');
            other.push_str(&fold(text));
        }
        for package in &entry.packages {
            other.push('\n');
            other.push_str(&fold(package));
        }
        let fields = entry
            .fields
            .iter()
            .map(|f| fold(&f.text))
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            name,
            mpn: entry.mpn.as_deref().map(|m| fold(m.trim())),
            words,
            lineage: fold(&entry.lineage.to_string()),
            other,
            fields,
        }
    }

    fn text_tier(&self, text: &str) -> Option<MatchTier> {
        if self.name == text || self.lineage == text || self.mpn.as_deref() == Some(text) {
            Some(MatchTier::Exact)
        } else if self.name.starts_with(text)
            || self.mpn.as_ref().is_some_and(|m| m.starts_with(text))
        {
            Some(MatchTier::Prefix)
        } else if self.words.iter().any(|w| w == text) {
            Some(MatchTier::Word)
        } else if self.name.contains(text) || self.other.contains(text) {
            Some(MatchTier::Substring)
        } else if self.fields.contains(text) {
            Some(MatchTier::Field)
        } else {
            None
        }
    }
}

fn field_hit(entry: &CatalogEntry, test: &FieldTest) -> bool {
    entry
        .fields
        .iter()
        .any(|f| f.field == test.field && meets(f, test))
}

/// A test on one entry value. On a Part's rated value an equality (`:` or a
/// bare value) means "meets", decided by `fields::satisfies` exactly as
/// `instance.assign_part` and purchasing decide it; other operators keep
/// their literal sense.
fn meets(field: &EntryField, test: &FieldTest) -> bool {
    let Some(rated) = &field.rated else {
        return test.test.accepts(&field.value);
    };
    match requirement_of(field, rated, test) {
        Some(requirement) => requirement
            .satisfied_by(&rated.spec, &rated.value_type)
            .is_empty(),
        None => test.test.accepts(&field.value),
    }
}

/// The requirement an equality predicate states, read in the predicate's
/// value type; `None` for explicit comparisons and text, which keep their
/// literal meaning.
fn requirement_of(
    field: &EntryField,
    rated: &RatedReading,
    test: &FieldTest,
) -> Option<Requirement> {
    let value_type = test
        .value_type
        .clone()
        .unwrap_or_else(|| rated.value_type.clone());
    let value = match (&test.test, &value_type) {
        (Test::Compare { op: Op::Eq, value }, ValueType::Quantity { .. }) => FieldValue::Quantity {
            si: *value,
            display_unit: None,
        },
        (Test::Compare { op: Op::Eq, value }, ValueType::Integer { .. })
            if value.fract() == 0.0 && value.abs() < 9.0e15 =>
        {
            FieldValue::Integer(*value as i64)
        }
        (Test::Compare { op: Op::Eq, value }, ValueType::Range { .. }) => FieldValue::Range {
            min: *value,
            max: *value,
        },
        (Test::Between { min, max }, ValueType::Range { .. }) => FieldValue::Range {
            min: *min,
            max: *max,
        },
        (
            Test::Tolerance {
                op: Op::Eq,
                minus,
                plus,
            },
            ValueType::Tolerance { .. },
        ) => FieldValue::Tolerance {
            minus: *minus,
            plus: *plus,
        },
        (Test::Choice { choice }, ValueType::Enum { .. }) => FieldValue::Enum(*choice),
        (Test::Boolean { value }, ValueType::Boolean) => FieldValue::Boolean(*value),
        _ => return None,
    };
    Some(Requirement {
        field: rated.spec.field,
        label: field.label.clone(),
        value,
        value_type,
        nominal: rated.nominal,
    })
}

/// The worst tier over the text terms, or `None` when a term fails.
fn tier(entry: &CatalogEntry, folded: &Folded, query: &CatalogQuery) -> Option<MatchTier> {
    let mut worst = None::<MatchTier>;
    let mut text_seen = false;
    for term in &query.terms {
        let tier = match term {
            Term::Text { text } => Some(folded.text_tier(text)?),
            Term::Value { text, readings } => Some(
                folded
                    .text_tier(text)
                    .filter(|t| *t < MatchTier::Field)
                    .or_else(|| {
                        readings
                            .iter()
                            .any(|r| field_hit(entry, r))
                            .then_some(MatchTier::Field)
                    })
                    .or_else(|| folded.text_tier(text))?,
            ),
            Term::Field { tests, .. } => {
                if !tests.iter().any(|t| field_hit(entry, t)) {
                    return None;
                }
                None
            }
            Term::Package { text } => {
                if !entry
                    .packages
                    .iter()
                    .any(|p| fold(p).contains(text.as_str()))
                {
                    return None;
                }
                None
            }
            Term::Has { badge, present } => {
                if entry.badges.get(*badge) != *present {
                    return None;
                }
                None
            }
            Term::Project => {
                if entry.source != Source::Project {
                    return None;
                }
                None
            }
            Term::Components | Term::Parts => {
                let kind = match term {
                    Term::Parts => DefinitionKind::Part,
                    _ => DefinitionKind::Component,
                };
                if entry.kind != kind {
                    return None;
                }
                None
            }
            Term::Mpn { text } => {
                if !folded
                    .mpn
                    .as_ref()
                    .is_some_and(|m| m.contains(text.as_str()))
                {
                    return None;
                }
                None
            }
        };
        if let Some(tier) = tier {
            text_seen = true;
            worst = Some(worst.map_or(tier, |w| w.max(tier)));
        }
    }
    if text_seen {
        worst
    } else {
        Some(MatchTier::Filter)
    }
}

/// Entries hidden from results, by reason. They are counted, never dropped
/// silently.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Hidden {
    pub reason: String,
    pub count: usize,
}

/// One ranked result.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Hit {
    pub entry: CatalogEntry,
    pub tier: MatchTier,
    pub favourite: bool,
}

/// `library.search`: ranked results and how the query was read.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SearchReport {
    /// The parsed terms, as the palette shows them.
    pub chips: Vec<String>,
    /// Matching visible entries, before the limit.
    pub total: usize,
    pub hidden: Vec<Hidden>,
    pub results: Vec<Hit>,
}

/// Search parameters beyond the query text.
#[derive(Clone, Copy, Debug)]
pub struct SearchOptions {
    pub limit: usize,
    /// List lineages that exist only as drafts instead of counting them as
    /// hidden.
    pub include_drafts: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
            include_drafts: false,
        }
    }
}

/// Entries with their folded text and the vocabulary their fields use.
pub struct Index {
    entries: Vec<CatalogEntry>,
    folded: Vec<Folded>,
    vocabulary: Vocabulary,
}

impl Index {
    pub fn new(entries: Vec<CatalogEntry>, vocabulary: Vocabulary) -> Result<Self> {
        if entries.len() > MAX_INDEX_ENTRIES {
            return Err(Error::Limit(format!(
                "a catalog index holds at most {MAX_INDEX_ENTRIES} entries"
            )));
        }
        let folded = entries.iter().map(Folded::of).collect();
        Ok(Self {
            entries,
            folded,
            vocabulary,
        })
    }

    /// The project's own index.
    pub fn of_project(project: &Project, drafts: &Drafts) -> Result<Self> {
        Self::new(entries(project, drafts), Vocabulary::of(&project.library))
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }
    pub fn vocabulary(&self) -> &Vocabulary {
        &self.vocabulary
    }

    pub fn parse(&self, text: &str) -> std::result::Result<CatalogQuery, QueryError> {
        parse_query(text, &self.vocabulary)
    }

    /// Indices of matching visible entries with their tiers, and the hidden
    /// counts.
    fn matching(
        &self,
        query: &CatalogQuery,
        include_drafts: bool,
    ) -> (Vec<(usize, MatchTier)>, Vec<Hidden>) {
        let mut out = Vec::new();
        let mut drafts = 0usize;
        for (i, (entry, folded)) in self.entries.iter().zip(&self.folded).enumerate() {
            let Some(tier) = tier(entry, folded, query) else {
                continue;
            };
            if entry.revision.is_none() && !include_drafts {
                drafts += 1;
            } else {
                out.push((i, tier));
            }
        }
        let hidden = (drafts > 0)
            .then(|| Hidden {
                reason: "unpublished draft".into(),
                count: drafts,
            })
            .into_iter()
            .collect();
        (out, hidden)
    }

    /// Ranked results: match tier, then used in this project, favourite,
    /// recently used, then folded name, source and lineage. Equal inputs
    /// always give the same order.
    pub fn search(
        &self,
        query: &CatalogQuery,
        context: &CatalogContext,
        options: SearchOptions,
    ) -> SearchReport {
        let (mut matched, hidden) = self.matching(query, options.include_drafts);
        let total = matched.len();
        // A bare value's chip lists only the readings that found something,
        // so `10k` shows "Resistance = 10 kΩ or 10k", not every kind it
        // could be read in.
        let chips = query
            .terms
            .iter()
            .map(|term| match term {
                Term::Value { text, readings } => Term::Value {
                    text: text.clone(),
                    readings: readings
                        .iter()
                        .filter(|r| matched.iter().any(|(i, _)| field_hit(&self.entries[*i], r)))
                        .cloned()
                        .collect(),
                }
                .chip(),
                other => other.chip(),
            })
            .collect();
        let recent: BTreeMap<DefinitionId, usize> = context
            .recent
            .iter()
            .take(MAX_CONTEXT)
            .enumerate()
            .rev()
            .map(|(i, id)| (*id, i))
            .collect();
        let key = |&(i, tier): &(usize, MatchTier)| {
            let entry = &self.entries[i];
            (
                tier,
                entry.used == 0,
                !context.favourites.contains(&entry.lineage),
                recent.get(&entry.lineage).copied().unwrap_or(usize::MAX),
            )
        };
        let compare = |a: &(usize, MatchTier), b: &(usize, MatchTier)| {
            key(a)
                .cmp(&key(b))
                .then_with(|| self.folded[a.0].name.cmp(&self.folded[b.0].name))
                .then_with(|| self.entries[a.0].source.cmp(&self.entries[b.0].source))
                .then_with(|| self.entries[a.0].lineage.cmp(&self.entries[b.0].lineage))
        };
        let limit = options.limit.clamp(1, MAX_RESULTS);
        if matched.len() > limit {
            matched.select_nth_unstable_by(limit - 1, compare);
            matched.truncate(limit);
        }
        matched.sort_unstable_by(compare);
        SearchReport {
            chips,
            total,
            hidden,
            results: matched
                .into_iter()
                .map(|(i, tier)| Hit {
                    entry: self.entries[i].clone(),
                    tier,
                    favourite: context.favourites.contains(&self.entries[i].lineage),
                })
                .collect(),
        }
    }

    /// Filters the matching entries offer: the fields they actually carry,
    /// their package labels and badge counts.
    pub fn facets(&self, query: &CatalogQuery, include_drafts: bool) -> FacetReport {
        let (matched, _) = self.matching(query, include_drafts);
        let mut by_field: BTreeMap<DefinitionId, FieldTally> = BTreeMap::new();
        let mut packages: BTreeMap<String, usize> = BTreeMap::new();
        let mut badges = BadgeCounts::default();
        for (i, _) in &matched {
            let entry = &self.entries[*i];
            for field in &entry.fields {
                let tally = by_field.entry(field.field).or_insert_with(|| FieldTally {
                    label: field.label.clone(),
                    ..FieldTally::default()
                });
                tally.entries += 1;
                *tally.values.entry(field.text.clone()).or_default() += 1;
                if let FieldValue::Quantity { si, .. } = field.value {
                    tally.min = Some(tally.min.map_or(si, |m: f64| m.min(si)));
                    tally.max = Some(tally.max.map_or(si, |m: f64| m.max(si)));
                }
            }
            for package in &entry.packages {
                *packages.entry(package.clone()).or_default() += 1;
            }
            for badge in Badge::ALL {
                if entry.badges.get(badge) {
                    *badges.slot(badge) += 1;
                }
            }
        }
        let mut fields: Vec<FieldFacet> = by_field
            .into_iter()
            .map(|(field, tally)| {
                let known = self.vocabulary.field(field);
                let kind = known.and_then(|f| f.kind.as_ref());
                let text = |si: f64| {
                    kind.map(|k| {
                        crate::quantity::format(
                            si,
                            &k.quantity.units,
                            &k.quantity.default_unit,
                            None,
                        )
                    })
                };
                FieldFacet {
                    field,
                    label: tally.label,
                    kind: kind.map(|k| k.name.clone()),
                    entries: tally.entries,
                    values: top(tally.values),
                    min: tally.min.and_then(text),
                    max: tally.max.and_then(text),
                }
            })
            .collect();
        fields.sort_by(|a, b| {
            b.entries
                .cmp(&a.entries)
                .then_with(|| a.label.cmp(&b.label))
                .then_with(|| a.field.cmp(&b.field))
        });
        FacetReport {
            total: matched.len(),
            fields,
            packages: top(packages),
            badges,
        }
    }
}

#[derive(Default)]
struct FieldTally {
    label: String,
    entries: usize,
    values: BTreeMap<String, usize>,
    min: Option<f64>,
    max: Option<f64>,
}

/// The most frequent values, ties in text order.
fn top(values: BTreeMap<String, usize>) -> Vec<FacetValue> {
    let mut values: Vec<FacetValue> = values
        .into_iter()
        .map(|(text, count)| FacetValue { text, count })
        .collect();
    values.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.text.cmp(&b.text)));
    values.truncate(MAX_FACET_VALUES);
    values
}

/// One value of a facet and how many matching entries carry it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FacetValue {
    pub text: String,
    pub count: usize,
}

/// A field the matching entries carry.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FieldFacet {
    pub field: DefinitionId,
    pub label: String,
    /// The quantity kind's name, for quantity fields.
    pub kind: Option<String>,
    pub entries: usize,
    pub values: Vec<FacetValue>,
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct BadgeCounts {
    pub symbol: usize,
    pub footprint: usize,
    pub model: usize,
    pub mpn: usize,
}

impl BadgeCounts {
    fn slot(&mut self, badge: Badge) -> &mut usize {
        match badge {
            Badge::Symbol => &mut self.symbol,
            Badge::Footprint => &mut self.footprint,
            Badge::Model => &mut self.model,
            Badge::Mpn => &mut self.mpn,
        }
    }
}

/// `library.facets`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FacetReport {
    pub total: usize,
    pub fields: Vec<FieldFacet>,
    pub packages: Vec<FacetValue>,
    pub badges: BadgeCounts,
}

/// One revision of a lineage, for `library.inspect`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RevisionSummary {
    pub revision: Revision,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SymbolSummary {
    pub symbol: SymbolRef,
    pub name: String,
    pub units: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FootprintSummary {
    pub footprint: FootprintRef,
    pub name: String,
    pub pads: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ImplementationSummary {
    pub binding: BindingRef,
    pub name: String,
    pub package: PackageRef,
    pub package_name: String,
    /// Envelope length, width and height in mm.
    pub envelope: [f64; 3],
    pub footprints: Vec<FootprintSummary>,
}

/// `library.inspect`: one revision of a lineage with what it pins, for the
/// preview beside the Place palette.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ItemReport {
    pub entry: CatalogEntry,
    /// Whether this is the lineage's latest published revision.
    pub latest: bool,
    pub revisions: Vec<RevisionSummary>,
    pub provenance: String,
    pub terminals: usize,
    pub symbols: Vec<SymbolSummary>,
    pub implementations: Vec<ImplementationSummary>,
}

/// Inspect one published component revision, the latest when `revision`
/// is `None`.
pub fn inspect(
    project: &Project,
    drafts: &Drafts,
    lineage: DefinitionId,
    revision: Option<Revision>,
) -> Result<ItemReport> {
    let library = &project.library;
    let mut revisions: Vec<_> = library
        .components
        .iter()
        .filter(|c| c.id == lineage)
        .collect();
    revisions.sort_by_key(|c| c.revision);
    let latest = revisions
        .last()
        .map(|c| c.revision)
        .ok_or_else(|| Error::NotFound(format!("{lineage} has no published component revision")))?;
    let wanted = revision.unwrap_or(latest);
    let published = revisions
        .iter()
        .find(|c| c.revision == wanted)
        .ok_or_else(|| Error::NotFound(format!("{lineage} has no revision {wanted}")))?;
    let definition = &published.definition;
    let mut entry = entry_of(library, lineage, Some(wanted), &published.meta, definition);
    entry.draft = drafts.get(lineage).is_some();
    entry.used = project
        .instances
        .iter()
        .filter(|i| i.component.id == lineage)
        .count();
    let symbols = definition
        .representations
        .iter()
        .filter_map(|r| library.symbol(*r))
        .map(|s| SymbolSummary {
            symbol: SymbolRef::new(s.id, s.revision),
            name: s.meta.name.clone(),
            units: s.definition.units.iter().map(|u| u.label.clone()).collect(),
        })
        .collect();
    let implementations = definition
        .implementations
        .iter()
        .filter_map(|r| library.binding(*r))
        .map(|b| {
            let package = library.package(b.definition.package);
            ImplementationSummary {
                binding: BindingRef::new(b.id, b.revision),
                name: b.meta.name.clone(),
                package: b.definition.package,
                package_name: package.map_or_else(String::new, |p| p.meta.name.clone()),
                envelope: package.map_or([0.0; 3], |p| p.definition.envelope),
                footprints: b
                    .definition
                    .footprints
                    .iter()
                    .filter_map(|f| library.footprint(*f))
                    .map(|f| FootprintSummary {
                        footprint: FootprintRef::new(f.id, f.revision),
                        name: f.meta.name.clone(),
                        pads: f.definition.pads.len(),
                    })
                    .collect(),
            }
        })
        .collect();
    Ok(ItemReport {
        latest: wanted == latest,
        revisions: revisions
            .iter()
            .map(|c| RevisionSummary {
                revision: c.revision,
                name: c.meta.name.clone(),
            })
            .collect(),
        provenance: published.meta.provenance.clone(),
        terminals: definition.terminals.len(),
        symbols,
        implementations,
        entry,
    })
}

#[cfg(test)]
mod tests;
