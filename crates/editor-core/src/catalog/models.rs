//! Slice D1: the 3D library is the catalog filtered to
//! models, over the project and every source, plus the generated families
//! as virtual entries. Facets: format, mount, pins, pitch, body L×W×H
//! (from bounds), licence/redistribution, source, `has:leads`,
//! `has:marking`, `has:textures`. Opened from a package, hits are ranked
//! "fits this package": lead-tagged models by `fit_leads` rms against the
//! land pads, the rest by bounds against the envelope. A score is shown,
//! never auto-assigned.
use super::query::{MAX_QUERY, MAX_TERMS, Op, fold};
use super::{CatalogContext, Source};
use crate::diagnostics::{Error, Result};
use crate::ids::{DefinitionId, ModelRef};
use crate::library::model3d::{
    BOUNDS_TOLERANCE_MM, ModelDefinition, ModelFormat, ModelHints, Mount, Redistribution,
    bounds_excess,
};
use crate::library::package_fit::{self, FitRotation, LeadPair};
use crate::library::templates::catalog::FamilyKind;
use crate::library::{
    Definition, Drafts, FootprintDefinition, Library, ModelPlacement, PackageDefinition, PadKind,
    Published,
};
use crate::model::Project;
use crate::units::{LengthUnit, parse_length};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// One facet value and how many matching hits carry it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelFacetValue {
    pub value: String,
    pub count: usize,
}

/// Counts over every matching hit (not only the returned ones), so the
/// chooser can offer filters the data supports. Values are the query
/// words that select them (`format:step`, `mount:smd`, …).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelFacets {
    pub format: Vec<ModelFacetValue>,
    pub mount: Vec<ModelFacetValue>,
    pub pins: Vec<ModelFacetValue>,
    pub pitch: Vec<ModelFacetValue>,
    pub licence: Vec<ModelFacetValue>,
    pub source: Vec<ModelFacetValue>,
    pub has_leads: usize,
    pub has_marking: usize,
    pub has_textures: usize,
    /// Generated-family entries among the matches.
    pub generated: usize,
}

/// `library.models`'s result.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelReport {
    pub hits: Vec<ModelHit>,
    pub more: usize,
    /// D1 (additive): facet counts over every match.
    #[serde(default)]
    pub facets: ModelFacets,
}

/// Pitch values within this distance match, mm.
const PITCH_TOLERANCE_MM: f64 = 0.005;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flag {
    Leads,
    Marking,
    Textures,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SourceFilter {
    Project,
    Library,
}

/// One parsed term of a model query. Terms are ANDed.
#[derive(Clone, Debug, PartialEq)]
enum Filter {
    Text(String),
    Format(ModelFormat),
    Mount(Mount),
    Pins(Op, u32),
    Pitch(Op, f64),
    /// Length and width in either order, then height if given, mm.
    Body(Vec<f64>),
    Redistribution(Redistribution),
    /// Licence text (case-folded substring).
    Licence(String),
    Source(SourceFilter),
    Has(Flag, bool),
    Family(String),
    /// `kind:generated`: generated-family entries only.
    Generated,
}

fn invalid(term: &str, message: &str) -> Error {
    Error::Invalid(format!("models.query: {term}: {message}"))
}

/// `>=8` → (Ge, "8").
fn op_value(value: &str) -> (Op, &str) {
    for (prefix, op) in [(">=", Op::Ge), ("<=", Op::Le), (">", Op::Gt), ("<", Op::Lt)] {
        if let Some(rest) = value.strip_prefix(prefix) {
            return (op, rest);
        }
    }
    (Op::Eq, value.strip_prefix('=').unwrap_or(value))
}

/// A positive length in mm: a number in mm or with a unit (`50mil`).
fn length_mm(text: &str) -> Option<f64> {
    let mm = parse_length(text, LengthUnit::Mm).ok()?.to_mm_f64();
    (mm > 0.0).then_some(mm)
}

fn filter(token: &str) -> Result<Option<Filter>> {
    let Some((key, value)) = token.split_once(':').filter(|(k, _)| !k.is_empty()) else {
        return Ok(Some(Filter::Text(fold(token))));
    };
    let key = fold(key);
    let lower = fold(value);
    let fail = |message: &str| invalid(token, message);
    Ok(Some(match key.as_str() {
        "kind" => match lower.as_str() {
            // Every entry is a model.
            "model" | "models" => return Ok(None),
            "generated" => Filter::Generated,
            _ => {
                return Err(fail(
                    "the 3D library lists models only (kind:model or kind:generated)",
                ));
            }
        },
        "format" => Filter::Format(match lower.as_str() {
            "step" | "stp" => ModelFormat::Step,
            "gltf" | "glb" => ModelFormat::Gltf,
            "solidcraft" => ModelFormat::Solidcraft,
            _ => return Err(fail("formats are step, gltf (glb) and solidcraft")),
        }),
        "mount" => Filter::Mount(match lower.as_str() {
            "smd" | "smt" => Mount::Smd,
            "tht" => Mount::Tht,
            _ => return Err(fail("mounts are smd and tht")),
        }),
        "pins" => {
            let (op, number) = op_value(&lower);
            let pins = number
                .parse::<u32>()
                .map_err(|_| fail("pins takes a whole number, such as pins:8 or pins:>=14"))?;
            Filter::Pins(op, pins)
        }
        "pitch" => {
            let (op, number) = op_value(&lower);
            let pitch = length_mm(number)
                .ok_or_else(|| fail("pitch takes a length, such as pitch:1.27 or pitch:50mil"))?;
            Filter::Pitch(op, pitch)
        }
        "body" => {
            let dims: Option<Vec<f64>> = lower.split(['x', '×', '*']).map(length_mm).collect();
            match dims {
                Some(dims) if (2..=3).contains(&dims.len()) => Filter::Body(dims),
                _ => return Err(fail("body takes L×W or L×W×H in mm, such as body:4.9x3.9")),
            }
        }
        "licence" | "license" | "redistribution" => match lower.as_str() {
            "permitted" => Filter::Redistribution(Redistribution::Permitted),
            "forbidden" => Filter::Redistribution(Redistribution::Forbidden),
            "unknown" => Filter::Redistribution(Redistribution::Unknown),
            "" => {
                return Err(fail(
                    "licence takes permitted, forbidden, unknown or licence text",
                ));
            }
            text => Filter::Licence(text.to_owned()),
        },
        "src" | "source" => Filter::Source(match lower.as_str() {
            "project" => SourceFilter::Project,
            "library" | "libraries" => SourceFilter::Library,
            _ => return Err(fail("sources are project and library")),
        }),
        "has" | "missing" => {
            let flag = match lower.as_str() {
                "leads" | "lead" => Flag::Leads,
                "marking" | "markings" => Flag::Marking,
                "textures" | "texture" => Flag::Textures,
                _ => return Err(fail("has takes leads, marking or textures")),
            };
            Filter::Has(flag, key == "has")
        }
        "family" => Filter::Family(lower),
        _ => {
            return Err(fail(
                "unknown filter; use kind, format, mount, pins, pitch, body, licence, src, has, missing or family",
            ));
        }
    }))
}

fn parse(query: &str) -> Result<Vec<Filter>> {
    if query.len() > MAX_QUERY {
        return Err(Error::Invalid(format!(
            "models.query: a query is at most {MAX_QUERY} bytes"
        )));
    }
    let tokens: Vec<&str> = query.split_whitespace().collect();
    if tokens.len() > MAX_TERMS {
        return Err(Error::Invalid(format!(
            "models.query: a query has at most {MAX_TERMS} terms"
        )));
    }
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        out.extend(filter(token)?);
    }
    Ok(out)
}

/// One searchable entry before filtering.
struct Candidate<'a> {
    kind: ModelHitKind,
    source: Source,
    name: String,
    /// Case-folded searchable text besides the name.
    other: String,
    hints: ModelHints,
    definition: Option<&'a ModelDefinition>,
    licence: String,
}

impl Candidate<'_> {
    fn generated(&self) -> Option<FamilyKind> {
        match self.kind {
            ModelHitKind::Generated { family } => Some(family),
            ModelHitKind::Library { .. } => None,
        }
    }

    fn meets(&self, filter: &Filter, tier: &mut u8) -> bool {
        let model = self.definition;
        match filter {
            Filter::Text(text) => {
                let name = fold(&self.name);
                let found = if name == *text {
                    Some(0)
                } else if name.starts_with(text.as_str()) {
                    Some(1)
                } else if name
                    .split(|c: char| !c.is_alphanumeric())
                    .any(|w| w == text)
                {
                    Some(2)
                } else if name.contains(text.as_str()) || self.other.contains(text.as_str()) {
                    Some(3)
                } else {
                    None
                };
                found.is_some_and(|t| {
                    *tier = (*tier).max(t);
                    true
                })
            }
            Filter::Format(format) => model.is_some_and(|m| m.format == *format),
            Filter::Mount(mount) => self.hints.mount == Some(*mount),
            Filter::Pins(op, pins) => model
                .and_then(ModelDefinition::pin_count)
                .is_some_and(|n| op.holds(f64::from(n), f64::from(*pins))),
            Filter::Pitch(op, pitch) => self.hints.pitch_mm.is_some_and(|p| match op {
                Op::Eq => (p - pitch).abs() <= PITCH_TOLERANCE_MM,
                _ => op.holds(p, *pitch),
            }),
            Filter::Body(dims) => model.is_some_and(|m| body_matches(m.size_mm(), dims)),
            Filter::Redistribution(r) => model.is_some_and(|m| m.provenance.redistribution == *r),
            Filter::Licence(text) => model.is_some() && self.licence.contains(text.as_str()),
            Filter::Source(SourceFilter::Project) => self.source == Source::Project,
            Filter::Source(SourceFilter::Library) => matches!(self.source, Source::Library(_)),
            Filter::Has(flag, present) => {
                let has = model.is_some_and(|m| match flag {
                    Flag::Leads => !m.leads.is_empty(),
                    Flag::Marking => m.marking_area.is_some(),
                    Flag::Textures => m.has_textures(),
                });
                has == *present
            }
            Filter::Family(text) => self
                .hints
                .family
                .as_deref()
                .is_some_and(|f| fold(f).contains(text.as_str())),
            Filter::Generated => self.generated().is_some(),
        }
    }
}

fn body_matches(size: [f64; 3], dims: &[f64]) -> bool {
    let near = |a: f64, b: f64| (a - b).abs() <= BOUNDS_TOLERANCE_MM;
    let (Some(l), Some(w)) = (dims.first(), dims.get(1)) else {
        return false;
    };
    let plan = (near(size[0], *l) && near(size[1], *w)) || (near(size[0], *w) && near(size[1], *l));
    plan && dims.get(2).is_none_or(|h| near(size[2], *h))
}

/// The label of a generated family entry.
fn family_label(family: FamilyKind) -> &'static str {
    match family {
        FamilyKind::Chip => "chip",
        FamilyKind::GullWing => "gull-wing",
        FamilyKind::NoLead => "QFN / DFN",
        FamilyKind::Header => "pin header",
        FamilyKind::Tabbed => "SOT / DPAK",
        FamilyKind::Molded => "SOD / SMA / MELF",
        FamilyKind::Grid => "BGA / LGA",
        FamilyKind::Tht => "through-hole",
        FamilyKind::Module => "module",
        FamilyKind::UsbC => "USB-C receptacle",
    }
}

/// The latest revision of each model lineage, oldest lineage first.
fn latest(models: &[Published<ModelDefinition>]) -> Vec<&Published<ModelDefinition>> {
    let mut by: BTreeMap<DefinitionId, &Published<ModelDefinition>> = BTreeMap::new();
    for model in models {
        let slot = by.entry(model.id).or_insert(model);
        if model.revision > slot.revision {
            *slot = model;
        }
    }
    by.into_values().collect()
}

fn library_candidate(source: Source, model: &Published<ModelDefinition>) -> Candidate<'_> {
    let d = &model.definition;
    let mut other = fold(&model.meta.description);
    for text in [
        d.hints.family.as_deref(),
        d.provenance.vendor.as_deref(),
        Some(d.asset.file_name.as_str()),
        Some(d.format.label()),
    ]
    .into_iter()
    .flatten()
    {
        other.push('\n');
        other.push_str(&fold(text));
    }
    Candidate {
        kind: ModelHitKind::Library {
            model: ModelRef::new(model.id, model.revision),
            format: d.format,
            redistribution: d.provenance.redistribution,
            has_leads: !d.leads.is_empty(),
            has_marking: d.marking_area.is_some(),
            has_textures: d.has_textures(),
        },
        source,
        name: model.meta.name.clone(),
        other,
        hints: ModelHints {
            pins: d.pin_count(),
            ..d.hints.clone()
        },
        definition: Some(d),
        licence: fold(d.provenance.record.licence.as_deref().unwrap_or_default()),
    }
}

fn generated_candidate<'a>(family: FamilyKind) -> Candidate<'a> {
    let label = family_label(family);
    let mount = match family {
        FamilyKind::Header | FamilyKind::Tht => Mount::Tht,
        _ => Mount::Smd,
    };
    Candidate {
        kind: ModelHitKind::Generated { family },
        source: Source::Project,
        name: format!("Generated · {label}"),
        other: "generated parametric".into(),
        hints: ModelHints {
            mount: Some(mount),
            family: Some(label.to_owned()),
            ..ModelHints::default()
        },
        definition: None,
        licence: String::new(),
    }
}

/// The package the chooser was opened from, and the footprint that lands
/// it (a draft first, else the latest published revision naming it).
struct Target<'a> {
    package: &'a PackageDefinition,
    footprint: Option<&'a FootprintDefinition>,
}

fn target<'a>(project: &'a Project, drafts: &'a Drafts, id: DefinitionId) -> Result<Target<'a>> {
    let library: &Library = &project.library;
    let package = match drafts.get(id).map(|d| &d.definition) {
        Some(Definition::Package(p)) => p,
        Some(_) => {
            return Err(Error::Invalid(format!(
                "models.fits: draft {id} is not a package"
            )));
        }
        None => library
            .packages
            .iter()
            .filter(|p| p.id == id)
            .max_by_key(|p| p.revision)
            .map(|p| &p.definition)
            .ok_or_else(|| Error::NotFound(format!("models.fits: package {id} does not exist")))?,
    };
    let footprint = drafts
        .items
        .iter()
        .find_map(|d| match &d.definition {
            Definition::Footprint(f) if f.package.id == id => Some(f),
            _ => None,
        })
        .or_else(|| {
            library
                .footprints
                .iter()
                .filter(|f| f.definition.package.id == id)
                .max_by_key(|f| (f.definition.package.revision, f.id, f.revision))
                .map(|f| &f.definition)
        });
    Ok(Target { package, footprint })
}

/// "Fits this package": lead-tagged models by the rms of a free rigid fit
/// of their contacts onto the land pads, when every landed terminal finds
/// a lead of its designation (a proposal for scoring only, never a map);
/// the rest by bounds against the envelope.
fn fit(target: &Target<'_>, model: &ModelDefinition) -> FitScore {
    if let Some(footprint) = target.footprint
        && !model.leads.is_empty()
    {
        let mut landed = 0usize;
        let mut pairs = Vec::new();
        for terminal in &target.package.terminals {
            let pads: Vec<_> = footprint
                .pads
                .iter()
                .filter(|p| {
                    p.terminal == Some(terminal.id)
                        && matches!(p.kind, PadKind::Smd | PadKind::PlatedThrough)
                })
                .collect();
            let Some(first) = pads.first() else {
                continue;
            };
            landed += 1;
            let Some(lead) = model
                .leads
                .iter()
                .find(|l| l.designation == terminal.designation)
            else {
                continue;
            };
            let centres: Vec<[f64; 2]> = pads
                .iter()
                .map(|p| package_fit::to_datum(&footprint.datum, p.position))
                .collect();
            let n = f64::from(u32::try_from(centres.len()).unwrap_or(u32::MAX));
            let sum = centres
                .iter()
                .fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
            pairs.push(LeadPair {
                terminal: terminal.id,
                pad: first.id,
                lead: lead.contact,
                pad_centre: [sum[0] / n, sum[1] / n],
            });
        }
        if pairs.len() >= 2
            && pairs.len() == landed
            && let Ok(result) =
                package_fit::fit_leads(&ModelPlacement::default(), &pairs, FitRotation::Free)
        {
            return FitScore::Leads {
                rms_mm: result.rms_mm,
            };
        }
    }
    FitScore::Bounds {
        excess_mm: bounds_excess(model, target.package.envelope),
    }
}

/// Fit order: lead fits by rms, then bounds by distance from a snug fit,
/// then unscored.
fn fit_key(fit: Option<FitScore>) -> (u8, f64) {
    match fit {
        Some(FitScore::Leads { rms_mm }) => (0, rms_mm),
        Some(FitScore::Bounds { excess_mm }) => (1, excess_mm.abs()),
        None => (2, 0.0),
    }
}

fn count(values: BTreeMap<String, usize>) -> Vec<ModelFacetValue> {
    let mut out: Vec<ModelFacetValue> = values
        .into_iter()
        .map(|(value, count)| ModelFacetValue { value, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    out.truncate(super::MAX_FACET_VALUES);
    out
}

fn facets(hits: &[ModelHit]) -> ModelFacets {
    let mut format = BTreeMap::new();
    let mut mount = BTreeMap::new();
    let mut pins = BTreeMap::new();
    let mut pitch = BTreeMap::new();
    let mut licence = BTreeMap::new();
    let mut source = BTreeMap::new();
    let mut out = ModelFacets::default();
    let add = |map: &mut BTreeMap<String, usize>, key: String| {
        *map.entry(key).or_insert(0) += 1;
    };
    for hit in hits {
        if let Some(m) = hit.hints.mount {
            add(&mut mount, fold(m.label()));
        }
        if let Some(n) = hit.hints.pins {
            add(&mut pins, n.to_string());
        }
        if let Some(p) = hit.hints.pitch_mm {
            add(&mut pitch, format!("{p}"));
        }
        add(
            &mut source,
            match hit.source {
                Source::Project => "project".into(),
                Source::Library(_) => "library".into(),
            },
        );
        match &hit.kind {
            ModelHitKind::Library {
                format: f,
                redistribution,
                has_leads,
                has_marking,
                has_textures,
                ..
            } => {
                add(&mut format, fold(f.label()));
                add(&mut licence, redistribution.label().to_owned());
                out.has_leads += usize::from(*has_leads);
                out.has_marking += usize::from(*has_marking);
                out.has_textures += usize::from(*has_textures);
            }
            ModelHitKind::Generated { .. } => out.generated += 1,
        }
    }
    out.format = count(format);
    out.mount = count(mount);
    out.pins = count(pins);
    out.pitch = count(pitch);
    out.licence = count(licence);
    out.source = count(source);
    out
}

/// `library.models`: the project's models and every loaded source's
/// (their latest revisions), plus the built generated families, filtered by
/// `query` and ranked by fit (when `fits` names a package draft or
/// lineage), then match tier, then name. `fits` scores; it never assigns.
///
/// A source's models are read from its catalog's `fields` library, which
/// the source catalog fills with the source's models.
pub fn search(
    project: &Project,
    drafts: &Drafts,
    context: &CatalogContext,
    query: &str,
    fits: Option<DefinitionId>,
    limit: usize,
) -> Result<ModelReport> {
    let filters = parse(query)?;
    let target = fits.map(|id| target(project, drafts, id)).transpose()?;
    let mut candidates: Vec<Candidate<'_>> = latest(&project.library.models)
        .into_iter()
        .map(|m| library_candidate(Source::Project, m))
        .collect();
    for source in &context.sources {
        candidates.extend(
            latest(&source.fields.models)
                .into_iter()
                .map(|m| library_candidate(Source::Library(source.library), m)),
        );
    }
    candidates.extend(FamilyKind::ALL.into_iter().map(generated_candidate));
    if candidates.len() > super::MAX_INDEX_ENTRIES {
        return Err(Error::Limit(format!(
            "models.query: at most {} models are searched",
            super::MAX_INDEX_ENTRIES
        )));
    }
    let own_family = target
        .as_ref()
        .and_then(|t| FamilyKind::of(&t.package.form));
    let mut ranked: Vec<(u8, ModelHit)> = Vec::new();
    for candidate in candidates {
        let mut tier = 0u8;
        if !filters.iter().all(|f| candidate.meets(f, &mut tier)) {
            continue;
        }
        let fit = match (&target, candidate.definition) {
            (Some(t), Some(model)) => Some(fit(t, model)),
            // The package's own generated family is built from its form,
            // so it fills the envelope exactly.
            (Some(_), None) if candidate.generated() == own_family && own_family.is_some() => {
                Some(FitScore::Bounds { excess_mm: 0.0 })
            }
            _ => None,
        };
        ranked.push((
            tier,
            ModelHit {
                body_mm: candidate.definition.map(ModelDefinition::size_mm),
                kind: candidate.kind,
                source: candidate.source,
                name: candidate.name,
                hints: candidate.hints,
                fit,
            },
        ));
    }
    let id_of = |hit: &ModelHit| match hit.kind {
        ModelHitKind::Library { model, .. } => (0u8, model.id.0, model.revision.0),
        ModelHitKind::Generated { family } => (
            1,
            FamilyKind::ALL
                .iter()
                .position(|f| *f == family)
                .and_then(|p| u64::try_from(p).ok())
                .unwrap_or(u64::MAX),
            0,
        ),
    };
    ranked.sort_by(|(ta, a), (tb, b)| {
        let (ca, sa) = fit_key(a.fit);
        let (cb, sb) = fit_key(b.fit);
        ca.cmp(&cb)
            .then(sa.total_cmp(&sb))
            .then(ta.cmp(tb))
            // The library's own models before the generated families.
            .then_with(|| id_of(a).0.cmp(&id_of(b).0))
            .then_with(|| fold(&a.name).cmp(&fold(&b.name)))
            .then(a.source.cmp(&b.source))
            .then_with(|| id_of(a).cmp(&id_of(b)))
    });
    let hits: Vec<ModelHit> = ranked.into_iter().map(|(_, hit)| hit).collect();
    let facets = facets(&hits);
    let total = hits.len();
    let shown = limit.min(MAX_MODEL_HITS);
    let hits: Vec<ModelHit> = hits.into_iter().take(shown).collect();
    Ok(ModelReport {
        more: total.saturating_sub(hits.len()),
        hits,
        facets,
    })
}

#[cfg(test)]
mod tests;
